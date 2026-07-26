use glam::DVec3;
use shalrath::parser::repr::parse_map;
use std::collections::HashMap;
use std::fs::{File, read_to_string};
use std::io::{Read, Seek, Write};
use std::time::Instant;
use unreal_asset::exports::ExportBaseTrait;
use unreal_asset::exports::ExportNormalTrait;
use unreal_asset::reader::ArchiveTrait;
use unreal_asset::types::PackageIndex;

mod brush_to_mesh;
use brush_to_mesh::convert_to_mesh;

// For debugging purposes; may not be called
#[allow(dead_code)]
mod obj_export;

const MISE_FILES: &[(&str, &[u8])] = &[
    (
        "BP_ExaminableGrave.uasset",
        include_bytes!("mise/BP_ExaminableGrave.uasset"),
    ),
    (
        "BP_ExaminableGrave.uexp",
        include_bytes!("mise/BP_ExaminableGrave.uexp"),
    ),
    ("BP_Hazard.uasset", include_bytes!("mise/BP_Hazard.uasset")),
    ("BP_Hazard.uexp", include_bytes!("mise/BP_Hazard.uexp")),
    ("M_HazMat.uasset", include_bytes!("mise/M_HazMat.uasset")),
    ("M_HazMat.uexp", include_bytes!("mise/M_HazMat.uexp")),
    (
        "SM_ExampleBox.uasset",
        include_bytes!("mise/SM_ExampleBox.uasset"),
    ),
    (
        "SM_ExampleBox.uexp",
        include_bytes!("mise/SM_ExampleBox.uexp"),
    ),
    (
        "SM_ExampleBox.ubulk",
        include_bytes!("mise/SM_ExampleBox.ubulk"),
    ),
];

const MISE_UMAP: &[u8] = include_bytes!("mise/mise.umap");
const MISE_UEXP: &[u8] = include_bytes!("mise/mise.uexp");

// World-space (map-unit) spacing between generated interior face vertices;
// see `brush_to_mesh::convert_to_mesh`. Smaller values give smoother
// per-vertex lighting at the cost of more geometry.
const FACE_VERTEX_SPACING: f64 = 64.0;
const MESH_CONVERSION_SCALE: f64 = 4.0;

fn default_opts() -> pseudocooker::CookOptions {
    pseudocooker::CookOptions {
        body_setup_guid: None,
        lighting_guid: None,
        package_guid: None,
    }
}

type UnrealExportConstraint<'a, C> = Box<
    dyn Fn(&unreal_asset::Asset<C>, &unreal_asset::exports::NormalExport<PackageIndex>) -> bool
        + 'a,
>;

fn with_import<'a, C: Read + Seek>(
    obj_prop_name: &'a str,
    import_name: &'a str,
) -> UnrealExportConstraint<'a, C> {
    Box::new(move |asset, export| {
        let mut matching_prop = None;
        for prop in &export.properties {
            if let unreal_asset::properties::Property::ObjectProperty(obj_prop) = prop {
                if obj_prop
                    .name
                    .get_content(|content| content == obj_prop_name)
                {
                    matching_prop = Some(obj_prop);
                }
            }
        }

        let Some(prop) = matching_prop else {
            return false;
        };

        // this is expected to be an import
        assert!(prop.value.index < 0);
        let import = asset
            .get_import(prop.value)
            .expect(&format!("failed to get import {}", prop.value.index));
        return import
            .object_name
            .get_content(|content| content == import_name);
    })
}
fn with_name<'a, C: Read + Seek>(name: &'a str) -> UnrealExportConstraint<'a, C> {
    Box::new(move |_, export| {
        export
            .base_export
            .object_name
            .get_content(|content| content == name)
    })
}

fn find_export<'a, C: Read + Seek>(
    asset: &'a unreal_asset::Asset<C>,
    constraints: &[UnrealExportConstraint<C>],
) -> Option<PackageIndex> {
    let mut maybe_idx = None;
    for (i, export) in asset.asset_data.exports.iter().enumerate() {
        if let Some(normal_export) = export.get_normal_export() {
            if constraints.iter().all(|f| f(asset, normal_export)) {
                maybe_idx = Some(PackageIndex::new((i + 1) as i32));
            }
        }
    }
    maybe_idx
}

