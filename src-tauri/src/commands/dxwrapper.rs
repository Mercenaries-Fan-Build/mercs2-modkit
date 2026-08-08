//! Non-destructive mod-loader setup for LICENSED copies of the game.
//!
//! Instead of cracking the exe (which de-DRMs it and rewrites its import table),
//! a legitimately activated copy loads mods through **dxwrapper**: a stub the game
//! already imports (`d3d9.dll` — it's a Direct3D 9 title) hands off to
//! `dxwrapper.dll`, which via `LoadCustomDllPath` side-loads modkit's `pmc_bb.dll`.
//! The exe is never touched and SecuROM stays intact, satisfied by the machine's
//! real activation.
//!
//! # Exactly one ASI loader, and which one is no longer a constant
//!
//! dxwrapper is *also* an ASI loader (`[Plugins] LoadPlugins`), so exactly one of
//! the two has to own plugin scanning. This module used to hardcode
//! `LoadPlugins = 0`, on the reasoning that pmc_bb is the loader — which was true
//! when the licensed path installed a pmc_bb build that had one.
//!
//! It no longer is. pmc-blackbox now publishes six feature variants, and the build
//! [`super::managed::pmc_bb`] picks for an uncracked exe is `pmc_bb_log_only.dll`:
//! logging and crash reports, with the ASI loader compiled **out**. Leaving
//! `LoadPlugins = 0` against that build means neither side scans and **no mod
//! loads at all** — silently, with every install step reporting success.
//!
//! So the value is derived from the variant, never written from memory:
//!
//! ```text
//!   pmc_bb carries `asi`  ->  LoadPlugins = 0   (pmc_bb scans, as before)
//!   pmc_bb does not       ->  LoadPlugins = 1   (dxwrapper scans)
//! ```
//!
//! Final layout next to an untouched `Mercenaries2.exe`:
//! ```text
//!   d3d9.dll       <- dxwrapper stub (from the release zip)
//!   dxwrapper.dll  <- dxwrapper itself
//!   dxwrapper.ini  <- EnableD3d9Wrapper=1; LoadCustomDllPath=pmc_bb.dll; LoadPlugins per above
//!   pmc_bb.dll     <- whichever build install_pmc_bb resolved
//!   scripts/*.asi  <- mods, scanned by whichever side owns it
//! ```

use std::path::PathBuf;

use serde::Serialize;
use tauri::Window;

use crate::commands::managed::pmc_bb::{self, ExeKind};
use crate::commands::managed::{self, place, Component, Ledger, PlaceOpts};
use crate::commands::net::{self, archive, AssetRule, ReleaseHost};

/// Upstream dxwrapper releases.
const DXWRAPPER_REPO: &str = "elishacloud/dxwrapper";

/// Ledger key.
const KEY: &str = "dxwrapper";

/// Which DLL dxwrapper masquerades as. Mercenaries 2 is a Direct3D 9 title, so it
/// always loads `d3d9.dll` (and it's not a KnownDLL, so a local copy wins the
/// search) — the one stub guaranteed to load. It's also dxwrapper's primary
/// purpose, and the proxy verified working on the reference install.
const PROXY_DLL: &str = "d3d9.dll";

/// A dxwrapper DLL smaller than this is not a build.
const MIN_DLL_SIZE: u64 = 32 * 1024;

/// Patch the release's bundled `dxwrapper.ini` — already preconfigured for D3D9 in
/// `dx9.games.zip` — with just the two keys modkit owns. Every other setting the
/// release ships is preserved verbatim.
fn patch_ini(bundled: &str, loads_plugins: bool) -> String {
    let plugins = u8::from(loads_plugins);
    let mut out = String::with_capacity(bundled.len() + 32);
    for line in bundled.lines() {
        let key = line.trim_start().to_ascii_lowercase();
        if key.starts_with("loadcustomdllpath") && line.contains('=') {
            out.push_str("LoadCustomDllPath = pmc_bb.dll");
        } else if key.starts_with("loadplugins") && line.contains('=') {
            out.push_str(&format!("LoadPlugins = {plugins}"));
        } else {
            out.push_str(line);
        }
        out.push_str("\r\n");
    }
    out
}

/// The config modkit writes when the release ships none usable (dxwrapper always
/// reads `dxwrapper.ini` regardless of the stub's name).
///
/// `LoadCustomDllPath` must be the DLL, not a folder: the reference install had
/// `scripts/` there and dxwrapper answered "Cannot load custom library".
fn dxwrapper_ini(loads_plugins: bool) -> String {
    let plugins = u8::from(loads_plugins);
    let who = if loads_plugins {
        "ON: the installed pmc_bb build has no ASI loader, so dxwrapper scans."
    } else {
        "OFF on purpose: pmc_bb is the ASI loader, not dxwrapper (avoids two loaders)."
    };
    format!(
        "; Written by mercs2-modkit - licensed (non-destructive) mod path.\r\n\
         ; The exe is NOT modified. dxwrapper wraps d3d9 (the game is D3D9) and\r\n\
         ; side-loads pmc_bb.dll.\r\n\
         \r\n\
         [General]\r\n\
         RealDllPath       = AUTO\r\n\
         WrapperMode       = AUTO\r\n\
         ; The mod bridge. MUST be the DLL, not a folder.\r\n\
         LoadCustomDllPath = pmc_bb.dll\r\n\
         DisableLogging    = 0\r\n\
         \r\n\
         [Plugins]\r\n\
         ; {who}\r\n\
         LoadPlugins         = {plugins}\r\n\
         LoadFromScriptsOnly = 0\r\n\
         \r\n\
         [Compatibility]\r\n\
         EnableD3d9Wrapper = 1\r\n"
    )
}

