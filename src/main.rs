use glam::DVec3;
use shalrath::parser::repr::parse_map;
use std::collections::HashSet;
use std::fs::{File, read_to_string};
use std::io::{Read, Seek, Write};
use std::time::Instant;
use unreal_asset::exports::ExportBaseTrait;
use unreal_asset::exports::ExportNormalTrait;
use unreal_asset::reader::ArchiveTrait;
use unreal_asset::types::PackageIndex;

mod brush_to_mesh;
use brush_to_mesh::convert_to_mesh;

mod deep_clone;
use deep_clone::{collect_owned_exports, deep_clone_export, remap_property};
use std::collections::HashMap;

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

macro_rules! debug_println {
    ($($arg:tt)*) => {
        #[cfg(debug_assertions)]
        println!($($arg)*);
    };
}

#[allow(dead_code)]
#[derive(Debug)]
struct Error {
    msg: String,
}

#[allow(dead_code)]
fn slice_to_string<T: std::fmt::Display>(v: &[T]) -> String {
    v.iter()
        .map(|i| i.to_string())
        .collect::<Vec<String>>()
        .join(", ")
}

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

fn remove_actors_from_level<C: Read + Seek>(
    asset: &mut unreal_asset::Asset<C>,
    to_remove: &HashSet<PackageIndex>,
) {
    let level_idx = find_export(asset, &vec![with_name("PersistentLevel")]).unwrap();
    let export = asset.get_export_mut(level_idx).unwrap();
    let unreal_asset::Export::LevelExport(level_export) = export else {
        panic!("PersistentLevel was not a LevelExport");
    };
    // Preserve order: the engine requires WorldSettings to stay at Actors[0]
    // (ULevel::PostLoad does WorldSettings = Cast<AWorldSettings>(Actors[0])).
    level_export.actors.retain(|idx| {
        if !to_remove.contains(idx) {
            return true;
        } else {
            debug_println!("Removed actor {} from PersistentLevel", idx);
            return false;
        }
    });
}