fn find_vec_property_mut<'a>(
    export: &'a mut unreal_asset::Export<PackageIndex>,
    name: &str,
) -> Option<&'a mut unreal_asset::properties::vector_property::VectorProperty> {
    let mut result = None;
    let props = &mut export.get_normal_export_mut().unwrap().properties;
    for prop in props {
        if let unreal_asset::properties::Property::StructProperty(struct_prop) = prop {
            if struct_prop.name.get_content(|content| content == name) {
                for prop in &mut struct_prop.value {
                    if let unreal_asset::properties::Property::VectorProperty(vec_prop) = prop {
                        result = Some(vec_prop);
                    }
                }
            }
        }
    }
    return result;
}

fn find_obj_property<'a>(
    export: &'a unreal_asset::Export<PackageIndex>,
    name: &str,
) -> Option<&'a unreal_asset::properties::object_property::ObjectProperty> {
    let mut result = None;
    let props = &export.get_normal_export().unwrap().properties;
    for prop in props {
        if let unreal_asset::properties::Property::ObjectProperty(obj_prop) = prop {
            if obj_prop.name.get_content(|content| content == name) {
                result = Some(obj_prop);
            }
        }
    }
    return result;
}

fn find_obj_property_mut<'a>(
    export: &'a mut unreal_asset::Export<PackageIndex>,
    name: &str,
) -> Option<&'a mut unreal_asset::properties::object_property::ObjectProperty> {
    let mut result = None;
    let props = &mut export.get_normal_export_mut().unwrap().properties;
    for prop in props {
        if let unreal_asset::properties::Property::ObjectProperty(obj_prop) = prop {
            if obj_prop.name.get_content(|content| content == name) {
                result = Some(obj_prop);
            }
        }
    }
    return result;
}

fn find_import<C: Read + Seek>(
    asset: &mut unreal_asset::Asset<C>,
    class_name: &str,
    object_name: &str,
) -> Option<PackageIndex> {
    for (i, import) in asset.imports.iter().enumerate() {
        if import
            .class_name
            .get_content(|content| content == class_name)
            && import
                .object_name
                .get_content(|content| content == object_name)
        {
            return Some(PackageIndex::new(-((i + 1) as i32)));
        }
    }
    return None;
}

use std::sync::atomic::{AtomicI32, Ordering};

fn export_clone_counter() -> i32 {
    static COUNT: AtomicI32 = AtomicI32::new(100);
    COUNT.fetch_add(1, Ordering::Relaxed)
}

fn clone_export<C: Read + Seek>(
    asset: &mut unreal_asset::Asset<C>,
    idx: PackageIndex,
) -> Option<PackageIndex> {
    let mut export = asset.get_export(idx)?.clone();
    let old_name = export.get_base_export().object_name.get_owned_content();
    let new_name = format!("pseudochef_{}", old_name);
    let new_fname = asset.add_fname_with_number(&new_name, export_clone_counter());
    export.get_base_export_mut().object_name = new_fname;
    asset.asset_data.exports.push(export);
    return Some(PackageIndex::new(asset.asset_data.exports.len() as i32));
}

fn add_actor_to_level<C: Read + Seek>(asset: &mut unreal_asset::Asset<C>, idx: PackageIndex) {
    let level_idx = find_export(asset, &vec![with_name("PersistentLevel")]).unwrap();
    let export = asset.get_export_mut(level_idx).unwrap();
    if let unreal_asset::Export::LevelExport(level_export) = export {
        level_export.actors.push(idx);
    } else {
        panic!();
    }
}

fn pak_add_brush<W: Write + Seek>(
    pak: &mut repak::PakWriter<W>,
    brush: &shalrath::repr::Brush,
    level_name: &str,
    asset_name: &str,
) -> Option<(String, DVec3)> {
    let (mesh, origin) = convert_to_mesh(&brush, FACE_VERTEX_SPACING);
    let mut origin = DVec3::from(origin);
    origin *= MESH_CONVERSION_SCALE;
    let cooked = pseudocooker::cook(
        &mesh,
        asset_name,
        false,
        MESH_CONVERSION_SCALE,
        &default_opts(),
    );
    let uasset_path = format!("Mods/Maps/{}/gen/{}.uasset", level_name, asset_name);
    pak.write_file(&uasset_path, true, &cooked.uasset)
        .expect("failed to write uasset to pak");
    let uexp_path = format!("Mods/Maps/{}/gen/{}.uexp", level_name, asset_name);
    pak.write_file(&uexp_path, true, &cooked.uexp)
        .expect("failed to write uexp to pak");
    let abs_path_no_ext = format!("/Game/Mods/Maps/{}/gen/{}", level_name, asset_name);
    Some((abs_path_no_ext, origin))
}

