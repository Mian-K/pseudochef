use shalrath::parser::repr::parse_map;
use std::fs::{File, read_to_string};
use std::io::{Read, Seek};
use unreal_asset::exports::ExportBaseTrait;
use unreal_asset::exports::ExportNormalTrait;
use unreal_asset::reader::ArchiveTrait;

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
            &unreal_asset::exports::NormalExport<unreal_asset::types::PackageIndex>,
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
) -> Option<unreal_asset::types::PackageIndex> {
    let mut maybe_idx = None;
    for (i, export) in asset.asset_data.exports.iter().enumerate() {
        if let Some(normal_export) = export.get_normal_export() {
            if constraints.iter().all(|f| f(asset, normal_export)) {
                maybe_idx = Some(unreal_asset::types::PackageIndex::new((i + 1) as i32));
            }
        }
    }
    maybe_idx
}

fn find_vec_property_mut<'a>(
    export: &'a mut unreal_asset::Export<unreal_asset::types::PackageIndex>,
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
    export: &'a mut unreal_asset::Export<unreal_asset::types::PackageIndex>,
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

fn main() {
    let args: Vec<String> = std::env::args().collect();

    assert_eq!(args.len(), 3, "usage: pseudochef IN_MAP OUT_PAK");

    let map_contents = read_to_string(&args[1]).expect("failed to read map file");

    let (_, ast) = parse_map(&map_contents).expect("failed to parse map file");

    let mut pak = repak::PakBuilder::new().compression(vec![repak::Compression::Zlib]).writer(
        File::create(&args[2]).unwrap(),
        repak::Version::V11,
        "../../../pseudoregalia/Content/".to_string(),
        None,
    );

    for ent in ast.0 {
        for (i, brush) in ent.brushes.0.iter().enumerate() {
            let (mesh, _origin) = convert_to_mesh(&brush, FACE_VERTEX_SPACING);
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
        }
    }

    let mut umap = unreal_asset::Asset::new(
        std::io::Cursor::new(MISE_UMAP),
        Some(std::io::Cursor::new(MISE_UEXP)),
        unreal_asset::engine_version::EngineVersion::VER_UE5_1,
        None,
    )
    .expect("failed to parse umap");

    // clone Package import
    {
        let import = umap
            .get_import(unreal_asset::types::PackageIndex::new(-32))
            .unwrap();
        let mut new_import = import.clone();
        new_import.object_name = umap.add_fname("/Game/Mods/Maps/slop/tb0");
        umap.add_import(new_import);
    }

    // clone StaticMesh import
    {
        let import = umap
            .get_import(unreal_asset::types::PackageIndex::new(-40))
            .unwrap();
        let mut new_import = import.clone();
        new_import.outer_index = unreal_asset::types::PackageIndex::new(-46);
        new_import.object_name = umap.add_fname("tb0");
        umap.add_import(new_import);
    }

    {
        let idx = find_export(
            &umap,
            &vec![
                with_name("StaticMeshComponent0"),
                with_import("StaticMesh", "SM_ExampleBox"),
            ],
        )
        .expect("couldn't find StaticMeshComponent0 with StaticMesh property: SM_ExampleBox");
        {
            let export = umap.get_export_mut(idx).unwrap().clone();
            let idx2 = export.get_base_export().outer_index;
            let export2 = umap.get_export_mut(idx2).unwrap().clone();
            umap.asset_data.exports.push(export2);
            umap.asset_data.exports.push(export);
        }
        let idx3 =
            unreal_asset::types::PackageIndex::new((umap.asset_data.exports.len() - 1) as i32);
        let idx4 = unreal_asset::types::PackageIndex::new(umap.asset_data.exports.len() as i32);
        {
            // StaticMeshActor
            let fname = umap.add_fname_with_number("StaticMeshActor", 7);
            let export3 = umap.get_export_mut(idx3).unwrap();
            export3.get_base_export_mut().object_name = fname;
            export3
                .get_base_export_mut()
                .create_before_serialization_dependencies[0] = idx4;
        }
        {
            // StaticMeshComponent0
            let export4 = umap.get_export_mut(idx4).unwrap();
            export4.get_base_export_mut().outer_index = idx3;
            export4
                .get_base_export_mut()
                .create_before_create_dependencies[0] = idx3;
            export4
                .get_base_export_mut()
                .create_before_serialization_dependencies
                .push(unreal_asset::types::PackageIndex::new(-47));
            {
                let prop = find_obj_property_mut(export4, "StaticMesh")
                    .expect("couldn't find StaticMesh property");
                prop.value = unreal_asset::types::PackageIndex::new(-47);
            }
            {
                let prop = find_vec_property_mut(export4, "RelativeLocation")
                    .expect("couldn't find RelativeLocation property");
                prop.value.x.0 = 200.0;
            }
        }
        {
            let idx = find_export(&umap, &vec![with_name("PersistentLevel")])
                .expect("couldn't find PersistentLevel");
            let export = umap.get_export_mut(idx).unwrap();
            if let unreal_asset::Export::LevelExport(level_export) = export {
                level_export.actors.push(idx3);
            }
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
