//! Import a community-made, pre-built `vz-patch.wad` and merge it with everything else.
//!
//! Most mods that exist today were shipped as a finished `vz-patch.wad`. You can only have
//! one of those installed at a time, because the game loads exactly one patch per base WAD
//! — which is why "install two mods" has historically meant "pick one." Merging them is the
//! whole point of this feature.
//!
//! # Why merging is safe
//!
//! A patch WAD is an FFCS archive: an INDX of blocks, an ASET table mapping asset hashes to
//! `(block index, sub-entry)`, a PTHS path list, and the compressed block payloads. The
//! block index in an ASET row is **local to its own WAD** (the engine tracks which library
//! slot supplied a hit — `docs/patch_wad_globalenter_livelock_analysis.md` §14.1; §14's
//! "global index" claim was self-retracted). And `build_patch_wad_multi` rewrites every
//! ASET row's block index from the block's position in the output.
//!
//! So merging is: parse each WAD, take its blocks **together with their own ASET rows**,
//! concatenate in load order, rebuild. The indices fix themselves. The one thing we must
//! never do is split a block from its ASET rows.
//!
//! # What merging cannot fix
//!
//! Two mods that both ship a compiled `scripts_vz` block. That block is a whole-block
//! override, so the later one silently deletes the earlier one's Lua — no error, the mod
//! just doesn't work. There is no way to compose two *compiled* Lua blocks. We detect this
//! at import and say so, rather than letting it fail quietly in-game.

use std::path::Path;

use mercs2_formats::patch_wad::read_patch_wad;
use serde::{Deserialize, Serialize};

use crate::models::claim::ClaimGroup;

/// A pre-built patch WAD the user dropped in, described for the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrebuiltWad {
    /// Stable id (used in the load order).
    pub id: String,
    /// Display name (file stem by default).
    pub name: String,
    pub path: String,
    pub block_count: usize,
    /// How many assets this WAD claims (primary ASET rows).
    pub asset_count: usize,
    /// True if it ships a compiled `scripts_vz` block — see the module note.
    pub has_scripts: bool,
    /// Human note about anything that needs the user's attention.
    pub warnings: Vec<String>,
}

/// Inspect a patch WAD without importing it.
#[tauri::command(async)]
pub fn inspect_patch_wad(path: String) -> Result<PrebuiltWad, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("read {path}: {e}"))?;
    let contents = read_patch_wad(&bytes)
        .map_err(|e| format!("{path} is not a patch WAD: {e}"))?;

    let asset_count = contents
        .blocks
        .iter()
        .flat_map(|b| &b.aset_entries)
        .filter(|e| e.u32_2 & 0xFFFF == 0xFFFF)
        .count();

    let has_scripts = contents
        .blocks
        .iter()
        .any(|b| b.path_string.to_lowercase().contains("scripts_vz"));

    let mut warnings = Vec::new();
    if has_scripts {
        warnings.push(
            "This mod changes the game's scripts. Compiled scripts can't be combined with \
             another mod's — if you install two mods that both do this, only the last one \
             will work. Wardrobe outfits added in modkit are unaffected."
                .to_string(),
        );
    }

    let name = Path::new(&path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "patch".into());

    Ok(PrebuiltWad {
        id: format!("prebuilt:{name}"),
        name,
        path,
        block_count: contents.blocks.len(),
        asset_count,
        has_scripts,
        warnings,
    })
}