fn add_static_mesh_import<C: Read + Seek>(
    umap: &mut unreal_asset::Asset<C>,
    path: &str,
) -> PackageIndex {
    let last_slash_idx = path.rfind('/').expect(&format!(
        "invalid input to add_static_mesh_import: \"{}\"",
        path
    ));
    // Hardcode to find SM_ExampleBox and use it as the reference import.
    let idx1 = find_import(umap, "Package", "/Game/Mods/Maps/mise/SM_ExampleBox").unwrap();
    let idx2 = find_import(umap, "StaticMesh", "SM_ExampleBox").unwrap();

    // Clone 'Package' import (contains actual absolute path to asset in pak)
    let mut import1c = umap.get_import(idx1).unwrap().clone();
    import1c.object_name = umap.add_fname(path);
    let idx1c = umap.add_import(import1c);

    // Clone 'StaticMesh' import, which should reference 'Package' import
    let mut import2c = umap.get_import(idx2).unwrap().clone();
    let basename = &path[last_slash_idx + 1..];
    import2c.object_name = umap.add_fname(basename);
    import2c.outer_index = idx1c;

    // Return the index of the newly-added import.
    umap.add_import(import2c)
}

// Collects `idx` together with every export it owns, directly or transitively
// (i.e. every export whose outer index chain leads back to `idx`). This mirrors
// Unreal's subobject model, where e.g. an actor's components are separate exports
// outered to the actor, and is exactly the set of exports that must be duplicated
// together to deep clone `idx` as a self-contained subtree.
fn collect_owned_exports<C: Read + Seek>(
    asset: &unreal_asset::Asset<C>,
    idx: PackageIndex,
) -> Vec<PackageIndex> {
    let mut owned = vec![idx];
    let mut i = 0;
    while i < owned.len() {
        let outer = owned[i];
        for j in 0..asset.asset_data.exports.len() {
            let candidate = PackageIndex::new((j + 1) as i32);
            if owned.contains(&candidate) {
                continue;
            }
            if asset.get_export(candidate).unwrap().get_base_export().outer_index == outer {
                owned.push(candidate);
            }
        }
        i += 1;
    }
    owned
}

fn remap_package_index(index: &mut PackageIndex, old_to_new: &HashMap<PackageIndex, PackageIndex>) {
    if let Some(&new_index) = old_to_new.get(index) {
        *index = new_index;
    }
}

// Rewrites any PackageIndex found in `prop` (recursing into arrays, sets, maps and
// structs) according to `old_to_new`. References outside `old_to_new` (e.g. imports,
// or exports outside the cloned subtree) are left untouched.
fn remap_property(
    prop: &mut unreal_asset::properties::Property,
    old_to_new: &HashMap<PackageIndex, PackageIndex>,
) {
    use unreal_asset::properties::Property;
    match prop {
        Property::ObjectProperty(p) => remap_package_index(&mut p.value, old_to_new),
        Property::ArrayProperty(p) => {
            for item in &mut p.value {
                remap_property(item, old_to_new);
            }
        }
        Property::SetProperty(p) => {
            for item in &mut p.value.value {
                remap_property(item, old_to_new);
            }
            for item in &mut p.removed_items.value {
                remap_property(item, old_to_new);
            }
        }
        Property::MapProperty(p) => {
            for value in p.value.values_mut() {
                remap_property(value, old_to_new);
            }
        }
        Property::StructProperty(p) => {
            for item in &mut p.value {
                remap_property(item, old_to_new);
            }
        }
        _ => {}
    }
}