/// Turns the export subtree rooted at `root` into inert placeholder exports and removes deleted
/// exports from PersistentLevel.
fn deep_delete_export<C: Read + Seek>(umap: &mut unreal_asset::Asset<C>, root: PackageIndex) {
    let doomed = collect_owned_exports(umap, root);

    let class_idx = find_import(umap, "Class", "SceneComponent").unwrap();
    let cdo_idx = match find_import(umap, "SceneComponent", "Default__SceneComponent") {
        Some(idx) => idx,
        None => {
            // Clone an existing /Script/Engine CDO import as a pattern.
            let pattern = find_import(umap, "PlayerStart", "Default__PlayerStart").unwrap();
            let mut import = umap.get_import(pattern).unwrap().clone();
            import.class_name = umap.add_fname("SceneComponent");
            import.object_name = umap.add_fname("Default__SceneComponent");
            umap.add_import(import)
        }
    };

    let mut tombstone_number = 0;
    for &idx in &doomed {
        let export = umap.get_export(idx).unwrap();
        let old_name = export.get_base_export().object_name.get_owned_content();
        let new_name = format!("pseudochef_tombstone_{}", old_name);
        tombstone_number += 1;
        let name = umap.add_fname_with_number(&new_name, tombstone_number);
        let export = umap.get_export_mut(idx).unwrap();
        let normal = export
            .get_normal_export_mut()
            .expect("tombstone target must be a normal export");
        normal.properties.clear();
        // A SceneComponent's body beyond tagged properties: UObject's "serialize
        // guid" flag and UActorComponent's UCSModifiedProperties array, both zero.
        normal.extras = vec![0; 8];
        let base = export.get_base_export_mut();
        base.object_name = name;
        base.class_index = class_idx;
        base.super_index = PackageIndex::new(0);
        base.template_index = cdo_idx;
        base.serialization_before_serialization_dependencies.clear();
        base.create_before_serialization_dependencies.clear();
        base.serialization_before_create_dependencies = vec![class_idx, cdo_idx];
        // create_before_create_dependencies (the outer chain) stays valid as-is.
    }

    // Scrub surviving references to the doomed exports: drop them from
    // dependency lists and null out object/delegate properties.
    let doomed_set: HashSet<PackageIndex> = doomed.iter().copied().collect();
    let to_null: HashMap<PackageIndex, PackageIndex> = doomed
        .iter()
        .map(|&idx| (idx, PackageIndex::new(0)))
        .collect();
    for i in 0..umap.asset_data.exports.len() {
        let idx = PackageIndex::new((i + 1) as i32);
        if doomed_set.contains(&idx) {
            continue;
        }
        let export = &mut umap.asset_data.exports[i];
        let base = export.get_base_export_mut();
        base.serialization_before_serialization_dependencies
            .retain(|d| !doomed_set.contains(d));
        base.create_before_serialization_dependencies
            .retain(|d| !doomed_set.contains(d));
        base.serialization_before_create_dependencies
            .retain(|d| !doomed_set.contains(d));
        base.create_before_create_dependencies
            .retain(|d| !doomed_set.contains(d));
        if let Some(normal) = export.get_normal_export_mut() {
            for prop in &mut normal.properties {
                remap_property(prop, &to_null);
            }
        }
    }
    remove_actors_from_level(umap, &doomed_set);
    debug_println!("- deleted {} more dependent exports", tombstone_number - 1);
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

fn add_hazard_actor<C: Read + Seek>(
    umap: &mut unreal_asset::Asset<C>,
    import_idx: PackageIndex,
    origin: DVec3,
) {
    // Hardcoded to find the BP_Hazard_C and use it as the reference export.
    let idx = find_export(&umap, &vec![with_name("BP_Hazard_C")]).unwrap();
    let idx = deep_clone_export(umap, idx);
    add_actor_to_level(umap, idx);

    let export_sm = get_linked_export_mut(umap, idx, "StaticMesh").unwrap();
    set_obj_property(export_sm, "StaticMesh", import_idx);

    let export_root = get_linked_export_mut(umap, idx, "DefaultSceneRoot").unwrap();
    set_location(export_root, origin);
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

    let mut num_world_brushes = 0;
    let mut num_hazard_brushes = 0;
    let start = Instant::now();
    for ent in ast.0 {
        for prop in ent.properties.0 {
            if prop.key == "classname" {
                if prop.value == "worldspawn" {
                    for brush in &ent.brushes.0 {
                        num_world_brushes += 1;
                        let name = format!("WorldBrush{}", num_world_brushes);
                        let (abs_path, origin) =
                            pak_add_brush(&mut pak, brush, "slop", &name).unwrap();
                        let idx = add_static_mesh_import(&mut umap, &abs_path);
                        add_static_mesh_actor(&mut umap, idx, origin);
                    }
                }
                if prop.value == "trigger_hazard" {
                    for brush in &ent.brushes.0 {
                        num_hazard_brushes += 1;
                        let name = format!("HazardBrush{}", num_hazard_brushes);
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

    // Remove reference actors.
    {
        let idxs = vec![
            find_export(&umap, &vec![with_name("BP_Hazard_C")]).unwrap(),
            find_export(&umap, &vec![with_name("BP_SavePoint_C")]).unwrap(),
            find_export(&umap, &vec![with_name("BP_JumpBubble_C")]).unwrap(),
            find_export(&umap, &vec![with_name("BP_ExaminableGrave_C")]).unwrap(),
            find_export(
                &umap,
                &vec![with_name(
                    "ChildActor_GEN_VARIABLE_BP_ExamineTextPopup_C_CAT",
                )],
            )
            .unwrap(),
        ];

        debug_println!("Removing reference actors: {}", slice_to_string(&idxs));

        for idx in idxs {
            deep_delete_export(&mut umap, idx);
        }
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
mod tombstone_tests {
    use super::*;

    fn load_umap() -> unreal_asset::Asset<std::io::Cursor<&'static [u8]>> {
        unreal_asset::Asset::new(
            std::io::Cursor::new(MISE_UMAP),
            Some(std::io::Cursor::new(MISE_UEXP)),
            unreal_asset::engine_version::EngineVersion::VER_UE5_1,
            None,
        )
        .expect("failed to parse umap")
    }

    // Removing + tombstoning an actor must leave a package that unreal_asset can round-trip, with
    // the same export count (indices stable), no export left whose class or name identifies the
    // removed actor, and no surviving reference (level actor list, dependency lists, properties) to
    // the doomed subtree.
    #[test]
    fn tombstone_bp_jump_bubble_c_round_trips() {
        let mut umap = load_umap();
        let root = find_export(&umap, &vec![with_name("BP_JumpBubble_C")]).unwrap();
        let doomed = collect_owned_exports(&umap, root);
        assert_eq!(doomed.len(), 4);
        let export_count = umap.asset_data.exports.len();

        deep_delete_export(&mut umap, root);

        let mut umap_bytes = std::io::Cursor::new(vec![]);
        let mut uexp_bytes = std::io::Cursor::new(vec![]);
        umap.write_data(&mut umap_bytes, Some(&mut uexp_bytes))
            .expect("failed to serialize tombstoned umap");

        let reloaded = unreal_asset::Asset::new(
            std::io::Cursor::new(umap_bytes.into_inner()),
            Some(std::io::Cursor::new(uexp_bytes.into_inner())),
            unreal_asset::engine_version::EngineVersion::VER_UE5_1,
            None,
        )
        .expect("failed to re-parse tombstoned umap");

        assert_eq!(reloaded.asset_data.exports.len(), export_count);
        assert!(find_export(&reloaded, &vec![with_name("BP_JumpBubble_C")]).is_none());

        let scene_component_class = find_import(&mut umap, "Class", "SceneComponent").unwrap();
        for &idx in &doomed {
            let base = reloaded.get_export(idx).unwrap().get_base_export();
            assert_eq!(base.class_index, scene_component_class);
            assert!(
                base.object_name
                    .get_content(|content| content.starts_with("pseudochef_tombstone"))
            );
            let normal = reloaded
                .get_export(idx)
                .unwrap()
                .get_normal_export()
                .unwrap();
            assert!(normal.properties.is_empty());
        }

        // No surviving export may reference the doomed subtree.
        let doomed_set: std::collections::HashSet<PackageIndex> = doomed.iter().copied().collect();
        for (i, export) in reloaded.asset_data.exports.iter().enumerate() {
            let idx = PackageIndex::new((i + 1) as i32);
            if doomed_set.contains(&idx) {
                continue;
            }
            let base = export.get_base_export();
            for deps in [
                &base.serialization_before_serialization_dependencies,
                &base.create_before_serialization_dependencies,
                &base.serialization_before_create_dependencies,
                &base.create_before_create_dependencies,
            ] {
                assert!(deps.iter().all(|d| !doomed_set.contains(d)));
            }
            if let unreal_asset::Export::LevelExport(level) = export {
                assert!(level.actors.iter().all(|a| !doomed_set.contains(a)));
            }
        }
    }
}
