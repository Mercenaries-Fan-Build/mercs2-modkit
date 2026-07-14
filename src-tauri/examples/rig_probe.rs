//! Which models are human skins, and how do they compare to the three player characters?
//!
//! `cargo run --release --example rig_probe -- "<game root>" [limit]`
//!
//! The wardrobe currently offers a HARDCODED list of candidate model names. That is a guess.
//! The game knows the truth: a wearable player skin is a model rigged to the same human
//! skeleton the heroes use. Bones are HIER nodes with name-hashes, so "is this the same rig"
//! is a set comparison — no heuristics, no name matching.

use std::collections::{HashMap, HashSet};

use mercs2_formats::ffcs::load_ffcs_archive;
use mercs2_formats::hash::pandemic_hash_m2;
use mercs2_formats::orchestrator::parse_hier;
use mercs2_formats::texture::extract_model;
use mercs2_formats::types::TYPE_ID_MODEL;

const NAMES: &str = include_str!("../data/asset_names.txt");
const HEROES: [&str; 3] = ["pmc_hum_mattias", "pmc_hum_chris", "pmc_hum_jen"];

fn main() {
    let game = std::env::args().nth(1).expect("usage: <game root> [limit]");
    let limit: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);

    let wad = std::path::Path::new(&game).join("data").join("vz.wad");
    let mut f = std::fs::File::open(&wad).expect("open");
    let size = f.metadata().unwrap().len();
    let archive = load_ffcs_archive(&mut f, size).expect("ffcs");

    let names: HashMap<u32, &str> = NAMES
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|n| (pandemic_hash_m2(n), n))
        .collect();

    let bones_of = |f: &mut std::fs::File, h: u32| -> HashSet<u32> {
        extract_model(f, &archive, h)
            .map(|c| parse_hier(&c).into_iter().map(|n| n.hash).collect())
            .unwrap_or_default()
    };

    // The reference rig: the union of what the three heroes share.
    let hero_rigs: Vec<(&str, HashSet<u32>)> = HEROES
        .iter()
        .map(|n| (*n, bones_of(&mut f, pandemic_hash_m2(n))))
        .collect();
    for (n, b) in &hero_rigs {
        println!("{n:<22} {} bones", b.len());
    }
    let common: HashSet<u32> = hero_rigs
        .iter()
        .skip(1)
        .fold(hero_rigs[0].1.clone(), |acc, (_, b)| {
            acc.intersection(b).copied().collect()
        });
    println!("\nbones common to all three heroes: {}\n", common.len());

    // Every model, scored against that shared rig.
    let models: Vec<u32> = {
        let mut seen = HashSet::new();
        archive
            .aset
            .iter()
            .filter(|e| e.type_id == TYPE_ID_MODEL)
            .map(|e| e.asset_hash)
            .filter(|h| seen.insert(*h))
            .collect()
    };

    let mut hits: Vec<(f32, usize, u32)> = Vec::new();
    for &m in models.iter().take(limit) {
        let b = bones_of(&mut f, m);
        if b.is_empty() {
            continue;
        }
        // What fraction of the heroes' shared skeleton does this model have?
        let have = common.iter().filter(|h| b.contains(h)).count();
        let pct = have as f32 / common.len().max(1) as f32;
        if pct > 0.5 {
            hits.push((pct, b.len(), m));
        }
    }
    hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.2.cmp(&b.2)));

    println!("models with >50% of the hero skeleton: {}\n", hits.len());
    println!("  {:<34} {:>6} {:>7}", "model", "bones", "rig");
    for (pct, nb, m) in hits.iter().take(30) {
        println!(
            "  {:<34} {nb:>6} {:>6.0}%",
            names.get(m).copied().unwrap_or("<unnamed>"),
            pct * 100.0
        );
    }

    // Histogram of how many clear the bar, to size the feature.
    let full = hits.iter().filter(|(p, _, _)| *p >= 0.99).count();
    let most = hits.iter().filter(|(p, _, _)| *p >= 0.9 && *p < 0.99).count();
    println!("\n100% of the hero rig: {full}   90-99%: {most}   50-90%: {}", hits.len() - full - most);
}
