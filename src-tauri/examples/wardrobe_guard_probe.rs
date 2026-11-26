//! The invariant: every skin the wardrobe OFFERS must build. A typo (something not in the
//! game) must be refused with an actionable message.
//!
//! `cargo run --release --example wardrobe_guard_probe -- "<game root>" <qm> <scratch home>`
//!
//! Both go through `wad_builder::assemble`, as the app's builds do. `HOME` is pointed at
//! `<scratch home>` first, so Modkit's own app data is not touched.

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
    let args = support::enter("wardrobe_guard_probe <game root> <qm> <scratch home>")?;

    let offered = list_wardrobe_models(args.game.clone())?;
    let base = offered.iter().filter(|m| m.in_base_wardrobe).count();
    println!(
        "offered: {}   ({base} already in the base wardrobe, badged)\n",
        offered.len()
    );

    // A genuine typo must be refused before qm builds anything: qm builds an outfit for
    // whatever model name it is given.
    let typo = [WardrobeOutfit {
        hero: "mattias".into(),
        model: "pmc_hum_notarealskin".into(),
        label: "nope".into(),
    }];
    match support::build(&args, &typo) {
        Ok(built) => {
            return Err(format!(
                "a model the game does not have was built into {}",
                built.wad_path.display()
            ))
        }
        Err(e) if e.contains("no wearable model called \"pmc_hum_notarealskin\"") => {
            println!("refused a typo -> {e}\n")
        }
        Err(e) => return Err(format!("the typo was refused for another reason: {e}")),
    }

    // Every skin the picker offers must actually build — the invariant that broke before
    // (detection and validation asked different questions). One build, every skin on one hero.
    println!("building every offered skin...");
    let outfits: Vec<WardrobeOutfit> = offered
        .iter()
        .map(|m| WardrobeOutfit {
            hero: "mattias".into(),
            model: m.model.clone(),
            label: m.label.clone(),
        })
        .collect();
    let built = support::build(&args, &outfits)?;
    println!("built {} ({} block(s), sha256 {})", built.wad_path.display(), built.block_count, built.sha256);
    for m in &offered {
        if !built.interior_luaq.windows(m.model.len()).any(|w| w == m.model.as_bytes()) {
            return Err(format!("OFFERED BUT NOT IN THE BUILT WARDROBE: {}", m.model));
        }
    }

    println!("\nOK — all {} offered skins build; a typo is refused up front.", offered.len());
    Ok(())
}
