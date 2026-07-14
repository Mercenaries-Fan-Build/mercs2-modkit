//! The invariant: anything the wardrobe OFFERS must build; anything it refuses must be
//! refused for a reason the user can act on.
//!
//! `cargo run --release --example wardrobe_guard_probe -- "<game root>"`

use mercs2_modkit_lib::commands::human_skins::human_skins;
use mercs2_modkit_lib::commands::wardrobe::{list_wardrobe_models, wardrobe_block, WardrobeOutfit};

fn main() {
    let game = std::env::args().nth(1).expect("usage: <game root>");

    let idx = human_skins(game.clone()).expect("skins");
    let offered = list_wardrobe_models(game.clone()).expect("wardrobe");
    println!(
        "offered: {}   excluded (rigged but not standalone models): {}\n",
        offered.len(),
        idx.not_standalone
    );

    // 1. The two the user hit must now be refused BEFORE anything is compiled.
    for bad in ["al_hum_boss", "ch_hum_pilot_a"] {
        assert!(
            !offered.iter().any(|m| m.model == bad),
            "{bad} must no longer be offered"
        );
        let err = wardrobe_block(
            &game,
            &[WardrobeOutfit {
                hero: "mattias".into(),
                model: bad.into(),
                label: bad.into(),
            }],
        )
        .unwrap_err();
        println!("refused {bad}\n  -> {err}");
    }

    // 2. Every skin the picker DOES offer must actually build. This is the invariant that
    //    was broken: detection walked all model rows, validation only primary ones.
    println!("\nbuilding every offered skin...");
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
        if i % 10 == 0 {
            println!("  ok {}/{}", i + 1, offered.len());
        }
    }

    println!("\nOK — all {} offered skins build; the two bad ones are refused up front.", offered.len());
}
