//! Every part, in every model, that paints a texture.
//!
//! `cargo run --release --example parts_probe -- "<game root>" <texture>`

use mercs2_modkit_lib::commands::model_view::texture_parts;

fn main() {
    let mut a = std::env::args().skip(1);
    let game = a.next().expect("usage: <game root> <texture>");
    let tex = a.next().expect("texture");

    let parts = texture_parts(game, tex.clone()).expect("parts");
    println!("{tex}: painted by {} part(s)\n", parts.len());
    println!("  {:<30} {:>5} {:>6} {:>8}  slot", "model", "part", "tier", "tris");
    for p in parts.iter().take(20) {
        println!(
            "  {:<30} {:>5} {:>6} {:>8}  {}",
            p.model_name.clone().unwrap_or(format!("0x{:08X}", p.model_hash)),
            p.part,
            p.tier.map(|t| format!("{t:#04x}")).unwrap_or("auto".into()),
            p.triangles,
            p.slot,
        );
    }
    let models: std::collections::HashSet<u32> = parts.iter().map(|p| p.model_hash).collect();
    println!("\nacross {} model(s)", models.len());
}
