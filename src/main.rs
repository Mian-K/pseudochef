use shalrath::parser::repr::parse_map;
use std::fs::{File, read_to_string};
use std::io::{Read, Seek, Write};
use std::ops::Mul;
use std::time::{Duration, Instant};
use unreal_asset::exports::ExportBaseTrait;
use unreal_asset::exports::ExportNormalTrait;
use unreal_asset::reader::ArchiveTrait;
use unreal_asset::types::PackageIndex;

mod brush_to_mesh;
use brush_to_mesh::convert_to_mesh;

// For debugging purposes; may not be called
#[allow(dead_code)]
mod obj_export;

const MISE_UMAP: &[u8] = include_bytes!("mise.umap");
const MISE_UEXP: &[u8] = include_bytes!("mise.uexp");
// World-space (map-unit) spacing between generated interior face vertices;
// see `brush_to_mesh::convert_to_mesh`. Smaller values give smoother
// per-vertex lighting at the cost of more geometry.
const FACE_VERTEX_SPACING: f32 = 64.0;

fn default_opts() -> pseudocooker::CookOptions {
    pseudocooker::CookOptions {
        body_setup_guid: None,
        lighting_guid: None,
        package_guid: None,
    }
}

type UnrealExportConstraint<'a, C> = Box<
    dyn Fn(
            &unreal_asset::Asset<C>,
            &unreal_asset::exports::NormalExport<PackageIndex>,
        ) -> bool
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

fn add_actor<C: Read + Seek>(asset: &mut unreal_asset::Asset<C>, idx: PackageIndex) {
    let level_idx = find_export(asset, &vec![with_name("PersistentLevel")]).unwrap();
    let export = asset.get_export_mut(level_idx).unwrap();
    if let unreal_asset::Export::LevelExport(level_export) = export {
        level_export.actors.push(idx);
    } else {
        panic!();
    }
}

fn process_brush<C: Read + Seek, W: Write + Seek>(
    umap: &mut unreal_asset::Asset<C>,
    pak: &mut repak::PakWriter<W>,
    i: usize,
    brush: &shalrath::repr::Brush,
) {
    let (mesh, origin) = convert_to_mesh(&brush, FACE_VERTEX_SPACING);
    let mut origin = glam::DVec3::from(origin);
    origin *= 4.0;
    let name = format!("tb{}", i);
    let cooked = pseudocooker::cook(&mesh, &name, false, 4.0, &default_opts());
    pak.write_file(
        &format!("Mods/Maps/slop/{}.uasset", name),
        true,
        &cooked.uasset,
    )
    .expect("failed to write uasset to pak");
    pak.write_file(&format!("Mods/Maps/slop/{}.uexp", name), true, &cooked.uexp)
        .expect("failed to write uasset to pak");
    println!("cooked {}", name);

    let new_import_idx = {
        let idx1 = find_import(umap, "Package", "/Game/Mods/Maps/MyLevel/SM_ExampleBox").unwrap();
        let idx2 = find_import(umap, "StaticMesh", "SM_ExampleBox").unwrap();
        let mut import1c = umap.get_import(idx1).unwrap().clone();
        import1c.object_name = umap.add_fname(&format!("/Game/Mods/Maps/slop/{}", name));
        let idx1c = umap.add_import(import1c);
        let mut import2c = umap.get_import(idx2).unwrap().clone();
        import2c.object_name = umap.add_fname(&name);
        import2c.outer_index = idx1c;
        umap.add_import(import2c)
    };

    {
        let idx1 = find_export(
            &umap,
            &vec![
                with_name("StaticMeshComponent0"),
                with_import("StaticMesh", "SM_ExampleBox"),
            ],
        )
        .unwrap();
        let export1 = umap.get_export_mut(idx1).unwrap();
        // find parent StaticMeshActor
        let idx2 = export1.get_base_export().outer_index;
        let idx1c = clone_export(umap, idx1).unwrap();
        let idx2c = clone_export(umap, idx2).unwrap();
        add_actor(umap, idx2c);
        {
            // StaticMeshComponent0
            let export1c = umap.get_export_mut(idx1c).unwrap();
            let export1c_base = export1c.get_base_export_mut();
            // Point to new parent
            export1c_base.outer_index = idx2c;
            export1c_base.create_before_create_dependencies[0] = idx2c;
            // Point to new import
            export1c_base
                .create_before_serialization_dependencies
                .push(new_import_idx);
            {
                let prop = find_obj_property_mut(export1c, "StaticMesh")
                    .expect("couldn't find StaticMesh property");
                prop.value = new_import_idx;
            }
            // Set world position
            {
                let prop = find_vec_property_mut(export1c, "RelativeLocation")
                    .expect("couldn't find RelativeLocation property");
                prop.value.x.0 = origin.x;
                prop.value.y.0 = origin.y;
                prop.value.z.0 = origin.z;
            }
        }
        {
            // StaticMeshActor
            let export2c = umap.get_export_mut(idx2c).unwrap();
            // Point to new child
            export2c
                .get_base_export_mut()
                .create_before_serialization_dependencies[0] = idx1c;
        }
    }
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

    for ent in ast.0 {
        for (i, brush) in ent.brushes.0.iter().enumerate() {
            process_brush(&mut umap, &mut pak, i, brush);
        }
    }

    // rename level export (for swag only; seemingly inconsequential)
    {
        let fname = umap.add_fname("pseudochef_slop");
        let idx = find_export(&umap, &vec![with_name("mise")]).expect("couldn't find mise");
        let export = umap.get_export_mut(idx).unwrap();
        export.get_base_export_mut().object_name = fname;
    }

    // TODO generate obj files and write them to the umap and the pak

    let mut final_umap = std::io::Cursor::new(vec![]);
    let mut final_uexp = std::io::Cursor::new(vec![]);
    umap.write_data(&mut final_umap, Some(&mut final_uexp))
        .expect("failed to serialize umap");

    // TODO also rename mise export
    pak.write_file("Mods/Maps/slop.umap", true, final_umap.get_ref())
        .expect("failed to write umap to pak");
    pak.write_file("Mods/Maps/slop.uexp", true, final_uexp.get_ref())
        .expect("failed to write uexp to pak");
    pak.write_index().unwrap();
}
