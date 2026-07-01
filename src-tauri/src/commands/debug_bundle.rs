//! "Build debug bundle" — package everything a maintainer needs to diagnose a
//! broken install into a single dated `.zip`: the game logs, an inventory of
//! installed mods, the versions of every moving part, and a fresh file-integrity
//! check.
//!
//! The frontend already knows the app/game/mod/dependency versions, so it hands
//! them over as a structured `meta` blob; the backend owns the parts that need
//! disk access — the integrity check (reusing [`verify::verify_install`]) and
//! collecting the log files — then renders a human-readable report plus a
//! machine-readable JSON dump and compresses the lot.
//!
//! Every bundled log is SHA-256'd (via `loadprobe::sha256`, the same digest the
//! log analyzer uses) so the archive carries a `manifest.sha256` — a
//! `sha256sum -c`-compatible list a recipient can use to confirm the logs
//! weren't altered in transit — mirrored into `debug-info.json` under `files`.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::path::BaseDirectory;
use tauri::{Emitter, Manager, Window};
use zip::write::SimpleFileOptions;

use crate::commands::verify::{verify_install, Manifest, VerifyReport};

/// Resource-relative path to the known-good manifest bundled with the app
/// (kept in sync with `verify.rs`'s copy).
const BUNDLED_MANIFEST: &str = "manifests/mercs2.manifest.json";

/// Outcome of building a debug bundle.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugZipResult {
    /// Absolute path to the written `.zip`.
    pub path: String,
    /// Size of the written archive.
    pub bytes: u64,
    /// Number of log files bundled.
    pub log_count: usize,
    /// Whether the integrity check found the install clean (or could not run).
    pub integrity_ok: bool,
    /// Non-fatal notes (e.g. the integrity check couldn't load a manifest).
    pub notes: Vec<String>,
}

