//! Ingest a Workshop **Shipment** — a Quartermaster *source* project (`manifest.{yaml,json,toml}`
//! plus `src/`), not a finished WAD — and orchestrate `qm` to build and link it into the load order.
//!
//! # Why this is not `prebuilt`
//!
//! [`super::prebuilt`] imports an already-built `vz-patch.wad` and block-merges it. Two mods that
//! both ship a **compiled** `scripts_vz` block cannot be block-merged: the whole-block override
//! silently deletes one's Lua. A Shipment carries its Lua as *source*, so the right merge is at the
//! source level — which is exactly what Quartermaster does:
//!
//! * `qm build <shipment>` emits that Shipment's overlay WAD, whose `scripts_vz` is valid *standalone*.
//! * `qm link <all shipments>` re-links every Shipment's Lua into **one** reconciled `scripts_vz`
//!   block, so none annihilate another. This is the deploy step qm was designed to have Modkit run
//!   (`qm.rs` module docs; the wad_simulator `build.rs` "the cross-Shipment relink belongs to
//!   deploy (Modkit)").
//!
//! # Collapsing qm's stack into one `vz-patch.wad`
//!
//! qm's native model is per-Shipment overlays + a link WAD mounted last. The game (and
//! [`super::deploy_wad`]) load a single `vz-patch.wad`, so we collapse: each Shipment's overlay
//! contributes its blocks **with `scripts_vz` dropped**, and the linker's reconciled `scripts_vz` is
//! added once, last. Dropping the per-Shipment scripts is what keeps `claim::resolve` from seeing a
//! scripts-only group partially overriding a larger overlay group (an atomic partial-overlap
//! conflict). Non-script overlaps across Shipments still resolve last-in-load-order-wins.
//!
//! # A Shipment is not only WAD blocks
//!
//! The blocks are one of qm's outputs, not all of them. A `native_hook` contribution lowers to an
//! `.asi` plugin and a `place_file` to a companion, and **neither produces WAD content at all** —
//! they are loose files in the game folder, described by the `placement.json` qm writes beside the
//! overlay. Reading only the blocks meant such a Shipment built clean, reported success, and
//! deployed nothing.
//!
//! So [`shipment_groups`] returns a [`ShipmentBuild`]: the claim groups **and** the staged files,
//! which [`super::wad_builder`] copies into the build output and [`super::deploy_wad`] installs and
//! records for undo. See [`super::placement`] for the record's two shapes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use mercs2_formats::patch_wad::read_patch_wad;
use serde::{Deserialize, Serialize};
use tauri::Window;

use super::placement::{self, StagedFile};
use super::proc::NoWindow;
use super::toolchain::{ensure_tool, installed_tool_path};
use crate::models::claim::ClaimGroup;

/// A deep-link Shipment path that arrived before the frontend was listening (a cold start launched
/// by the link). The webview drains it once via [`take_pending_shipment`]; live handoffs into an
/// already-running app come through the `deep-link-shipment` event instead.
#[derive(Default)]
pub struct PendingShipment(pub Mutex<Option<String>>);

/// Drain the buffered cold-start deep link, if any. Called once from the frontend on mount.
#[tauri::command]
pub fn take_pending_shipment(pending: tauri::State<'_, PendingShipment>) -> Option<String> {
    pending.0.lock().ok().and_then(|mut g| g.take())
}

/// Parse a `mercs2-modkit://ship?path=<percent-encoded>` deep link into the Shipment path it names.
/// `None` for any other URL, so the caller can ignore unrelated schemes/paths.
pub fn parse_ship_url(url: &str) -> Option<String> {
    let query = url.strip_prefix("mercs2-modkit://ship?")?;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("path="))
        .map(percent_decode)
}

