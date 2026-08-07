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

use std::path::{Path, PathBuf};

use mercs2_formats::patch_wad::{build_patch_wad_multi, AsetEntry, PatchBlock, FFCS_CERT_BLOB};
use mercs2_formats::types::*;
use serde::{Deserialize, Serialize};
use tauri::Window;

use crate::commands::placement::StagedFile;
use crate::commands::prebuilt::{self, PrebuiltWad};
use crate::commands::shipment::{self, ShipmentRef};
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
    /// Workshop **Shipments** (qm source projects), **in load order** (later wins). Built and Lua-
    /// linked through `qm` so their scripts reconcile instead of clobbering — see
    /// [`crate::commands::shipment`].
    #[serde(default)]
    pub shipments: Vec<ShipmentRef>,
}

/// Result of an [`assemble_patch_wad`] call.
#[derive(Debug, Serialize)]
pub struct BuildResult {
    /// The built `vz-patch.wad`, or **empty** when the load order resolved to no blocks at all — a
    /// Shipment whose only contributions are `native_hook` / `place_file` is a real build with
    /// nothing to put in a WAD.
    pub path: String,
    /// The build output directory. Deploy reads its `placement.json`, so this is the handle that
    /// survives the WAD being absent.
    pub staging_dir: String,
    pub block_count: usize,
    pub byte_size: usize,
    /// sha256 of the bytes written — verify deployments by hash, never size/mtime.
    pub sha256: String,
    /// Per-group report: what applied, what was cleanly overridden.
    pub outcomes: Vec<GroupOutcome>,
    /// Non-fatal advisories surfaced to the user (e.g. a Shipment's scripts and the wardrobe both
    /// rebuild `scripts_vz`, so only one can win — see the known limitation in `shipment`).
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Loose files a Shipment places into the **game folder** — `native_hook` plugins and
    /// `place_file` companions. These are not WAD content, so they are staged beside `vz-patch.wad`
    /// and installed by `deploy_patch_wad`, which also writes them down so uninstall can undo them.
    ///
    /// Surfaced in the result so the user can see what a Shipment will drop into their game
    /// install before they install it. An `.asi` is unrestricted native code in the game process.
    #[serde(default)]
    pub placed_files: Vec<StagedFile>,
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
fn all_groups(options: &BuildOptions, include_wardrobe: bool) -> Result<Vec<ClaimGroup>, String> {
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

    if include_wardrobe && !options.wardrobe.is_empty() {
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
pub async fn assemble_patch_wad(
    window: Window,
    options: BuildOptions,
) -> Result<BuildResult, String> {
    let mut warnings: Vec<String> = Vec::new();

    // When any Shipment is staged, qm runs — so route the WARDROBE through qm too (as add_outfit
    // contributions with no model file), and drop modkit's own compiled scripts_vz block. That way
    // `qm link` reconciles wardrobe outfits AND every Shipment's Lua into ONE scripts_vz, instead of
    // one clobbering the other. With no Shipments, qm never runs and the wardrobe keeps its proven
    // standalone Rust path.
    let route_wardrobe_through_qm = !options.shipments.is_empty() && !options.wardrobe.is_empty();

    let mut groups = all_groups(&options, !route_wardrobe_through_qm)?;
    let mut placed_files: Vec<StagedFile> = Vec::new();

    if !options.shipments.is_empty() {
        let game_path = options
            .game_path
            .as_deref()
            .ok_or("Set the game folder before building Shipments.")?;
        let mut ship_refs = options.shipments.clone();
        if route_wardrobe_through_qm {
            if let Some(wr) = shipment::synthesize_wardrobe_shipment(&options.wardrobe)? {
                ship_refs.push(wr);
            }
        }
        let built = shipment::shipment_groups(window, &ship_refs, game_path, None).await?;
        groups.extend(built.groups);
        warnings.extend(built.warnings);
        placed_files = built.files;
    }

    let resolved = claim::resolve(&groups);

    if !resolved.conflicts.is_empty() {
        return Err(resolved
            .conflicts
            .iter()
            .map(|c| c.message.clone())
            .collect::<Vec<_>>()
            .join("\n\n"));
    }
    // A Shipment whose only contributions are `native_hook` / `place_file` produces no WAD content
    // at all, and it is still a build with something to install — so "nothing to build" is about the
    // union of both outputs, not the blocks alone.
    if resolved.blocks.is_empty() && placed_files.is_empty() {
        return Err("No assets to build (no mods loaded).".to_string());
    }

    let out_dir = match options.output_dir {
        Some(d) => PathBuf::from(d),
        None => crate::commands::paths::deployed_dir()?.join("build"),
    };
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| format!("Failed to create output dir {}: {e}", out_dir.display()))?;

    // The loose files, copied out of qm's scratch dirs into the build output so the build is a
    // self-contained artifact: `work_dir` wipes qm's output on the NEXT assemble, and deploy happens
    // whenever the user clicks. Staging here also means one record describes the whole build.
    let placed_files = stage_placements(&out_dir, placed_files)?;

    // A native-code-only Shipment resolves to no blocks at all, and `build_patch_wad_multi` would
    // have nothing to serialize. That is a real build with something to install, so it emits no
    // `vz-patch.wad` rather than an empty one — deploy installs the files and leaves whatever WAD is
    // already in the game alone.
    let (out_path, wad_bytes) = if resolved.blocks.is_empty() {
        (PathBuf::new(), Vec::new())
    } else {
        // csum_value = 0 and an explicit csum_meta = 0: correct for an assets-only patch WAD
        // that isn't derived from an Xbox source. Passing it explicitly stops an imported block
        // whose path ends in `\resident_p000_q3.block` from silently choosing it for us.
        let bytes = build_patch_wad_multi(&resolved.blocks, 0, Some(0), &FFCS_CERT_BLOB)?;
        let path = out_dir.join("vz-patch.wad");
        std::fs::write(&path, &bytes)
            .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
        (path, bytes)
    };

    Ok(BuildResult {
        path: out_path.to_string_lossy().to_string(),
        staging_dir: out_dir.to_string_lossy().to_string(),
        block_count: resolved.blocks.len(),
        byte_size: wad_bytes.len(),
        sha256: if wad_bytes.is_empty() {
            String::new()
        } else {
            loadprobe::sha256::sha256_hex(&wad_bytes)
        },
        outcomes: resolved.outcomes,
        warnings,
        placed_files,
    })
}

/// Copy the Shipments' loose files into `<out_dir>/files/<relative>` and write modkit's own
/// `placement.json` beside them, returning the staged files re-pointed at their new sources.
///
/// The tree mirrors the game folder, exactly as qm's own output does, so the destination is legible
/// from the staged path and a human can look at the build directory and see what will land where.
/// The record is what `deploy_patch_wad` reads — build and deploy are separate steps by design, and
/// a file with no record is a file nothing can install or take back out.
fn stage_placements(out_dir: &Path, files: Vec<StagedFile>) -> Result<Vec<StagedFile>, String> {
    let stage_root = out_dir.join("files");
    // Clear first, so a file dropped from the load order since the last build cannot linger and get
    // re-installed as though it were still staged.
    let _ = std::fs::remove_dir_all(&stage_root);
    let record_path = out_dir.join(crate::commands::placement::PLACEMENT_FILE);
    let _ = std::fs::remove_file(&record_path);
    if files.is_empty() {
        return Ok(Vec::new());
    }
    std::fs::create_dir_all(&stage_root)
        .map_err(|e| format!("Failed to create {}: {e}", stage_root.display()))?;

    let mut staged = Vec::with_capacity(files.len());
    for file in files {
        let dest = stage_root.join(&file.relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
        }
        std::fs::copy(&file.source, &dest)
            .map_err(|e| format!("Failed to stage {}: {e}", file.relative))?;
        staged.push(StagedFile {
            source: dest.to_string_lossy().to_string(),
            ..file
        });
    }

    let record = serde_json::to_string_pretty(&serde_json::json!({
        "format": 1,
        "files": &staged,
    }))
    .map_err(|e| format!("Failed to describe the staged files: {e}"))?;
    std::fs::write(&record_path, record)
        .map_err(|e| format!("Failed to write {}: {e}", record_path.display()))?;
    Ok(staged)
}

/// Dry-run the load order: report what would apply, what would be overridden, and any
/// unresolvable overlap — without writing anything.
#[tauri::command(async)]
pub fn preview_conflicts(options: BuildOptions) -> Result<BuildResult, BuildConflicts> {
    // Preview covers the in-memory kinds (including the wardrobe on its Rust path); Shipment groups
    // and any qm-routed wardrobe need qm and are resolved at assemble.
    let groups = match all_groups(&options, true) {
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
        staging_dir: String::new(),
        block_count: resolved.blocks.len(),
        byte_size: 0,
        sha256: String::new(),
        outcomes: resolved.outcomes,
        // Preview covers the in-memory kinds; Shipment conflicts surface at assemble (they need qm).
        warnings: Vec::new(),
        // Same: the placements come out of a qm build, which preview deliberately does not run.
        placed_files: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::placement;

    /// Write the output directory a `qm build` of a `native_hook` Shipment leaves behind: the
    /// plugin under the tree it will be copied into, and the record describing it.
    fn qm_output(dir: &Path, wad: Option<&str>, files: &[(&str, &str)]) {
        std::fs::create_dir_all(dir).unwrap();
        let mut entries: Vec<serde_json::Value> = Vec::new();
        if let Some(name) = wad {
            std::fs::write(dir.join(name), b"wad bytes").unwrap();
            entries.push(serde_json::json!({
                "name": name,
                "bytes": 9,
                "sha256": loadprobe::sha256::sha256_hex(b"wad bytes"),
                "destination": { "kind": "overlay" },
            }));
        }
        for (relative, body) in files {
            let path = dir.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
            entries.push(serde_json::json!({
                "name": relative.rsplit('/').next().unwrap(),
                "bytes": body.len(),
                "sha256": loadprobe::sha256::sha256_hex(body.as_bytes()),
                "destination": { "kind": "game_folder", "relative": relative },
            }));
        }
        let record = serde_json::json!({ "format": 1, "placements": entries });
        std::fs::write(
            dir.join(placement::PLACEMENT_FILE),
            serde_json::to_string_pretty(&record).unwrap(),
        )
        .unwrap();
    }

    /// The chain from a qm output directory to an installable build: the record is read, the files
    /// are copied into the build output mirroring the game folder, and modkit's own record round
    /// trips. This is the link that used to be missing entirely.
    #[test]
    fn a_qm_output_stages_into_an_installable_build() {
        let tmp = tempfile::tempdir().unwrap();
        let (qm_out, build) = (tmp.path().join("qm"), tmp.path().join("build"));
        qm_output(
            &qm_out,
            Some("my-shipment.wad"),
            &[
                ("scripts/hook.asi", "MZ plugin"),
                ("scripts/OnBoot/init.lua", "-- boot"),
            ],
        );
        std::fs::create_dir_all(&build).unwrap();

        let (wad, files) = placement::read_output(&qm_out, "Hooky").unwrap();
        assert_eq!(wad.unwrap().file_name().unwrap(), "my-shipment.wad");
        assert_eq!(files.len(), 2);

        let staged = stage_placements(&build, files).unwrap();
        // The staged tree mirrors the game folder, so the destination is legible from the path.
        assert!(build.join("files/scripts/hook.asi").is_file());
        assert!(build.join("files/scripts/OnBoot/init.lua").is_file());
        for f in &staged {
            assert!(Path::new(&f.source).starts_with(&build));
            assert_eq!(f.shipment, "Hooky");
        }

        // And the record deploy reads describes exactly what was staged.
        let read_back = placement::read_staged(&build).unwrap();
        assert_eq!(read_back.len(), 2);
        assert_eq!(
            read_back.iter().map(|f| f.relative.clone()).collect::<Vec<_>>(),
            vec!["scripts/hook.asi", "scripts/OnBoot/init.lua"]
        );
        assert_eq!(read_back[0].sha256, loadprobe::sha256::sha256_hex(b"MZ plugin"));
    }

    /// A rebuild that no longer places a file must not leave it staged: deploy reads the record,
    /// and a stale entry would install something the load order no longer contains.
    #[test]
    fn restaging_clears_what_the_previous_build_left() {
        let tmp = tempfile::tempdir().unwrap();
        let (qm_out, build) = (tmp.path().join("qm"), tmp.path().join("build"));
        std::fs::create_dir_all(&build).unwrap();

        qm_output(&qm_out, None, &[("scripts/gone.asi", "old")]);
        let (_, files) = placement::read_output(&qm_out, "S").unwrap();
        stage_placements(&build, files).unwrap();
        assert!(build.join("files/scripts/gone.asi").is_file());

        stage_placements(&build, Vec::new()).unwrap();
        assert!(!build.join("files/scripts/gone.asi").exists());
        assert!(placement::read_staged(&build).unwrap().is_empty());
    }
}