/// Bundle logs, a mod inventory, versions, and a fresh integrity check into a
/// dated `.zip` at `dest_path`. `meta` is the frontend-collected environment
/// (app/game/mod/dependency versions) — see `DiagnosticsView.vue`.
#[tauri::command]
pub async fn build_debug_zip(
    window: Window,
    game_root: String,
    dest_path: String,
    meta: Value,
) -> Result<DebugZipResult, String> {
    let root = PathBuf::from(&game_root);
    if !root.is_dir() {
        return Err(format!("Game folder not found: {game_root}"));
    }

    // Resolve the bundled manifest up front — it needs the window's resource
    // resolver, which isn't available inside the blocking task. Missing manifest
    // is non-fatal: the bundle is still worth building without the integrity pass.
    let _ = window.emit("debug-status", "Reading manifest…");
    let manifest = load_bundled_manifest(&window);

    let dest = PathBuf::from(&dest_path);
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<DebugZipResult, String> {
        let mut notes: Vec<String> = Vec::new();

        // 1. Fresh integrity check (reuses the verify engine, no UI plumbing).
        let _ = window.emit("debug-status", "Verifying game files…");
        let verify = match manifest {
            Ok(m) => Some(verify_install(&root, m, "bundled".to_string())),
            Err(e) => {
                notes.push(format!("Integrity check skipped: {e}"));
                None
            }
        };

        // 2. Gather log files from the install and SHA-256 each for the manifest.
        let _ = window.emit("debug-status", "Collecting logs…");
        let logs = read_logs(collect_logs(&root), &mut notes);

        // 3. Render the report, the machine-readable dump, and a
        //    sha256sum-compatible manifest of the bundled logs.
        let _ = window.emit("debug-status", "Writing report…");
        let stem = zip_stem(&dest);
        let manifest_txt = render_manifest(&logs);
        let report_txt = render_report(&meta, verify.as_ref(), &logs, &notes);
        let info_json = render_info_json(&meta, verify.as_ref(), &logs);

        // 4. Compress everything under a single top-level folder.
        let _ = window.emit("debug-status", "Compressing…");
        write_zip(&dest, &stem, &report_txt, &info_json, &manifest_txt, &logs)?;

        let bytes = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        let integrity_ok = verify.as_ref().map(is_clean).unwrap_or(true);
        Ok(DebugZipResult {
            path: dest.to_string_lossy().to_string(),
            bytes,
            log_count: logs.len(),
            integrity_ok,
            notes,
        })
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(result)
}

/// Read + parse the manifest bundled with the app as a resource.
fn load_bundled_manifest(window: &Window) -> Result<Manifest, String> {
    let res = window
        .path()
        .resolve(BUNDLED_MANIFEST, BaseDirectory::Resource)
        .map_err(|e| format!("couldn't locate the bundled manifest: {e}"))?;
    let bytes = std::fs::read(&res)
        .map_err(|e| format!("couldn't read the bundled manifest at {}: {e}", res.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("manifest isn't valid JSON: {e}"))
}

/// A candidate log file found on disk: its path inside the zip (under `logs/`)
/// and its location. The true size comes from [`read_logs`], which sizes the
/// bytes it hashes.
struct LogFile {
    arcname: String,
    path: PathBuf,
}

/// A log that's been read and hashed, ready to write into the archive and list
/// in the manifest.
struct BundledLog {
    arcname: String,
    size: u64,
    /// Lowercase hex SHA-256 of `bytes`.
    sha256: String,
    bytes: Vec<u8>,
}

/// Read each candidate log's bytes and SHA-256 them. A log that can't be read
/// (vanished mid-run, permissions) is skipped with a note rather than aborting
/// the whole bundle.
fn read_logs(candidates: Vec<LogFile>, notes: &mut Vec<String>) -> Vec<BundledLog> {
    let mut out = Vec::new();
    for c in candidates {
        match std::fs::read(&c.path) {
            Ok(bytes) => {
                let sha256 = loadprobe::sha256::sha256_hex(&bytes);
                out.push(BundledLog { arcname: c.arcname, size: bytes.len() as u64, sha256, bytes });
            }
            Err(e) => notes.push(format!("Skipped log {}: {e}", c.arcname)),
        }
    }
    out
}

/// A `sha256sum -c`-compatible manifest of the bundled logs: `<hash>  <path>`,
/// one per line (two spaces, matching coreutils' text-mode format).
fn render_manifest(logs: &[BundledLog]) -> String {
    let mut o = String::new();
    for l in logs {
        o.push_str(&format!("{}  {}\n", l.sha256, l.arcname));
    }
    o
}

/// Collect `*.log` files from the install root (shallow) and the ASI loader
/// subfolders (`scripts`/`plugins`/`update`, recursive), named by their path
/// relative to the game root so duplicates in different folders stay distinct.
fn collect_logs(root: &Path) -> Vec<LogFile> {
    let mut out: Vec<LogFile> = Vec::new();
    let mut push = |p: &Path| {
        if p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("log")) {
            let rel = p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/");
            out.push(LogFile { arcname: format!("logs/{rel}"), path: p.to_path_buf() });
        }
    };

    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file() {
                push(&p);
            }
        }
    }
    for sub in ["scripts", "plugins", "update"] {
        walk_logs(&root.join(sub), 0, &mut push);
    }

    out.sort_by(|a, b| a.arcname.cmp(&b.arcname));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

/// Recurse a directory (depth-limited) invoking `push` for each file found.
fn walk_logs(dir: &Path, depth: usize, push: &mut impl FnMut(&Path)) {
    if depth > 4 {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk_logs(&p, depth + 1, push);
            } else if p.is_file() {
                push(&p);
            }
        }
    }
}

/// Filename stem (no `.zip`) of the destination, used as the archive's top-level
/// folder so extraction yields one tidy directory.
fn zip_stem(dest: &Path) -> String {
    dest.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "mercs2-modkit-debug".to_string())
}

fn is_clean(v: &VerifyReport) -> bool {
    v.missing.is_empty() && v.corrupt.is_empty()
}