/// Inverse of the Workshop's tiny percent-encoder: turn `%XX` back into bytes (and, tolerantly, `+`
/// into a space). Kept dependency-free to match the sender.
fn percent_decode(s: &str) -> String {
    let hex = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => match (hex(b[i + 1]), hex(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h * 16 + l);
                    i += 3;
                }
                _ => {
                    out.push(b[i]);
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A Workshop Shipment staged in the load order (a qm source directory, later wins).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShipmentRef {
    /// Stable id used in the load order and for dedupe.
    pub id: String,
    /// Display name (the Shipment folder's name, which the Workshop names after the Shipment).
    pub name: String,
    /// Absolute path to the Shipment source directory.
    pub path: String,
}

/// The manifest filenames `qm` accepts, in the order it looks.
const MANIFEST_NAMES: [&str; 4] = [
    "manifest.yaml",
    "manifest.yml",
    "manifest.json",
    "manifest.toml",
];

/// True when `dir` looks like a qm Shipment (carries one of the manifest names).
fn has_manifest(dir: &Path) -> bool {
    MANIFEST_NAMES.iter().any(|n| dir.join(n).is_file())
}

/// A block is the Lua scripts block if its PTHS path names `scripts_vz` — same test the prebuilt
/// importer uses to flag script-shipping WADs.
fn is_scripts_block(path_string: &str) -> bool {
    path_string.to_lowercase().contains("scripts_vz")
}

/// Validate a Shipment source directory and describe it for the load order, without building it.
///
/// Contrast [`super::prebuilt::inspect_patch_wad`], which *rejects* anything that isn't already a
/// built FFCS WAD; here we require the opposite — a source tree with a manifest.
#[tauri::command(async)]
pub fn inspect_shipment(path: String) -> Result<ShipmentRef, String> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(format!("{path}: not a folder"));
    }
    if !has_manifest(&root) {
        return Err(format!(
            "{path}: no manifest.yaml/.yml/.json/.toml — this is not a Quartermaster Shipment. \
             (A finished vz-patch.wad goes through Import Patch WAD instead.)"
        ));
    }
    let name = root
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "shipment".into());
    Ok(ShipmentRef {
        id: format!("shipment:{name}"),
        name,
        path,
    })
}

/// Locate the Workshop reference bundle whose `lua/` subtree is the corpus `qm` needs for
/// script-touching Shipments. Resolution mirrors qm's own: an explicit hint, then
/// `MERCS2_WORKSHOP_DATA`, then the `workshop_data/` companion that the toolset installs in qm's own
/// version directory. `None` means non-script Shipments still build but a script Shipment will error
/// clearly, telling the user to install the Workshop data.
fn resolve_corpus_bundle(qm: &Path, hint: Option<&Path>) -> Option<PathBuf> {
    let is_bundle = |p: &Path| p.join("lua").is_dir();
    if let Some(h) = hint {
        if is_bundle(h) {
            return Some(h.to_path_buf());
        }
    }
    if let Some(env) = std::env::var_os("MERCS2_WORKSHOP_DATA") {
        let p = PathBuf::from(env);
        if is_bundle(&p) {
            return Some(p);
        }
    }
    let beside = qm.parent()?.join("workshop_data");
    is_bundle(&beside).then_some(beside)
}

/// Fresh, empty working directory under the app's managed area (cleared first so a prior build's
/// WADs can't be mistaken for this one's).
fn work_dir(sub: &str) -> Result<PathBuf, String> {
    let dir = super::paths::app_data_dir()?.join("qm-work").join(sub);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create work dir: {e}"))?;
    Ok(dir)
}

/// Run one `qm` subcommand, returning a readable error on any nonzero exit. `qm` prints findings to
/// stderr and exits 1 (findings) / 2 (could not run); either way we refuse rather than ship a WAD
/// built from a Shipment qm rejected.
fn run_qm(qm: &Path, args: &[&std::ffi::OsStr], what: &str) -> Result<(), String> {
    let output = Command::new(qm)
        .args(args)
        .no_window()
        .output()
        .map_err(|e| format!("running {what}: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!(
        "{what} failed (exit {}):\n{}",
        output.status.code().unwrap_or(-1),
        stderr.trim()
    ))
}

