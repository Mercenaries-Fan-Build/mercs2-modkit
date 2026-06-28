//! Game-file integrity verification ("verify game contents"), Steam-style, with
//! per-block drill-down inside WAD archives.
//!
//! A *manifest* describes a known-good install in three parts:
//!   - `files` — every shared, version-independent file as
//!     `relative_path → { size, md5 }`. The main exe, runtime caches
//!     (`Precache/`), per-user config (`*.ini`), logs, and modkit-managed files
//!     are excluded, so the same set is valid for every version and crack.
//!   - `exes`  — a catalog of known-good `Mercenaries2.exe` builds (signed/
//!     unsigned, v1.1 patched, the cracks), each keyed by hash so same-size
//!     builds are told apart.
//!   - `wads`  — for each base WAD, the md5 of every contained block (an FFCS
//!     archive is a set of SGES blocks indexed by INDX, named by PTHS, and
//!     referenced by ASET-entry `asset_hash`). This is what lets verify say
//!     *which* asset inside a WAD changed, not merely that the WAD differs.
//!
//! Verify runs a fast file-level pass first; only for a WAD whose whole-file
//! hash mismatches does it parse the archive and diff block-by-block, reporting
//! modified / missing / added blocks (and how many catalogued assets they cover,
//! i.e. the scope of what changed). This is diagnosis only — it identifies
//! damage, it does not repair it.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use mercs2_formats::ffcs::{load_ffcs_archive, PAGE_SIZE};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tauri::path::BaseDirectory;
use tauri::{Emitter, Manager, Window};

use crate::commands::paths::app_data_dir;

/// Manifest filename, both as the in-app generator output and the bundled
/// resource path (see `bundle.resources` in tauri.conf.json).
const MANIFEST_ASSET: &str = "mercs2.manifest.json";
/// Resource-relative path to the manifest bundled with the app.
const BUNDLED_MANIFEST: &str = "manifests/mercs2.manifest.json";

/// The base-game executable filename the catalog identifies.
const MAIN_EXE: &str = "Mercenaries2.exe";
/// The de-DRM'd exe modkit writes; also identified against the catalog.
const CRACKED_EXE: &str = "Mercenaries2.cracked.exe";

/// Progress notifier: `(done, total)` over the whole hashing job.
type Progress<'a> = &'a (dyn Fn(usize, usize) + Sync);

/// One shared file's expected fingerprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub size: u64,
    /// Lowercase hex MD5.
    pub hash: String,
}

/// One known-good `Mercenaries2.exe` build.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExeEntry {
    pub version: String,
    pub variant: String,
    #[serde(default)]
    pub label: Option<String>,
    /// Sidecar DLL this exe imports and needs present to start (e.g.
    /// `cruise.dll`, `pmc_bb.dll`). `None` for stock SecuROM exes.
    #[serde(default)]
    pub requires: Option<String>,
    /// Build-level caveat surfaced on identification (e.g. "bypass only — does
    /// not load ASI mods").
    #[serde(default)]
    pub note: Option<String>,
    pub size: u64,
    pub hash: String,
}

impl ExeEntry {
    fn describe(&self) -> String {
        match &self.label {
            Some(l) => format!("{} {} — {l}", self.version, self.variant),
            None => format!("{} {}", self.version, self.variant),
        }
    }
}

/// One block inside a WAD: its PTHS name and the md5 of its stored bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockFp {
    pub path: String,
    pub size: u64,
    pub hash: String,
}

/// An ASET asset, recording which block carries it (to scope what a changed
/// block affects).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetRef {
    /// Lowercase hex `asset_hash`.
    pub hash: String,
    pub type_id: u32,
    /// Index into the WAD's `blocks` (the INDX/PTHS order).
    pub block: usize,
}

/// Per-WAD block catalogue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WadManifest {
    pub blocks: Vec<BlockFp>,
    #[serde(default)]
    pub assets: Vec<AssetRef>,
}

