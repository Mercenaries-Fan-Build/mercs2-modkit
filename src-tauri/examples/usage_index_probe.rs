//! How expensive is a texture -> models reverse index, and how good is its coverage?
//!
//! `cargo run --release --example usage_index_probe -- "<game root>"`
//!
//! "Where is this texture used?" means inverting the model->MTRL->texture-slot relation.
//! There is no such index in the WAD, so we have to build it: decompress every model
//! container and read its MTRL slots. This measures what that costs, which decides whether
//! it can run on demand or must be cached.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use mercs2_formats::ffcs::load_ffcs_archive;
use mercs2_formats::hash::pandemic_hash_m2;
use mercs2_formats::texture::{extract_model, parse_mtrl};
use mercs2_formats::types::{TYPE_ID_MODEL, TYPE_ID_TEXTURE};

const NAMES: &str = include_str!("../data/asset_names.txt");

fn main() {
    let game = std::env::args().nth(1).expect("usage: <game root>");
    let wad = std::path::Path::new(&game).join("data").join("vz.wad");
    let mut f = std::fs::File::open(&wad).expect("open");
    let size = f.metadata().unwrap().len();
    let archive = load_ffcs_archive(&mut f, size).expect("ffcs");

    let mut by_hash: HashMap<u32, &str> = HashMap::new();
    for n in NAMES.lines().map(str::trim).filter(|l| !l.is_empty()) {
        by_hash.insert(pandemic_hash_m2(n), n);
    }

    // ALL model rows, not just primary ones. Most models are shared/aliased and appear only
    // as ASET *sub-entries* of another asset's block; `extract_container` resolves either.
    // Walking primaries alone finds 1,771 models and misses the majority — which is why a
    // texture as obvious as `al_veh_tank_m1a1_dm` came back with no user at all.
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
    let textures: HashSet<u32> = archive
        .aset
        .iter()
        .filter(|e| e.type_id == TYPE_ID_TEXTURE)
        .map(|e| e.asset_hash)
        .collect();

    println!("models: {}  textures: {}", models.len(), textures.len());

    let t0 = Instant::now();
    // texture hash -> model hashes that reference it
    let mut usage: HashMap<u32, Vec<u32>> = HashMap::new();
    let (mut ok, mut failed, mut no_mtrl) = (0usize, 0usize, 0usize);

    for &m in &models {
        let Ok(container) = extract_model(&mut f, &archive, m) else {
            failed += 1;
            continue;
        };
        let mats = parse_mtrl(&container);
        if mats.is_empty() {
            no_mtrl += 1;
            continue;
        }
        ok += 1;
        let mut seen = HashSet::new();
        for mat in &mats {
            for &tex in &mat.textures {
                if tex != 0 && textures.contains(&tex) && seen.insert(tex) {
                    usage.entry(tex).or_default().push(m);
                }
            }
        }
    }
    let dt = t0.elapsed();

    println!(
        "scanned {} models in {:.1}s  (ok={ok} no-mtrl={no_mtrl} failed={failed})",
        models.len(),
        dt.as_secs_f64()
    );
    println!(
        "textures DECLARED by some model: {} / {} ({:.1}%)",
        usage.len(),
        textures.len(),
        100.0 * usage.len() as f64 / textures.len() as f64
    );

    // The set that matters for the 3D view: textures a draw group actually PAINTS. Anything
    // here is guaranteed to highlight; anything only in `declared` cannot be shown.
    let mut painted: std::collections::HashSet<u32> = Default::default();
    for &m in &models {
        let Ok(c) = mercs2_formats::texture::extract_model(&mut f, &archive, m) else { continue };
        if let Ok((_, _, groups, _)) = mercs2_mesh::build_indexed_all(&c) {
            for d in groups {
                for t in d.textures {
                    if textures.contains(&t) {
                        painted.insert(t);
                    }
                }
            }
        }
    }
    println!(
        "textures PAINTED by a draw group: {} / {} ({:.1}%)  <- what the 3D view can show",
        painted.len(),
        textures.len(),
        100.0 * painted.len() as f64 / textures.len() as f64
    );

    // Spot-check a few well-known ones.
    for probe in [
        "pmc_hum_chris_ub",
        "al_hum_boss_head",
        "al_veh_tank_m1a1_dm",
        "pmc_hum_eva_lb",
    ] {
        let h = pandemic_hash_m2(probe);
        match usage.get(&h) {
            Some(users) => {
                let names: Vec<&str> = users
                    .iter()
                    .map(|u| by_hash.get(u).copied().unwrap_or("<unnamed>"))
                    .collect();
                println!("\n{probe}  used by {} model(s): {:?}", users.len(), names);
            }
            None => println!("\n{probe}  -> no model references it"),
        }
    }
}
