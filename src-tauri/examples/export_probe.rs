//! Exercise `export_texture` (the "Save as PNG" button).
//!
//! `cargo run --release --example export_probe -- "<game root>" <texture> <out.png>`

use mercs2_modkit_lib::commands::texture_swap::export_texture;

fn main() {
    let mut a = std::env::args().skip(1);
    let game = a.next().expect("usage: <game root> <texture> <out.png>");
    let name = a.next().expect("texture");
    let dest = a.next().expect("out.png");

    let e = export_texture(game, name.clone(), dest).expect("export");
    println!(
        "{name}: wrote {}x{} (real size {}x{}) -> {}",
        e.width, e.height, e.full_width, e.full_height, e.path
    );
    println!(
        "full resolution: {}",
        if e.is_full_resolution {
            "yes"
        } else {
            "no — the game streams the rest of this texture's detail"
        }
    );
}