/// Write a throwaway qm Shipment expressing the wardrobe as `add_outfit` contributions with no model
/// file — the "wear an existing in-game model" form. Routing the wardrobe through qm (instead of
/// modkit's own compiled `scripts_vz` block) is what lets it reconcile with script-touching Shipments
/// via `qm link` instead of clobbering, or being clobbered by, them.
///
/// Returns `Ok(None)` when there are no outfits. The dir is a `ShipmentRef` the caller folds into the
/// set handed to [`shipment_groups`].
pub fn synthesize_wardrobe_shipment(
    outfits: &[crate::commands::wardrobe::WardrobeOutfit],
) -> Result<Option<ShipmentRef>, String> {
    if outfits.is_empty() {
        return Ok(None);
    }
    let dir = work_dir("wardrobe-shipment")?;

    let contributions: Vec<serde_json::Value> = outfits
        .iter()
        .map(|o| {
            serde_json::json!({
                "kind": "add_outfit",
                // No `model` file → qm treats `name` as an existing engine model and only adds the
                // wardrobe row. `slug` = the model name so the (wearer, slug) merge key is unique
                // per skin, matching modkit's own dedupe-by-model.
                "name": o.model,
                "slug": o.model,
                "display": o.label,
                "wearer": o.hero,
            })
        })
        .collect();

    let manifest = serde_json::json!({
        "format": 1,
        "shipment": { "name": "modkit-wardrobe", "version": "1.0.0", "target": "retail" },
        "contributions": contributions,
    });
    let json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("building the wardrobe manifest: {e}"))?;
    std::fs::write(dir.join("manifest.json"), json)
        .map_err(|e| format!("writing the wardrobe manifest: {e}"))?;

    Ok(Some(ShipmentRef {
        id: "shipment:modkit-wardrobe".into(),
        name: "Wardrobe".into(),
        path: dir.to_string_lossy().to_string(),
    }))
}

/// Everything a `qm` run over the staged Shipments produced.
///
/// Two halves because qm emits two kinds of artifact and both have to reach the game: the WAD
/// blocks, resolved by [`crate::models::claim`] and folded into `vz-patch.wad`, and the loose files
/// (`native_hook` plugins, `place_file` companions) that live in the game folder and are described
/// by `placement.json`. Returning only the first is what made a code-layer Shipment a silent no-op.
#[derive(Debug, Default)]
pub struct ShipmentBuild {
    pub groups: Vec<ClaimGroup>,
    /// Loose files to install into the game folder, **in load order** — a later Shipment's file at
    /// the same destination wins, matching how the blocks resolve.
    pub files: Vec<StagedFile>,
    /// Non-fatal advisories: currently, one per destination two Shipments both claimed.
    pub warnings: Vec<String>,
}

/// Reduce the per-Shipment file lists to one file per destination, later-wins, warning about each
/// collision.
///
/// Two Shipments placing different files at one path is a real `FileArtifact` conflict, and the
/// alternative to naming it is one silently overwriting the other during the copy — the same class
/// of failure as not copying at all, just later in the pipeline. Resolution matches the blocks
/// (last in the load order wins) so a user's mental model holds across both halves of a Shipment.
fn resolve_file_collisions(files: Vec<StagedFile>) -> (Vec<StagedFile>, Vec<String>) {
    let mut winner: HashMap<String, usize> = HashMap::new();
    let mut warnings = Vec::new();
    let mut kept: Vec<Option<StagedFile>> = Vec::with_capacity(files.len());

    for file in files {
        if let Some(prev) = winner.insert(file.relative.clone(), kept.len()) {
            let displaced = kept[prev].take().expect("a destination wins at most once");
            warnings.push(format!(
                "“{}” and “{}” both place {} — the later one ({}) wins, as it does for assets.",
                displaced.shipment, file.shipment, file.relative, file.shipment
            ));
        }
        kept.push(Some(file));
    }
    (kept.into_iter().flatten().collect(), warnings)
}