// Deep-clones the export at `idx` together with every export it owns (see
// `collect_owned_exports`), e.g. cloning an actor also clones its components. All
// cross references among the cloned subtree are rewritten to point at the new
// clones: outer/class/super/template indices, the four X_before_Y_dependencies
// lists, and object properties (including ones nested in arrays/sets/maps/structs).
// References to anything outside the cloned subtree (imports, or exports not owned
// by `idx`, such as the level the export lives in) are left pointing at the
// originals, so the clone is added alongside the original rather than replacing it.
fn deep_clone_export<C: Read + Seek>(
    umap: &mut unreal_asset::Asset<C>,
    idx: PackageIndex,
) -> PackageIndex {
    let old_indices = collect_owned_exports(umap, idx);

    let mut old_to_new = HashMap::new();
    for &old_idx in &old_indices {
        let new_idx = clone_export(umap, old_idx).expect("failed to clone export");
        old_to_new.insert(old_idx, new_idx);
    }

    for &old_idx in &old_indices {
        let new_idx = old_to_new[&old_idx];
        let export = umap.get_export_mut(new_idx).unwrap();
        let base = export.get_base_export_mut();
        remap_package_index(&mut base.class_index, &old_to_new);
        remap_package_index(&mut base.super_index, &old_to_new);
        remap_package_index(&mut base.template_index, &old_to_new);
        remap_package_index(&mut base.outer_index, &old_to_new);
        for dep in &mut base.serialization_before_serialization_dependencies {
            remap_package_index(dep, &old_to_new);
        }
        for dep in &mut base.create_before_serialization_dependencies {
            remap_package_index(dep, &old_to_new);
        }
        for dep in &mut base.serialization_before_create_dependencies {
            remap_package_index(dep, &old_to_new);
        }
        for dep in &mut base.create_before_create_dependencies {
            remap_package_index(dep, &old_to_new);
        }
        if let Some(normal) = export.get_normal_export_mut() {
            for prop in &mut normal.properties {
                remap_property(prop, &old_to_new);
            }
        }
    }

    old_to_new[&idx]
}

fn add_hazard_actor<C: Read + Seek>(
    umap: &mut unreal_asset::Asset<C>,
    _import_idx: PackageIndex,
    _origin: DVec3,
) {
    // Hardcode to find BP_Hazard_C and use it as the reference export.
    let idx = find_export(&umap, &vec![with_name("BP_Hazard_C")]).unwrap();
    let idx = deep_clone_export(umap, idx);
    add_actor_to_level(umap, idx);

    // TODO put import_idx and origin in deep cloned export
}

/// Get a mutable reference to the export referenced by the ObjectProperty with name |object_name|
/// on the export at index |idx|.
fn get_linked_export_mut<'a, C: Read + Seek>(
    umap: &'a mut unreal_asset::Asset<C>,
    idx: PackageIndex,
    object_name: &str,
) -> Option<&'a mut unreal_asset::Export<PackageIndex>> {
    let export = umap.get_export(idx)?;
    let prop = find_obj_property(export, object_name)?;
    assert!(prop.value.index > 0); // must be export
    umap.get_export_mut(prop.value)
}

fn set_obj_property(
    export: &mut unreal_asset::Export<PackageIndex>,
    object_name: &str,
    idx: PackageIndex,
) {
    let base_export = export.get_base_export_mut();
    let export_name = base_export.object_name.get_owned_content();
    base_export
        .create_before_serialization_dependencies
        .push(idx);
    // this error message should really be in the function.
    let prop = find_obj_property_mut(export, object_name).expect(&format!(
        "couldn't find object property \"{}\" in {}",
        object_name, export_name
    ));
    prop.value = idx;
}

fn set_location(export: &mut unreal_asset::Export<PackageIndex>, location: DVec3) {
    let prop = find_vec_property_mut(export, "RelativeLocation")
        .expect("couldn't find RelativeLocation property");
    prop.value.x.0 = location.x;
    prop.value.y.0 = location.y;
    prop.value.z.0 = location.z;
}

