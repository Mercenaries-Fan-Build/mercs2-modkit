//! Build a real wardrobe WAD with the community's verified skins, dump the linked
//! wifpmcinterior, and hand it back so it can be decompiled and eyeballed.
//!
//! `cargo run --release --example wardrobe_verify -- "<game root>" <qm> <scratch home> <out.bin>`
//!
//! The build goes through `wad_builder::assemble` and `qm`, as the app's does. `HOME` is pointed
//! at `<scratch home>` first, so Modkit's own app data is not touched.

#[path = "support/wardrobe_build.rs"]
mod support;

use std::process::ExitCode;

use mercs2_modkit_lib::commands::human_skins::human_skins;
use mercs2_modkit_lib::commands::wardrobe::{list_wardrobe_models, WardrobeOutfit};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("FAILED: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let usage = "wardrobe_verify <game root> <qm> <scratch home> <out.bin>";
    let args = support::enter(usage)?;
    let out = &std::env::args().nth(4).ok_or_else(|| format!("usage: {usage}"))?;

    // The community's verified-working skins that were WRONGLY rejected before, plus one
    // genuinely-new PMC skin.
    let skins = ["al_hum_boss", "oc_hum_pilot", "ch_hum_boss", "pmc_hum_fiona"];

    // They must all be offered now (they weren't, under the standalone gate).
    let offered = list_wardrobe_models(args.game.clone())?;
    let idx = human_skins(args.game.clone())?;
    println!("wearable skins offered: {}", offered.len());
    for s in skins {
        let entry = offered.iter().find(|m| m.model == s);
        let badged = entry.map(|m| m.in_base_wardrobe);
        println!("  offered {s}: {}   (in_base_wardrobe={badged:?})", entry.is_some());
        if entry.is_none() {
            return Err(format!("{s} must be offered"));
        }
    }
    let base = offered.iter().filter(|m| m.in_base_wardrobe).count();
    println!("of {} offered, {base} are already in the base wardrobe (badged)\n", offered.len());
    println!("hero skeleton: {} shared bones", idx.hero_bone_count);

    // Build with those skins added to mattias.
    let outfits: Vec<WardrobeOutfit> = skins
        .iter()
        .map(|s| WardrobeOutfit {
            hero: "mattias".into(),
            model: s.to_string(),
            label: s.to_string(),
        })
        .collect();
    let built = support::build(&args, &outfits)?;
    println!("built {} ({} block(s), sha256 {})", built.wad_path.display(), built.block_count, built.sha256);

    // The linked wifpmcinterior's LuaQ, for offline decompile.
    std::fs::write(out, &built.interior_luaq).map_err(|e| format!("writing {out}: {e}"))?;
    println!("\nwrote the linked wifpmcinterior ({} bytes) -> {out}", built.interior_luaq.len());
    println!("built OK — al_hum_boss et al. are wearable and qm linked the wardrobe.");
    Ok(())
}
