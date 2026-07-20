use std::env;
use std::fs;
use shalrath::parser::repr::parse_map;

fn main() {
    let args: Vec<String> = env::args().collect();

    assert_eq!(args.len(), 2, "please pass in a map file");

    let map_contents = fs::read_to_string(&args[1]).expect("failed to read map file");

    let (_, ast) = parse_map(&map_contents).expect("failed to parse map file");

    println!("{:#?}", ast);
}
