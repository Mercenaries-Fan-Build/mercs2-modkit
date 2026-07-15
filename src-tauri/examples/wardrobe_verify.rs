//! Build a real wardrobe WAD with the community's verified skins, dump the recompiled
//! wifpmcinterior, and hand it back so it can be decompiled and eyeballed.
//!
//! `cargo run --release --example wardrobe_verify -- "<game root>" <out_scripts.bin>`

use mercs2_formats::scripts_block::ScriptsBlock;
use mercs2_formats::sges::decompress_sges;
use mercs2_modkit_lib::commands::human_skins::human_skins;
use mercs2_modkit_lib::commands::wardrobe::{list_wardrobe_models, wardrobe_block, WardrobeOutfit};

fn main() {
    let mut a = std::env::args().skip(1);
    let game = a.next().expect("usage: <game root> <out.bin>");
    let out = a.next().expect("out.bin");

    // The community's verified-working skins that were WRONGLY rejected before, plus one
    // genuinely-new PMC skin.
    let skins = ["al_hum_boss", "oc_hum_pilot", "ch_hum_boss", "pmc_hum_fiona"];

    // They must all be offered now (they weren't, under the standalone gate).
    let offered = list_wardrobe_models(game.clone()).expect("list");
    let idx = human_skins(game.clone()).expect("skins");
    println!("wearable skins offered: {}", offered.len());
    for s in skins {
        let ok = offered.iter().any(|m| m.model == s);
        let badged = offered.iter().find(|m| m.model == s).map(|m| m.in_base_wardrobe);
        println!("  offered {s}: {ok}   (in_base_wardrobe={badged:?})");
        assert!(ok, "{s} must be offered now");
    }
    let base = offered.iter().filter(|m| m.in_base_wardrobe).count();
    println!("of {} offered, {base} are already in the base wardrobe (badged)\n", offered.len());
    println!("hero skeleton: {} shared bones", idx.hero_bone_count);

    // Build the block with those skins added to mattias.
    let outfits: Vec<WardrobeOutfit> = skins
        .iter()
        .map(|s| WardrobeOutfit {
            hero: "mattias".into(),
            model: s.to_string(),
            label: s.to_string(),
        })
        .collect();

    let block = wardrobe_block(&game, &outfits)
        .expect("build")
        .expect("outfits requested");

    // Decompress -> pull wifpmcinterior -> write its LuaQ for offline decompile.
    let raw = decompress_sges(&block.compressed_data).expect("inflate");
    let sb = ScriptsBlock::parse(&raw).expect("parse");
    let e = sb.find_by_name("wifpmcinterior").expect("script present");
    let luaq = sb.extract_lua(e).expect("extract");
    std::fs::write(&out, &luaq).expect("write");

    assert_eq!(&luaq[..4], b"\x1bLua", "must be LuaQ bytecode");
    println!("\nwrote recompiled wifpmcinterior ({} bytes) -> {out}", luaq.len());
    println!("built OK — al_hum_boss et al. are wearable and the block compiled.");
}
