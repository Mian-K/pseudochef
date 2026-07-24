use shalrath::parser::repr::parse_map;
use std::fs::{File, read_to_string};
use unreal_asset::exports::ExportBaseTrait;
use unreal_asset::exports::ExportNormalTrait;

mod brush_to_mesh;
// Not called from the pipeline yet; used ad hoc (e.g. from a debugger or a
// scratch `main`) to dump a `MeshInput` for visual inspection in Blender.
#[allow(dead_code)]
mod obj_export;
use brush_to_mesh::convert_to_mesh;

const MISE_UMAP: &[u8] = include_bytes!("mise.umap");
const MISE_UEXP: &[u8] = include_bytes!("mise.uexp");

fn default_opts() -> pseudocooker::CookOptions {
    pseudocooker::CookOptions {
        body_setup_guid: None,
        lighting_guid: None,
        package_guid: None,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    assert_eq!(args.len(), 3, "usage: pseudochef IN_MAP OUT_PAK");

    let map_contents = read_to_string(&args[1]).expect("failed to read map file");

    let (_, ast) = parse_map(&map_contents).expect("failed to parse map file");

    let mut pak = repak::PakBuilder::new().writer(
        File::create(&args[2]).unwrap(),
        repak::Version::V11,
        "../../../pseudoregalia/Content/".to_string(),
        None,
    );

    for (i, ent) in ast.0.iter().enumerate() {
        for brush in &ent.brushes.0 {
            let mesh = convert_to_mesh(&brush);
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
            .get_import(unreal_asset::types::PackageIndex::new(-39))
            .unwrap();
        let mut new_import = import.clone();
        new_import.object_name = umap.add_fname("/Game/Mods/Maps/slop/tb0");
        umap.add_import(new_import);
    }

    // clone StaticMesh import
    {
        let import = umap
            .get_import(unreal_asset::types::PackageIndex::new(-49))
            .unwrap();
        let mut new_import = import.clone();
        new_import.outer_index = unreal_asset::types::PackageIndex::new(-55);
        new_import.object_name = umap.add_fname("tb0");
        umap.add_import(new_import);
    }

    // to clone a static mesh actor export, we must:
    // - clone the StaticMeshActor
    // - set create_before_serialization_dependencies to the StaticMeshComponent0
    // - clone the StaticMeshComponent0
    // - set create_before_create_dependencies to the StaticMeshActor
    // to make it point to a different static mesh asset, we must:
    // - set the value of the ObjectProperty of the StaticMeshComponent0 to point to the package
    //   index of the import
    /*
    {
        let export = umap
            .get_export(unreal_asset::types::PackageIndex::new(19))
            .unwrap();
        let mut new_export = export.clone();
        let props = &mut new_export.get_normal_export_mut().unwrap().properties;
        for prop in props {
            if let unreal_asset::properties::Property::StructProperty(struct_prop) = prop {
                for prop in &mut struct_prop.value {
                    if let unreal_asset::properties::Property::VectorProperty(vec_prop) = prop {
                        vec_prop.value.x = ordered_float::OrderedFloat(400.0);
                    }
                }
            }
        }
        umap.asset_data.exports.push(new_export);
    }
    */

    // edit the static mesh asset of a StaticMeshActor
    {
        let props = &mut umap
            .get_export_mut(unreal_asset::types::PackageIndex::new(19))
            .unwrap()
            .get_normal_export_mut()
            .unwrap()
            .properties;
        for prop in props {
            if let unreal_asset::properties::Property::ObjectProperty(obj_prop) = prop {
                obj_prop.value = unreal_asset::types::PackageIndex::new(-56);
                break;
            }
        }
    }

    // rename level export (for swag only; seemingly inconsequential)
    {
        let fname = umap.add_fname("pseudochef_slop");
        umap.get_export_mut(unreal_asset::types::PackageIndex::new(20))
            .unwrap()
            .get_base_export_mut()
            .object_name = fname;
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
