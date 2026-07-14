//! Patch-WAD assembly: resolve the load order, then emit one `vz-patch.wad`.
//!
//! Each mod contributes [`ClaimGroup`]s; `claim::resolve` decides who wins (last in the
//! load order) and hands back a coherent block list, which `build_patch_wad_multi`
//! serializes. The writer re-validates the result before a byte reaches disk.
//!
//! ## What used to be here, and why it's gone
//!
//! * **`split_by_patch` / `target_patch`** emitted `scripts-patch.wad`, `assets-patch.wad`
//!   and friends. The engine mounts `vz.wad` and then `vz-patch.wad` — it never opens
//!   those files. Their output was inert: the mod appeared to build and did nothing.
//! * **`merge_into`** called `merge_patch_wads(existing, blocks, replace=false)`, which
//!   appends unconditionally. If the target already claimed an asset the new block also
//!   claimed, the result carried **two primary ASET rows for one hash** and the engine's
//!   winner was undefined. Resolution now happens before assembly, so there is nothing to
//!   merge into.
//! * **First-wins dedupe** (`seen.insert`) kept the *earliest* mod's asset. The engine is
//!   last-wins. See [`crate::models::claim`].

use std::path::PathBuf;

use mercs2_formats::patch_wad::{build_patch_wad_multi, AsetEntry, PatchBlock, FFCS_CERT_BLOB};
use mercs2_formats::types::*;
use serde::{Deserialize, Serialize};

use crate::commands::prebuilt::{self, PrebuiltWad};
use crate::commands::texture_swap::{self, TextureSwap};
use crate::commands::wardrobe::{self, WardrobeOutfit};
use crate::models::claim::{self, ClaimConflict, ClaimGroup, GroupOutcome};
use crate::models::project::{DetectedAsset, LoadedMod};

/// Options controlling a build, supplied by the frontend.
#[derive(Debug, Deserialize)]
pub struct BuildOptions {
    /// Mods to include, **in load order**: index 0 loads first; the LAST entry wins ties.
    pub mods: Vec<LoadedMod>,
    /// Where to write `vz-patch.wad`. Defaults to the app's managed staging dir.
    ///
    /// Building never targets the game directly — `deploy_patch_wad` installs, and it
    /// snapshots whatever it replaces. (The old screen defaulted this to the game's own
    /// `data/` dir and clobbered the live `vz-patch.wad` with no copy kept.)
    #[serde(default)]
    pub output_dir: Option<String>,
    /// Game install root — required only when `wardrobe` is non-empty (we read the user's
    /// own `vz.wad` to source the scripts block and to validate every model name).
    #[serde(default)]
    pub game_path: Option<String>,
    /// Extra wardrobe outfits to add. modkit owns the `scripts_vz` block and unions every
    /// mod's Lua into it, so several wardrobe mods compose instead of clobbering.
    #[serde(default)]
    pub wardrobe: Vec<WardrobeOutfit>,
    /// Pre-built community `vz-patch.wad`s, **in load order** (later wins), merged in
    /// alongside the asset mods. Each is one atomic group.
    #[serde(default)]
    pub prebuilt: Vec<PrebuiltWad>,
    /// Texture replacements (donor BODY-swaps against the user's own `vz.wad`).
    #[serde(default)]
    pub textures: Vec<TextureSwap>,
}

/// Result of an [`assemble_patch_wad`] call.
#[derive(Debug, Serialize)]
pub struct BuildResult {
    pub path: String,
    pub block_count: usize,
    pub byte_size: usize,
    /// sha256 of the bytes written — verify deployments by hash, never size/mtime.
    pub sha256: String,
    /// Per-group report: what applied, what was cleanly overridden.
    pub outcomes: Vec<GroupOutcome>,
}

/// A build refused because the load order is incoherent.
#[derive(Debug, Serialize)]
pub struct BuildConflicts {
    pub conflicts: Vec<ClaimConflict>,
}

