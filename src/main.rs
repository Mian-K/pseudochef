use shalrath::parser::repr::parse_map;
use std::fs::{File, read_to_string};
use unreal_asset::exports::ExportBaseTrait;
use unreal_asset::exports::ExportNormalTrait;

const MISE_UMAP: &[u8] = include_bytes!("mise.umap");
const MISE_UEXP: &[u8] = include_bytes!("mise.uexp");

fn main() {
    let args: Vec<String> = std::env::args().collect();

    assert_eq!(args.len(), 3, "usage: pseudochef IN_MAP OUT_PAK");

    let map_contents = read_to_string(&args[1]).expect("failed to read map file");

    let (_, ast) = parse_map(&map_contents).expect("failed to parse map file");
    _ = ast;

    let mut pak = repak::PakBuilder::new().writer(
        File::create(&args[2]).unwrap(),
        repak::Version::V11,
        "../../../pseudoregalia/Content/".to_string(),
        None,
    );

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
            .get_import(unreal_asset::types::PackageIndex::new(-35))
            .unwrap();
        let mut new_import = import.clone();
        new_import.object_name = umap.add_fname("/Game/Mods/Maps/MyLevel/SM_Pedestal");
        umap.add_import(new_import);
    }

    // clone StaticMesh import
    {
        let import = umap
            .get_import(unreal_asset::types::PackageIndex::new(-45))
            .unwrap();
        let mut new_import = import.clone();
        new_import.outer_index = unreal_asset::types::PackageIndex::new(-51);
        new_import.object_name = umap.add_fname("SM_Pedestal");
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
                obj_prop.value = unreal_asset::types::PackageIndex::new(-52);
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

    // Print all exports
    for i in 0..umap.asset_data.exports.len() as i32 {
        let export = umap
            .get_export(unreal_asset::types::PackageIndex::new(i + 1))
            .unwrap();
        let object_name = &export.get_base_export().object_name;
        println!("{}: {}", i + 1, fname_to_str(object_name));
        let props = export.get_normal_export().unwrap().properties.clone();
        for prop in props {
            if let unreal_asset::properties::Property::ObjectProperty(obj_prop) = prop {
                //println!("  {}: {}", obj_prop.value.index, obj_prop.name.get_owned_content());
            }
        }
    }

    // TODO generate obj files and write them to the umap and the pak

    let mut final_umap = std::io::Cursor::new(vec![]);
    let mut final_uexp = std::io::Cursor::new(vec![]);
    umap.write_data(&mut final_umap, Some(&mut final_uexp))
        .expect("failed to serialize umap");

    // TODO temporary for debugging
    std::fs::write("slop.umap", final_umap.get_ref()).expect("failed to write umap file");
    std::fs::write("slop.uexp", final_uexp.get_ref()).expect("failed to write uexp file");

    // TODO also rename mise export
    pak.write_file("Mods/Maps/slop.umap", true, final_umap.get_ref())
        .expect("failed to write umap to pak");
    pak.write_file("Mods/Maps/slop.uexp", true, final_uexp.get_ref())
        .expect("failed to write uexp to pak");
    pak.write_index().unwrap();
}

fn fname_to_str(fname: &unreal_asset::types::FName) -> String {
    let n = fname.get_number();
    if n > 0 {
        format!("{}_{}", fname.get_owned_content(), n - 1)
    } else {
        format!("{}", fname.get_owned_content())
    }
}