/// A known-good baseline: shared files, the exe catalog, and per-WAD blocks.
#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub algo: String,
    pub files: BTreeMap<String, ManifestEntry>,
    #[serde(default)]
    pub exes: Vec<ExeEntry>,
    /// WAD key (normalized rel path) → block catalogue.
    #[serde(default)]
    pub wads: BTreeMap<String, WadManifest>,
}

/// A file that exists but doesn't match its manifest fingerprint.
#[derive(Debug, Serialize)]
pub struct FileDiff {
    pub path: String,
    pub expected_size: u64,
    pub actual_size: u64,
    pub expected_hash: String,
    pub actual_hash: String,
}

/// Identification of one on-disk executable against the catalog.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExeReport {
    pub file: String,
    pub size: u64,
    pub hash: String,
    pub identified_as: Option<String>,
    /// Caveats/warnings: an unrecognized-build hint, a missing sidecar DLL the
    /// exe needs to start, or a build's modding limitation.
    pub notes: Vec<String>,
}

/// Block-level diff for a single WAD whose file hash didn't match.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WadDiff {
    pub wad: String,
    /// Vanilla blocks present but changed.
    pub modified: Vec<String>,
    /// Vanilla blocks absent from the user's WAD.
    pub missing: Vec<String>,
    /// Blocks in the user's WAD with no vanilla counterpart (added content).
    pub added: Vec<String>,
    /// Catalogued assets carried by the modified/missing blocks — the scope of
    /// what changed.
    pub affected_assets: usize,
}

/// Outcome of a verify run.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReport {
    pub ok: usize,
    pub missing: Vec<String>,
    pub corrupt: Vec<FileDiff>,
    pub extra: Vec<String>,
    pub ignored: usize,
    pub exes: Vec<ExeReport>,
    /// Block-level breakdown for each mismatched WAD.
    pub wad_details: Vec<WadDiff>,
    pub manifest_source: String,
}

/// Result of generating a manifest from a clean install.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateManifestResult {
    pub path: String,
    pub file_count: usize,
    pub block_count: usize,
    pub total_bytes: u64,
}

/// A `Mercenaries2.exe` build to catalogue during generation.
pub struct ExeSpec {
    pub path: PathBuf,
    pub version: String,
    pub variant: String,
    pub label: Option<String>,
    /// Sidecar DLL the exe imports (see [`ExeEntry::requires`]).
    pub requires: Option<String>,
    /// Build-level caveat (see [`ExeEntry::note`]).
    pub note: Option<String>,
}

#[derive(Clone, Serialize)]
struct ProgressEvent {
    done: usize,
    total: usize,
}

// ----------------------------------------------------------------------------
// Exclusion + walking
// ----------------------------------------------------------------------------

/// True for files that aren't part of the verifiable, version-independent
/// distribution: executables (cataloged separately), runtime caches, per-user
/// config/logs, OS/editor cruft, and modkit-deployed files. Input is a
/// normalized (lowercase, forward-slashed) relative path.
/// **Keep in sync with `examples/gen_manifest.rs`'s copy.**
fn is_excluded(rel: &str) -> bool {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    if rel.split('/').any(|seg| seg == "precache" || seg == ".vs") {
        return true;
    }
    if [".ini", ".log", ".bak", ".asi"].iter().any(|e| rel.ends_with(e)) {
        return true;
    }
    base == ".ds_store"
        || base == "thumbs.db"
        // Any Mercenaries2 executable — stock, cracked, or a named variant
        // (e.g. "Mercenaries2 (v1.0 signed).exe"). The exe is the only thing that
        // varies by build, and it's cataloged separately in `exes`; it must never
        // land in the shared `files` baseline.
        || (base.starts_with("mercenaries2") && base.ends_with(".exe"))
        || base == "pmc_bb.dll"
        || base == "vz-patch.wad"
        || base.ends_with("-patch.wad")
}