fn fmt_bytes(n: u64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    let units = ["KB", "MB", "GB"];
    let mut v = n as f64 / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", units[i])
}

// ----------------------------------------------------------------------------
// Report rendering
// ----------------------------------------------------------------------------

/// Convenience helpers for pulling string/array fields out of the meta blob
/// without exploding when a field is absent.
fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}
fn s_or<'a>(v: &'a Value, key: &str, dflt: &'a str) -> &'a str {
    s(v, key).unwrap_or(dflt)
}

/// The full machine-readable dump: the frontend meta with the (backend-computed)
/// integrity report and the bundled-log manifest (`files`) grafted on.
fn render_info_json(meta: &Value, verify: Option<&VerifyReport>, logs: &[BundledLog]) -> String {
    let mut obj = meta.clone();
    if let Value::Object(map) = &mut obj {
        map.insert(
            "integrity".into(),
            verify
                .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
                .unwrap_or(Value::Null),
        );
        let files: Vec<Value> = logs
            .iter()
            .map(|l| json!({ "path": l.arcname, "size": l.size, "sha256": l.sha256 }))
            .collect();
        map.insert("files".into(), Value::Array(files));
    }
    serde_json::to_string_pretty(&obj).unwrap_or_else(|_| "{}".into())
}

/// Human-readable report: environment, versions, mod inventory, integrity, logs.
fn render_report(meta: &Value, verify: Option<&VerifyReport>, logs: &[BundledLog], notes: &[String]) -> String {
    let mut o = String::new();
    let line = |o: &mut String, k: &str, val: &str| {
        o.push_str(&format!("{k}: {val}\n"));
    };

    o.push_str("Mercenaries 2 Modkit — Debug Bundle\n");
    o.push_str("===================================\n\n");
    line(&mut o, "Generated", s_or(meta, "generatedAt", "unknown"));
    line(&mut o, "Modkit version", s_or(meta, "modkitVersion", "unknown"));
    line(&mut o, "Platform", &format!("{} {}", std::env::consts::OS, std::env::consts::ARCH));

    // --- Game ---
    o.push_str("\n== Game ==\n");
    if let Some(game) = meta.get("game").filter(|g| !g.is_null()) {
        line(&mut o, "Root", s_or(game, "root", "?"));
        let size = game.get("exe_size").and_then(Value::as_u64).unwrap_or(0);
        line(&mut o, "Executable", &format!("{} ({} bytes)", s_or(game, "exe_path", "?"), size));
        line(
            &mut o,
            "Version",
            &format!("{} {}", s_or(game, "version", "unknown"), s_or(game, "variant", "")),
        );
        line(&mut o, "pmc_bb.dll", if game.get("has_pmc_bb").and_then(Value::as_bool).unwrap_or(false) { "present" } else { "absent" });
        line(&mut o, "ASI loader", s_or(game, "asi_loader_proxy", "none"));
        line(&mut o, "Data dir", s_or(game, "data_dir", "?"));
        line(&mut o, "Log discovered", s_or(game, "log_path", "none"));
    } else {
        o.push_str("(no game detected)\n");
    }

    // --- Versions ---
    o.push_str("\n== Versions ==\n");
    line(&mut o, "Modkit", s_or(meta, "modkitVersion", "unknown"));
    if let Some(game) = meta.get("game").filter(|g| !g.is_null()) {
        line(
            &mut o,
            "Game",
            &format!("{} / {}", s_or(game, "version", "unknown"), s_or(game, "variant", "unknown")),
        );
    }
    line(&mut o, "pmc_bb.dll (ASI loader)", s_or(meta, "pmcBbVersion", "unknown"));
    line(&mut o, "apply_crack (SecuROM bypass)", s_or(meta, "crackVersion", "unknown"));
    if let Some(vc) = meta.get("vcRedist").filter(|v| !v.is_null()) {
        let applicable = vc.get("applicable").and_then(Value::as_bool).unwrap_or(false);
        let installed = vc.get("installed").and_then(Value::as_bool).unwrap_or(false);
        let state = if !applicable {
            "not applicable (provided by the Proton prefix)".to_string()
        } else if installed {
            "installed".to_string()
        } else {
            format!("MISSING — {}", s_or(vc, "detail", "the game needs the 32-bit VC++ 2008 runtime"))
        };
        line(&mut o, "VC++ 2008 runtime", &state);
    }
    if let Some(region) = meta.get("region").filter(|r| !r.is_null()) {
        if region.get("applicable").and_then(Value::as_bool).unwrap_or(false) {
            line(
                &mut o,
                "Matchmaking region",
                &format!(
                    "{} (pool expects {})",
                    s_or(region, "currentRegion", "unset"),
                    s_or(region, "expectedRegion", "?")
                ),
            );
        }
    }

    // --- Mods ---
    o.push_str("\n== Installed mods ==\n");
    render_mod_list(&mut o, "WAD-asset mods", meta.get("wadMods"), true);
    render_mod_list(&mut o, "ASI plugins", meta.get("asiMods"), false);
    if let Some(arr) = meta.get("deployedAsi").and_then(Value::as_array).filter(|a| !a.is_empty()) {
        o.push_str(&format!("Deployed .asi in game folder ({}):\n", arr.len()));
        for d in arr {
            let known = s(d, "known").map(|k| format!(" — {k}")).unwrap_or_default();
            o.push_str(&format!("  - {}{}\n", s_or(d, "rel_path", "?"), known));
        }
    }
    if let Some(arr) = meta.get("deployedPatches").and_then(Value::as_array).filter(|a| !a.is_empty()) {
        o.push_str(&format!("Deployed patch WADs ({}):\n", arr.len()));
        for p in arr {
            if let Some(name) = p.as_str() {
                o.push_str(&format!("  - {name}\n"));
            }
        }
    }

    // --- Integrity ---
    o.push_str("\n== File integrity check ==\n");
    match verify {
        None => o.push_str("(not run — see notes)\n"),
        Some(v) => render_integrity(&mut o, v),
    }

    // --- Logs ---
    o.push_str("\n== Logs ==\n");
    if logs.is_empty() {
        o.push_str("(no log files found in the install)\n");
    } else {
        o.push_str(&format!(
            "Included {} log file(s) — sha256 also in manifest.sha256:\n",
            logs.len()
        ));
        for l in logs {
            o.push_str(&format!(
                "  - {} ({})\n      sha256: {}\n",
                l.arcname,
                fmt_bytes(l.size),
                l.sha256
            ));
        }
    }

    if !notes.is_empty() {
        o.push_str("\n== Notes ==\n");
        for n in notes {
            o.push_str(&format!("  - {n}\n"));
        }
    }

    o
}

