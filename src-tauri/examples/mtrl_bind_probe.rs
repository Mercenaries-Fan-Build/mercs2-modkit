//! Why does a model declare textures that no draw group paints?
//!
//! `cargo run --release --example mtrl_bind_probe -- "<game root>" <model>...`
//!
//! Compares the MTRL material table (what `used_by` is derived from) against the materials
//! actually bound by a PRMT draw group in this container, across every SEGM tier.

use std::collections::HashSet;

use mercs2_formats::ffcs::load_ffcs_archive;
use mercs2_formats::hash::pandemic_hash_m2;
use mercs2_formats::orchestrator::parse_state_machine;
use mercs2_formats::texture::{extract_model, parse_mtrl};
use mercs2_mesh::{build_indexed_all, state_tiers};

const NAMES: &str = include_str!("../data/asset_names.txt");

fn main() {
    let mut a = std::env::args().skip(1);
    let game = a.next().expect("usage: <game root> <model>...");
    let wad = std::path::Path::new(&game).join("data").join("vz.wad");
    let mut f = std::fs::File::open(&wad).expect("open");
    let size = f.metadata().unwrap().len();
    let archive = load_ffcs_archive(&mut f, size).expect("ffcs");

    let names: std::collections::HashMap<u32, &str> = NAMES
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|n| (pandemic_hash_m2(n), n))
        .collect();

    for model in a {
        let h = pandemic_hash_m2(&model);
        let Ok(c) = extract_model(&mut f, &archive, h) else {
            println!("{model}: not in this WAD\n");
            continue;
        };

        let mats = parse_mtrl(&c);
        let declared: HashSet<u32> = mats
            .iter()
            .flat_map(|m| m.textures.iter().copied())
            .filter(|&t| t != 0)
            .collect();

        let all = build_indexed_all(&c).map(|b| b.2).unwrap_or_default();
        let bound: HashSet<u32> = all
            .iter()
            .flat_map(|d| [d.diffuse, d.specular, d.normal])
            .flatten()
            .collect();

        println!("=== {model}");
        println!("  MTRL records          : {}", mats.len());
        println!("  distinct textures decl: {}", declared.len());
        println!("  draw groups (all tiers): {}", all.len());
        println!("  textures actually bound: {}", bound.len());
        println!("  SEGM tiers            : {:?}", state_tiers(&c));
        println!("  destruction machine   : {}", parse_state_machine(&c).is_some());

        let orphan: Vec<&str> = declared
            .difference(&bound)
            .filter_map(|t| names.get(t).copied())
            .take(8)
            .collect();
        println!("  declared but NEVER painted by any group ({}): {orphan:?}", declared.len() - bound.len());
        println!();
    }
}