fn add_static_mesh_actor<C: Read + Seek>(
    umap: &mut unreal_asset::Asset<C>,
    import_idx: PackageIndex,
    origin: DVec3,
) {
    // Hardcode to find StaticMeshComponent0 and use it as the reference export.
    let idx1 = find_export(
        &umap,
        &vec![
            with_name("StaticMeshComponent0"),
            with_import("StaticMesh", "SM_ExampleBox"),
        ],
    )
    .unwrap();
    let export1 = umap.get_export_mut(idx1).unwrap();

    // Find the parent (a StaticMeshActor) and deep clone it.
    let idx2 = export1.get_base_export().outer_index;
    let idx3 = deep_clone_export(umap, idx2);
    add_actor_to_level(umap, idx3);

    // Find the cloned StaticMeshComponent0 and redirect it to the new import.
    let export4 = get_linked_export_mut(umap, idx3, "StaticMeshComponent").unwrap();
    set_obj_property(export4, "StaticMesh", import_idx);
    set_location(export4, origin);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    assert_eq!(args.len(), 3, "usage: pseudochef IN_MAP OUT_PAK");

    let map_contents = read_to_string(&args[1]).expect("failed to read map file");

    let (_, ast) = parse_map(&map_contents).expect("failed to parse map file");

    let mut umap = unreal_asset::Asset::new(
        std::io::Cursor::new(MISE_UMAP),
        Some(std::io::Cursor::new(MISE_UEXP)),
        unreal_asset::engine_version::EngineVersion::VER_UE5_1,
        None,
    )
    .expect("failed to parse umap");

    let mut pak = repak::PakBuilder::new()
        .compression(vec![repak::Compression::Zlib])
        .writer(
            File::create(&args[2]).expect("failed to open pak file for writing"),
            repak::Version::V11,
            "../../../pseudoregalia/Content/".to_string(),
            None,
        );

    let start = Instant::now();
    for ent in ast.0 {
        for prop in ent.properties.0 {
            if prop.key == "classname" {
                if prop.value == "worldspawn" {
                    for (i, brush) in ent.brushes.0.iter().enumerate() {
                        let name = format!("WorldBrush{}", i);
                        let (abs_path, origin) =
                            pak_add_brush(&mut pak, brush, "slop", &name).unwrap();
                        let idx = add_static_mesh_import(&mut umap, &abs_path);
                        add_static_mesh_actor(&mut umap, idx, origin);
                    }
                }
                if prop.value == "trigger_hazard" {
                    for (i, brush) in ent.brushes.0.iter().enumerate() {
                        // make this counter global
                        let name = format!("HazardBrush{}", i);
                        let (abs_path, origin) =
                            pak_add_brush(&mut pak, brush, "slop", &name).unwrap();
                        let idx = add_static_mesh_import(&mut umap, &abs_path);
                        add_hazard_actor(&mut umap, idx, origin);
                    }
                }
            }
        }
    }
    let elapsed = start.elapsed();
    println!("mesh generation completed in {}ms", elapsed.as_millis());

    // rename level export (for swag only; seemingly inconsequential)
    {
        let fname = umap.add_fname("pseudochef_slop");
        let idx = find_export(&umap, &vec![with_name("mise")]).expect("couldn't find mise");
        let export = umap.get_export_mut(idx).unwrap();
        export.get_base_export_mut().object_name = fname;
    }

    let mut final_umap = std::io::Cursor::new(vec![]);
    let mut final_uexp = std::io::Cursor::new(vec![]);
    umap.write_data(&mut final_umap, Some(&mut final_uexp))
        .expect("failed to serialize umap");

    //std::fs::write("slop.umap", final_umap.get_ref()).unwrap();
    //std::fs::write("slop.uexp", final_uexp.get_ref()).unwrap();

    // TODO also rename mise export
    pak.write_file("Mods/Maps/slop.umap", true, final_umap.get_ref())
        .expect("failed to write umap to pak");
    pak.write_file("Mods/Maps/slop.uexp", true, final_uexp.get_ref())
        .expect("failed to write uexp to pak");

    for (name, bytes) in MISE_FILES {
        let path = format!("Mods/Maps/mise/{}", name);
        pak.write_file(&path, true, bytes)
            .expect(&format!("failed to write {} to pak", name));
    }

    let mut writer = pak.write_index().expect("failed to write pak file");
    let bytes_written = writer
        .stream_position()
        .expect("failed to seek in pak file");
    println!(
        "wrote {} to \"{}\"",
        humansize::format_size(bytes_written, humansize::DECIMAL),
        args[2]
    );
}

#[cfg(test)]
mod deep_clone_export_tests {
    use super::*;
    use std::collections::HashSet;
    use unreal_asset::properties::Property;