/// Render one mod section from a meta array of `{name, version, enabled, ...}`.
fn render_mod_list(o: &mut String, title: &str, arr: Option<&Value>, wad: bool) {
    let arr = arr.and_then(Value::as_array);
    let count = arr.map(|a| a.len()).unwrap_or(0);
    o.push_str(&format!("{title} ({count}):\n"));
    let Some(arr) = arr else { return };
    for m in arr {
        let name = s_or(m, "name", "?");
        let ver = s(m, "version").map(|v| format!(" {v}")).unwrap_or_default();
        let mut flags: Vec<&str> = Vec::new();
        if m.get("enabled").and_then(Value::as_bool).unwrap_or(true) {
            flags.push("enabled");
        } else {
            flags.push("disabled");
        }
        if !wad && m.get("deployed").and_then(Value::as_bool).unwrap_or(false) {
            flags.push("deployed");
        }
        let extra = if wad {
            m.get("assetCount")
                .and_then(Value::as_u64)
                .map(|n| format!(", {n} assets"))
                .unwrap_or_default()
        } else {
            String::new()
        };
        o.push_str(&format!("  - {name}{ver}  [{}{}]\n", flags.join(", "), extra));
    }
}

/// Render the integrity section from the backend-computed [`VerifyReport`].
fn render_integrity(o: &mut String, v: &VerifyReport) {
    o.push_str(&format!("Baseline: {}\n", v.manifest_source));
    if is_clean(v) {
        o.push_str(&format!("Result: all {} vanilla files present and intact ✓\n", v.ok));
    } else {
        o.push_str(&format!(
            "Result: {} missing · {} corrupt · {} OK\n",
            v.missing.len(),
            v.corrupt.len(),
            v.ok
        ));
    }

    if !v.missing.is_empty() {
        o.push_str(&format!("Missing files ({}):\n", v.missing.len()));
        for m in &v.missing {
            o.push_str(&format!("  - {m}\n"));
        }
    }
    if !v.corrupt.is_empty() {
        o.push_str(&format!("Corrupt / changed ({}):\n", v.corrupt.len()));
        for c in &v.corrupt {
            o.push_str(&format!(
                "  - {} ({} on disk, expected {})\n",
                c.path,
                fmt_bytes(c.actual_size),
                fmt_bytes(c.expected_size)
            ));
        }
    }
    if !v.wad_details.is_empty() {
        o.push_str("WAD block-level:\n");
        for w in &v.wad_details {
            o.push_str(&format!(
                "  - {}: {} modified · {} missing · {} added · {} asset(s) affected\n",
                w.wad,
                w.modified.len(),
                w.missing.len(),
                w.added.len(),
                w.affected_assets
            ));
        }
    }
    if !v.exes.is_empty() {
        o.push_str("Executables:\n");
        for e in &v.exes {
            match &e.identified_as {
                Some(id) => o.push_str(&format!("  - {} → {id} ✓\n", e.file)),
                None => o.push_str(&format!("  - {} → unrecognized\n", e.file)),
            }
            for n in &e.notes {
                o.push_str(&format!("      ⚠ {n}\n"));
            }
        }
    }
    o.push_str(&format!("{} excluded file(s) ignored (exe, caches, config, mods)\n", v.ignored));
}

