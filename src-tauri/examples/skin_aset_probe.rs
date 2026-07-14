//! Do the skins the wardrobe offers actually resolve the way the wardrobe validates them?
//!
//! `cargo run --release --example skin_aset_probe -- "<game root>"`
//!
//! The wardrobe detects skins by walking every MODEL ASET row (primary AND sub-entry), but
//! validates them against PRIMARY rows only. Those disagree, and the user sees it as
//! "al_hum_boss exists but is not a model (asset type 34)". Classify every offered skin by
//! the shape of its ASET rows so we can decide what is safe to offer.

use std::collections::HashMap;

use mercs2_formats::ffcs::load_ffcs_archive;
use mercs2_formats::hash::pandemic_hash_m2;
use mercs2_formats::types::TYPE_ID_MODEL;
use mercs2_modkit_lib::commands::human_skins::human_skins;

fn main() {
    let game = std::env::args().nth(1).expect("usage: <game root>");
    let wad = std::path::Path::new(&game).join("data").join("vz.wad");
    let mut f = std::fs::File::open(&wad).expect("open");
    let size = f.metadata().unwrap().len();
    let archive = load_ffcs_archive(&mut f, size).expect("ffcs");

    // hash -> every (type_id, is_primary) row it has.
    let mut rows: HashMap<u32, Vec<(u32, bool)>> = HashMap::new();
    for e in &archive.aset {
        rows.entry(e.asset_hash)
            .or_default()
            .push((e.type_id, e.is_primary()));
    }

    let idx = human_skins(game).expect("skins");
    let wearable: Vec<_> = idx.skins.iter().filter(|s| s.wearable).collect();

    let (mut prim, mut sub_only, mut absent) = (Vec::new(), Vec::new(), Vec::new());
    for s in &wearable {
        let name = s.name.clone().unwrap_or_default();
        let h = pandemic_hash_m2(&name);
        let r = rows.get(&h).cloned().unwrap_or_default();

        let has_primary_model = r.iter().any(|(t, p)| *t == TYPE_ID_MODEL && *p);
        let has_model_row = r.iter().any(|(t, _)| *t == TYPE_ID_MODEL);

        if has_primary_model {
            prim.push(name);
        } else if has_model_row {
            let others: Vec<u32> = r.iter().filter(|(t, p)| *t != TYPE_ID_MODEL && *p).map(|(t, _)| *t).collect();
            sub_only.push(format!("{name}  (model is a SUB-ENTRY; primary row type {others:?})"));
        } else {
            absent.push(format!("{name}  (hash 0x{h:08X}: rows {r:?})"));
        }
    }

    println!("wardrobe offers {} skins\n", wearable.len());
    println!("  primary MODEL row  : {}   <- the shape every proven skin has", prim.len());
    println!("  model is SUB-ENTRY : {}   <- rejected by the validator today", sub_only.len());
    println!("  no model row at all: {}   <- \"your game has no model called ...\"", absent.len());

    println!("\n-- sub-entry only:");
    for n in sub_only.iter().take(12) {
        println!("   {n}");
    }
    println!("\n-- no model row:");
    for n in absent.iter().take(12) {
        println!("   {n}");
    }
    println!("\n-- a few with a primary model row (should all be safe):");
    for n in prim.iter().take(8) {
        println!("   {n}");
    }
}
