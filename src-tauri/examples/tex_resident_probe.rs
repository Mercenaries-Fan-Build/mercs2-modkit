//! Where does a STREAMED texture's inline body sit in the mip chain?
//!
//! `cargo run --example tex_resident_probe -- "<game root>" <name>...`
//!
//! A streamed texture ships only a small resident slice inline. To draw an honest preview
//! we must know *which* mips those bytes are. The hypothesis (from
//! `texture-high-mip-streaming`) is that the resident body is the **tail** of the chain —
//! the smallest levels — with the big mips paged in from elsewhere. Test it: for each
//! level k, the chain from k to the end has a known size; if one of them equals the body
//! length exactly, the body is the tail starting at level k.

use mercs2_formats::ffcs::load_ffcs_archive;
use mercs2_formats::hash::pandemic_hash_m2;
use mercs2_formats::texsize::{dxt_format, dxt_mip_count};
use mercs2_formats::texture::{extract_container, parse_texture_container};
use mercs2_formats::types::{TYPE_HASH_TEXTURE, TYPE_ID_TEXTURE};

fn main() {
    let mut args = std::env::args().skip(1);
    let game = args.next().expect("usage: <game root> <name>...");
    let wad = std::path::Path::new(&game).join("data").join("vz.wad");
    let mut f = std::fs::File::open(&wad).expect("open");
    let size = f.metadata().unwrap().len();
    let archive = load_ffcs_archive(&mut f, size).expect("ffcs");

    for name in args {
        let hash = pandemic_hash_m2(&name);
        let c = match extract_container(&mut f, &archive, hash, TYPE_ID_TEXTURE, TYPE_HASH_TEXTURE) {
            Ok(c) => c,
            Err(e) => {
                println!("{name}: {e}");
                continue;
            }
        };
        let t = parse_texture_container(&c).expect("parse");
        let (bpx, pitch, _) = dxt_format(t.format.fourcc()).unwrap();
        let mips = dxt_mip_count(t.width as usize, t.height as usize);
        let body = t.all_mips.len();

        // Size of the chain from level k down to the last level.
        let tail_from = |k: usize| -> usize {
            (k..mips)
                .map(|i| {
                    let w = (t.width as usize >> i).max(1);
                    let h = (t.height as usize >> i).max(1);
                    w.div_ceil(bpx).max(1) * h.div_ceil(bpx).max(1) * pitch
                })
                .sum()
        };

        let start = (0..mips).find(|&k| tail_from(k) == body);
        println!(
            "{name}\n  {}x{} {} · {mips} mips · body={body}B · full={}B",
            t.width,
            t.height,
            String::from_utf8_lossy(t.format.fourcc()),
            tail_from(0)
        );
        match start {
            Some(0) => println!("  -> FULLY RESIDENT (body is the whole chain)"),
            Some(k) => println!(
                "  -> resident TAIL starting at mip {k} = {}x{} (the big mips stream in)",
                (t.width >> k).max(1),
                (t.height >> k).max(1)
            ),
            None => println!("  -> body matches NO tail-of-chain; layout is something else"),
        }
    }
}