fn rel_key(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    Some(rel.to_string_lossy().replace('\\', "/").to_ascii_lowercase())
}

/// Recursively collect every non-excluded file under `root` as
/// `(key, absolute_path)`, plus the count of excluded files skipped.
pub fn collect_files(root: &Path) -> (Vec<(String, PathBuf)>, usize) {
    let mut out = Vec::new();
    let mut excluded = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => stack.push(path),
                Ok(ft) if ft.is_file() => {
                    if let Some(key) = rel_key(root, &path) {
                        if is_excluded(&key) {
                            excluded += 1;
                        } else {
                            out.push((key, path));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    (out, excluded)
}

// ----------------------------------------------------------------------------
// Hashing
// ----------------------------------------------------------------------------

/// Stream a file through MD5, returning `(size, lowercase_hex)`.
fn md5_file(path: &Path) -> std::io::Result<(u64, String)> {
    use md5::{Digest, Md5};
    let mut file = File::open(path)?;
    let mut hasher = Md5::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        hasher.update(&buf[..n]);
    }
    Ok((size, format!("{:x}", hasher.finalize())))
}

/// MD5 a byte range `[start, start+len)` of an already-open file.
fn md5_range(file: &mut File, start: u64, len: u64) -> std::io::Result<String> {
    use md5::{Digest, Md5};
    file.seek(SeekFrom::Start(start))?;
    let mut hasher = Md5::new();
    let mut remaining = len;
    let mut buf = vec![0u8; 1 << 16];
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        let n = file.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        remaining -= n as u64;
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Shared progress counter that emits throttled `(done, total)` on each tick.
struct Ticker<'a> {
    done: AtomicUsize,
    last_pct: AtomicUsize,
    total: usize,
    progress: Progress<'a>,
}

impl<'a> Ticker<'a> {
    fn new(total: usize, progress: Progress<'a>) -> Self {
        progress(0, total);
        Self {
            done: AtomicUsize::new(0),
            last_pct: AtomicUsize::new(usize::MAX),
            total,
            progress,
        }
    }
    fn tick(&self) {
        let d = self.done.fetch_add(1, Ordering::Relaxed) + 1;
        let pct = (d * 100).checked_div(self.total).unwrap_or(100);
        if self.last_pct.swap(pct, Ordering::Relaxed) != pct {
            (self.progress)(d, self.total);
        }
    }
}

/// Read every block of a WAD and md5 its stored bytes. `ticker` advances once
/// per block. Returns the block catalogue, or `None` if the file isn't an FFCS
/// archive (caller treats it as an ordinary file).
fn read_wad_blocks(wad_path: &Path, ticker: Option<&Ticker>) -> Option<WadManifest> {
    let mut file = File::open(wad_path).ok()?;
    let file_size = file.metadata().ok()?.len();
    let arc = load_ffcs_archive(&mut file, file_size).ok()?;
    drop(file);

    // Block byte ranges from the INDX page layout, clamped to the file.
    let ranges: Vec<(String, u64, u64)> = arc
        .indx
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let start = (e.page_index as u64) * PAGE_SIZE;
            let span = (e.compressed_page_count() as u64) * PAGE_SIZE;
            let len = (start + span).min(file_size).saturating_sub(start);
            let path = arc.paths.get(i).cloned().unwrap_or_else(|| format!("block{i}"));
            (path, start, len)
        })
        .collect();

    let blocks: Vec<BlockFp> = ranges
        .par_iter()
        .map(|(path, start, len)| {
            let hash = File::open(wad_path)
                .and_then(|mut f| md5_range(&mut f, *start, *len))
                .unwrap_or_default();
            if let Some(t) = ticker {
                t.tick();
            }
            BlockFp { path: path.clone(), size: *len, hash }
        })
        .collect();

    let assets: Vec<AssetRef> = arc
        .aset
        .iter()
        .filter(|a| a.is_primary())
        .map(|a| AssetRef {
            hash: format!("{:08x}", a.asset_hash),
            type_id: a.type_id,
            block: a.block_index() as usize,
        })
        .collect();

    Some(WadManifest { blocks, assets })
}

