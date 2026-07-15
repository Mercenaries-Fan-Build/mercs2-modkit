//! The invariant: every skin the wardrobe OFFERS must build. A typo (something not in the
//! game) must be refused with an actionable message.
//!
//! `cargo run --release --example wardrobe_guard_probe -- "<game root>"`

use mercs2_modkit_lib::commands::wardrobe::{list_wardrobe_models, wardrobe_block, WardrobeOutfit};

fn main() {
    let game = std::env::args().nth(1).expect("usage: <game root>");

    let offered = list_wardrobe_models(game.clone()).expect("wardrobe");
    let base = offered.iter().filter(|m| m.in_base_wardrobe).count();
    println!(
        "offered: {}   ({base} already in the base wardrobe, badged)\n",
        offered.len()
    );

    // A genuine typo must be refused before anything is compiled.
    let err = wardrobe_block(
        &game,
        &[WardrobeOutfit {
            hero: "mattias".into(),
            model: "pmc_hum_notarealskin".into(),
            label: "nope".into(),
        }],
    )
    .unwrap_err();
    println!("refused a typo -> {err}\n");

    // Every skin the picker offers must actually build — the invariant that broke before
    // (detection and validation asked different questions).
    println!("building every offered skin...");
    for (i, m) in offered.iter().enumerate() {
        wardrobe_block(
            &game,
            &[WardrobeOutfit {
                hero: "mattias".into(),
                model: m.model.clone(),
                label: m.label.clone(),
            }],
        )
        .unwrap_or_else(|e| panic!("OFFERED BUT FAILED TO BUILD: {} -> {e}", m.model));
        if i % 15 == 0 {
            println!("  ok {}/{}", i + 1, offered.len());
        }
    }

    println!("\nOK — all {} offered skins build; a typo is refused up front.", offered.len());
}
