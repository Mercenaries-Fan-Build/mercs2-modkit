//! The macOS Wine build modkit downloads, keeps, and launches the game with.
//!
//! # Why modkit owns Wine on macOS
//!
//! Homebrew no longer packages Wine, and leaning on a launcher that manages its
//! own Wine (Heroic, CrossOver, Whisky) ties the game's runtime to another app's
//! update cycle and settings. modkit targets one game, so it fetches one Wine
//! itself and records it like any other managed artifact.
//!
//! The source is `Heroic-Games-Launcher/wine-crossover`: its `23.7.1-1` build
//! (`wine64`, experimental wow64 mode) is the one verified to boot Mercenaries 2 on
//! Apple Silicon. Its assets are `Wine-Crossover-<version>.tar.xz`, selected by
//! rule, never by a hardcoded filename.
//!
//! # Layout and record
//!
//! ```text
//! <app-data>/runtimes/wine/<tag>/Wine-Crossover-<version>/Contents/Resources/wine/bin/wine64
//! ```
//!
//! Every release tag gets its own directory, so several builds can be installed
//! and the user picks one ([`select_wine`]); the pick lives in
//! [`crate::commands::runtime_settings`]. Each install is a ledger entry keyed
//! `wine:<tag>`. The entry's files are the two binaries the launcher runs
//! (`wine64`, `wineserver`) rather than all ~4,600 files: hashing 840 MB on every
//! status check would make the page unusable, and those two are what a launch
//! depends on finding. The downloaded archive itself is checked against the
//! digest GitHub publishes for it before anything is unpacked.
//!
//! Nothing is selected silently. Installing a build selects it only when nothing
//! is selected yet; removing the selected build clears the selection, and a launch
//! with no selection fails with what to do.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::Window;

use super::{ledger, Component, InstalledFile, Ledger};
use crate::commands::net::{self, AssetRule, ReleaseHost};
use crate::commands::runtime_settings;

/// Upstream releases.
pub const REPO: &str = "Heroic-Games-Launcher/wine-crossover";

/// Ledger keys are `wine:<tag>`.
const KEY_PREFIX: &str = "wine:";

/// The binary the launcher runs, found by name inside the unpacked build.
const WINE_BIN: &str = "wine64";
/// Its server, which must sit beside it.
const WINESERVER_BIN: &str = "wineserver";

fn is_wine_asset(name: &str) -> bool {
    name.starts_with("wine-crossover-") && name.ends_with(".tar.xz")
}

fn asset_rules() -> [AssetRule<'static>; 1] {
    [AssetRule::Pred(&is_wine_asset)]
}

fn key(tag: &str) -> String {
    format!("{KEY_PREFIX}{tag}")
}

/// `<app-data>/runtimes/wine`.
fn wine_root() -> Result<PathBuf, String> {
    Ok(crate::commands::paths::app_data_dir()?.join("runtimes").join("wine"))
}

/// A tag is author-chosen text and becomes a directory name; refuse anything
/// that is not one plain path segment.
fn tag_dir(root: &Path, tag: &str) -> Result<PathBuf, String> {
    if tag.is_empty() || tag == "." || tag == ".." || tag.contains(['/', '\\', '\0']) {
        return Err(format!("Release tag '{tag}' cannot be used as a directory name."));
    }
    Ok(root.join(tag))
}

/// The single `bin/wine64` inside an unpacked build, with `wineserver` beside it.
fn find_wine(dir: &Path) -> Result<(PathBuf, PathBuf), String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = std::fs::read_dir(&d).map_err(|e| format!("Could not read {}: {e}", d.display()))?;
        for e in rd.flatten() {
            let p = e.path();
            let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
            if meta.is_dir() {
                stack.push(p);
            } else if meta.is_file()
                && p.file_name().is_some_and(|n| n == WINE_BIN)
                && p.parent().and_then(|b| b.file_name()).is_some_and(|n| n == "bin")
            {
                found.push(p);
            }
        }
    }
    match found.as_slice() {
        [one] => {
            let server = one.with_file_name(WINESERVER_BIN);
            if !server.is_file() {
                return Err(format!(
                    "The Wine build has {} but no {WINESERVER_BIN} beside it.",
                    one.display()
                ));
            }
            Ok((one.clone(), server))
        }
        [] => Err(format!("The Wine build in {} has no bin/{WINE_BIN}.", dir.display())),
        many => Err(format!(
            "The Wine build in {} has {} bin/{WINE_BIN} files; expected one.",
            dir.display(),
            many.len()
        )),
    }
}