/// True if a manifest key names a base WAD we drill into (excludes patch WADs,
/// already filtered by `is_excluded`).
fn is_wad(key: &str) -> bool {
    key.ends_with(".wad")
}

/// A WAD whose every block hashed to empty is truncated/damaged (e.g. a 2.4 GB
/// vz.wad cut down to its 2 MB header). The generator uses this to refuse to
/// ship a bogus baseline.
pub fn wad_looks_truncated(wm: &WadManifest) -> bool {
    !wm.blocks.is_empty() && wm.blocks.iter().all(|b| b.size == 0)
}

// ----------------------------------------------------------------------------
// Manifest building (shared by the command and the offline generator)
// ----------------------------------------------------------------------------

/// Hash a clean install into a manifest: shared files, per-WAD blocks, and the
/// given exe catalog. `progress` is called as `(done, total)` across all work.
pub fn build_manifest(
    tree_root: &Path,
    exe_specs: &[ExeSpec],
    progress: Progress,
) -> Result<(Manifest, u64), String> {
    let (files, _excluded) = collect_files(tree_root);

    // Pre-parse WAD layouts so the progress total includes every block.
    let wad_keys: Vec<(String, PathBuf)> = files
        .iter()
        .filter(|(k, _)| is_wad(k))
        .map(|(k, p)| (k.clone(), p.clone()))
        .collect();

    let total = files.len() + count_wad_blocks(&wad_keys);
    let ticker = Ticker::new(total, progress);

    // Shared files (the WADs are also hashed here at file level).
    let hashes: Vec<std::io::Result<(u64, String)>> = files
        .par_iter()
        .map(|(_, path)| {
            let r = md5_file(path);
            ticker.tick();
            r
        })
        .collect();

    let mut file_map = BTreeMap::new();
    let mut total_bytes = 0u64;
    for ((key, path), res) in files.iter().zip(hashes) {
        let (size, hash) = res.map_err(|e| format!("Failed to hash {}: {e}", path.display()))?;
        total_bytes += size;
        file_map.insert(key.clone(), ManifestEntry { size, hash });
    }

    // Per-WAD block catalogues.
    let mut wads = BTreeMap::new();
    for (key, path) in &wad_keys {
        if let Some(wm) = read_wad_blocks(path, Some(&ticker)) {
            wads.insert(key.clone(), wm);
        }
    }

    // Exe catalog.
    let mut exes = Vec::new();
    for spec in exe_specs {
        if !spec.path.is_file() {
            continue;
        }
        let (size, hash) = md5_file(&spec.path).map_err(|e| e.to_string())?;
        exes.push(ExeEntry {
            version: spec.version.clone(),
            variant: spec.variant.clone(),
            label: spec.label.clone(),
            requires: spec.requires.clone(),
            note: spec.note.clone(),
            size,
            hash,
        });
    }

    Ok((Manifest { algo: "md5".into(), files: file_map, exes, wads }, total_bytes))
}

/// Sum the INDX block counts of the given WAD files (header-only parse).
fn count_wad_blocks(wads: &[(String, PathBuf)]) -> usize {
    wads.iter()
        .filter_map(|(_, p)| {
            let mut f = File::open(p).ok()?;
            let sz = f.metadata().ok()?.len();
            Some(load_ffcs_archive(&mut f, sz).ok()?.indx.len())
        })
        .sum()
}

// ----------------------------------------------------------------------------
// Commands
// ----------------------------------------------------------------------------