/// Build each staged Shipment with `qm`, link their Lua across the whole set, and return the claim
/// groups to fold into `vz-patch.wad` plus the loose files to install alongside it. See the module
/// docs for the collapse rules.
///
/// `corpus_hint` is an optional explicit reference-bundle path; when `None`, the corpus is resolved
/// from the environment / the installed toolset.
pub async fn shipment_groups(
    window: Window,
    shipments: &[ShipmentRef],
    game_path: &str,
    corpus_hint: Option<&Path>,
) -> Result<ShipmentBuild, String> {
    if shipments.is_empty() {
        return Ok(ShipmentBuild::default());
    }
    if game_path.trim().is_empty() {
        return Err("Set the game folder before building Shipments.".into());
    }

    // Prefer an already-installed qm (this is called on a build, and we don't want to block a build
    // on a download unless we must); ensure_tool otherwise fetches it.
    let qm = match installed_tool_path("qm") {
        Some(p) => p,
        None => ensure_tool(window, "qm").await?,
    };
    let corpus = resolve_corpus_bundle(&qm, corpus_hint);

    let os = |s: &str| std::ffi::OsString::from(s);
    let corpus_args: Vec<std::ffi::OsString> = match &corpus {
        Some(dir) => vec![os("--workshop-data"), dir.as_os_str().to_os_string()],
        None => Vec::new(),
    };

    // 1) Build each Shipment's overlay and keep all of its blocks (the collapse drops scripts_vz),
    //    plus every loose file its placement record names.
    let mut overlays: Vec<(String, String, Vec<mercs2_formats::patch_wad::PatchBlock>)> = Vec::new();
    let mut files: Vec<StagedFile> = Vec::new();
    for (i, ship) in shipments.iter().enumerate() {
        let out = work_dir(&format!("build-{i}"))?;
        let mut args: Vec<std::ffi::OsString> = vec![
            os("build"),
            os(&ship.path),
            os("--game"),
            os(game_path),
            os("--out"),
            out.as_os_str().to_os_string(),
        ];
        args.extend(corpus_args.iter().cloned());
        let arg_refs: Vec<&std::ffi::OsStr> = args.iter().map(|a| a.as_os_str()).collect();
        run_qm(&qm, &arg_refs, &format!("qm build for \"{}\"", ship.name))?;

        let (wad, placed) = placement::read_output(&out, &ship.name)?;
        let placed_here = placed.len();
        files.extend(placed);

        // No WAD is now a legitimate outcome, not an error: a Shipment whose only contributions are
        // `native_hook` / `place_file` emits loose files and no overlay at all. Refusing it here
        // would be the "narrow the feature to dodge the gap" answer to the same defect.
        if let Some(wad) = wad {
            let bytes =
                std::fs::read(&wad).map_err(|e| format!("reading {}'s overlay: {e}", ship.name))?;
            let contents = read_patch_wad(&bytes)
                .map_err(|e| format!("{}'s overlay is not a patch WAD: {e}", ship.name))?;
            overlays.push((ship.id.clone(), ship.name.clone(), contents.blocks));
        } else if placed_here == 0 {
            return Err(format!(
                "qm build for \"{}\" produced neither a WAD nor any placed files",
                ship.name
            ));
        }
    }

    // 2) Link the whole set's Lua into one reconciled scripts_vz, mounted last. Emits no WAD when no
    //    Shipment touches scripts — then `link_scripts` stays empty and nothing is added.
    let link_out = work_dir("link")?;
    let mut link_args: Vec<std::ffi::OsString> = vec![os("link")];
    for ship in shipments {
        link_args.push(os(&ship.path));
    }
    link_args.extend([
        os("--game"),
        os(game_path),
        os("--out"),
        link_out.as_os_str().to_os_string(),
    ]);
    link_args.extend(corpus_args.iter().cloned());
    let link_refs: Vec<&std::ffi::OsStr> = link_args.iter().map(|a| a.as_os_str()).collect();
    run_qm(&qm, &link_refs, "qm link")?;

    // `link_installed` emits `zz-quartermaster-link.wad`, and in releases before the record was
    // added on that path it emits NO `placement.json` — so this consumer must work either way.
    // `read_output` handles both: with a record the WAD is named, without one the name-sorted scan
    // picks it, and the file list is simply empty. Any `game_folder` entry a future qm does record
    // here flows through the same path as a build's, rather than needing a second one.
    let (link_wad, link_files) = placement::read_output(&link_out, "Quartermaster link")?;
    files.extend(link_files);

    let link_scripts = match link_wad {
        Some(wad) => {
            let bytes = std::fs::read(&wad).map_err(|e| format!("reading the link WAD: {e}"))?;
            read_patch_wad(&bytes)
                .map_err(|e| format!("the link WAD is not a patch WAD: {e}"))?
                .blocks
        }
        None => Vec::new(),
    };

    let (files, warnings) = resolve_file_collisions(files);
    Ok(ShipmentBuild {
        groups: collapse(overlays, link_scripts, shipments.len()),
        files,
        warnings,
    })
}

