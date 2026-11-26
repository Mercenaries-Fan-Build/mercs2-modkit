//! End-to-end exercise of the wardrobe path against a real `vz.wad`.
//!
//! ```text
//! cargo run --release --example wardrobe_e2e -- "<game install root>" <qm> <scratch home>
//! ```
//!
//! Drives exactly what the GUI drives: enumerate the wearable models actually present in
//! this install, add two outfits for two different characters, and build them through
//! `wad_builder::assemble`, which hands them to `qm`. Then re-read the WAD and check the
//! invariants. `HOME` is pointed at `<scratch home>` first, so Modkit's own app data is not
//! touched; the WAD is written to `<scratch home>/out/vz-patch.wad`.

#[path = "support/wardrobe_build.rs"]
mod support;

use std::process::ExitCode;

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
    let args = support::enter("wardrobe_e2e <game root> <qm> <scratch home>")?;

    println!("== wearable models found in this install ==");
    let models = list_wardrobe_models(args.game.clone())?;
    for m in &models {
        println!("  {:<32} 0x{:08X}  {}", m.model, m.asset_hash, m.label);
    }
    println!("  ({} wearable models)\n", models.len());

    // Two outfits, two different heroes — the case that proves several additions union.
    // Pick from what this install actually has, so the example works on a DLC-less copy.
    let pick = |want: &str| models.iter().any(|m| m.model == want);
    let mut outfits = Vec::new();
    if pick("pmc_hum_mechanic") {
        outfits.push(WardrobeOutfit {
            hero: "mattias".into(),
            model: "pmc_hum_mechanic".into(),
            label: "Eva".into(),
        });
    }
    if pick("vz_hum_solano") {
        outfits.push(WardrobeOutfit {
            hero: "chris".into(),
            model: "vz_hum_solano".into(),
            label: "Solano".into(),
        });
    }
    if outfits.is_empty() {
        return Err("no known wearable model in this install".into());
    }
    println!(
        "adding {} outfit(s): {:?}\n",
        outfits.len(),
        outfits.iter().map(|o| (&o.hero, &o.model)).collect::<Vec<_>>()
    );

    println!("== building through qm ==");
    let built = support::build(&args, &outfits)?;
    println!("  wrote {}", built.wad_path.display());
    println!("  sha256 {}", built.sha256);
    println!("  {} block(s)", built.block_count);

    // The linked wardrobe script is game-format bytecode that names every added model.
    let luaq = &built.interior_luaq;
    if luaq[8] != 4 {
        return Err(format!("wifpmcinterior has a {}-byte size_t, the game's is 4", luaq[8]));
    }
    if luaq[10] != 4 {
        return Err(format!("wifpmcinterior has a {}-byte lua_Number, the game's is 4", luaq[10]));
    }
    for o in &outfits {
        if !luaq.windows(o.model.len()).any(|w| w == o.model.as_bytes()) {
            return Err(format!("the built wifpmcinterior does not name {}", o.model));
        }
        println!("  wifpmcinterior names {}", o.model);
    }

    println!("\nOK — the wardrobe WAD is built by qm and wifpmcinterior carries the added outfits.");
    Ok(())
}
