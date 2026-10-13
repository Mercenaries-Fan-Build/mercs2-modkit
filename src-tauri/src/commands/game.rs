//! Base-game detection: locate the install, identify its exes, and report
//! modding state (pmc_bb.dll, ASI loader, deployed patch WADs).

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::Window;

use super::exe_identity::ExeIdentity;
use super::launch_plan::{self, LaunchPlan};
use super::managed::pmc_bb::ExeKind;

/// A detected Mercenaries 2 installation and its current modding state.
#[derive(Debug, Serialize)]
pub struct GameInfo {
    /// Absolute path to the folder the user selected.
    pub root: String,
    /// Every `Mercenaries2*.exe` in the folder (plus a pinned exe elsewhere).
    pub exes: Vec<ExeIdentity>,
    /// `apply_crack`'s input: `Mercenaries2.exe`, else the first stock SecuROM exe.
    pub base_exe: Option<ExeIdentity>,
    /// What a launch would run. `None` with `launch_error` when it can't resolve.
    pub launch: Option<LaunchPlan>,
    pub launch_error: Option<String>,
    /// `pmc_bb.dll` present in the install.
    pub has_pmc_bb: bool,
    /// `dxwrapper.dll` present in the install.
    pub has_dxwrapper: bool,
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

/// `apply_crack`'s input: the game's own `Mercenaries2.exe`, else the first stock
/// SecuROM exe.
fn base_exe(exes: &[ExeIdentity]) -> Option<ExeIdentity> {
    exes.iter()
        .find(|e| e.name.eq_ignore_ascii_case("mercenaries2.exe"))
        .or_else(|| exes.iter().find(|e| e.kind == ExeKind::SecuRom))
        .cloned()
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
#[tauri::command(async)]
pub fn detect_game(window: Window, path: String) -> Result<GameInfo, String> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(format!("Not a folder: {path}"));
    }

    let detected = launch_plan::detect(&window, &root)?;
    if detected.exes.is_empty() {
        return Err("No Mercenaries2*.exe found in that folder".to_string());
    }
    let (launch, launch_error) = match detected.plan {
        Ok(p) => (Some(p), None),
        Err(e) => (None, Some(e)),
    };

    let data_dir = find_data_dir(&root);
    let deployed_patches = data_dir
        .as_deref()
        .map(list_deployed_patches)
        .unwrap_or_default();

    Ok(GameInfo {
        root: root.to_string_lossy().to_string(),
        base_exe: base_exe(&detected.exes),
        exes: detected.exes,
        launch,
        launch_error,
        has_pmc_bb: root.join("pmc_bb.dll").is_file(),
        has_dxwrapper: root.join(super::dxwrapper::DLL).is_file(),
        asi_loader_proxy: find_asi_loader(&root),
        data_dir: data_dir.map(|d| d.to_string_lossy().to_string()),
        deployed_patches,
        deployed_asi: list_deployed_asi(&root),
        log_path: discover_log(&root),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe(name: &str, id: Option<&str>) -> ExeIdentity {
        ExeIdentity {
            path: format!("/g/{name}"),
            name: name.into(),
            size: 1,
            sha256: "00".into(),
            id: id.map(String::from),
            version: None,
            variant: None,
            label: None,
            requires: None,
            kind: ExeKind::from_exe_id(id),
        }
    }

    #[test]
    fn the_base_is_the_stock_name_else_a_securom_exe() {
        let exes = [
            exe("Mercenaries2.cracked.exe", Some("v11-cracked-pmcbb")),
            exe("Mercenaries2.EXE", None),
        ];
        assert_eq!(base_exe(&exes).unwrap().name, "Mercenaries2.EXE");

        let exes = [
            exe("Mercenaries2.cracked.exe", Some("v11-cracked-pmcbb")),
            exe("Mercenaries2_orig.exe", Some("v11-patched-securom")),
        ];
        assert_eq!(base_exe(&exes).unwrap().name, "Mercenaries2_orig.exe");

        assert!(base_exe(&[exe("Mercenaries2.cracked.exe", Some("v11-cracked-pmcbb"))]).is_none());
    }
}