fn installed_file(path: &Path) -> Result<InstalledFile, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    Ok(InstalledFile {
        abs_path: path.to_string_lossy().into_owned(),
        sha256: super::place::sha256_of_file(path)?,
        size: meta.len(),
        backup: None,
    })
}

/// The wine64 path recorded for an install.
fn wine64_of(c: &Component) -> Option<PathBuf> {
    c.files
        .iter()
        .map(|f| PathBuf::from(&f.abs_path))
        .find(|p| p.file_name().is_some_and(|n| n == WINE_BIN))
}

/// A tag's digit runs as numbers (`23.7.1-1` → `[23, 7, 1, 1]`), so tags order
/// numerically. [`super::is_newer`] is not a total order for tags like
/// `23.7.1-1`, and a sort needs one.
fn version_key(tag: &str) -> Vec<u64> {
    tag.split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap_or(u64::MAX))
        .collect()
}

/// Every Wine install the ledger records, newest tag first.
fn installed(ledger: &Ledger) -> Vec<Component> {
    let mut v: Vec<Component> = ledger
        .read()
        .into_values()
        .filter(|c| c.key.starts_with(KEY_PREFIX))
        .collect();
    v.sort_by(|a, b| {
        version_key(&b.tag)
            .cmp(&version_key(&a.tag))
            .then_with(|| a.tag.cmp(&b.tag))
    });
    v
}

/// The `wine64` the launcher runs: the selected build, which must be installed
/// and still on disk.
pub fn selected_wine64(settings: &runtime_settings::RuntimeSettings) -> Result<PathBuf, String> {
    let tag = settings.wine_tag.as_deref().ok_or(
        "No Wine build is selected. Install one in Game Info → Runtime before launching.",
    )?;
    let c = Ledger::app()?.get(&key(tag)).ok_or_else(|| {
        format!("Wine {tag} is selected but not installed. Install it in Game Info → Runtime.")
    })?;
    if !c.is_present() {
        return Err(format!(
            "Wine {tag}'s files are missing from disk. Reinstall it in Game Info → Runtime."
        ));
    }
    wine64_of(&c).ok_or_else(|| format!("The record for Wine {tag} names no {WINE_BIN}."))
}

fn require_macos() -> Result<(), String> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err("modkit manages Wine only on macOS. On Linux, pick a Proton build instead.".into())
    }
}

/// One installed build, for the UI.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledWine {
    pub tag: String,
    pub asset: String,
    pub wine64: Option<String>,
    pub installed_at: u64,
    /// Both binaries are on disk.
    pub present: bool,
    /// Both are on disk but one no longer has the bytes modkit unpacked.
    pub modified: bool,
    pub selected: bool,
}

/// One published build the user can install.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WineRelease {
    pub tag: String,
    pub asset: String,
    pub size: Option<u64>,
    /// The asset has finished uploading.
    pub ready: bool,
    pub url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WineStatus {
    /// Whether modkit manages Wine on this host (macOS only).
    pub applicable: bool,
    pub repo: String,
    pub selected: Option<String>,
    pub installed: Vec<InstalledWine>,
    /// Published builds, newest first; empty unless a remote check was asked for.
    pub releases: Vec<WineRelease>,
    /// Why the remote check produced nothing, when it failed.
    pub remote_error: Option<String>,
}

