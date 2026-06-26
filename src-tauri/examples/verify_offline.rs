//! Offline round-trip check: verify an install against a manifest, no GUI.
//!
//!   cargo run --release --example verify_offline -- <game_root> [manifest.json]
//!
//! Defaults (run from `src-tauri/`): game_root =
//! `../storage/Mercenaries 2 World in Flames`, manifest =
//! `../manifests/mercs2.manifest.json`. Verifying the reference tree against its
//! own manifest must come back perfectly clean.

use std::path::PathBuf;

use mercs2_modkit_lib::commands::verify::{verify_install, Manifest};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "../storage/Mercenaries 2 World in Flames".into()),
    );
    let manifest_path =
        PathBuf::from(args.next().unwrap_or_else(|| "../manifests/mercs2.manifest.json".into()));

    let bytes = std::fs::read(&manifest_path)?;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    let source = manifest_path.display().to_string();

    println!("verifying {} against {source}…", root.display());
    let r = verify_install(&root, manifest, source);

    println!("\n--- report ---");
    println!("ok (matched files): {}", r.ok);
    println!("missing:            {}", r.missing.len());
    println!("corrupt:            {}", r.corrupt.len());
    println!("extra:              {}", r.extra.len());
    println!("ignored (excluded): {}", r.ignored);
    println!("wad_details:        {}", r.wad_details.len());
    for e in &r.exes {
        match &e.identified_as {
            Some(id) => println!("exe {}: {id} ✓", e.file),
            None => println!("exe {}: unrecognized", e.file),
        }
        for n in &e.notes {
            println!("    ↳ {n}");
        }
    }
    for m in r.missing.iter().take(10) {
        println!("  MISSING {m}");
    }
    for c in r.corrupt.iter().take(10) {
        println!("  CORRUPT {}", c.path);
    }

    let clean = r.missing.is_empty() && r.corrupt.is_empty() && r.wad_details.is_empty();
    println!("\n{}", if clean { "CLEAN ✓ — manifest validates the reference install" } else { "MISMATCH ✗" });
    if !clean {
        std::process::exit(1);
    }
    Ok(())
}
