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

use crate::commands::incompatibility::IncompatibilityCheck;
use crate::commands::mercsink;
use crate::commands::placement::{self, StagedBuild, StagedFile, StreamCopy};
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
    ///
    /// The merged `data/<language>-patch.wad` and `data/shell-patch.wad` are among them.
    #[serde(default)]
    pub placed_files: Vec<StagedFile>,
    /// Copies of game data the deploy step makes inside the game folder (`stream_copy`
    /// placements), each verified against the digest qm recorded before it is made.
    pub stream_copies: Vec<StreamCopy>,
    /// What mercs.ink's community incompatibility list said about the Shipments: which list was
    /// used (current, a cached copy and its age, or none ever downloaded) and the unconfirmed
    /// reports that apply. A confirmed report refuses the build, so it never appears here.
    /// `None` when the build has no Shipments, since the list is not consulted.
    pub incompatibilities: Option<IncompatibilityCheck>,
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
    let mut merged: Vec<MergedWad> = Vec::new();
    let mut stream_copies: Vec<StreamCopy> = Vec::new();
    let mut incompatibilities: Option<IncompatibilityCheck> = None;

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
        let list = mercsink::load_incompatibility_list().await?;
        let built =
            shipment::shipment_groups(window, &ship_refs, game_path, None, list.index.as_ref()).await?;
        groups.extend(built.groups);
        warnings.extend(built.warnings);
        placed_files = built.files;
        merged = merge_patches(&built.language_patches, &built.shell_patch)?;
        stream_copies = built.stream_copies;
        incompatibilities = Some(IncompatibilityCheck { list: list.state, notices: built.notices });
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
    if resolved.blocks.is_empty()
        && placed_files.is_empty()
        && merged.is_empty()
        && stream_copies.is_empty()
    {
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
    let staged = stage_placements(&out_dir, placed_files, merged, stream_copies)?;

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
        placed_files: staged.files,
        stream_copies: staged.stream_copies,
        incompatibilities,
    })
}

/// A patch WAD merged from every Shipment's patch for one engine-mounted target.
#[derive(Debug)]
struct MergedWad {
    /// Destination under the game folder: `data/<language>-patch.wad` or `data/shell-patch.wad`.
    relative: String,
    /// The language token, for a language patch.
    language: Option<String>,
    /// The Shipments whose groups the WAD carries blocks from, in load order.
    shipments: String,
    bytes: Vec<u8>,
}

/// Resolve and serialize one target's claim groups, exactly as `vz-patch.wad` is: `claim::resolve`
/// (last in load order wins, an atomic partial overlap refuses the build) and
/// `build_patch_wad_multi`. `None` when the target has no groups.
fn merge_target(groups: &[ClaimGroup], target: &str) -> Result<Option<(Vec<u8>, String)>, String> {
    if groups.is_empty() {
        return Ok(None);
    }
    let resolved = claim::resolve(groups);
    if !resolved.conflicts.is_empty() {
        return Err(format!(
            "{target} cannot be merged:\n\n{}",
            resolved
                .conflicts
                .iter()
                .map(|c| c.message.clone())
                .collect::<Vec<_>>()
                .join("\n\n")
        ));
    }
    let bytes = build_patch_wad_multi(&resolved.blocks, 0, Some(0), &FFCS_CERT_BLOB)
        .map_err(|e| format!("building {target}: {e}"))?;
    let mut shipments: Vec<&str> = Vec::new();
    for group in groups {
        if !shipments.contains(&group.mod_name.as_str()) {
            shipments.push(&group.mod_name);
        }
    }
    Ok(Some((bytes, shipments.join(", "))))
}

/// Merge every language's patch groups into `data/<language>-patch.wad` and the shell patch groups
/// into `data/shell-patch.wad`.
fn merge_patches(
    language_patches: &std::collections::BTreeMap<String, Vec<ClaimGroup>>,
    shell_patch: &[ClaimGroup],
) -> Result<Vec<MergedWad>, String> {
    let mut merged = Vec::new();
    for (language, groups) in language_patches {
        let relative = format!("data/{language}-patch.wad");
        if let Some((bytes, shipments)) = merge_target(groups, &relative)? {
            merged.push(MergedWad {
                relative,
                language: Some(language.clone()),
                shipments,
                bytes,
            });
        }
    }
    let relative = "data/shell-patch.wad".to_string();
    if let Some((bytes, shipments)) = merge_target(shell_patch, &relative)? {
        merged.push(MergedWad {
            relative,
            language: None,
            shipments,
            bytes,
        });
    }
    Ok(merged)
}