// ----------------------------------------------------------------------------
// Archive
// ----------------------------------------------------------------------------

/// Write the report, JSON dump, manifest, and log files into a `.zip` at `dest`,
/// each entry under the `stem/` top-level folder.
fn write_zip(
    dest: &Path,
    stem: &str,
    report_txt: &str,
    info_json: &str,
    manifest_txt: &str,
    logs: &[BundledLog],
) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("couldn't create output folder: {e}"))?;
    }
    let file = File::create(dest).map_err(|e| format!("couldn't create {}: {e}", dest.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut add = |name: &str, bytes: &[u8]| -> Result<(), String> {
        zip.start_file(format!("{stem}/{name}"), opts)
            .map_err(|e| format!("zip error on {name}: {e}"))?;
        zip.write_all(bytes).map_err(|e| format!("zip write error on {name}: {e}"))
    };

    add("debug-report.txt", report_txt.as_bytes())?;
    add("debug-info.json", info_json.as_bytes())?;
    // Only emit a manifest when there's something to attest to.
    if !logs.is_empty() {
        add("manifest.sha256", manifest_txt.as_bytes())?;
    }
    for l in logs {
        add(&l.arcname, &l.bytes)?;
    }

    zip.finish().map_err(|e| format!("couldn't finalize the zip: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::verify::{ExeReport, FileDiff, WadDiff};
    use serde_json::json;
    use std::io::Read;

    fn write(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
    }

    /// A verify report with two damaged files, one drilled WAD, and an exe.
    fn dirty_report() -> VerifyReport {
        VerifyReport {
            ok: 1200,
            missing: vec!["data/english.wad".into()],
            corrupt: vec![FileDiff {
                path: "binkw32.dll".into(),
                expected_size: 100,
                actual_size: 90,
                expected_hash: "aaa".into(),
                actual_hash: "bbb".into(),
            }],
            extra: vec!["mods/foo.wad".into()],
            ignored: 42,
            exes: vec![ExeReport {
                file: "Mercenaries2.exe".into(),
                size: 53_482_288,
                hash: "deadbeef".into(),
                identified_as: Some("v1.1 cracked".into()),
                notes: vec!["bypass only — does not load ASI mods".into()],
            }],
            wad_details: vec![WadDiff {
                wad: "data/vz.wad".into(),
                modified: vec!["a".into(), "b".into()],
                missing: vec![],
                added: vec!["c".into()],
                affected_assets: 5,
            }],
            manifest_source: "bundled".into(),
        }
    }

    fn clean_report() -> VerifyReport {
        VerifyReport {
            ok: 1234,
            missing: vec![],
            corrupt: vec![],
            extra: vec![],
            ignored: 10,
            exes: vec![],
            wad_details: vec![],
            manifest_source: "bundled".into(),
        }
    }

    #[test]
    fn fmt_bytes_scales_units() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(1024), "1.0 KB");
        assert_eq!(fmt_bytes(1536), "1.5 KB");
        assert_eq!(fmt_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(fmt_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn zip_stem_strips_extension_and_falls_back() {
        assert_eq!(zip_stem(Path::new("/tmp/mercs2-modkit-debug-2026-06-30.zip")), "mercs2-modkit-debug-2026-06-30");
        assert_eq!(zip_stem(Path::new("bundle.zip")), "bundle");
        // No usable stem → the safe default.
        assert_eq!(zip_stem(Path::new("")), "mercs2-modkit-debug");
    }

    #[test]
    fn is_clean_reflects_missing_and_corrupt() {
        assert!(is_clean(&clean_report()));
        assert!(!is_clean(&dirty_report()));
    }

    #[test]
    fn collect_logs_walks_root_and_loader_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("scripts/nested")).unwrap();
        std::fs::create_dir_all(root.join("plugins")).unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();

        write(&root.join("pmc_blackbox.log"), "root log");
        write(&root.join("d3d.LOG"), "upper ext still a log"); // case-insensitive
        write(&root.join("notes.txt"), "not a log");
        write(&root.join("scripts/pmc_blackbox.log"), "scripts log");
        write(&root.join("scripts/nested/deep.log"), "nested log");
        write(&root.join("plugins/plugin.log"), "plugin log");
        // A .log sitting in a non-scanned folder must be ignored.
        write(&root.join("data/ignored.log"), "not scanned");

        let logs = collect_logs(root);
        let names: Vec<&str> = logs.iter().map(|l| l.arcname.as_str()).collect();

        assert_eq!(logs.len(), 5, "got: {names:?}");
        assert!(names.contains(&"logs/pmc_blackbox.log"));
        assert!(names.contains(&"logs/d3d.LOG"));
        assert!(names.contains(&"logs/scripts/pmc_blackbox.log"));
        assert!(names.contains(&"logs/scripts/nested/deep.log"));
        assert!(names.contains(&"logs/plugins/plugin.log"));
        assert!(!names.iter().any(|n| n.contains("notes.txt")));
        assert!(!names.iter().any(|n| n.contains("ignored.log")));
        // Sorted for stable output.
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    fn sample_meta() -> Value {
        json!({
            "generatedAt": "2026-06-30T12:00:00.000Z",
            "modkitVersion": "0.5.1",
            "game": {
                "root": "/games/Mercs2",
                "exe_path": "/games/Mercs2/Mercenaries2.exe",
                "exe_size": 53_482_288u64,
                "version": "v1.1",
                "variant": "cracked",
                "has_pmc_bb": true,
                "asi_loader_proxy": "pmc_bb.dll",
                "data_dir": "/games/Mercs2/data",
                "log_path": "/games/Mercs2/pmc_blackbox.log"
            },
            "pmcBbVersion": "v0.3.0",
            "crackVersion": "v1.2.0",
            "vcRedist": { "applicable": true, "installed": false, "detail": "not found in WinSxS" },
            "region": { "applicable": true, "currentRegion": "us", "expectedRegion": "global" },
            "wadMods": [
                { "name": "Cool Skins", "version": "1.2.0", "enabled": true, "assetCount": 12 }
            ],
            "asiMods": [
                { "name": "Windowed Mode", "version": "0.9", "enabled": false, "deployed": false }
            ],
            "deployedPatches": ["vz-patch.wad"]
        })
    }

    fn bundled(arcname: &str, body: &str) -> BundledLog {
        BundledLog {
            arcname: arcname.into(),
            size: body.len() as u64,
            sha256: loadprobe::sha256::sha256_hex(body.as_bytes()),
            bytes: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn render_report_covers_every_section() {
        let meta = sample_meta();
        let report = dirty_report();
        // 2048 bytes so the size renders as "2.0 KB".
        let logs = vec![bundled("logs/pmc_blackbox.log", &"x".repeat(2048))];
        let r = render_report(&meta, Some(&report), &logs, &["a note".into()]);

        // Header + environment
        assert!(r.contains("Mercenaries 2 Modkit — Debug Bundle"));
        assert!(r.contains("Generated: 2026-06-30T12:00:00.000Z"));
        assert!(r.contains("Modkit version: 0.5.1"));
        // Game + versions
        assert!(r.contains("/games/Mercs2/Mercenaries2.exe"));
        assert!(r.contains("v1.1 cracked"));
        assert!(r.contains("pmc_bb.dll (ASI loader): v0.3.0"));
        assert!(r.contains("apply_crack (SecuROM bypass): v1.2.0"));
        // VC++ missing surfaces the detail
        assert!(r.contains("VC++ 2008 runtime: MISSING — not found in WinSxS"));
        // Region present
        assert!(r.contains("Matchmaking region: us (pool expects global)"));
        // Mods
        assert!(r.contains("WAD-asset mods (1):"));
        assert!(r.contains("Cool Skins 1.2.0  [enabled, 12 assets]"));
        assert!(r.contains("ASI plugins (1):"));
        assert!(r.contains("Windowed Mode 0.9  [disabled]"));
        assert!(r.contains("vz-patch.wad"));
        // Integrity
        assert!(r.contains("Baseline: bundled"));
        assert!(r.contains("1 missing · 1 corrupt · 1200 OK"));
        assert!(r.contains("data/english.wad"));
        assert!(r.contains("binkw32.dll"));
        assert!(r.contains("data/vz.wad: 2 modified · 0 missing · 1 added · 5 asset(s) affected"));
        assert!(r.contains("Mercenaries2.exe → v1.1 cracked ✓"));
        assert!(r.contains("bypass only"));
        // Logs + notes (with per-log sha256 surfaced)
        assert!(r.contains("Included 1 log file(s) — sha256 also in manifest.sha256:"));
        assert!(r.contains("logs/pmc_blackbox.log (2.0 KB)"));
        assert!(r.contains(&format!("sha256: {}", loadprobe::sha256::sha256_hex(&[b'x'; 2048]))));
        assert!(r.contains("a note"));
    }

    #[test]
    fn render_report_handles_clean_and_absent_integrity() {
        let meta = sample_meta();
        let clean = render_report(&meta, Some(&clean_report()), &[], &[]);
        assert!(clean.contains("all 1234 vanilla files present and intact"));
        assert!(clean.contains("(no log files found in the install)"));

        // Skipped integrity (no manifest) is reported, not fatal.
        let skipped = render_report(&meta, None, &[], &["Integrity check skipped: no manifest".into()]);
        assert!(skipped.contains("(not run — see notes)"));
        assert!(skipped.contains("Integrity check skipped: no manifest"));
    }

    #[test]
    fn render_info_json_merges_integrity_and_files() {
        let logs = vec![bundled("logs/a.log", "hello")];
        let out = render_info_json(&sample_meta(), Some(&dirty_report()), &logs);
        let v: Value = serde_json::from_str(&out).unwrap();
        // Original meta preserved…
        assert_eq!(v["modkitVersion"], "0.5.1");
        // …the backend-computed integrity report grafted on…
        assert_eq!(v["integrity"]["ok"], 1200);
        assert_eq!(v["integrity"]["missing"][0], "data/english.wad");
        assert_eq!(v["integrity"]["manifestSource"], "bundled");
        // …and the bundled-log manifest.
        assert_eq!(v["files"][0]["path"], "logs/a.log");
        assert_eq!(v["files"][0]["size"], 5);
        assert_eq!(v["files"][0]["sha256"], loadprobe::sha256::sha256_hex(b"hello"));

        // With no report, integrity is null and files is an empty array.
        let none = render_info_json(&sample_meta(), None, &[]);
        let v2: Value = serde_json::from_str(&none).unwrap();
        assert!(v2["integrity"].is_null());
        assert_eq!(v2["files"], json!([]));
    }

    #[test]
    fn render_manifest_is_sha256sum_compatible() {
        let logs = vec![bundled("logs/a.log", "abc"), bundled("logs/b.log", "")];
        let m = render_manifest(&logs);
        // Known SHA-256 vectors, two-space separator, one entry per line.
        assert_eq!(
            m,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  logs/a.log\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  logs/b.log\n"
        );
    }

    #[test]
    fn read_logs_hashes_and_skips_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("real.log"), "abc");
        let candidates = vec![
            LogFile { arcname: "logs/real.log".into(), path: dir.path().join("real.log") },
            LogFile { arcname: "logs/gone.log".into(), path: dir.path().join("missing.log") },
        ];
        let mut notes = Vec::new();
        let logs = read_logs(candidates, &mut notes);

        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].arcname, "logs/real.log");
        assert_eq!(logs[0].size, 3);
        assert_eq!(logs[0].sha256, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("Skipped log logs/gone.log"));
    }

    #[test]
    fn write_zip_round_trips_report_json_manifest_and_logs() {
        let dir = tempfile::tempdir().unwrap();
        let logs = vec![bundled("logs/pmc_blackbox.log", "the log body")];
        let manifest = render_manifest(&logs);

        let dest = dir.path().join("out/bundle.zip");
        write_zip(&dest, "bundle", "REPORT", "{\"k\":1}", &manifest, &logs).unwrap();

        let mut archive = zip::ZipArchive::new(File::open(&dest).unwrap()).unwrap();
        let read = |a: &mut zip::ZipArchive<File>, name: &str| {
            let mut s = String::new();
            a.by_name(name).unwrap().read_to_string(&mut s).unwrap();
            s
        };
        assert_eq!(read(&mut archive, "bundle/debug-report.txt"), "REPORT");
        assert_eq!(read(&mut archive, "bundle/debug-info.json"), "{\"k\":1}");
        assert_eq!(read(&mut archive, "bundle/logs/pmc_blackbox.log"), "the log body");
        // The manifest is present and names the log with its digest.
        let m = read(&mut archive, "bundle/manifest.sha256");
        assert!(m.contains("  logs/pmc_blackbox.log\n"));
        assert!(m.starts_with(&loadprobe::sha256::sha256_hex(b"the log body")));
    }

    #[test]
    fn write_zip_omits_manifest_when_no_logs() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("bundle.zip");
        write_zip(&dest, "bundle", "REPORT", "{}", "", &[]).unwrap();
        let mut archive = zip::ZipArchive::new(File::open(&dest).unwrap()).unwrap();
        assert!(archive.by_name("bundle/debug-report.txt").is_ok());
        // No logs → no manifest entry.
        assert!(archive.by_name("bundle/manifest.sha256").is_err());
    }
}