/// Outcome of a dxwrapper install.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DxwrapperResult {
    pub ok: bool,
    /// Release tag of the dxwrapper build installed.
    pub version: String,
    pub proxy_path: String,
    pub dxwrapper_path: String,
    pub ini_path: String,
    /// Whether dxwrapper was configured to scan for plugins itself.
    pub loads_plugins: bool,
    /// Which pmc_bb build that decision was made against.
    pub pmc_bb_asset: String,
    /// Non-fatal notes (e.g. what was displaced).
    pub notes: Vec<String>,
}

/// Download dxwrapper and install it as the non-destructive loader in `game_root`.
/// Does NOT modify the exe. Pair with `install_pmc_bb`, which resolves the build
/// this config is written for.
#[tauri::command]
pub async fn install_dxwrapper(
    window: Window,
    game_root: String,
) -> Result<DxwrapperResult, String> {
    let root = PathBuf::from(&game_root);
    if !root.is_dir() {
        return Err(format!("Game folder not found: {game_root}"));
    }

    // Which pmc_bb build this install will get decides who owns plugin scanning.
    // Resolved rather than assumed — the assumption is what would silently stop
    // mods loading.
    let report = crate::commands::verify::identify_main_exe(&window, &root);
    let kind = ExeKind::from_exe_id(report.as_ref().and_then(|r| r.identified_id.as_deref()));
    let choice = pmc_bb::resolve(kind, None)?;
    let loads_plugins = !choice.features.asi;

    let client = net::client()?;
    let release = net::latest_release(&client, ReleaseHost::GitHub, DXWRAPPER_REPO).await?;

    // A dxwrapper release ships several zips: dx7/dx8/dx9.games.zip (per-API
    // preconfigured bundles), dxwrapper.zip (full release build),
    // dxwrapper.debug.zip (a DEBUG build whose different init flow does NOT enable
    // the wrapper from a minimal ini, so LoadCustomDllPath never runs), plus
    // symbols.*.zip. Mercs2 is D3D9, so dx9.games.zip is the right package. A
    // "contains dxwrapper" match grabbed the debug build, which sorts first.
    let loose = |n: &str| {
        n.ends_with(".zip") && n.contains("dxwrapper") && !n.contains("debug") && !n.contains("symbol")
    };
    let asset = release
        .require(
            &[
                AssetRule::Named("dx9.games.zip"),
                AssetRule::Named("dxwrapper.zip"),
                AssetRule::Pred(&loose),
            ],
            "the dxwrapper package",
        )?
        .clone();

    let zip_bytes = net::download(
        &client,
        &asset.url,
        net::DownloadOpts::new(KEY, "dxwrapper").with_window(Some(&window)),
    )
    .await?;

    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))
        .map_err(|e| format!("Bad dxwrapper zip: {e}"))?;

    let dxwrapper_dll =
        archive::read_entry(&mut zip, "dxwrapper.dll").ok_or("dxwrapper zip has no dxwrapper.dll")?;
    // In dx9.games.zip the stub is `d3d9.dll` at the root; the full dxwrapper.zip
    // keeps stubs under `Stub/`. Try both so either package works.
    let stub_dll = archive::read_entry(&mut zip, &format!("stub/{PROXY_DLL}"))
        .or_else(|| archive::read_entry(&mut zip, PROXY_DLL))
        .ok_or_else(|| format!("dxwrapper zip has no {PROXY_DLL} (root or Stub/)"))?;

    // Prefer the release's own preconfigured ini, patched; else modkit's fallback.
    let ini_text = archive::read_entry(&mut zip, "dxwrapper.ini")
        .and_then(|b| String::from_utf8(b).ok())
        .filter(|s| s.to_ascii_lowercase().contains("loadcustomdllpath"))
        .map(|s| patch_ini(&s, loads_plugins))
        .unwrap_or_else(|| dxwrapper_ini(loads_plugins));

    let proxy_path = root.join(PROXY_DLL);
    let dxwrapper_path = root.join("dxwrapper.dll");
    let ini_path = root.join("dxwrapper.ini");

    let placed_proxy = place(
        &proxy_path,
        &stub_dll,
        PlaceOpts::default().at_least(MIN_DLL_SIZE),
    )?;
    let placed_dll = place(
        &dxwrapper_path,
        &dxwrapper_dll,
        PlaceOpts::default().at_least(MIN_DLL_SIZE),
    )?;
    let placed_ini = place(&ini_path, ini_text.as_bytes(), PlaceOpts::default())?;

    let notes = [&placed_proxy, &placed_dll, &placed_ini]
        .iter()
        .filter_map(|p| {
            p.backup.as_ref().map(|b| {
                format!(
                    "Backed up the existing {} (recoverable at {b})",
                    PathBuf::from(&p.abs_path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                )
            })
        })
        .collect::<Vec<_>>();

    Ledger::app()?.record(Component {
        key: KEY.to_string(),
        tag: release.tag.clone(),
        asset: asset.name.clone(),
        features: if loads_plugins {
            vec!["loads-plugins".to_string()]
        } else {
            Vec::new()
        },
        source: DXWRAPPER_REPO.to_string(),
        installed_at: managed::ledger::now_unix(),
        files: vec![
            placed_proxy.clone().into(),
            placed_dll.clone().into(),
            placed_ini.clone().into(),
        ],
    })?;

    Ok(DxwrapperResult {
        ok: true,
        version: release.tag,
        proxy_path: placed_proxy.abs_path,
        dxwrapper_path: placed_dll.abs_path,
        ini_path: placed_ini.abs_path,
        loads_plugins,
        pmc_bb_asset: choice.asset,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value_of(ini: &str, key: &str) -> Option<String> {
        ini.lines()
            .find(|l| {
                l.trim_start()
                    .to_ascii_lowercase()
                    .starts_with(&key.to_ascii_lowercase())
                    && l.contains('=')
            })
            .and_then(|l| l.split('=').nth(1))
            .map(|v| v.trim().to_string())
    }

    /// The consequence the module docs spell out. An uncracked exe gets
    /// `pmc_bb_log_only.dll`, which has no ASI loader — so if dxwrapper is also
    /// told not to scan, nothing loads plugins and every step still reports
    /// success.
    #[test]
    fn dxwrapper_takes_over_scanning_when_pmc_bb_has_no_loader() {
        let choice = pmc_bb::resolve(ExeKind::NotCracked, None).unwrap();
        assert!(!choice.features.asi, "the premise of this test changed");

        let loads_plugins = !choice.features.asi;
        assert_eq!(value_of(&dxwrapper_ini(loads_plugins), "LoadPlugins").as_deref(), Some("1"));
    }

    #[test]
    fn dxwrapper_stands_down_when_pmc_bb_carries_the_loader() {
        let choice = pmc_bb::resolve(ExeKind::NotCracked, Some("pmc_bb_asi_log.dll")).unwrap();
        assert!(choice.features.asi);

        let ini = dxwrapper_ini(!choice.features.asi);
        assert_eq!(value_of(&ini, "LoadPlugins").as_deref(), Some("0"));
    }

    /// Whichever side scans, the bridge always points at the DLL — pointing it at
    /// a folder is what made dxwrapper log "Cannot load custom library".
    #[test]
    fn the_custom_dll_path_is_always_the_dll() {
        for loads in [true, false] {
            assert_eq!(
                value_of(&dxwrapper_ini(loads), "LoadCustomDllPath").as_deref(),
                Some("pmc_bb.dll"),
            );
        }
    }

    #[test]
    fn patching_rewrites_only_the_two_keys_modkit_owns() {
        let bundled = "[General]\r\n\
             RealDllPath = AUTO\r\n\
             LoadCustomDllPath = scripts/\r\n\
             DisableLogging = 1\r\n\
             [Plugins]\r\n\
             LoadPlugins = 1\r\n\
             LoadFromScriptsOnly = 1\r\n";

        let out = patch_ini(bundled, false);
        assert_eq!(value_of(&out, "LoadCustomDllPath").as_deref(), Some("pmc_bb.dll"));
        assert_eq!(value_of(&out, "LoadPlugins").as_deref(), Some("0"));
        // Untouched settings survive verbatim.
        assert_eq!(value_of(&out, "RealDllPath").as_deref(), Some("AUTO"));
        assert_eq!(value_of(&out, "DisableLogging").as_deref(), Some("1"));
        assert_eq!(value_of(&out, "LoadFromScriptsOnly").as_deref(), Some("1"));
        assert!(out.contains("[General]") && out.contains("[Plugins]"));
    }

    #[test]
    fn patching_can_also_hand_scanning_to_dxwrapper() {
        let bundled = "[Plugins]\r\nLoadPlugins = 0\r\n";
        assert_eq!(
            value_of(&patch_ini(bundled, true), "LoadPlugins").as_deref(),
            Some("1")
        );
    }

    /// Exactly one side scans, for every exe kind modkit can identify. Two loaders
    /// double-load plugins; zero loaders load none.
    #[test]
    fn exactly_one_side_owns_plugin_scanning() {
        for kind in [
            ExeKind::CrackedImportingPmcBb,
            ExeKind::CrackedImportingOther,
            ExeKind::NotCracked,
            ExeKind::Unknown,
        ] {
            let choice = pmc_bb::resolve(kind, None).unwrap();
            let loads_plugins = !choice.features.asi;
            assert_ne!(
                loads_plugins, choice.features.asi,
                "{kind:?}: dxwrapper and pmc_bb must not both scan, nor both stand down"
            );
        }
    }
}
