//! Why does a model fail to build at the default render tier?
//!
//! `cargo run --release --example tier_probe -- "<game root>" <model>...`

use mercs2_formats::ffcs::load_ffcs_archive;
use mercs2_formats::hash::pandemic_hash_m2;
use mercs2_formats::texture::extract_model;
use mercs2_mesh::{build_indexed_all, build_indexed_state, state_tiers};

fn main() {
    let mut a = std::env::args().skip(1);
    let game = a.next().expect("usage: <game root> <model>...");
    let wad = std::path::Path::new(&game).join("data").join("vz.wad");
    let mut f = std::fs::File::open(&wad).expect("open");
    let size = f.metadata().unwrap().len();
    let archive = load_ffcs_archive(&mut f, size).expect("ffcs");

    for model in a {
        let h = pandemic_hash_m2(&model);
        let Ok(c) = extract_model(&mut f, &archive, h) else {
            println!("{model}: not in this WAD\n");
            continue;
        };

        let tiers = state_tiers(&c);
        println!("=== {model} (0x{h:08X})");
        println!("  state/LOD tier bits: {tiers:?}");

        // What the viewer currently does: bake the 0x01 tier.
        match build_indexed_state(&c, 0x01) {
            Ok((v, i, g, _)) => println!(
                "  tier 0x01     -> {} verts, {} tris, {} groups",
                v.len(),
                i.len() / 3,
                g.len()
            ),
            Err(e) => println!("  tier 0x01     -> FAILS: {e}"),
        }

        // Each tier the model actually declares.
        for t in &tiers {
            match build_indexed_state(&c, *t) {
                Ok((v, i, g, _)) => println!(
                    "  tier 0x{t:02X}     -> {} verts, {} tris, {} groups",
                    v.len(),
                    i.len() / 3,
                    g.len()
                ),
                Err(e) => println!("  tier 0x{t:02X}     -> FAILS: {e}"),
            }
        }

        // Everything, unfiltered.
        match build_indexed_all(&c) {
            Ok((v, i, g, _)) => println!(
                "  all groups    -> {} verts, {} tris, {} groups",
                v.len(),
                i.len() / 3,
                g.len()
            ),
            Err(e) => println!("  all groups    -> FAILS: {e}"),
        }
        println!();
    }
}
