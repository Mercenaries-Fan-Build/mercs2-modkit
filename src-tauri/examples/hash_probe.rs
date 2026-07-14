//! Is a given hash in the ASET, and as what?
//!
//! `cargo run --release --example hash_probe -- "<game root>" 0xHASH...`

use mercs2_formats::ffcs::load_ffcs_archive;
use mercs2_formats::texture::extract_model;

fn main() {
    let mut a = std::env::args().skip(1);
    let game = a.next().expect("usage: <game root> 0xHASH...");
    let wad = std::path::Path::new(&game).join("data").join("vz.wad");
    let mut f = std::fs::File::open(&wad).expect("open");
    let size = f.metadata().unwrap().len();
    let archive = load_ffcs_archive(&mut f, size).expect("ffcs");

    for arg in a {
        let h = u32::from_str_radix(arg.trim_start_matches("0x").trim_start_matches("0X"), 16)
            .expect("hex");
        let rows: Vec<String> = archive
            .aset
            .iter()
            .filter(|e| e.asset_hash == h)
            .map(|e| {
                format!(
                    "type={} block={} sub={:#06X} primary={}",
                    e.type_id,
                    e.block_index(),
                    e.sub_entry(),
                    e.is_primary()
                )
            })
            .collect();
        println!("0x{h:08X}: {} ASET row(s)", rows.len());
        for r in &rows {
            println!("    {r}");
        }
        match extract_model(&mut f, &archive, h) {
            Ok(c) => println!("    extract_model: OK ({} bytes)", c.len()),
            Err(e) => println!("    extract_model: FAILS -> {e}"),
        }
    }
}
