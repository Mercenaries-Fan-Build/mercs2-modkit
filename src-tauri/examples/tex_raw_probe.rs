//! Why won't a texture decode? Dump its raw INFO/BODY, bypassing the DXT-only parser.
//!
//! `cargo run --release --example tex_raw_probe -- "<game root>" <texture>...`

use mercs2_formats::ffcs::load_ffcs_archive;
use mercs2_formats::hash::pandemic_hash_m2;
use mercs2_formats::texture::{extract_container, parse_texture_container};
use mercs2_formats::types::{TYPE_HASH_TEXTURE, TYPE_ID_TEXTURE};

fn main() {
    let mut a = std::env::args().skip(1);
    let game = a.next().expect("usage: <game root> <texture>...");
    let wad = std::path::Path::new(&game).join("data").join("vz.wad");
    let mut f = std::fs::File::open(&wad).expect("open");
    let size = f.metadata().unwrap().len();
    let archive = load_ffcs_archive(&mut f, size).expect("ffcs");

    for name in a {
        let h = pandemic_hash_m2(&name);
        println!("=== {name}  (0x{h:08X})");

        let c = match extract_container(&mut f, &archive, h, TYPE_ID_TEXTURE, TYPE_HASH_TEXTURE) {
            Ok(c) => c,
            Err(e) => {
                println!("  extract: {e}\n");
                continue;
            }
        };
        println!("  container {} bytes", c.len());

        // Walk the descriptor rows by hand — the typed parser bails on unknown formats, which
        // is exactly the case we're trying to see.
        let rd = |o: usize| u32::from_le_bytes(c[o..o + 4].try_into().unwrap()) as usize;
        let data_off = rd(4);
        let ndesc = rd(16);
        for i in 0..ndesc {
            let ro = 20 + i * 20;
            let tag = String::from_utf8_lossy(&c[ro..ro + 4]).to_string();
            let u0 = rd(ro + 4);
            let sz = rd(ro + 8);
            if u0 == 0xFFFF_FFFF {
                println!("    {tag:<5} (container)");
                continue;
            }
            let start = if data_off > 0 { data_off + u0 } else { 8 + u0 };
            println!("    {tag:<5} off {start:>6}  size {sz:>8}");

            if tag == "INFO" && start + 34 <= c.len() {
                let info = &c[start..start + sz.min(40)];
                let u16at = |o: usize| u16::from_le_bytes([info[o], info[o + 1]]);
                println!(
                    "      w={} h={} mips={} fourcc={:?} raw[14..18]={:02X?}",
                    u16at(0),
                    u16at(2),
                    u16at(6),
                    String::from_utf8_lossy(&info[14..18]),
                    &info[14..18],
                );
                println!("      INFO bytes: {:02X?}", &info[..info.len().min(36)]);
            }
        }

        match parse_texture_container(&c) {
            Ok(t) => println!("  parsed: {}x{} {:?} body {}", t.width, t.height, t.format, t.all_mips.len()),
            Err(e) => println!("  parse FAILS: {e}"),
        }
        println!();
    }
}
