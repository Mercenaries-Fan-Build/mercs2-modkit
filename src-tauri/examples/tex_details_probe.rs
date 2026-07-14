//! What does the details page actually show?
//!
//! `cargo run --release --example tex_details_probe -- "<game root>" <name>...`

use mercs2_modkit_lib::commands::texture_swap::texture_details;

fn main() {
    let mut args = std::env::args().skip(1);
    let game = args.next().expect("usage: <game root> <name>...");

    for name in args {
        let t0 = std::time::Instant::now();
        let d = match texture_details(game.clone(), name.clone()) {
            Ok(d) => d,
            Err(e) => {
                println!("{name}: {e}\n");
                continue;
            }
        };
        println!("=== {}  (0x{:08X})", d.name, d.asset_hash);
        println!(
            "  {}x{} {} · {} mips · {} KB chain · {}",
            d.width,
            d.height,
            d.format,
            d.mip_count,
            d.chain_bytes / 1024,
            if d.fully_resident { "stored in full" } else { "streamed" }
        );
        println!("  group {} · {}", d.category, d.kind);
        if let Some(p) = &d.preview {
            println!("  preview {}x{} ({} B)", p.preview_width, p.preview_height, p.data_url.len());
        }

        println!("  USED BY ({}){}:", d.used_by.len(), if d.shared { "  ** SHARED **" } else { "" });
        for m in d.used_by.iter().take(6) {
            println!("    {}", m.name.clone().unwrap_or(format!("0x{:08X}", m.hash)));
        }

        println!("  SIBLING MAPS ({}):", d.siblings.len());
        for s in d.siblings.iter().take(4) {
            println!("    {} ({})", s.name, s.kind);
        }

        println!("  SEEN WITH ({}):", d.seen_with.len());
        for s in d.seen_with.iter().take(6) {
            println!("    {}", s.name);
        }
        println!("  [{:.2}s]\n", t0.elapsed().as_secs_f64());
    }
}
