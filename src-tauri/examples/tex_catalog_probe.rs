//! How much of the game's texture set can we actually NAME?
//!
//! `cargo run --example tex_catalog_probe -- "<game root>"`
//!
//! The ASET table only stores 32-bit hashes, so a browsable texture list is only possible
//! for textures whose name we know. This measures the coverage of the bundled name list
//! against the real WAD, and reports what's left unnamed.

use std::collections::{HashMap, HashSet};

use mercs2_formats::ffcs::load_ffcs_archive;
use mercs2_formats::hash::pandemic_hash_m2;
use mercs2_formats::types::TYPE_ID_TEXTURE;

const NAMES: &str = include_str!("../data/asset_names.txt");

fn main() {
    let game = std::env::args().nth(1).expect("usage: tex_catalog_probe <game root>");
    let wad = std::path::Path::new(&game).join("data").join("vz.wad");

    let mut f = std::fs::File::open(&wad).expect("open vz.wad");
    let size = f.metadata().unwrap().len();
    let archive = load_ffcs_archive(&mut f, size).expect("ffcs");

    // Every texture hash in the WAD (primary rows).
    let tex_hashes: HashSet<u32> = archive
        .aset
        .iter()
        .filter(|e| e.is_primary() && e.type_id == TYPE_ID_TEXTURE)
        .map(|e| e.asset_hash)
        .collect();

    // Name -> hash, for every name we know.
    let mut named: HashMap<u32, &str> = HashMap::new();
    for n in NAMES.lines().filter(|l| !l.trim().is_empty()) {
        named.insert(pandemic_hash_m2(n), n);
    }

    let matched: Vec<&str> = tex_hashes
        .iter()
        .filter_map(|h| named.get(h).copied())
        .collect();

    println!("textures in vz.wad (primary): {}", tex_hashes.len());
    println!("names in bundled list:        {}", named.len());
    println!(
        "textures we can NAME:         {}  ({:.1}%)",
        matched.len(),
        100.0 * matched.len() as f64 / tex_hashes.len() as f64
    );

    let mut sample: Vec<&str> = matched.clone();
    sample.sort_unstable();
    println!("\nsample:");
    for n in sample.iter().take(15) {
        println!("  {n}");
    }
}