/// Map a detected type name to its ASET `type_id` (0 = singleton/unknown).
fn type_id_for_name(name: &str) -> u32 {
    match name {
        "script" => TYPE_ID_SCRIPT,
        "stringdb" => TYPE_ID_STRINGDB,
        "texture" => TYPE_ID_TEXTURE,
        "model" => TYPE_ID_MODEL,
        "animation" => TYPE_ID_ANIMATION,
        "layer" => TYPE_ID_LAYER,
        "material_params" => TYPE_ID_MATERIAL_PARAMS,
        "font" => TYPE_ID_FONT,
        _ => 0,
    }
}

/// Turn one declared asset into a single-entry, by-hash override block.
///
/// `from_decompressed` is what sets `packed_field` to the block's real decompressed page
/// count. The old code used `PatchBlock::new`, which leaves it at the placeholder `1` —
/// and that word sizes the engine's decompression buffer (`pages << 15` = 32 KB), so any
/// asset above 32 KB overran the heap at load.
fn build_block(mod_id: &str, asset: &DetectedAsset) -> Result<PatchBlock, String> {
    let raw = std::fs::read(&asset.abs_path)
        .map_err(|e| format!("Failed to read asset {}: {e}", asset.abs_path))?;

    // Primary, by-hash ASET row: u32_1 = 0xFFFFFFFF, u32_2 low16 = 0xFFFF (resolve-by-hash;
    // the high16 block index is filled in by the writer from the block's output position).
    let aset = AsetEntry::new(asset.asset_hash, 0xFFFF_FFFF, 0x0000_FFFF, type_id_for_name(&asset.detected_type));

    // Scope the path by mod id: two mods overriding the same asset would otherwise emit
    // the same `path_string`, and a path ending in `\resident_p000_q3.block` additionally
    // hijacks the writer's `csum_meta` auto-detect.
    let path_string = format!(
        "blocks\\modkit\\{}\\{}.block",
        mod_id,
        asset.name.replace('/', "_")
    );

    PatchBlock::from_decompressed(&raw, path_string, vec![aset], None)
}

/// One ClaimGroup per mod: everything a mod ships wins or loses together.
///
/// (When recipe ops land, a mod will emit one group *per op* instead, so an unrelated
/// texture tweak in the same mod doesn't drag a model swap down with it.)
fn groups_for(mods: &[LoadedMod]) -> Result<Vec<ClaimGroup>, String> {
    mods.iter()
        .map(|m| {
            let blocks = m
                .assets
                .iter()
                .map(|a| build_block(&m.id, a))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ClaimGroup {
                mod_id: m.id.clone(),
                mod_name: m.manifest.name.clone(),
                label: m.manifest.name.clone(),
                atomic: true,
                blocks,
            })
        })
        .collect()
}

/// Assemble `vz-patch.wad` from the resolved load order.
///
/// Returns `Err` with a human-readable message on an unresolvable load order (a proper
/// partial overlap between two mods) — the frontend surfaces the structured conflicts via
/// [`preview_conflicts`].
/// Every claim group for a build, in load order (later wins).
///
/// Order is: imported pre-built WADs → asset mods → the wardrobe. The wardrobe goes last
/// deliberately: it rebuilds `scripts_vz` from the user's own `vz.wad`, so if a pre-built
/// mod also ships that block, modkit's version — which actually contains the user's
/// outfits — is the one that survives.
fn all_groups(options: &BuildOptions) -> Result<Vec<ClaimGroup>, String> {
    let mut groups = Vec::new();

    for w in &options.prebuilt {
        groups.push(prebuilt::group_for(w)?);
    }
    groups.extend(groups_for(&options.mods)?);

    // Texture swaps: one group each, so two mods replacing *different* textures both apply
    // and two replacing the *same* one resolve cleanly by load order.
    if !options.textures.is_empty() {
        let game_path = options
            .game_path
            .as_deref()
            .ok_or("Set the game folder before swapping textures.")?;
        for swap in &options.textures {
            groups.push(ClaimGroup {
                mod_id: format!("modkit-tex:{}", swap.name),
                mod_name: "Texture swap".into(),
                label: format!("Texture: {}", swap.name),
                atomic: true,
                blocks: vec![texture_swap::swap_block(game_path, swap)?],
            });
        }
    }

    if !options.wardrobe.is_empty() {
        let game_path = options
            .game_path
            .as_deref()
            .ok_or("Set the game folder before building wardrobe outfits.")?;
        if let Some(block) = wardrobe::wardrobe_block(game_path, &options.wardrobe)? {
            groups.push(ClaimGroup {
                mod_id: "modkit-wardrobe".into(),
                mod_name: "Wardrobe".into(),
                label: format!("Wardrobe ({} outfit(s))", options.wardrobe.len()),
                atomic: true,
                blocks: vec![block],
            });
        }
    }

    Ok(groups)
}

