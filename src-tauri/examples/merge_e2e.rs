//! Merge two real, independently-built `vz-patch.wad` mods into one.
//!
//! ```text
//! cargo run --example merge_e2e -- a.wad b.wad out.wad
//! ```
//!
//! The game loads exactly one patch WAD, so historically these two mods could not be
//! installed together — you picked one. This drives the real import + resolve + assemble
//! path and asserts the merged WAD satisfies the invariant that makes the engine's asset
//! lookup well-defined: exactly one primary ASET row per asset hash.

use mercs2_formats::patch_wad::{build_patch_wad_multi, read_patch_wad, validate_blocks, FFCS_CERT_BLOB};
use mercs2_modkit_lib::commands::prebuilt::{group_for, inspect_patch_wad};
use mercs2_modkit_lib::models::claim::{resolve, GroupOutcome};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (a, b, out) = (&args[1], &args[2], &args[3]);

    println!("== importing ==");
    let mut groups = Vec::new();
    for p in [a, b] {
        let info = inspect_patch_wad(p.clone()).expect("inspect");
        println!(
            "  {:<20} {} blocks, {} assets{}",
            info.name,
            info.block_count,
            info.asset_count,
            if info.has_scripts { "  [ships scripts]" } else { "" }
        );
        for w in &info.warnings {
            println!("    ! {w}");
        }
        groups.push(group_for(&info).expect("group"));
    }

    // Load order: index 0 first, last one wins ties.
    println!("\n== resolving load order (later wins) ==");
    let r = resolve(&groups);
    for o in &r.outcomes {
        match o {
            GroupOutcome::Applied { label, asset_count, .. } => {
                println!("  applied     {label} ({asset_count} assets)")
            }
            GroupOutcome::Overridden { label, overridden_by_label, .. } => {
                println!("  overridden  {label}  <- {overridden_by_label}")
            }
            GroupOutcome::PartiallyApplied { label, applied, overridden, .. } => {
                println!("  partial     {label} ({applied} kept, {overridden} lost)")
            }
        }
    }
    for c in &r.conflicts {
        println!("  CONFLICT: {}", c.message);
    }
    assert!(r.conflicts.is_empty(), "these two mods do not overlap, so there must be no conflict");

    println!("\n== assembling ==");
    // This is the invariant: two primary rows for one hash would leave the engine's winner
    // undefined. The writer enforces it too, but assert it here so the failure is legible.
    validate_blocks(&r.blocks).expect("exactly one primary ASET row per asset hash");

    let wad = build_patch_wad_multi(&r.blocks, 0, Some(0), &FFCS_CERT_BLOB).expect("assemble");
    std::fs::write(out, &wad).expect("write");
    println!("  {} blocks -> {} ({} bytes)", r.blocks.len(), out, wad.len());

    // Re-read and prove every block survived with its ASET rows and page count.
    let back = read_patch_wad(&wad).expect("re-read merged wad");
    println!("  re-read: {} blocks", back.blocks.len());
    assert_eq!(back.blocks.len(), r.blocks.len());
    for blk in &back.blocks {
        assert!(
            blk.packed_field & 0x00FF_FFFF >= 1,
            "every block must declare a decompression page count"
        );
    }

    println!("\nOK — two separately-built mods merged into one loadable patch WAD.");
}
