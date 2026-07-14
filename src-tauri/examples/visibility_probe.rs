//! When a texture is "used by" a model but highlights nothing, WHY?
//!
//! `cargo run --release --example visibility_probe -- "<game root>" [limit]`
//!
//! `used_by` comes from `parse_mtrl`, which reads every MTRL record in the container. But a
//! material only *draws* if some PRMT group binds it, and that group only draws if its SEGM
//! LOD/state bit is active. So a texture can be genuinely referenced by a model and still be
//! on no visible surface. This measures how often that happens, and whether ANY tier shows it.

use std::collections::{HashMap, HashSet};

use mercs2_formats::ffcs::load_ffcs_archive;
use mercs2_formats::hash::pandemic_hash_m2;
use mercs2_formats::orchestrator::parse_state_machine;
use mercs2_formats::texture::{extract_model, parse_mtrl};
use mercs2_formats::types::{TYPE_ID_MODEL, TYPE_ID_TEXTURE};
use mercs2_mesh::{build_indexed_all, build_indexed_state, state_tiers, DrawGroup};

fn main() {
    let game = std::env::args().nth(1).expect("usage: <game root> [limit]");
    let limit: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(400);

    let wad = std::path::Path::new(&game).join("data").join("vz.wad");
    let mut f = std::fs::File::open(&wad).expect("open");
    let size = f.metadata().unwrap().len();
    let archive = load_ffcs_archive(&mut f, size).expect("ffcs");

    let textures: HashSet<u32> = archive
        .aset
        .iter()
        .filter(|e| e.type_id == TYPE_ID_TEXTURE)
        .map(|e| e.asset_hash)
        .collect();
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

    let uses = |groups: &[DrawGroup], t: u32| {
        groups
            .iter()
            .any(|d| d.diffuse == Some(t) || d.normal == Some(t) || d.specular == Some(t))
    };

    let (mut pairs, mut in_default, mut in_other_tier, mut nowhere) = (0usize, 0, 0, 0);
    let mut nowhere_has_machine = 0usize;
    let mut examples: Vec<(u32, u32, Vec<u8>)> = Vec::new();

    for &m in models.iter().take(limit) {
        let Ok(c) = extract_model(&mut f, &archive, m) else { continue };

        // Every texture the model's MTRLs declare — i.e. exactly what `used_by` is built from.
        let mut declared: HashSet<u32> = HashSet::new();
        for mat in parse_mtrl(&c) {
            for t in mat.textures {
                if t != 0 && textures.contains(&t) {
                    declared.insert(t);
                }
            }
        }
        if declared.is_empty() {
            continue;
        }

        let tiers = state_tiers(&c);
        // Build each tier once.
        let mut per_tier: Vec<(u8, Vec<DrawGroup>)> = Vec::new();
        for t in &tiers {
            if let Ok((_, _, g, _)) = build_indexed_state(&c, *t) {
                per_tier.push((*t, g));
            }
        }
        let all_groups = build_indexed_all(&c).map(|b| b.2).unwrap_or_default();
        let has_machine = parse_state_machine(&c).is_some();

        for t in declared {
            pairs += 1;
            let default_ok = per_tier
                .iter()
                .find(|(b, _)| *b == 0x01)
                .map(|(_, g)| uses(g, t))
                .unwrap_or(false);
            let any_tier: Vec<u8> =
                per_tier.iter().filter(|(_, g)| uses(g, t)).map(|(b, _)| *b).collect();

            if default_ok {
                in_default += 1;
            } else if !any_tier.is_empty() {
                in_other_tier += 1;
                if examples.len() < 6 {
                    examples.push((m, t, any_tier.clone()));
                }
            } else if uses(&all_groups, t) {
                // Drawn by some group, but no declared tier surfaced it.
                in_other_tier += 1;
            } else {
                nowhere += 1;
                if has_machine {
                    nowhere_has_machine += 1;
                }
            }
        }
    }

    println!("(model, texture) pairs examined: {pairs}");
    println!("  visible at the default tier 0x01 : {in_default}");
    println!("  visible only at ANOTHER tier     : {in_other_tier}");
    println!("  on NO drawn group at all         : {nowhere}   (of which {nowhere_has_machine} have a destruction state machine)");
    println!("\nexamples needing a non-default tier:");
    for (m, t, tiers) in &examples {
        println!("  model 0x{m:08X}  texture 0x{t:08X}  -> tiers {tiers:?}");
    }
    let _ = pandemic_hash_m2("");
}