/// Fold per-Shipment overlays + the linker's reconciled scripts into final claim groups.
///
/// Each Shipment contributes its non-`scripts_vz` blocks (a scripts-only Shipment contributes no
/// group); the linker supplies the single `scripts_vz` group, appended **last** so it wins the block
/// on last-in-load-order resolution. Because every per-Shipment `scripts_vz` is removed, the linker
/// group never *partially* overrides an overlay group — which would be an atomic-group conflict.
///
/// Pure and side-effect-free so the residency invariant is unit-testable without `qm` or a game.
fn collapse(
    overlays: Vec<(String, String, Vec<mercs2_formats::patch_wad::PatchBlock>)>,
    link_scripts: Vec<mercs2_formats::patch_wad::PatchBlock>,
    shipment_count: usize,
) -> Vec<ClaimGroup> {
    let mut groups: Vec<ClaimGroup> = Vec::new();
    for (id, name, blocks) in overlays {
        let kept: Vec<_> = blocks
            .into_iter()
            .filter(|b| !is_scripts_block(&b.path_string))
            .collect();
        if kept.is_empty() {
            continue;
        }
        groups.push(ClaimGroup {
            mod_id: id,
            mod_name: name.clone(),
            label: name,
            atomic: true,
            blocks: kept,
        });
    }
    let scripts: Vec<_> = link_scripts
        .into_iter()
        .filter(|b| is_scripts_block(&b.path_string))
        .collect();
    if !scripts.is_empty() {
        groups.push(ClaimGroup {
            mod_id: "qm-link:scripts".into(),
            mod_name: "Quartermaster link".into(),
            label: format!("Reconciled scripts ({shipment_count} Shipment(s))"),
            atomic: true,
            blocks: scripts,
        });
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_rejects_a_folder_with_no_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let err = inspect_shipment(dir.path().to_string_lossy().into()).unwrap_err();
        assert!(err.contains("not a Quartermaster Shipment"), "got: {err}");
    }

    #[test]
    fn inspect_accepts_a_manifest_bearing_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("manifest.yaml"), "shipment:\n  name: x\n").unwrap();
        let info = inspect_shipment(dir.path().to_string_lossy().into()).unwrap();
        assert!(info.id.starts_with("shipment:"));
        assert_eq!(info.path, dir.path().to_string_lossy());
    }

    #[test]
    fn scripts_block_is_detected_by_path() {
        assert!(is_scripts_block("blocks\\VZ\\scripts_vz_P000_Q3.block"));
        assert!(!is_scripts_block("blocks\\modkit\\some_model.block"));
    }

    /// The exact URL the Workshop's `modkit_ship_url` emits must decode back to the original path,
    /// backslashes / drive colon / spaces intact.
    #[test]
    fn parses_the_workshop_deep_link() {
        let url = "mercs2-modkit://ship?path=C%3A%5CUsers%5CAda%5CAppData%5CRoaming%5Cmercs2-modkit%5Cshipments%5CMy%20Mod";
        assert_eq!(
            parse_ship_url(url).unwrap(),
            r"C:\Users\Ada\AppData\Roaming\mercs2-modkit\shipments\My Mod"
        );
        assert_eq!(parse_ship_url("mercs2-modkit://ship?path=/tmp/a"), Some("/tmp/a".into()));
        assert_eq!(parse_ship_url("mercs2-modkit://other?x=1"), None);
        assert_eq!(parse_ship_url("https://example.com"), None);
    }

    use mercs2_formats::patch_wad::{
        build_patch_wad_multi, validate_blocks, AsetEntry, PatchBlock, FFCS_CERT_BLOB,
    };

    fn block(path: &str, hash: u32) -> PatchBlock {
        PatchBlock::from_decompressed(
            format!("payload {hash}").as_bytes(),
            path.to_string(),
            vec![AsetEntry::new(hash, 0xFFFF_FFFF, 0x0000_FFFF, 19)],
            None,
        )
        .unwrap()
    }

    /// The residency headline: two script-touching Shipments, each with its own standalone
    /// `scripts_vz`, collapse so the FINAL merged WAD carries exactly **one** scripts block — the
    /// linker's reconciled one — and never a partial-override conflict.
    #[test]
    fn collapse_yields_a_single_reconciled_scripts_block() {
        // Reuse one scripts hash across both overlays and the link, as the real blocks do.
        let scripts_hash = 0x5C21_7000;
        let overlays = vec![
            (
                "shipment:a".into(),
                "A".into(),
                vec![
                    block("blocks\\a\\model.block", 0x1111),
                    block("blocks\\VZ\\scripts_vz_A.block", scripts_hash),
                ],
            ),
            (
                "shipment:b".into(),
                "B".into(),
                vec![
                    block("blocks\\b\\texture.block", 0x2222),
                    block("blocks\\VZ\\scripts_vz_B.block", scripts_hash),
                ],
            ),
        ];
        let link_scripts = vec![block("blocks\\VZ\\scripts_vz_linked.block", scripts_hash)];

        let groups = collapse(overlays, link_scripts, 2);
        let resolved = crate::models::claim::resolve(&groups);
        assert!(
            resolved.conflicts.is_empty(),
            "no atomic partial-overlap: {:?}",
            resolved.conflicts
        );

        let scripts: Vec<_> = resolved
            .blocks
            .iter()
            .filter(|b| is_scripts_block(&b.path_string))
            .collect();
        assert_eq!(scripts.len(), 1, "exactly one scripts_vz survives");
        assert!(
            scripts[0].path_string.contains("linked"),
            "and it is the linker's: {}",
            scripts[0].path_string
        );
        // Both Shipments' non-script assets survive alongside it.
        assert_eq!(resolved.blocks.len(), 3);
        validate_blocks(&resolved.blocks).expect("one primary ASET row per hash");
        build_patch_wad_multi(&resolved.blocks, 0, Some(0), &FFCS_CERT_BLOB)
            .expect("the collapsed WAD assembles");
    }

    fn staged(shipment: &str, relative: &str) -> StagedFile {
        StagedFile {
            source: format!("/build/{shipment}/{relative}"),
            relative: relative.into(),
            sha256: String::new(),
            shipment: shipment.into(),
        }
    }

    /// Two Shipments claiming one destination resolve the way their assets do — last in the load
    /// order wins — and the user is told. The alternative is one file silently overwriting the
    /// other during the copy, which is the same defect as not copying at all, only later.
    #[test]
    fn two_shipments_claiming_one_destination_resolve_last_wins() {
        let (files, warnings) = resolve_file_collisions(vec![
            staged("A", "scripts/shared.ini"),
            staged("A", "scripts/a-only.asi"),
            staged("B", "scripts/shared.ini"),
        ]);

        let kept: Vec<(&str, &str)> = files
            .iter()
            .map(|f| (f.shipment.as_str(), f.relative.as_str()))
            .collect();
        assert_eq!(
            kept,
            vec![("A", "scripts/a-only.asi"), ("B", "scripts/shared.ini")],
            "the later Shipment's file survives, the unrelated one is untouched"
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("scripts/shared.ini"), "{}", warnings[0]);
        assert!(warnings[0].contains('B'), "names the winner: {}", warnings[0]);
    }

    /// No collision, no warning — and the load order is preserved exactly.
    #[test]
    fn distinct_destinations_pass_through_untouched() {
        let (files, warnings) = resolve_file_collisions(vec![
            staged("A", "scripts/a.asi"),
            staged("B", "plugins/b.ini"),
            staged("B", "scripts/OnBoot/b.lua"),
        ]);
        assert_eq!(files.len(), 3);
        assert!(warnings.is_empty());
        assert_eq!(files[0].relative, "scripts/a.asi");
        assert_eq!(files[2].relative, "scripts/OnBoot/b.lua");
    }

    /// A set with no script Shipments adds no linker group and keeps every overlay block.
    #[test]
    fn collapse_without_scripts_is_a_passthrough() {
        let overlays = vec![(
            "shipment:a".into(),
            "A".into(),
            vec![block("blocks\\a\\model.block", 0x1111)],
        )];
        let groups = collapse(overlays, Vec::new(), 1);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].blocks.len(), 1);
    }
}
