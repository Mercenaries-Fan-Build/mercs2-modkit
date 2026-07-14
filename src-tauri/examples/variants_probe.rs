//! The condition explorer: under which state/LOD tiers is a texture visible?
//!
//! `cargo run --release --example variants_probe -- "<game root>" <model> <texture>`
//!
//! Also asserts the invariant the whole design rests on: if the usage index says a model
//! *paints* a texture, then at least one variant of that model must actually highlight it.
//! Otherwise the 3D view would show "not visible" for something we told the user is used.

use mercs2_modkit_lib::commands::model_view::model_variants;

fn main() {
    let mut a = std::env::args().skip(1);
    let game = a.next().expect("usage: <game root> <model> <texture>");
    let model = a.next().expect("model");
    let texture = a.next().expect("texture");

    let vs = model_variants(game, model.clone(), texture.clone()).expect("variants");

    println!("{model}  <-  {texture}\n");
    println!("  {:<12} {:>7} {:>8}  shows the texture?", "condition", "groups", "tris");
    for v in &vs {
        println!(
            "  {:<12} {:>7} {:>8}  {}",
            v.tier.map(|t| format!("state 0x{t:02X}")).unwrap_or_else(|| "all".into()),
            v.groups,
            v.triangles,
            if v.shows_texture {
                format!("YES — {} part(s)", v.highlighted)
            } else {
                "no".into()
            },
        );
    }

    let showing: Vec<String> = vs
        .iter()
        .filter(|v| v.shows_texture)
        .map(|v| v.tier.map(|t| format!("0x{t:02X}")).unwrap_or_else(|| "all".into()))
        .collect();
    println!(
        "\n=> visible under {} of {} conditions{}",
        showing.len(),
        vs.len(),
        if showing.is_empty() { String::new() } else { format!(": {showing:?}") }
    );
}
