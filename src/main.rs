use shalrath::parser::repr::parse_map;
use std::env;
use std::fs;
use std::fs::File;

fn main() {
    let args: Vec<String> = env::args().collect();

    assert_eq!(args.len(), 2, "please pass in a map file");

    let map_contents = fs::read_to_string(&args[1]).expect("failed to read map file");

    let (_, ast) = parse_map(&map_contents).expect("failed to parse map file");
    _ = ast;

    //println!("{:#?}", ast);

    let output = "out.pak";
    let mut pak = repak::PakBuilder::new().writer(
        File::create(&output).unwrap(),
        repak::Version::V11,
        "../../../pseudoregalia/Content/".to_string(),
        None,
    );
    // TODO generate obj files
    // TODO include prefab asset files
    let umap = "".to_string();
    pak.write_file("Mods/Maps/MyMap.umap", true, &umap).unwrap();
    pak.write_index().unwrap();
}