#[tauri::command]
pub fn assemble_patch_wad(options: BuildOptions) -> Result<BuildResult, String> {
    let groups = all_groups(&options)?;
    let resolved = claim::resolve(&groups);

    if !resolved.conflicts.is_empty() {
        return Err(resolved
            .conflicts
            .iter()
            .map(|c| c.message.clone())
            .collect::<Vec<_>>()
            .join("\n\n"));
    }
    if resolved.blocks.is_empty() {
        return Err("No assets to build (no mods loaded).".to_string());
    }

    // csum_value = 0 and an explicit csum_meta = 0: correct for an assets-only patch WAD
    // that isn't derived from an Xbox source. Passing it explicitly stops an imported block
    // whose path ends in `\resident_p000_q3.block` from silently choosing it for us.
    let wad_bytes = build_patch_wad_multi(&resolved.blocks, 0, Some(0), &FFCS_CERT_BLOB)?;

    let out_dir = match options.output_dir {
        Some(d) => PathBuf::from(d),
        None => crate::commands::paths::deployed_dir()?.join("build"),
    };
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| format!("Failed to create output dir {}: {e}", out_dir.display()))?;
    let out_path = out_dir.join("vz-patch.wad");
    std::fs::write(&out_path, &wad_bytes)
        .map_err(|e| format!("Failed to write {}: {e}", out_path.display()))?;

    Ok(BuildResult {
        path: out_path.to_string_lossy().to_string(),
        block_count: resolved.blocks.len(),
        byte_size: wad_bytes.len(),
        sha256: loadprobe::sha256::sha256_hex(&wad_bytes),
        outcomes: resolved.outcomes,
    })
}

/// Dry-run the load order: report what would apply, what would be overridden, and any
/// unresolvable overlap — without writing anything.
#[tauri::command]
pub fn preview_conflicts(options: BuildOptions) -> Result<BuildResult, BuildConflicts> {
    let groups = match all_groups(&options) {
        Ok(g) => g,
        // A missing asset file or an invalid outfit isn't a claim conflict; report it as
        // one entry so the UI shows the message rather than failing silently.
        Err(e) => {
            return Err(BuildConflicts {
                conflicts: vec![ClaimConflict {
                    mod_id: String::new(),
                    label: "load error".into(),
                    other_mod_id: String::new(),
                    other_label: String::new(),
                    shared: vec![],
                    only_mine: vec![],
                    message: e,
                }],
            })
        }
    };

    let resolved = claim::resolve(&groups);
    if !resolved.conflicts.is_empty() {
        return Err(BuildConflicts {
            conflicts: resolved.conflicts,
        });
    }

    Ok(BuildResult {
        path: String::new(),
        block_count: resolved.blocks.len(),
        byte_size: 0,
        sha256: String::new(),
        outcomes: resolved.outcomes,
    })
}
