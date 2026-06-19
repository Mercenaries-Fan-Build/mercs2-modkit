//! In-process `pmc_blackbox.log` analysis via the published `loadprobe` library
//! (the same engine behind the `/analyze-game-log` tool).

use std::path::{Path, PathBuf};

use loadprobe::{parse, report, sha256};

/// Default routine source tags suppressed from the line dump.
const ROUTINE: &[&str] = &["lua", "pool"];
/// Default high-signal Lua marker prefixes to surface.
const SIGNALS: &[&str] = &["###!", "###", "!!!", "##@", "@@@", "***", "=-="];

/// Analyze a `pmc_blackbox.log` and return loadprobe's full forensic report.
#[tauri::command]
pub fn analyze_log(path: String) -> Result<report::Report, String> {
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("Cannot read {path}: {e}"))?;
    let lines = parse::parse_log(&text);
    let log_sha256 = sha256::sha256_hex(text.as_bytes());

    let routine: Vec<String> = ROUTINE.iter().map(|s| s.to_string()).collect();
    let signals: Vec<String> = SIGNALS.iter().map(|s| s.to_string()).collect();

    Ok(report::analyze(
        &path,
        log_sha256,
        &lines,
        &routine,
        &signals,
        10, // hang threshold (seconds)
        5,  // top inter-line gaps
    ))
}

/// Try to locate `pmc_blackbox.log` near a game install (root, then scripts/).
#[tauri::command]
pub fn locate_log(game_root: String) -> Option<String> {
    let root = PathBuf::from(&game_root);
    let candidates = [root.join("pmc_blackbox.log"), root.join("scripts/pmc_blackbox.log")];
    candidates
        .iter()
        .find(|p| Path::new(p).is_file())
        .map(|p| p.to_string_lossy().to_string())
}
