//! End-to-end exercise of the wardrobe path against a real `vz.wad`.
//!
//! ```text
//! cargo run --example wardrobe_e2e -- "<game install root>" <out.wad>
//! ```
//!
//! Drives exactly what the GUI drives: enumerate the wearable models actually present in
//! this install, add two outfits for two different characters, rebuild `scripts_vz`, and
//! assemble a patch WAD — then re-read it and check the invariants.

use mercs2_modkit_lib::commands::wardrobe::{list_wardrobe_models, wardrobe_block, WardrobeOutfit};
use mercs2_formats::patch_wad::{build_patch_wad_multi, read_patch_wad, FFCS_CERT_BLOB};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let game = args.get(1).expect("usage: wardrobe_e2e <game root> <out.wad>");
    let out = args.get(2).expect("usage: wardrobe_e2e <game root> <out.wad>");

    println!("== wearable models found in this install ==");
    let models = list_wardrobe_models(game.clone()).expect("list models");
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
    assert!(!outfits.is_empty(), "no known wearable model in this install");
    println!("adding {} outfit(s): {:?}\n", outfits.len(), outfits.iter().map(|o| (&o.hero, &o.model)).collect::<Vec<_>>());

    println!("== building the scripts_vz block ==");
    let block = wardrobe_block(game, &outfits)
        .expect("build wardrobe block")
        .expect("outfits requested, so a block must be produced");

    println!("  path          {}", block.path_string);
    println!("  compressed    {} bytes", block.compressed_data.len());
    println!("  declared pages {}", block.declared_pages());
    println!("  ASET rows     {}", block.aset_entries.len());

    // packed_field sizes the engine's decompression buffer (pages << 15). The whole point
    // of `from_decompressed` is that this is never the placeholder 1.
    assert!(
        block.declared_pages() > 1,
        "scripts_vz is far bigger than one 32 KB page — packed_field was not recomputed"
    );

    println!("\n== assembling the patch WAD ==");
    let wad = build_patch_wad_multi(&[block], 0, Some(0), &FFCS_CERT_BLOB).expect("assemble");
    std::fs::write(out, &wad).expect("write");
    println!("  wrote {} ({} bytes)", out, wad.len());

    // Round-trip: the writer's own validation already ran; prove the block survives a read.
    let back = read_patch_wad(&wad).expect("re-read");
    assert_eq!(back.blocks.len(), 1);
    let b = &back.blocks[0];
    println!("  re-read: {} ASET rows, {} declared pages", b.aset_entries.len(), b.packed_field & 0x00FF_FFFF);

    // And the sges stream must inflate back to a parseable scripts block.
    let raw = mercs2_formats::sges::decompress_sges(&b.compressed_data).expect("inflate");
    let parsed = mercs2_formats::scripts_block::ScriptsBlock::parse(&raw).expect("parse");
    let idx = parsed
        .find_by_name("wifpmcinterior")
        .expect("wifpmcinterior still present");
    let luaq = parsed.extract_lua(idx).expect("extract");
    assert_eq!(&luaq[..4], b"\x1bLua", "the replaced script is LuaQ bytecode");
    assert_eq!(luaq[8], 4, "32-bit size_t");
    assert_eq!(luaq[10], 4, "float lua_Number");

    println!("\nOK — wardrobe WAD is structurally valid and wifpmcinterior carries game-format bytecode.");
}
