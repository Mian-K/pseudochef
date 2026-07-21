use shalrath::parser::repr::parse_map;
use std::fs::{File, read_to_string};

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

    let umap = unreal_asset::Asset::new(
        std::io::Cursor::new(MISE_UMAP),
        Some(std::io::Cursor::new(MISE_UEXP)),
        unreal_asset::engine_version::EngineVersion::VER_UE5_1,
        None,
    )
    .expect("failed to parse umap");

    // TODO generate obj files and write them to the umap and the pak

    let mut final_umap = std::io::Cursor::new(vec![]);
    let mut final_uexp = std::io::Cursor::new(vec![]);
    umap.write_data(&mut final_umap, Some(&mut final_uexp))
        .expect("failed to serialize umap");

    pak.write_file("Mods/Maps/mise.umap", true, final_umap.get_ref())
        .expect("failed to write umap to pak");
    pak.write_file("Mods/Maps/mise.uexp", true, final_uexp.get_ref())
        .expect("failed to write uexp to pak");
    pak.write_index().unwrap();
}