/// Copy the Shipments' loose files into `<out_dir>/files/<relative>`, write the merged language and
/// shell patch WADs into the same tree, and write modkit's own `placement.json` beside them —
/// returning the staged build with every source re-pointed into the build directory.
///
/// The tree mirrors the game folder, exactly as qm's own output does, so the destination is legible
/// from the staged path and a human can look at the build directory and see what will land where.
/// The record is what `deploy_patch_wad` reads — build and deploy are separate steps by design, and
/// a file with no record is a file nothing can install or take back out.
///
/// Every destination is claimed once: a merged WAD or a stream copy landing on a path something
/// else is staged at is refused, compared case-insensitively because the game's filesystem is.
fn stage_placements(
    out_dir: &Path,
    files: Vec<StagedFile>,
    merged: Vec<MergedWad>,
    stream_copies: Vec<StreamCopy>,
) -> Result<StagedBuild, String> {
    let stage_root = out_dir.join("files");
    // Clear first, so a file dropped from the load order since the last build cannot linger and get
    // re-installed as though it were still staged.
    let _ = std::fs::remove_dir_all(&stage_root);
    let record_path = out_dir.join(placement::PLACEMENT_FILE);
    let _ = std::fs::remove_file(&record_path);
    if files.is_empty() && merged.is_empty() && stream_copies.is_empty() {
        return Ok(StagedBuild::default());
    }

    let mut claimed: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut claim = |relative: &str, by: String| -> Result<(), String> {
        match claimed.insert(relative.to_ascii_lowercase(), by.clone()) {
            Some(other) => Err(format!(
                "{relative} is placed twice in one build, by {other} and by {by}. One path holds \
                 one file: take one of them out of the load order."
            )),
            None => Ok(()),
        }
    };
    for file in &files {
        claim(&file.relative, format!("“{}”", file.shipment))?;
    }
    for wad in &merged {
        claim(&wad.relative, format!("the merged patch of “{}”", wad.shipments))?;
    }
    for copy in &stream_copies {
        claim(&copy.to, format!("“{}”'s copy of {}", copy.shipment, copy.from))?;
    }

    std::fs::create_dir_all(&stage_root)
        .map_err(|e| format!("Failed to create {}: {e}", stage_root.display()))?;
    let make_parent = |dest: &Path| -> Result<(), String> {
        match dest.parent() {
            Some(parent) => std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create {}: {e}", parent.display())),
            None => Ok(()),
        }
    };

    let mut staged = Vec::with_capacity(files.len() + merged.len());
    for file in files {
        let dest = stage_root.join(&file.relative);
        make_parent(&dest)?;
        std::fs::copy(&file.source, &dest)
            .map_err(|e| format!("Failed to stage {}: {e}", file.relative))?;
        staged.push(StagedFile {
            source: dest.to_string_lossy().to_string(),
            ..file
        });
    }
    for wad in merged {
        let dest = stage_root.join(&wad.relative);
        make_parent(&dest)?;
        std::fs::write(&dest, &wad.bytes)
            .map_err(|e| format!("Failed to write {}: {e}", dest.display()))?;
        staged.push(StagedFile {
            source: dest.to_string_lossy().to_string(),
            sha256: loadprobe::sha256::sha256_hex(&wad.bytes),
            relative: wad.relative,
            shipment: wad.shipments,
            display: None,
            language: wad.language,
        });
    }

    let build = StagedBuild {
        files: staged,
        stream_copies,
    };
    std::fs::write(&record_path, placement::staged_record_json(&build)?)
        .map_err(|e| format!("Failed to write {}: {e}", record_path.display()))?;
    Ok(build)
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
        stream_copies: Vec::new(),
        // The list is consulted only by a build with Shipments, and preview builds none.
        incompatibilities: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let record = serde_json::json!({ "format": 2, "placements": entries });
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

        let output = placement::read_output(&qm_out, "Hooky").unwrap();
        assert_eq!(output.overlay.unwrap().file_name().unwrap(), "my-shipment.wad");
        assert_eq!(output.files.len(), 2);

        let staged = stage_placements(&build, output.files, Vec::new(), Vec::new()).unwrap();
        // The staged tree mirrors the game folder, so the destination is legible from the path.
        assert!(build.join("files/scripts/hook.asi").is_file());
        assert!(build.join("files/scripts/OnBoot/init.lua").is_file());
        for f in &staged.files {
            assert!(Path::new(&f.source).starts_with(&build));
            assert_eq!(f.shipment, "Hooky");
        }

        // And the record deploy reads describes exactly what was staged.
        let read_back = placement::read_staged(&build).unwrap();
        assert_eq!(read_back.files.len(), 2);
        assert_eq!(
            read_back.files.iter().map(|f| f.relative.clone()).collect::<Vec<_>>(),
            vec!["scripts/hook.asi", "scripts/OnBoot/init.lua"]
        );
        assert_eq!(read_back.files[0].sha256, loadprobe::sha256::sha256_hex(b"MZ plugin"));
    }

    use mercs2_formats::patch_wad::{read_patch_wad, PatchBlock};

    fn block(path: &str, hash: u32, payload: &str) -> PatchBlock {
        PatchBlock::from_decompressed(
            payload.as_bytes(),
            path.to_string(),
            vec![AsetEntry::new(hash, 0xFFFF_FFFF, 0x0000_FFFF, 19)],
            None,
        )
        .unwrap()
    }

    fn group(mod_id: &str, name: &str, blocks: Vec<PatchBlock>) -> ClaimGroup {
        ClaimGroup {
            mod_id: mod_id.into(),
            mod_name: name.into(),
            label: name.into(),
            atomic: true,
            blocks,
        }
    }

    /// Two Shipments' `english` language patches and link's, merged into one
    /// `data/english-patch.wad`: the assets link patches are link's (last wins), the other
    /// Shipment's asset survives, and the shell groups become `data/shell-patch.wad`. Both are real
    /// patch WADs, staged with a digest of their bytes and recorded for deploy.
    #[test]
    fn language_and_shell_patches_merge_into_their_wads() {
        let tmp = tempfile::tempdir().unwrap();
        let build = tmp.path().join("build");
        std::fs::create_dir_all(&build).unwrap();

        let mut languages = std::collections::BTreeMap::new();
        languages.insert(
            "english".to_string(),
            vec![
                group(
                    "shipment:a",
                    "A",
                    vec![
                        block(r"blocks\a\bank.block", 0xB0, "a bank"),
                        block(r"blocks\a\vo.block", 0xA1, "a vo"),
                    ],
                ),
                group("shipment:b", "B", vec![block(r"blocks\b\vo.block", 0xB1, "b vo")]),
                group(
                    "qm-link:english-patch.wad",
                    "Quartermaster link",
                    vec![
                        block(r"blocks\qm\bank.block", 0xB0, "merged bank"),
                        block(r"blocks\qm\vo.block", 0xA1, "merged vo"),
                    ],
                ),
            ],
        );
        let shell = vec![
            group("shipment:a", "A", vec![block(r"blocks\a\menu.block", 0x51, "a menu")]),
            group("shipment:b", "B", vec![block(r"blocks\b\menu.block", 0x51, "b menu")]),
        ];

        // A's group is fully overridden by link's (both of its assets), so it resolves cleanly.
        let merged = merge_patches(&languages, &shell).unwrap();
        assert_eq!(
            merged.iter().map(|m| m.relative.as_str()).collect::<Vec<_>>(),
            vec!["data/english-patch.wad", "data/shell-patch.wad"]
        );
        assert_eq!(merged[0].language.as_deref(), Some("english"));
        assert_eq!(merged[0].shipments, "A, B, Quartermaster link");
        assert_eq!(merged[1].language, None);

        let english = read_patch_wad(&merged[0].bytes).unwrap();
        let mut paths: Vec<String> = english.blocks.iter().map(|b| b.path_string.clone()).collect();
        paths.sort();
        assert_eq!(
            paths,
            vec![r"blocks\b\vo.block", r"blocks\qm\bank.block", r"blocks\qm\vo.block"]
        );
        let shell_wad = read_patch_wad(&merged[1].bytes).unwrap();
        assert_eq!(
            shell_wad.blocks.iter().map(|b| b.path_string.as_str()).collect::<Vec<_>>(),
            vec![r"blocks\b\menu.block"],
            "the later Shipment wins the shared shell asset"
        );

        let english_sha = loadprobe::sha256::sha256_hex(&merged[0].bytes);
        let staged = stage_placements(&build, Vec::new(), merged, Vec::new()).unwrap();
        assert!(build.join("files/data/english-patch.wad").is_file());
        assert!(build.join("files/data/shell-patch.wad").is_file());
        let back = placement::read_staged(&build).unwrap();
        assert_eq!(back.files.len(), 2);
        assert_eq!(back.files[0].relative, "data/english-patch.wad");
        assert_eq!(back.files[0].language.as_deref(), Some("english"));
        assert_eq!(back.files[0].sha256, english_sha);
        assert_eq!(staged.files[0].sha256, english_sha);
    }

    /// A merged patch that cannot be resolved refuses the build, naming the target.
    #[test]
    fn a_language_patch_partial_overlap_refuses_the_build() {
        let mut languages = std::collections::BTreeMap::new();
        languages.insert(
            "french".to_string(),
            vec![
                group(
                    "shipment:a",
                    "A",
                    vec![block(r"blocks\a\x.block", 0x1, "x"), block(r"blocks\a\y.block", 0x2, "y")],
                ),
                group("shipment:b", "B", vec![block(r"blocks\b\x.block", 0x1, "x2")]),
            ],
        );
        let err = merge_patches(&languages, &[]).unwrap_err();
        assert!(err.contains("data/french-patch.wad cannot be merged"), "got: {err}");
    }

    /// One destination claimed twice in a build — a Shipment's file and a merged patch, or a file
    /// and a stream copy — is refused.
    #[test]
    fn a_destination_claimed_twice_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let build = tmp.path().join("build");
        std::fs::create_dir_all(&build).unwrap();
        let src = tmp.path().join("x.wad");
        std::fs::write(&src, "x").unwrap();
        let file = |relative: &str, display: Option<&str>| StagedFile {
            source: src.to_string_lossy().to_string(),
            relative: relative.into(),
            sha256: String::new(),
            shipment: "Sneaky".into(),
            display: display.map(str::to_string),
            language: None,
        };
        let wad = MergedWad {
            relative: "data/english-patch.wad".into(),
            language: Some("english".into()),
            shipments: "A".into(),
            bytes: b"wad".to_vec(),
        };
        let err = stage_placements(&build, vec![file("data/English-patch.wad", None)], vec![wad], Vec::new())
            .unwrap_err();
        assert!(err.contains("placed twice"), "got: {err}");

        let copy = StreamCopy {
            from: "data/Audios/vo_stream.english.pws".into(),
            to: "data/polski.wad".into(),
            bytes: 1,
            sha256: "a".into(),
            shipment: "Lang".into(),
        };
        let err = stage_placements(
            &build,
            vec![file("data/polski.wad", Some("Polski"))],
            Vec::new(),
            vec![copy],
        )
        .unwrap_err();
        assert!(err.contains("placed twice"), "got: {err}");
    }

    /// Stream copies travel through the staged record untouched: no bytes are staged for them.
    #[test]
    fn stream_copies_are_recorded_for_deploy() {
        let tmp = tempfile::tempdir().unwrap();
        let build = tmp.path().join("build");
        std::fs::create_dir_all(&build).unwrap();
        let copy = StreamCopy {
            from: "data/Audios/vo_stream.english.pws".into(),
            to: "data/Audios/vo_stream.polski.pws".into(),
            bytes: 4,
            sha256: "ab".repeat(32),
            shipment: "Lang".into(),
        };
        stage_placements(&build, Vec::new(), Vec::new(), vec![copy.clone()]).unwrap();
        let back = placement::read_staged(&build).unwrap();
        assert!(back.files.is_empty());
        assert_eq!(back.stream_copies, vec![copy]);
    }

    /// A rebuild that no longer places a file must not leave it staged: deploy reads the record,
    /// and a stale entry would install something the load order no longer contains.
    #[test]
    fn restaging_clears_what_the_previous_build_left() {
        let tmp = tempfile::tempdir().unwrap();
        let (qm_out, build) = (tmp.path().join("qm"), tmp.path().join("build"));
        std::fs::create_dir_all(&build).unwrap();

        qm_output(&qm_out, None, &[("scripts/gone.asi", "old")]);
        let output = placement::read_output(&qm_out, "S").unwrap();
        stage_placements(&build, output.files, Vec::new(), Vec::new()).unwrap();
        assert!(build.join("files/scripts/gone.asi").is_file());

        stage_placements(&build, Vec::new(), Vec::new(), Vec::new()).unwrap();
        assert!(!build.join("files/scripts/gone.asi").exists());
        assert!(placement::read_staged(&build).unwrap().is_empty());
    }
}