/// Hash a clean install and write a manifest to
/// `<app-data>/manifests/mercs2.manifest.json`, seeding the exe catalog with the
/// install's own `Mercenaries2.exe`. Maintainer tool — for the full multi-exe
/// catalog, use the `gen_manifest` example against the reference folder.
#[tauri::command]
pub async fn generate_manifest(
    window: Window,
    game_root: String,
    version: String,
    variant: String,
) -> Result<GenerateManifestResult, String> {
    let root = PathBuf::from(&game_root);
    if !root.is_dir() {
        return Err(format!("Game folder not found: {game_root}"));
    }

    let out_dir = app_data_dir()?.join("manifests");
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let out_path = out_dir.join(MANIFEST_ASSET);

    let result = tauri::async_runtime::spawn_blocking(
        move || -> Result<GenerateManifestResult, String> {
            let _ = window.emit("manifest-status", "Hashing files & WAD blocks…");
            let emit = move |done, total| {
                let _ = window.emit("manifest-progress", ProgressEvent { done, total });
            };
            let specs = vec![ExeSpec {
                path: root.join(MAIN_EXE),
                version,
                variant,
                label: None,
                requires: None,
                note: None,
            }];
            let (manifest, total_bytes) = build_manifest(&root, &specs, &emit)?;
            let block_count: usize = manifest.wads.values().map(|w| w.blocks.len()).sum();

            let json = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
            std::fs::write(&out_path, json).map_err(|e| format!("Failed to write manifest: {e}"))?;

            Ok(GenerateManifestResult {
                path: out_path.to_string_lossy().to_string(),
                file_count: manifest.files.len(),
                block_count,
                total_bytes,
            })
        },
    )
    .await
    .map_err(|e| e.to_string())??;

    Ok(result)
}