/// Turn an imported WAD into a single, atomic claim group.
///
/// It is one group, not one per block, on purpose: a shipped WAD is a finished thing whose
/// blocks were built to work together (a vehicle swap is a model *and* its textures *and* a
/// spawn script). Letting another mod win half of it would produce something the author
/// never made and never tested.
pub fn group_for(wad: &PrebuiltWad) -> Result<ClaimGroup, String> {
    let bytes = std::fs::read(&wad.path).map_err(|e| format!("read {}: {e}", wad.path))?;
    let contents =
        read_patch_wad(&bytes).map_err(|e| format!("{} is not a patch WAD: {e}", wad.path))?;

    // Blocks carry their own ASET rows through `read_patch_wad`, and the writer re-derives
    // each row's block index from its output position — so they stay correctly wired.
    Ok(ClaimGroup {
        mod_id: wad.id.clone(),
        mod_name: wad.name.clone(),
        label: wad.name.clone(),
        atomic: true,
        blocks: contents.blocks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mercs2_formats::patch_wad::{
        build_patch_wad_multi, validate_blocks, AsetEntry, PatchBlock, FFCS_CERT_BLOB,
    };

    fn wad_with(prefix: &str, hashes: &[u32]) -> Vec<u8> {
        let blocks: Vec<PatchBlock> = hashes
            .iter()
            .map(|&h| {
                PatchBlock::from_decompressed(
                    format!("payload {h}").as_bytes(),
                    format!("blocks\\{prefix}\\{h:08x}.block"),
                    vec![AsetEntry::new(h, 0xFFFF_FFFF, 0x0000_FFFF, 19)],
                    None,
                )
                .unwrap()
            })
            .collect();
        build_patch_wad_multi(&blocks, 0, Some(0), &FFCS_CERT_BLOB).unwrap()
    }

    /// The headline: two independent community WADs merge into one that the engine can
    /// resolve unambiguously.
    #[test]
    fn two_disjoint_prebuilt_wads_merge_into_one_valid_wad() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.wad");
        let b = dir.path().join("b.wad");
        std::fs::write(&a, wad_with("a", &[1, 2])).unwrap();
        std::fs::write(&b, wad_with("b", &[3, 4])).unwrap();

        let ga = group_for(&inspect_patch_wad(a.to_string_lossy().into()).unwrap()).unwrap();
        let gb = group_for(&inspect_patch_wad(b.to_string_lossy().into()).unwrap()).unwrap();

        let resolved = crate::models::claim::resolve(&[ga, gb]);
        assert!(resolved.conflicts.is_empty());
        assert_eq!(resolved.blocks.len(), 4, "all four blocks survive");

        // The invariant that makes the engine's choice well-defined.
        validate_blocks(&resolved.blocks).expect("one primary ASET row per hash");
        build_patch_wad_multi(&resolved.blocks, 0, Some(0), &FFCS_CERT_BLOB)
            .expect("merged WAD assembles");
    }

    /// Two WADs that override the same assets: the one later in the load order wins, whole.
    #[test]
    fn overlapping_prebuilt_wads_resolve_by_load_order() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("old.wad");
        let b = dir.path().join("new.wad");
        std::fs::write(&a, wad_with("old", &[7, 8])).unwrap();
        std::fs::write(&b, wad_with("new", &[7, 8])).unwrap();

        let ga = group_for(&inspect_patch_wad(a.to_string_lossy().into()).unwrap()).unwrap();
        let gb = group_for(&inspect_patch_wad(b.to_string_lossy().into()).unwrap()).unwrap();

        let resolved = crate::models::claim::resolve(&[ga, gb]);
        assert!(resolved.conflicts.is_empty());
        assert_eq!(resolved.blocks.len(), 2);
        for blk in &resolved.blocks {
            assert!(blk.path_string.contains("new"), "the LAST WAD in the order wins");
        }
        validate_blocks(&resolved.blocks).unwrap();
    }

    /// A WAD whose blocks round-trip must keep its `packed_field` (the decompression-buffer
    /// size). Losing it is a heap overrun in the engine.
    #[test]
    fn import_preserves_the_decompression_page_count() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.wad");

        // A block well over one 32 KB page.
        let big = vec![0x5Au8; 200_000];
        let blk = PatchBlock::from_decompressed(
            &big,
            "blocks\\big\\a.block".into(),
            vec![AsetEntry::new(0xAA, 0xFFFF_FFFF, 0x0000_FFFF, 19)],
            None,
        )
        .unwrap();
        let want_pages = blk.declared_pages();
        assert!(want_pages > 1);
        std::fs::write(
            &p,
            build_patch_wad_multi(&[blk], 0, Some(0), &FFCS_CERT_BLOB).unwrap(),
        )
        .unwrap();

        let g = group_for(&inspect_patch_wad(p.to_string_lossy().into()).unwrap()).unwrap();
        assert_eq!(
            g.blocks[0].declared_pages(),
            want_pages,
            "page count must survive the import round-trip"
        );
    }

    #[test]
    fn a_scripts_shipping_wad_is_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("skins.wad");
        let blk = PatchBlock::from_decompressed(
            b"lua-ish",
            "blocks\\VZ\\scripts_vz_P000_Q3.block".into(),
            vec![AsetEntry::new(0x99, 0xFFFF_FFFF, 0x0000_FFFF, 35)],
            None,
        )
        .unwrap();
        std::fs::write(
            &p,
            build_patch_wad_multi(&[blk], 0, Some(0), &FFCS_CERT_BLOB).unwrap(),
        )
        .unwrap();

        let info = inspect_patch_wad(p.to_string_lossy().into()).unwrap();
        assert!(info.has_scripts);
        assert!(!info.warnings.is_empty(), "the user must be told");
    }
}