/// Installed and (with `check_remote`) published Wine builds.
#[tauri::command]
pub async fn wine_status(check_remote: bool) -> Result<WineStatus, String> {
    let applicable = cfg!(target_os = "macos");
    let settings = runtime_settings::read()?;
    let ledger = Ledger::app()?;
    let selected = settings.wine_tag.clone();
    let installed = installed(&ledger)
        .into_iter()
        .map(|c| InstalledWine {
            wine64: wine64_of(&c).map(|p| p.to_string_lossy().into_owned()),
            present: c.is_present(),
            modified: c.is_present() && !c.is_intact(),
            selected: selected.as_deref() == Some(c.tag.as_str()),
            installed_at: c.installed_at,
            asset: c.asset,
            tag: c.tag,
        })
        .collect();

    let mut status = WineStatus {
        applicable,
        repo: REPO.into(),
        selected,
        installed,
        releases: Vec::new(),
        remote_error: None,
    };
    if !(check_remote && applicable) {
        return Ok(status);
    }
    let lookup = async {
        let client = net::client()?;
        net::list_releases(&client, ReleaseHost::GitHub, REPO).await
    };
    match lookup.await {
        Ok(releases) => {
            status.releases = releases
                .iter()
                .filter_map(|r| {
                    let a = r.pick(&asset_rules())?;
                    Some(WineRelease {
                        tag: r.tag.clone(),
                        asset: a.name.clone(),
                        size: a.size,
                        ready: a.is_ready(),
                        url: r.url.clone(),
                    })
                })
                .collect();
        }
        Err(e) => status.remote_error = Some(e),
    }
    Ok(status)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WineInstall {
    pub tag: String,
    pub wine64: String,
    /// Whether this install became the selected build (nothing was selected).
    pub selected: bool,
}

/// Download, verify, unpack and record one Wine release (the latest when `tag` is
/// `None`). Reinstalling a tag moves the old copy to the trash first.
#[tauri::command]
pub async fn install_wine(window: Window, tag: Option<String>) -> Result<WineInstall, String> {
    require_macos()?;
    let client = net::client()?;
    let release = match tag.as_deref() {
        Some(t) => {
            let url = net::release::github_release_by_tag_url(net::release::GITHUB_API, REPO, t);
            net::release::github_release_by_tag_at(&client, &url, REPO, t).await?
        }
        None => net::latest_release(&client, ReleaseHost::GitHub, REPO).await?,
    };
    let asset = release.require(&asset_rules(), "the macOS Wine build")?.clone();

    let label = format!("Wine {}", release.tag);
    let bytes = net::download(
        &client,
        &asset.url,
        net::DownloadOpts::new("wine", &label).with_window(Some(&window)),
    )
    .await?;
    if let Some(want) = asset.sha256() {
        let got = super::place::sha256_hex(&bytes);
        if !got.eq_ignore_ascii_case(want) {
            return Err(format!(
                "{} failed its integrity check: GitHub publishes sha256 {want}, the download is {got}.",
                asset.name
            ));
        }
    }

    let tag = release.tag.clone();
    let root = wine_root()?;
    let dest = tag_dir(&root, &tag)?;
    let asset_name = asset.name.clone();
    // Unpacking ~840 MB is blocking work; keep it off the async runtime.
    let (wine64, server) = tokio::task::spawn_blocking(move || unpack(&bytes, &root, &dest))
        .await
        .map_err(|e| format!("Unpacking Wine stopped unexpectedly: {e}"))??;

    Ledger::app()?.record(Component {
        key: key(&tag),
        tag: tag.clone(),
        asset: asset_name,
        features: Vec::new(),
        source: REPO.into(),
        installed_at: ledger::now_unix(),
        files: vec![installed_file(&wine64)?, installed_file(&server)?],
    })?;

    let mut became_selected = false;
    runtime_settings::update(|s| {
        if s.wine_tag.is_none() {
            s.wine_tag = Some(tag.clone());
            became_selected = true;
        }
    })?;

    Ok(WineInstall {
        tag,
        wine64: wine64.to_string_lossy().into_owned(),
        selected: became_selected,
    })
}

/// Unpack into a sibling staging dir, then move it into place, so a failed
/// extraction never leaves a half-build where the launcher would find it.
fn unpack(bytes: &[u8], root: &Path, dest: &Path) -> Result<(PathBuf, PathBuf), String> {
    std::fs::create_dir_all(root).map_err(|e| format!("Could not create {}: {e}", root.display()))?;
    let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("wine");
    let staging = root.join(format!(".{name}.partial"));
    if staging.exists() {
        super::trash::discard(&staging, None)?;
    }
    net::archive::extract_tar_xz(bytes, &staging)?;
    find_wine(&staging)?;
    if dest.exists() {
        super::trash::discard(dest, None)?;
    }
    std::fs::rename(&staging, dest)
        .map_err(|e| format!("Could not move the Wine build into {}: {e}", dest.display()))?;
    find_wine(dest)
}

/// Make `tag` the build the launcher runs.
#[tauri::command(async)]
pub fn select_wine(tag: String) -> Result<(), String> {
    require_macos()?;
    let c = Ledger::app()?
        .get(&key(&tag))
        .ok_or_else(|| format!("Wine {tag} is not installed."))?;
    if !c.is_present() {
        return Err(format!("Wine {tag}'s files are missing from disk. Reinstall it first."));
    }
    runtime_settings::update(|s| s.wine_tag = Some(tag))?;
    Ok(())
}

/// Move an installed build to the trash and forget it. Removing the selected
/// build clears the selection; another build is never picked in its place.
#[tauri::command(async)]
pub fn remove_wine(tag: String) -> Result<(), String> {
    require_macos()?;
    let ledger = Ledger::app()?;
    if ledger.get(&key(&tag)).is_none() {
        return Err(format!("Wine {tag} is not installed."));
    }
    let dir = tag_dir(&wine_root()?, &tag)?;
    if dir.exists() {
        super::trash::discard(&dir, None)?;
    }
    ledger.forget(&key(&tag))?;
    runtime_settings::update(|s| {
        if s.wine_tag.as_deref() == Some(tag.as_str()) {
            s.wine_tag = None;
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_asset_rule_matches_the_published_name() {
        // The rule sees lowercased names.
        assert!(is_wine_asset("wine-crossover-23.7.1-1.tar.xz"));
        assert!(!is_wine_asset("wine-crossover-23.7.1-1.zip"));
        assert!(!is_wine_asset("wine-staging-11.18-osx64.tar.xz"));
    }

    #[test]
    fn a_tag_must_be_one_path_segment() {
        let root = Path::new("/r");
        assert_eq!(tag_dir(root, "23.7.1-1").unwrap(), root.join("23.7.1-1"));
        for bad in ["", ".", "..", "a/b", "a\\b"] {
            assert!(tag_dir(root, bad).is_err(), "{bad:?}");
        }
    }

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }

    #[test]
    fn find_wine_locates_the_one_wine64_and_its_server() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("Wine-Crossover-1/Contents/Resources/wine/bin");
        touch(&bin.join("wine64"));
        touch(&bin.join("wineserver"));
        touch(&dir.path().join("Wine-Crossover-1/Contents/Resources/wine/lib/wine64.so"));
        let (wine, server) = find_wine(dir.path()).unwrap();
        assert_eq!(wine, bin.join("wine64"));
        assert_eq!(server, bin.join("wineserver"));
    }

    #[test]
    fn find_wine_refuses_a_build_without_a_server_or_with_two_wines() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("a/bin/wine64"));
        assert!(find_wine(dir.path()).unwrap_err().contains("wineserver"));

        touch(&dir.path().join("a/bin/wineserver"));
        touch(&dir.path().join("b/bin/wine64"));
        assert!(find_wine(dir.path()).unwrap_err().contains("expected one"));

        let empty = tempfile::tempdir().unwrap();
        assert!(find_wine(empty.path()).unwrap_err().contains("no bin/wine64"));
    }

    #[test]
    fn installs_sort_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::at(&dir.path().join("installed.json"));
        for tag in ["9.0", "23.7.1-1", "11.18"] {
            ledger
                .record(Component {
                    key: key(tag),
                    tag: tag.into(),
                    asset: "a".into(),
                    features: vec![],
                    source: REPO.into(),
                    installed_at: 0,
                    files: vec![],
                })
                .unwrap();
        }
        ledger
            .record(Component {
                key: "pmc_bb".into(),
                tag: "v1".into(),
                asset: "a".into(),
                features: vec![],
                source: "x/y".into(),
                installed_at: 0,
                files: vec![],
            })
            .unwrap();
        let tags: Vec<_> = installed(&ledger).into_iter().map(|c| c.tag).collect();
        assert_eq!(tags.len(), 3, "only wine:* entries");
        assert_eq!(tags[0], "23.7.1-1");
    }
}
