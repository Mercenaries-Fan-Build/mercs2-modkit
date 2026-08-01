//! Base-game detection: locate the install, identify its version/variant, and
//! report modding state (pmc_bb.dll, ASI loader, deployed patch WADs).

use std::path::{Path, PathBuf};

use serde::Serialize;

/// A detected Mercenaries 2 installation and its current modding state.
#[derive(Debug, Serialize)]
pub struct GameInfo {
    /// Absolute path to the folder the user selected.
    pub root: String,
    /// Absolute path to the located `Mercenaries2.exe`. This is the *base* exe —
    /// the input `apply_crack` patches — not necessarily the one we launch.
    pub exe_path: String,
    pub exe_size: u64,
    /// `"v1.0"`, `"v1.1"`, or `"unknown"`.
    pub version: String,
    /// `"unsigned"`, `"ea-signed"`, `"patched"`, `"cracked"`, or `"unknown"`.
    pub variant: String,
    /// The de-DRM'd exe sitting alongside the base one (what setup writes as
    /// `Mercenaries2.cracked.exe`), if any. `None` when the base exe is itself
    /// the cracked build, or when no cracked sibling exists.
    pub cracked_exe: Option<ExeCandidate>,
    /// The exe `launch_game` will actually run: the cracked sibling when one is
    /// present, else `exe_path`.
    pub launch_exe_path: String,
    /// `pmc_bb.dll` present in the install (the ASI loader / logging bridge —
    /// the DRM-spoofing build on the crack path, or the logging-only build on
    /// the licensed dxwrapper path; both live at this name).
    pub has_pmc_bb: bool,
    /// `dxwrapper.dll` present in the install — the non-destructive loader used
    /// for licensed copies. When set, the modkit launches the stock exe and
    /// never the cracked one.
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

/// One `Mercenaries2*.exe` found in the install, identified by size.
#[derive(Debug, Clone, Serialize)]
pub struct ExeCandidate {
    pub path: String,
    pub name: String,
    pub size: u64,
    /// `"v1.0"`, `"v1.1"`, or `"unknown"`.
    pub version: String,
    /// `"unsigned"`, `"ea-signed"`, `"patched"`, `"cracked"`, or `"unknown"`.
    pub variant: String,
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

/// The stock executable name — the crack input and the game's own entry point.
const BASE_EXE: &str = "mercenaries2.exe";
/// The name setup writes the de-DRM'd exe to by default.
const CRACKED_EXE: &str = "mercenaries2.cracked.exe";

/// Every `Mercenaries2*.exe` in `root`, identified by size, sorted by filename
/// so the pick below is deterministic.
fn list_exes(root: &Path) -> Vec<ExeCandidate> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        if !(lower.starts_with("mercenaries2") && lower.ends_with(".exe")) {
            continue;
        }
        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        let (version, variant) = classify(size);
        out.push(ExeCandidate {
            path: p.to_string_lossy().to_string(),
            name: lower,
            size,
            version: version.to_string(),
            variant: variant.to_string(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// True for an exe we should launch in preference to the base one: either it
/// identifies as a cracked build, or it carries the name setup writes. The name
/// check matters because a future `apply_crack` release can change the output
/// size, and an unrecognised size must not silently demote it back to the
/// SecuROM exe.
fn is_cracked(e: &ExeCandidate) -> bool {
    e.variant == "cracked" || e.name == CRACKED_EXE
}

/// Split the install's executables into the base exe (what `apply_crack` takes
/// as input, and what we report as the install's identity) and the cracked exe
/// alongside it, if the user has produced one.
///
/// Returns `None` when the folder holds no Mercenaries 2 executable at all.
pub fn resolve_exes(root: &Path) -> Option<(ExeCandidate, Option<ExeCandidate>)> {
    let exes = list_exes(root);
    if exes.is_empty() {
        return None;
    }

    // Base: the stock filename, else the first exe that isn't a cracked build,
    // else whatever is there (an install where only the cracked exe remains).
    let base = exes
        .iter()
        .find(|e| e.name == BASE_EXE)
        .or_else(|| exes.iter().find(|e| !is_cracked(e)))
        .unwrap_or(&exes[0])
        .clone();

    // Cracked sibling: prefer setup's default name, else any other cracked build.
    // Never the base itself — if the base *is* cracked, the install is already
    // set up in place and there is no sibling to report.
    let cracked = exes
        .iter()
        .filter(|e| e.path != base.path && is_cracked(e))
        .find(|e| e.name == CRACKED_EXE)
        .or_else(|| exes.iter().find(|e| e.path != base.path && is_cracked(e)))
        .cloned();

    Some((base, cracked))
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
pub fn detect_game(path: String) -> Result<GameInfo, String> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(format!("Not a folder: {path}"));
    }

    let (base, cracked) = resolve_exes(&root)
        .ok_or_else(|| "No Mercenaries2.exe found in that folder".to_string())?;
    // What we launch, mirroring `launch::launch_exe`: on the licensed dxwrapper
    // path the stock exe is launched untouched; otherwise the cracked build wins.
    let has_dxwrapper = root.join("dxwrapper.dll").is_file();
    let launch_exe_path = if has_dxwrapper {
        base.path.clone()
    } else {
        cracked
            .as_ref()
            .map(|c| c.path.clone())
            .unwrap_or_else(|| base.path.clone())
    };

    let data_dir = find_data_dir(&root);
    let deployed_patches = data_dir
        .as_deref()
        .map(list_deployed_patches)
        .unwrap_or_default();

    Ok(GameInfo {
        root: root.to_string_lossy().to_string(),
        exe_path: base.path.clone(),
        exe_size: base.size,
        version: base.version.clone(),
        variant: base.variant.clone(),
        cracked_exe: cracked,
        launch_exe_path,
        has_pmc_bb: root.join("pmc_bb.dll").is_file(),
        has_dxwrapper,
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

    /// Write a file of exactly `size` bytes so `classify` sees the real variant.
    fn exe(dir: &Path, name: &str, size: u64) {
        std::fs::write(dir.join(name), vec![0u8; size as usize]).unwrap();
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mercs2-exes-{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn cracked_sibling_is_reported_without_renaming_the_original() {
        let d = tmpdir("sibling");
        exe(&d, "Mercenaries2.exe", SIZE_V11_PATCHED);
        exe(&d, "Mercenaries2.cracked.exe", SIZE_V11_CRACKED);

        let (base, cracked) = resolve_exes(&d).unwrap();
        assert_eq!(base.name, "mercenaries2.exe");
        assert_eq!(base.variant, "patched");
        let cracked = cracked.expect("the cracked sibling must be detected");
        assert_eq!(cracked.name, "mercenaries2.cracked.exe");
        assert_eq!((cracked.version.as_str(), cracked.variant.as_str()), ("v1.1", "cracked"));
    }

    #[test]
    fn cracked_in_place_reports_no_sibling() {
        let d = tmpdir("in-place");
        exe(&d, "Mercenaries2.exe", SIZE_V11_CRACKED);

        let (base, cracked) = resolve_exes(&d).unwrap();
        assert_eq!((base.version.as_str(), base.variant.as_str()), ("v1.1", "cracked"));
        assert!(cracked.is_none(), "the base exe is the cracked build; no sibling");
    }

    /// A crack whose size the catalog doesn't know must still be preferred for
    /// launch — the default filename identifies it.
    #[test]
    fn unrecognised_size_still_counts_by_name() {
        let d = tmpdir("unknown-size");
        exe(&d, "Mercenaries2.exe", SIZE_V11_PATCHED);
        exe(&d, "Mercenaries2.cracked.exe", 53_500_000);

        let (_, cracked) = resolve_exes(&d).unwrap();
        let cracked = cracked.expect("name alone is enough to prefer it");
        assert_eq!(cracked.variant, "unknown");
    }

    /// The user pointed the "Output exe" browser at a custom filename.
    #[test]
    fn custom_named_crack_is_found_by_classification() {
        let d = tmpdir("custom-name");
        exe(&d, "Mercenaries2.exe", SIZE_V11_PATCHED);
        exe(&d, "Mercenaries2-nodrm.exe", SIZE_V11_CRACKED);

        let (base, cracked) = resolve_exes(&d).unwrap();
        assert_eq!(base.name, "mercenaries2.exe");
        assert_eq!(cracked.expect("classified as cracked").name, "mercenaries2-nodrm.exe");
    }

    #[test]
    fn only_a_cracked_exe_is_still_the_base() {
        let d = tmpdir("only-cracked");
        exe(&d, "Mercenaries2.cracked.exe", SIZE_V11_CRACKED);

        let (base, cracked) = resolve_exes(&d).unwrap();
        assert_eq!(base.name, "mercenaries2.cracked.exe");
        assert!(cracked.is_none(), "nothing to prefer over the only exe present");
    }

    #[test]
    fn no_executable_is_none() {
        let d = tmpdir("empty");
        assert!(resolve_exes(&d).is_none());
    }
}
