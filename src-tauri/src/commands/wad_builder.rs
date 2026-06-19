//! Patch-WAD assembly: compress assets, build blocks, emit `vz-patch.wad`(s).
//!
//! Each declared asset becomes one SGES-compressed block carrying a single
//! by-hash ASET entry. Assets are grouped by their `target_patch`, deduped by
//! hash (first-write-wins), and either written as fresh patch WADs or merged
//! into an existing one via [`merge_patch_wads`].

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use mercs2_formats::patch_wad::{
    build_patch_wad_multi, merge_patch_wads, AsetEntry, PatchBlock, FFCS_CERT_BLOB,
};
use mercs2_formats::sges::compress_sges;
use mercs2_formats::types::*;
use serde::{Deserialize, Serialize};

use crate::models::project::{DetectedAsset, LoadedMod};

/// Options controlling a build, supplied by the frontend.
#[derive(Debug, Deserialize)]
pub struct BuildOptions {
    /// Mods to include, in priority order (earlier wins ties on a shared hash).
    pub mods: Vec<LoadedMod>,
    /// Asset hashes to drop entirely (from conflict resolution `Exclude`).
    #[serde(default)]
    pub excluded_assets: Vec<u32>,
    /// Directory to write the resulting WAD(s) into.
    pub output_dir: String,
    /// Emit one WAD per `target_patch` group instead of a single `vz-patch.wad`.
    #[serde(default)]
    pub split_by_patch: bool,
    /// Optional existing `vz-patch.wad` to merge the new blocks into.
    #[serde(default)]
    pub merge_into: Option<String>,
}

/// One WAD produced by a build.
#[derive(Debug, Serialize)]
pub struct BuiltWad {
    pub path: String,
    pub patch_group: String,
    pub block_count: usize,
    pub byte_size: usize,
}

/// Result of an [`assemble_patch_wad`] call.
#[derive(Debug, Serialize)]
pub struct BuildResult {
    pub outputs: Vec<BuiltWad>,
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

/// Turn one detected asset into a single-entry patch block.
fn build_block(asset: &DetectedAsset) -> Result<PatchBlock, String> {
    let raw = std::fs::read(&asset.abs_path)
        .map_err(|e| format!("Failed to read asset {}: {e}", asset.abs_path))?;
    let compressed = compress_sges(&raw)
        .map_err(|e| format!("SGES compression failed for {}: {e}", asset.name))?;

    let type_id = type_id_for_name(&asset.detected_type);
    // Primary, by-hash ASET entry: u32_1 = 0xFFFFFFFF, u32_2 low16 = 0xFFFF
    // (resolve-by-hash; high16 is overwritten with the block index on write).
    let aset = AsetEntry::new(asset.asset_hash, 0xFFFF_FFFF, 0x0000_FFFF, type_id);
    let path_string = format!("blocks\\modkit\\{}.block", asset.name.replace('/', "_"));

    Ok(PatchBlock::new(compressed, path_string, vec![aset]))
}

/// Assemble one or more patch WADs from the resolved project.
#[tauri::command]
pub fn assemble_patch_wad(options: BuildOptions) -> Result<BuildResult, String> {
    let excluded: HashSet<u32> = options.excluded_assets.into_iter().collect();

    // Group resolved assets by target patch, deduping by hash (first wins).
    let mut groups: BTreeMap<String, Vec<DetectedAsset>> = BTreeMap::new();
    let mut seen: HashSet<u32> = HashSet::new();
    for m in &options.mods {
        for a in &m.assets {
            if excluded.contains(&a.asset_hash) || !seen.insert(a.asset_hash) {
                continue;
            }
            let group = if options.split_by_patch {
                a.target_patch.clone()
            } else {
                "vz-patch".to_string()
            };
            groups.entry(group).or_default().push(a.clone());
        }
    }

    if groups.is_empty() {
        return Err("No assets to build (all excluded or no mods loaded).".to_string());
    }

    std::fs::create_dir_all(&options.output_dir)
        .map_err(|e| format!("Failed to create output dir {}: {e}", options.output_dir))?;

    let mut outputs = Vec::new();
    for (group, assets) in groups {
        let blocks: Vec<PatchBlock> = assets
            .iter()
            .map(build_block)
            .collect::<Result<Vec<_>, _>>()?;
        let block_count = blocks.len();

        // csum_value=0 / csum_meta=None: correct for assets-only patch WADs not
        // derived from an Xbox source (the builder auto-detects csum_meta).
        let wad_bytes = if let Some(existing_path) = &options.merge_into {
            let existing = std::fs::read(existing_path)
                .map_err(|e| format!("Failed to read merge target {existing_path}: {e}"))?;
            merge_patch_wads(&existing, blocks, false)?
        } else {
            build_patch_wad_multi(&blocks, 0, None, &FFCS_CERT_BLOB)
        };

        let filename = if options.split_by_patch {
            format!("{group}-patch.wad")
        } else {
            "vz-patch.wad".to_string()
        };
        let out_path = PathBuf::from(&options.output_dir).join(&filename);
        std::fs::write(&out_path, &wad_bytes)
            .map_err(|e| format!("Failed to write {}: {e}", out_path.display()))?;

        outputs.push(BuiltWad {
            path: out_path.to_string_lossy().to_string(),
            patch_group: group,
            block_count,
            byte_size: wad_bytes.len(),
        });
    }

    Ok(BuildResult { outputs })
}
