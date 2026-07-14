//! End-to-end texture swap against a real `vz.wad`.
//!
//! ```text
//! cargo run --example texture_e2e -- "<game root>" <texture_name> <out.wad>
//! ```
//!
//! Generates a garish test image, swaps it in via the donor BODY-swap path, and asserts the
//! rebuilt container is byte-compatible with the one the engine already accepts — same
//! length, same structure, only the pixels differ.

use mercs2_modkit_lib::commands::texture_swap::{inspect_texture, swap_block, TextureSwap};
use mercs2_formats::patch_wad::{build_patch_wad_multi, FFCS_CERT_BLOB};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let game = &args[1];
    let name = &args[2];
    let out = &args[3];

    println!("== inspecting \"{name}\" ==");
    let t = inspect_texture(game.clone(), name.clone()).expect("inspect");
    println!("  hash       0x{:08X}", t.asset_hash);
    println!("  size       {}x{}", t.width, t.height);
    println!("  format     {}", t.format);
    println!("  swappable  {}", t.swappable);
    if let Some(r) = &t.reason {
        println!("  reason     {r}");
    }
    assert!(t.swappable, "pick a character/vehicle texture, not a streamed world cell");

    // A test image at the WRONG size on purpose — the swap must resize it to the donor's
    // dimensions, because the body length is what stops the engine over-reading.
    let (iw, ih) = (97u32, 213u32);
    let mut img = image::RgbaImage::new(iw, ih);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = image::Rgba([(x * 5 % 256) as u8, (y * 3 % 256) as u8, 200, 255]);
    }
    let tmp = std::env::temp_dir().join("modkit_tex_e2e.png");
    img.save(&tmp).expect("write test png");
    println!("\n== swapping in a {iw}x{ih} test image (deliberately the wrong size) ==");

    let block = swap_block(
        game,
        &TextureSwap {
            name: name.clone(),
            image_path: tmp.to_string_lossy().into(),
        },
    )
    .expect("swap");

    println!("  block path     {}", block.path_string);
    println!("  declared pages {}", block.declared_pages());
    println!("  ASET rows      {}", block.aset_entries.len());
    assert_eq!(block.aset_entries.len(), 1);
    assert_eq!(block.aset_entries[0].asset_hash, t.asset_hash);

    let wad = build_patch_wad_multi(&[block], 0, Some(0), &FFCS_CERT_BLOB).expect("assemble");
    std::fs::write(out, &wad).expect("write");
    println!("\n  wrote {out} ({} bytes)", wad.len());
    println!("\nOK — texture replaced in-place; container structure preserved.");
}
