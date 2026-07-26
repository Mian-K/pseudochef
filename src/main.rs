use glam::DVec3;
use shalrath::parser::repr::parse_map;
use std::collections::HashMap;
use std::fs::{File, read_to_string};
use std::io::{Read, Seek, Write};
use std::sync::OnceLock;
use std::time::Instant;
use unreal_asset::exports::ExportBaseTrait;
use unreal_asset::exports::ExportNormalTrait;
use unreal_asset::types::PackageIndex;

mod brush_to_mesh;
use brush_to_mesh::{convert_to_mesh, tb_space_to_unreal_space};

mod unreal_asset_ext;
use unreal_asset_ext::{deep_clone_export, deep_delete_export};

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

fn find_export<C: Read + Seek>(
    asset: &unreal_asset::Asset<C>,
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
    let cooked = pseudocooker::cook(&mesh, asset_name);
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

fn add_player_start<C: Read + Seek>(umap: &mut unreal_asset::Asset<C>, origin: DVec3) {
    let idx = find_export(umap, &vec![with_name("PlayerStart")]).unwrap();
    let idx = deep_clone_export(umap, idx);
    add_actor_to_level(umap, idx);

    let root = get_linked_export_mut(umap, idx, "RootComponent").unwrap();
    set_location(root, origin);
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

fn lazy_find_original_static_mesh_actor<C: Read + Seek>(
    umap: &unreal_asset::Asset<C>,
) -> PackageIndex {
    static ORIGINAL_STATIC_MESH_ACTOR: OnceLock<PackageIndex> = OnceLock::new();
    *ORIGINAL_STATIC_MESH_ACTOR.get_or_init(|| {
        let idx = find_export(
            &umap,
            &vec![
                with_name("StaticMeshComponent0"),
                with_import("StaticMesh", "SM_ExampleBox"),
            ],
        )
        .unwrap();
        let export = umap.get_export(idx).unwrap();

        // Return the parent (a StaticMeshActor).
        export.get_base_export().outer_index
    })
}

fn add_static_mesh_actor<C: Read + Seek>(
    umap: &mut unreal_asset::Asset<C>,
    import_idx: PackageIndex,
    origin: DVec3,
) {
    let idx2 = lazy_find_original_static_mesh_actor(umap);
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
        let props: HashMap<String, String> = ent
            .properties
            .0
            .into_iter()
            .map(|p| (p.key, p.value))
            .collect();
        match props["classname"].as_ref() {
            "worldspawn" => {
                for brush in &ent.brushes.0 {
                    num_world_brushes += 1;
                    let name = format!("WorldBrush{}", num_world_brushes);
                    let (abs_path, origin) = pak_add_brush(&mut pak, brush, "slop", &name).unwrap();
                    let idx = add_static_mesh_import(&mut umap, &abs_path);
                    add_static_mesh_actor(&mut umap, idx, origin);
                }
            }
            "trigger_hazard" => {
                for brush in &ent.brushes.0 {
                    num_hazard_brushes += 1;
                    let name = format!("HazardBrush{}", num_hazard_brushes);
                    let (abs_path, origin) = pak_add_brush(&mut pak, brush, "slop", &name).unwrap();
                    let idx = add_static_mesh_import(&mut umap, &abs_path);
                    add_hazard_actor(&mut umap, idx, origin);
                }
            }
            "info_player_start" => {
                let numbers: Vec<f64> = props["origin"].split_whitespace().map(|n| n.parse().unwrap()).collect();
                let origin = DVec3::from_slice(&numbers);
                let origin = tb_space_to_unreal_space(origin);
                add_player_start(&mut umap, origin);
            }
            _ => {}
        };
    }
    let elapsed = start.elapsed();
    println!("Mesh generation completed in {}ms", elapsed.as_millis());

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
            find_export(&umap, &vec![with_name("PlayerStart")]).unwrap(),
            lazy_find_original_static_mesh_actor(&umap),
        ];

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