/// Verify the install against the manifest. With `manifest_path`, that local
/// file is used; otherwise the known-good manifest bundled with the app is read.
#[tauri::command]
pub async fn verify_game(
    window: Window,
    game_root: String,
    manifest_path: Option<String>,
) -> Result<VerifyReport, String> {
    let root = PathBuf::from(&game_root);
    if !root.is_dir() {
        return Err(format!("Game folder not found: {game_root}"));
    }

    let _ = window.emit("verify-status", "Reading manifest…");
    let (manifest, source) = match manifest_path {
        Some(p) => {
            let bytes = std::fs::read(&p).map_err(|e| format!("Couldn't read manifest {p}: {e}"))?;
            (parse_manifest(&bytes)?, p)
        }
        None => {
            // The canonical "known-good versions" manifest ships inside the app
            // (it can only be produced from the copyrighted originals; we bundle
            // just the hashes).
            let res = window
                .path()
                .resolve(BUNDLED_MANIFEST, BaseDirectory::Resource)
                .map_err(|e| format!("Couldn't locate the bundled manifest: {e}"))?;
            let bytes = std::fs::read(&res)
                .map_err(|e| format!("Couldn't read the bundled manifest at {}: {e}", res.display()))?;
            (parse_manifest(&bytes)?, "bundled".to_string())
        }
    };

    let report = tauri::async_runtime::spawn_blocking(move || {
        let status_win = window.clone();
        let progress = move |done, total| {
            let _ = window.emit("verify-progress", ProgressEvent { done, total });
        };
        let status = move |m: &str| {
            let _ = status_win.emit("verify-status", m.to_string());
        };
        run_verify(&root, manifest, source, &progress, &status)
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(report)
}

/// Verify an install against a manifest with no UI plumbing — used by the
/// offline `verify_offline` example and any non-Tauri caller.
pub fn verify_install(root: &Path, manifest: Manifest, source: String) -> VerifyReport {
    run_verify(root, manifest, source, &|_, _| {}, &|_| {})
}

/// The CPU/IO-bound diff: file-level pass, then block drill-down on mismatched
/// WADs, plus executable identification. `progress` ticks per hashed file;
/// `status` announces each phase.
fn run_verify(
    root: &Path,
    manifest: Manifest,
    source: String,
    progress: Progress,
    status: &dyn Fn(&str),
) -> VerifyReport {
    let (on_disk, ignored) = collect_files(root);
    let disk_keys: std::collections::HashSet<&str> =
        on_disk.iter().map(|(k, _)| k.as_str()).collect();

    let mut missing: Vec<String> = manifest
        .files
        .keys()
        .filter(|k| !disk_keys.contains(k.as_str()))
        .cloned()
        .collect();
    missing.sort();

    let mut to_check: Vec<(String, PathBuf)> = Vec::new();
    let mut extra: Vec<String> = Vec::new();
    for (key, path) in &on_disk {
        if manifest.files.contains_key(key) {
            to_check.push((key.clone(), path.clone()));
        } else {
            extra.push(key.clone());
        }
    }
    extra.sort();

    status(&format!("Hashing {} files…", to_check.len()));
    let ticker = Ticker::new(to_check.len(), progress);
    let hashes: Vec<std::io::Result<(u64, String)>> = to_check
        .par_iter()
        .map(|(_, path)| {
            let r = md5_file(path);
            ticker.tick();
            r
        })
        .collect();

    let mut ok = 0usize;
    let mut corrupt = Vec::new();
    let mut corrupt_wads: Vec<String> = Vec::new();
    for ((key, _), res) in to_check.iter().zip(hashes) {
        let expected = &manifest.files[key];
        match res {
            Ok((size, hash)) if size == expected.size && hash == expected.hash => ok += 1,
            Ok((size, hash)) => {
                if is_wad(key) && manifest.wads.contains_key(key) {
                    corrupt_wads.push(key.clone());
                }
                corrupt.push(FileDiff {
                    path: key.clone(),
                    expected_size: expected.size,
                    actual_size: size,
                    expected_hash: expected.hash.clone(),
                    actual_hash: hash,
                });
            }
            Err(_) => corrupt.push(FileDiff {
                path: key.clone(),
                expected_size: expected.size,
                actual_size: 0,
                expected_hash: expected.hash.clone(),
                actual_hash: String::new(),
            }),
        }
    }

    // Block-level drill-down on each mismatched WAD (the heavy, otherwise-silent
    // phase — announce each so the UI shows what it's chewing on).
    let mut wad_details: Vec<WadDiff> = Vec::new();
    for key in &corrupt_wads {
        status(&format!("Inspecting blocks in {key}…"));
        if let Some(d) = diff_wad(root, key, &manifest.wads[key]) {
            wad_details.push(d);
        }
    }

    status("Finishing…");

    let mut exes = Vec::new();
    for name in [MAIN_EXE, CRACKED_EXE] {
        if let Some(rep) = identify_exe(root, name, &manifest.exes) {
            exes.push(rep);
        }
    }

    VerifyReport {
        ok,
        missing,
        corrupt,
        extra,
        ignored,
        exes,
        wad_details,
        manifest_source: source,
    }
}

/// Diff one WAD's on-disk blocks against its manifest catalogue.
fn diff_wad(root: &Path, key: &str, expected: &WadManifest) -> Option<WadDiff> {
    let disk = read_wad_blocks(&root.join(key), None)?;

    let want: BTreeMap<&str, &str> =
        expected.blocks.iter().map(|b| (b.path.as_str(), b.hash.as_str())).collect();
    let have: BTreeMap<&str, &str> =
        disk.blocks.iter().map(|b| (b.path.as_str(), b.hash.as_str())).collect();

    let mut modified = Vec::new();
    let mut missing = Vec::new();
    for (path, hash) in &want {
        match have.get(path) {
            Some(h) if h == hash => {}
            Some(_) => modified.push(path.to_string()),
            None => missing.push(path.to_string()),
        }
    }
    let added: Vec<String> = have
        .keys()
        .filter(|p| !want.contains_key(*p))
        .map(|p| p.to_string())
        .collect();

    // How many catalogued assets live in the changed blocks → scope of change.
    let damaged: std::collections::HashSet<&str> =
        modified.iter().chain(&missing).map(String::as_str).collect();
    let affected_assets = expected
        .assets
        .iter()
        .filter(|a| {
            expected
                .blocks
                .get(a.block)
                .is_some_and(|b| damaged.contains(b.path.as_str()))
        })
        .count();

    modified.sort();
    missing.sort();
    let mut added = added;
    added.sort();

    Some(WadDiff { wad: key.to_string(), modified, missing, added, affected_assets })
}

/// Hash `root/name` (if present) and match it against the exe catalog, noting
/// any missing sidecar DLL it needs to start and any build-level caveat.
fn identify_exe(root: &Path, name: &str, catalog: &[ExeEntry]) -> Option<ExeReport> {
    let path = root.join(name);
    if !path.is_file() {
        return None;
    }
    let (size, hash) = md5_file(&path).ok()?;

    let mut notes = Vec::new();
    let identified_as = match catalog.iter().find(|e| e.hash == hash) {
        Some(e) => {
            // The crack's bypass DLL must sit beside the exe or it won't load —
            // the same class of failure as the binkw32.dll case.
            if let Some(dll) = &e.requires {
                if !root.join(dll).is_file() {
                    notes.push(format!(
                        "Imports {dll}, which isn't in the game folder — the game won't start until it's present."
                    ));
                }
            }
            if let Some(n) = &e.note {
                notes.push(n.clone());
            }
            Some(e.describe())
        }
        None => {
            notes.push(
                catalog
                    .iter()
                    .find(|e| e.size == size)
                    .map(|e| format!("Unrecognized build (size matches {})", e.describe()))
                    .unwrap_or_else(|| "Unrecognized build".to_string()),
            );
            None
        }
    };

    Some(ExeReport { file: name.to_string(), size, hash, identified_as, notes })
}

fn parse_manifest(bytes: &[u8]) -> Result<Manifest, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("Manifest isn't valid JSON: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn exe(version: &str, variant: &str, label: Option<&str>, requires: Option<&str>, hash: &str) -> ExeEntry {
        ExeEntry {
            version: version.into(),
            variant: variant.into(),
            label: label.map(Into::into),
            requires: requires.map(Into::into),
            note: Some("caveat".into()),
            size: 3,
            hash: hash.into(),
        }
    }

    #[test]
    fn excludes_executables_caches_and_config() {
        // Executables — incl. the named variants that leaked into a baseline once.
        for k in [
            "mercenaries2.exe",
            "mercenaries2.cracked.exe",
            "mercenaries2 (v1.0 signed).exe",
            "mercenaries2(v1.1 cruise.dll).exe",
            "pmc_bb.dll",
            "vz-patch.wad",
            "foo-patch.wad",
            "precache/display0.precache",
            "scripts/plugin.asi",
            "mercs2.ini",
            "data/cdbsizes.ini",
            "d3d.log",
            "x.bak",
            ".ds_store",
            ".vs/slnx.sqlite",
        ] {
            assert!(is_excluded(k), "{k} should be excluded");
        }
    }

    #[test]
    fn keeps_real_distribution_files() {
        // Including non-Mercenaries2 installer executables, which are real files.
        for k in [
            "binkw32.dll",
            "data/vz.wad",
            "data/english.wad",
            "msvcr71.dll",
            "support/winui.dll",
            "__installer/disk1/easetup.exe",
        ] {
            assert!(!is_excluded(k), "{k} should NOT be excluded");
        }
    }

    #[test]
    fn rel_key_lowercases_and_forward_slashes() {
        let root = Path::new("/game");
        let got = rel_key(root, Path::new("/game/Data/VZ.WAD")).unwrap();
        assert_eq!(got, "data/vz.wad");
    }

    #[test]
    fn md5_matches_known_vectors() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a");
        std::fs::write(&f, b"abc").unwrap();
        let (size, hash) = md5_file(&f).unwrap();
        assert_eq!(size, 3);
        assert_eq!(hash, "900150983cd24fb0d6963f7d28e17f72");

        std::fs::write(&f, b"").unwrap();
        assert_eq!(md5_file(&f).unwrap().1, "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn identifies_exe_and_warns_on_missing_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Mercenaries2.exe"), b"abc").unwrap();
        // md5("abc") = 900150983cd24fb0d6963f7d28e17f72
        let catalog = vec![exe(
            "v1.1",
            "cracked",
            Some("cruise.dll crack"),
            Some("cruise.dll"),
            "900150983cd24fb0d6963f7d28e17f72",
        )];

        // Sidecar absent → identified, plus a missing-DLL warning and the caveat.
        let r = identify_exe(dir.path(), "Mercenaries2.exe", &catalog).unwrap();
        assert_eq!(r.identified_as.as_deref(), Some("v1.1 cracked — cruise.dll crack"));
        assert_eq!(r.notes.len(), 2);
        assert!(r.notes.iter().any(|n| n.contains("cruise.dll") && n.contains("game folder")));

        // Sidecar present → only the caveat note.
        std::fs::write(dir.path().join("cruise.dll"), b"x").unwrap();
        let r = identify_exe(dir.path(), "Mercenaries2.exe", &catalog).unwrap();
        assert_eq!(r.notes, vec!["caveat".to_string()]);
    }

    #[test]
    fn unrecognized_exe_hints_by_size() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Mercenaries2.exe"), b"abc").unwrap(); // size 3
        let catalog = vec![exe("v1.1", "cracked", None, None, "deadbeef")]; // size 3, wrong hash
        let r = identify_exe(dir.path(), "Mercenaries2.exe", &catalog).unwrap();
        assert!(r.identified_as.is_none());
        assert_eq!(r.notes.len(), 1);
        assert!(r.notes[0].contains("Unrecognized"));
    }

    #[test]
    fn truncated_wad_is_flagged() {
        let empty = WadManifest {
            blocks: vec![
                BlockFp { path: "a".into(), size: 0, hash: "x".into() },
                BlockFp { path: "b".into(), size: 0, hash: "y".into() },
            ],
            assets: vec![],
        };
        assert!(wad_looks_truncated(&empty));

        let ok = WadManifest {
            blocks: vec![BlockFp { path: "a".into(), size: 10, hash: "x".into() }],
            assets: vec![],
        };
        assert!(!wad_looks_truncated(&ok));
        // An empty catalogue is not "truncated".
        assert!(!wad_looks_truncated(&WadManifest { blocks: vec![], assets: vec![] }));
    }

    #[test]
    fn manifest_json_roundtrips_and_tolerates_old_shape() {
        let m = Manifest {
            algo: "md5".into(),
            files: [("binkw32.dll".to_string(), ManifestEntry { size: 1, hash: "h".into() })]
                .into_iter()
                .collect(),
            exes: vec![exe("v1.0", "ea-signed", None, None, "h")],
            wads: BTreeMap::new(),
        };
        let json = serde_json::to_string(&m).unwrap();
        let back: Manifest = serde_json::from_slice(json.as_bytes()).unwrap();
        assert_eq!(back.files["binkw32.dll"].size, 1);
        assert_eq!(back.exes.len(), 1);

        // A pre-feature manifest (no exes/wads, exe without requires/note) still loads.
        let old = br#"{"algo":"md5","files":{"a":{"size":2,"hash":"z"}},
            "exes":[{"version":"v1.0","variant":"unsigned","size":1,"hash":"e"}]}"#;
        let parsed = parse_manifest(old).unwrap();
        assert!(parsed.wads.is_empty());
        assert_eq!(parsed.exes[0].requires, None);
        assert_eq!(parsed.exes[0].note, None);
    }
}
