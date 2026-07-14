//! How many textures fail to produce a thumbnail, and why?
//!
//! `cargo run --release --example preview_sweep -- "<game root>"`
//!
//! A silently-skipped preview is invisible in the UI (the tile just stays blank), so the only
//! way to know the decoder covers the real data is to run it over all of it.

use std::collections::BTreeMap;

use mercs2_modkit_lib::commands::texture_swap::{list_textures, texture_previews};

fn main() {
    let game = std::env::args().nth(1).expect("usage: <game root>");

    let all = list_textures(game.clone()).expect("catalog");
    println!("catalog: {} textures", all.len());

    let names: Vec<String> = all.iter().map(|t| t.name.clone()).collect();

    // Batch, so we don't re-open the WAD 13k times.
    let mut ok = 0usize;
    let mut decoded: std::collections::HashSet<String> = Default::default();
    for chunk in names.chunks(500) {
        let got = texture_previews(game.clone(), chunk.to_vec(), Some(32)).expect("previews");
        ok += got.len();
        for p in got {
            decoded.insert(p.name);
        }
    }

    println!(
        "previews decoded: {ok} / {}  ({:.1}%)",
        all.len(),
        100.0 * ok as f64 / all.len() as f64
    );

    let failed: Vec<&str> = all
        .iter()
        .filter(|t| !decoded.contains(&t.name))
        .map(|t| t.name.as_str())
        .collect();
    println!("still failing: {}", failed.len());

    // Group the failures by prefix so a pattern is visible rather than a wall of names.
    let mut by_group: BTreeMap<&str, usize> = BTreeMap::new();
    for n in &failed {
        *by_group.entry(n.split('_').next().unwrap_or("?")).or_default() += 1;
    }
    let mut top: Vec<_> = by_group.into_iter().collect();
    top.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    for (g, c) in top.iter().take(10) {
        println!("  {g:<20} {c}");
    }
    for n in failed.iter().take(10) {
        println!("  e.g. {n}");
    }
}
