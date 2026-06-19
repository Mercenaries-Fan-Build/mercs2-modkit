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
    /// `.asi` plugins already deployed (root shallow; scripts/plugins/update recursive).
    pub deployed_asi: Vec<DeployedAsi>,
    /// Discovered `pmc_blackbox.log`, if present in the install.
    pub log_path: Option<String>,
}

/// Discover the game's `pmc_blackbox.log` (written to the install root, with
/// `scripts/` as a fallback location).
fn discover_log(root: &Path) -> Option<String> {
    [root.join("pmc_blackbox.log"), root.join("scripts/pmc_blackbox.log")]
        .iter()
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().to_string())
}

/// A `.asi` plugin found deployed in the game install.
#[derive(Debug, Serialize)]
pub struct DeployedAsi {
    pub name: String,
    /// Path relative to the game root, forward-slashed.
    pub rel_path: String,
    pub abs_path: String,
    pub size: u64,
    /// Friendly label if this is a recognised project plugin.
    pub known: Option<String>,
}

/// ASI loader proxy DLL names, in preference order. For this project the
/// loader is `pmc_bb.dll` (injected via the exe import table) — NOT the
/// conventional `xinput1_3.dll`. The Ultimate ASI Loader proxies are listed
/// after it only as fallbacks for non-standard setups.
const ASI_PROXIES: &[&str] = &[
    "pmc_bb.dll",
];

/// Loader subfolders scanned recursively (root is scanned shallow). The loader
/// runs with `LoadRecursively=1`, so nested `.asi` files are also picked up.
const ASI_PLUGIN_SUBDIRS: &[&str] = &["scripts", "plugins", "update"];

fn find_asi_loader(root: &Path) -> Option<String> {
    ASI_PROXIES
        .iter()
        .find(|name| root.join(name).is_file())
        .map(|s| s.to_string())
}

fn is_asi(p: &Path) -> bool {
    p.extension()
        .and_then(|x| x.to_str())
        .map(|x| x.eq_ignore_ascii_case("asi"))
        .unwrap_or(false)
}

/// Friendly label for recognised project plugins.
fn known_label(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "cruise.asi" => Some("SecuROM spoof"),
        "dlc_enable.asi" => Some("DLC activator"),
        "net_hooks.asi" => Some("Network hooks"),
        "windowed_mode.asi" => Some("Windowed mode"),
        _ => None,
    }
}

fn push_asi(p: &Path, root: &Path, out: &mut Vec<DeployedAsi>) {
    let name = match p.file_name().and_then(|n| n.to_str()) {
        Some(n) => n.to_string(),
        None => return,
    };
    let rel = p
        .strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/");
    let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    out.push(DeployedAsi {
        known: known_label(&name).map(|s| s.to_string()),
        name,
        rel_path: rel,
        abs_path: p.to_string_lossy().to_string(),
        size,
    });
}

fn collect_recursive(dir: &Path, root: &Path, depth: usize, out: &mut Vec<DeployedAsi>) {
    if depth > 4 {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_recursive(&p, root, depth + 1, out);
            } else if is_asi(&p) {
                push_asi(&p, root, out);
            }
        }
    }
}

/// List deployed `.asi` plugins: the root (shallow) plus scripts/plugins/update
/// (recursive), deduped by absolute path.
fn list_deployed_asi(root: &Path) -> Vec<DeployedAsi> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file() && is_asi(&p) {
                push_asi(&p, root, &mut out);
            }
        }
    }
    for sub in ASI_PLUGIN_SUBDIRS {
        let d = root.join(sub);
        if d.is_dir() {
            collect_recursive(&d, root, 0, &mut out);
        }
    }
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    out.dedup_by(|a, b| a.abs_path == b.abs_path);
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
        log_path: discover_log(&root),
    })
}
