//! Base-game detection: locate the install, identify its version/variant, and
//! report modding state (pmc_bb.dll, ASI loader, deployed patch WADs).

use std::path::{Path, PathBuf};

use serde::Serialize;

/// A detected Mercenaries 2 installation and its current modding state.
#[derive(Debug, Serialize)]
pub struct GameInfo {
    /// Absolute path to the folder the user selected.
    pub root: String,
    /// Absolute path to the located `Mercenaries2.exe`.
    pub exe_path: String,
    pub exe_size: u64,
    /// `"v1.0"`, `"v1.1"`, or `"unknown"`.
    pub version: String,
    /// `"unsigned"`, `"ea-signed"`, `"patched"`, `"cracked"`, or `"unknown"`.
    pub variant: String,
    /// `pmc_bb.dll` present in the install (the DRM-spoof / debug DLL).
    pub has_pmc_bb: bool,
    /// Name of the Ultimate ASI Loader proxy DLL present, if any
    /// (e.g. `dinput8.dll`). `None` means no loader is installed.
    pub asi_loader_proxy: Option<String>,
    /// Folder that holds the game's WADs, if found (`data/` or the root).
    pub data_dir: Option<String>,
    /// Patch WADs already present in the data dir.
    pub deployed_patches: Vec<String>,
    /// `.asi` plugins already deployed (scripts/, plugins/, update/, root).
    pub deployed_asi: Vec<String>,
}

/// ASI loader proxy DLL names, in preference order. For this project the
/// loader is `pmc_bb.dll` (injected via the exe import table) — NOT the
/// conventional `xinput1_3.dll`. The Ultimate ASI Loader proxies are listed
/// after it only as fallbacks for non-standard setups.
const ASI_PROXIES: &[&str] = &[
    "pmc_bb.dll",
];

/// Folders the ASI Loader scans for `.asi` plugins, relative to the game root.
const ASI_PLUGIN_DIRS: &[&str] = &[".", "scripts", "plugins", "update"];

fn find_asi_loader(root: &Path) -> Option<String> {
    ASI_PROXIES
        .iter()
        .find(|name| root.join(name).is_file())
        .map(|s| s.to_string())
}

/// List deployed `.asi` plugins across the loader's search folders.
fn list_deployed_asi(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for sub in ASI_PLUGIN_DIRS {
        let dir = root.join(sub);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_file()
                    && p.extension()
                        .and_then(|x| x.to_str())
                        .map(|x| x.eq_ignore_ascii_case("asi"))
                        .unwrap_or(false)
                {
                    if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
                        let label = if *sub == "." {
                            n.to_string()
                        } else {
                            format!("{sub}/{n}")
                        };
                        out.push(label);
                    }
                }
            }
        }
    }
    out.sort();
    out
}

// Exact sizes of known retail variants (see forensic analysis).
const SIZE_V10_UNSIGNED: u64 = 16_846_848;
const SIZE_V10_EA_SIGNED: u64 = 17_122_568;
const SIZE_V11_PATCHED: u64 = 53_944_080;
const SIZE_V11_CRACKED: u64 = 53_482_288;

fn classify(size: u64) -> (&'static str, &'static str) {
    match size {
        SIZE_V10_UNSIGNED => ("v1.0", "unsigned"),
        SIZE_V10_EA_SIGNED => ("v1.0", "ea-signed"),
        SIZE_V11_PATCHED => ("v1.1", "patched"),
        SIZE_V11_CRACKED => ("v1.1", "cracked"),
        // Range fallback for unrecognised builds.
        s if (16_500_000..=17_500_000).contains(&s) => ("v1.0", "unknown"),
        s if (53_000_000..=54_500_000).contains(&s) => ("v1.1", "unknown"),
        _ => ("unknown", "unknown"),
    }
}

/// Find `Mercenaries2.exe` in `root`, or the first `*.exe` matching the name.
fn find_exe(root: &Path) -> Option<PathBuf> {
    let direct = root.join("Mercenaries2.exe");
    if direct.is_file() {
        return Some(direct);
    }
    let entries = std::fs::read_dir(root).ok()?;
    for e in entries.flatten() {
        let p = e.path();
        if p.is_file() {
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                let lower = name.to_ascii_lowercase();
                if lower.starts_with("mercenaries2") && lower.ends_with(".exe") {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Pick the folder holding WADs: prefer `data/`, else the install root.
fn find_data_dir(root: &Path) -> Option<PathBuf> {
    let data = root.join("data");
    if data.is_dir() {
        return Some(data);
    }
    if root.is_dir() {
        return Some(root.to_path_buf());
    }
    None
}

/// List patch WADs (`*-patch.wad` / `vz-patch.wad`) in a directory.
fn list_deployed_patches(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if let Some(name) = e.file_name().to_str() {
                let lower = name.to_ascii_lowercase();
                if lower.ends_with("-patch.wad") || lower == "vz-patch.wad" {
                    out.push(name.to_string());
                }
            }
        }
    }
    out.sort();
    out
}

/// Detect a Mercenaries 2 install from a folder the user selected.
#[tauri::command]
pub fn detect_game(path: String) -> Result<GameInfo, String> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(format!("Not a folder: {path}"));
    }

    let exe = find_exe(&root)
        .ok_or_else(|| "No Mercenaries2.exe found in that folder".to_string())?;
    let exe_size = std::fs::metadata(&exe)
        .map_err(|e| format!("Failed to stat exe: {e}"))?
        .len();
    let (version, variant) = classify(exe_size);

    let data_dir = find_data_dir(&root);
    let deployed_patches = data_dir
        .as_deref()
        .map(list_deployed_patches)
        .unwrap_or_default();

    Ok(GameInfo {
        root: root.to_string_lossy().to_string(),
        exe_path: exe.to_string_lossy().to_string(),
        exe_size,
        version: version.to_string(),
        variant: variant.to_string(),
        has_pmc_bb: root.join("pmc_bb.dll").is_file(),
        asi_loader_proxy: find_asi_loader(&root),
        data_dir: data_dir.map(|d| d.to_string_lossy().to_string()),
        deployed_patches,
        deployed_asi: list_deployed_asi(&root),
    })
}
