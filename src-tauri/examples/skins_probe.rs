//! Detected wearable skins vs the old hardcoded candidate list.
//!
//! `cargo run --release --example skins_probe -- "<game root>"`

use mercs2_modkit_lib::commands::human_skins::human_skins;
use mercs2_modkit_lib::commands::wardrobe::list_wardrobe_models;

fn main() {
    let game = std::env::args().nth(1).expect("usage: <game root>");

    let idx = human_skins(game.clone()).expect("skins");
    println!(
        "hero skeleton: {} shared bones\nmodels rigged like a player: {}\n",
        idx.hero_bone_count,
        idx.skins.len()
    );

    let wearable: Vec<_> = idx.skins.iter().filter(|s| s.wearable).collect();
    println!(
        "  {:<32} {:>5} {:>6} {:>9}  {}",
        "wearable skin", "rig", "bones", "tris", "closest hero"
    );
    for s in &wearable {
        println!(
            "  {:<32} {:>4.0}% {:>6} {:>9}  {}{}",
            s.name.clone().unwrap_or_default(),
            s.rig_match * 100.0,
            s.bones,
            s.triangles,
            s.closest_hero,
            if s.is_hero { "  (hero)" } else { "" },
        );
    }
    println!("\nwearable: {}", wearable.len());

    // Rigged well enough, but nameless — real skins we cannot offer, because SetOutfit needs
    // a name string. Worth knowing how much is left on the table.
    let nameless = idx
        .skins
        .iter()
        .filter(|s| s.name.is_none() && s.rig_match >= 0.9)
        .count();
    println!("well-rigged but UNNAMED (cannot be worn): {nameless}");

    // What the wardrobe actually offers now.
    let offered = list_wardrobe_models(game).expect("wardrobe");
    println!("\nwardrobe offers {} skins (was 19 from the hardcoded list)", offered.len());
    for m in offered.iter().take(8) {
        println!("  {:<32} {}", m.model, m.label);
    }
}