    fn load_umap() -> unreal_asset::Asset<std::io::Cursor<&'static [u8]>> {
        unreal_asset::Asset::new(
            std::io::Cursor::new(MISE_UMAP),
            Some(std::io::Cursor::new(MISE_UEXP)),
            unreal_asset::engine_version::EngineVersion::VER_UE5_1,
            None,
        )
        .expect("failed to parse umap")
    }

    // Collects every PackageIndex referenced by `props`, in the same traversal
    // order used by `remap_property`, so a before/after property tree can be
    // compared reference-by-reference.
    fn collect_property_refs(props: &[Property]) -> Vec<PackageIndex> {
        let mut refs = Vec::new();
        fn visit(prop: &Property, refs: &mut Vec<PackageIndex>) {
            match prop {
                Property::ObjectProperty(p) => refs.push(p.value),
                Property::ArrayProperty(p) => {
                    for item in &p.value {
                        visit(item, refs);
                    }
                }
                Property::SetProperty(p) => {
                    for item in &p.value.value {
                        visit(item, refs);
                    }
                    for item in &p.removed_items.value {
                        visit(item, refs);
                    }
                }
                Property::MapProperty(p) => {
                    for value in p.value.values() {
                        visit(value, refs);
                    }
                }
                Property::StructProperty(p) => {
                    for item in &p.value {
                        visit(item, refs);
                    }
                }
                _ => {}
            }
        }
        for prop in props {
            visit(prop, &mut refs);
        }
        refs
    }

    // Applies the same remap rule `deep_clone_export` uses: indices inside the
    // cloned subtree follow `old_to_new`, everything else (imports, or exports
    // outside the subtree, like the owning level) stays as-is.
    fn expected_remap(
        old: PackageIndex,
        old_to_new: &HashMap<PackageIndex, PackageIndex>,
    ) -> PackageIndex {
        old_to_new.get(&old).copied().unwrap_or(old)
    }

    // Deep-clones `object_name` in `umap` and checks that:
    //  - exactly one new export is added per export in the original's owned
    //    subtree (the export itself plus every export outered to it),
    //  - every clone's base fields (outer/class/super/template index and the
    //    four dependency lists) and object properties point at the *other*
    //    clones in the new subtree wherever the original pointed within its own
    //    subtree, and are left unchanged (still pointing at imports / exports
    //    outside the subtree) everywhere else,
    //  - the clones get fresh, distinct names,
    //  - the original subtree is left completely untouched.
    fn assert_deep_clone_is_self_contained(object_name: &str, expected_new_export_count: usize) {
        let mut umap = load_umap();
        let root = find_export(&umap, &vec![with_name(object_name)]).unwrap();

        let old_subtree = collect_owned_exports(&umap, root);
        assert_eq!(
            old_subtree.len(),
            expected_new_export_count,
            "unexpected owned-subtree size for {}",
            object_name
        );

        // Snapshot the original subtree so we can confirm it's untouched by the clone.
        let originals: Vec<_> = old_subtree
            .iter()
            .map(|&idx| umap.get_export(idx).unwrap().clone())
            .collect();

        let export_count_before = umap.asset_data.exports.len();
        let new_root = deep_clone_export(&mut umap, root);
        let export_count_after = umap.asset_data.exports.len();

        assert_eq!(
            export_count_after,
            export_count_before + expected_new_export_count,
            "deep_clone_export({}) should add exactly {} new exports",
            object_name,
            expected_new_export_count
        );

        let new_subtree = collect_owned_exports(&umap, new_root);
        assert_eq!(
            new_subtree.len(),
            old_subtree.len(),
            "cloned subtree shape doesn't match original"
        );

        let old_set: HashSet<PackageIndex> = old_subtree.iter().copied().collect();
        let new_set: HashSet<PackageIndex> = new_subtree.iter().copied().collect();
        assert!(
            old_set.is_disjoint(&new_set),
            "cloned subtree must not overlap the original"
        );
        assert!(
            new_subtree.iter().all(|idx| idx.index > export_count_before as i32),
            "every cloned export should be newly appended"
        );

        let old_to_new: HashMap<PackageIndex, PackageIndex> =
            old_subtree.iter().copied().zip(new_subtree.iter().copied()).collect();

        for (i, (&old_idx, &new_idx)) in old_subtree.iter().zip(new_subtree.iter()).enumerate() {
            let old_export = &originals[i];
            let old_base = old_export.get_base_export();
            let new_export = umap.get_export(new_idx).unwrap();
            let new_base = new_export.get_base_export();

            // Fresh, distinct name.
            assert_ne!(new_base.object_name.get_owned_content(), old_base.object_name.get_owned_content());
            assert!(
                new_base.object_name.get_owned_content().contains(&old_base.object_name.get_owned_content())
            );

            // outer/class/super/template indices follow the same remap rule.
            assert_eq!(new_base.class_index, expected_remap(old_base.class_index, &old_to_new));
            assert_eq!(new_base.super_index, expected_remap(old_base.super_index, &old_to_new));
            assert_eq!(new_base.template_index, expected_remap(old_base.template_index, &old_to_new));
            assert_eq!(new_base.outer_index, expected_remap(old_base.outer_index, &old_to_new));

            // The four X_before_Y_dependencies fields, remapped entry-by-entry.
            let old_deps = [
                &old_base.serialization_before_serialization_dependencies,
                &old_base.create_before_serialization_dependencies,
                &old_base.serialization_before_create_dependencies,
                &old_base.create_before_create_dependencies,
            ];
            let new_deps = [
                &new_base.serialization_before_serialization_dependencies,
                &new_base.create_before_serialization_dependencies,
                &new_base.serialization_before_create_dependencies,
                &new_base.create_before_create_dependencies,
            ];
            for (old_dep_list, new_dep_list) in old_deps.iter().zip(new_deps.iter()) {
                let expected: Vec<PackageIndex> = old_dep_list
                    .iter()
                    .map(|&d| expected_remap(d, &old_to_new))
                    .collect();
                assert_eq!(**new_dep_list, expected, "at export {:?}", old_idx);
            }

            // Object properties, remapped reference-by-reference.
            let old_refs = old_export
                .get_normal_export()
                .map(|n| collect_property_refs(&n.properties))
                .unwrap_or_default();
            let new_refs = new_export
                .get_normal_export()
                .map(|n| collect_property_refs(&n.properties))
                .unwrap_or_default();
            let expected_refs: Vec<PackageIndex> =
                old_refs.iter().map(|&r| expected_remap(r, &old_to_new)).collect();
            assert_eq!(new_refs, expected_refs, "property refs at export {:?}", old_idx);
        }

        // The original subtree must be completely unmodified.
        for (i, &old_idx) in old_subtree.iter().enumerate() {
            assert_eq!(umap.get_export(old_idx).unwrap(), &originals[i]);
        }
    }

    // BP_Hazard_C is a small actor: a DefaultSceneRoot (SceneComponent) plus a
    // StaticMeshComponent attached to it, both outered directly to the actor.
    // Deep-cloning it must therefore add exactly 3 exports: the actor itself and
    // its two owned components.
    #[test]
    fn deep_clone_bp_hazard_c() {
        assert_deep_clone_is_self_contained("BP_Hazard_C", 3);
    }

    // BP_JumpBubble_C owns a DefaultSceneRoot, a SphereComponent and a
    // StaticMeshComponent, so cloning it adds 4 exports.
    #[test]
    fn deep_clone_bp_jump_bubble_c() {
        assert_deep_clone_is_self_contained("BP_JumpBubble_C", 4);
    }

    // BP_SavePoint_C owns a DefaultSceneRoot, a SphereComponent, a
    // StaticMeshComponent, a BoxComponent, and a nested BP_HpHitable_C child
    // actor, so cloning it adds 6 exports.
    #[test]
    fn deep_clone_bp_save_point_c() {
        assert_deep_clone_is_self_contained("BP_SavePoint_C", 6);
    }

    // Cloning twice must not collide: each call produces its own independent,
    // uniquely-named subtree.
    #[test]
    fn deep_clone_twice_is_independent() {
        let mut umap = load_umap();
        let root = find_export(&umap, &vec![with_name("BP_Hazard_C")]).unwrap();

        let clone_a = deep_clone_export(&mut umap, root);
        let clone_b = deep_clone_export(&mut umap, root);
        assert_ne!(clone_a, clone_b);

        // Content (base string) is expected to match ("pseudochef_BP_Hazard_C" for
        // both); uniqueness comes from the FName instance number instead.
        let name_a = umap.get_export(clone_a).unwrap().get_base_export().object_name.clone();
        let name_b = umap.get_export(clone_b).unwrap().get_base_export().object_name.clone();
        assert_eq!(name_a.get_owned_content(), name_b.get_owned_content());
        assert_ne!(name_a.get_number(), name_b.get_number());

        let subtree_a: HashSet<_> = collect_owned_exports(&umap, clone_a).into_iter().collect();
        let subtree_b: HashSet<_> = collect_owned_exports(&umap, clone_b).into_iter().collect();
        assert!(subtree_a.is_disjoint(&subtree_b));
    }
}

