//! Non-destructive mod-loader setup for LICENSED copies of the game.
//!
//! Instead of cracking the exe (which de-DRMs it and rewrites its import table),
//! a legitimately activated copy loads mods through **dxwrapper**: a stub the
//! game already imports (`d3d9.dll` — it's a Direct3D 9 title) hands off to
//! `dxwrapper.dll`, which — via `LoadCustomDllPath` — side-loads the modkit's
//! logging-only `pmc_bb.dll` (no SecuROM event), and pmc_bb is the ASI loader
//! that loads `scripts/*.asi`. The exe is never touched and SecuROM stays intact,
//! satisfied by the machine's real activation.
//!
//! **One loader, deliberately.** dxwrapper is *also* an ASI loader
//! (`[Plugins] LoadPlugins`), so we set `LoadPlugins = 0` to keep it out of the
//! plugin business — pmc_bb's own folder scanner (the same one proven on the
//! crack path) is the single loader. This mirrors the config verified working on
//! the reference install, whose only bug was `LoadCustomDllPath = scripts/` (a
//! folder — dxwrapper logs "Cannot load custom library"); the correct value is
//! the DLL, `pmc_bb.dll`.
//!
//! Final layout next to an untouched `Mercenaries2.exe`:
//! ```text
//!   d3d9.dll       ← dxwrapper stub (from the release zip's Stub/ folder)
//!   dxwrapper.dll  ← dxwrapper itself
//!   dxwrapper.ini  ← EnableD3d9Wrapper=1; LoadPlugins=0; LoadCustomDllPath=pmc_bb.dll
//!   pmc_bb.dll     ← logging-only build (installed by `install_pmc_bb_log`)
//!   scripts/*.asi  ← mods (unchanged deploy path), loaded by pmc_bb
//! ```
//! dxwrapper is pulled from upstream `elishacloud/dxwrapper` releases; the `.ini`
//! is written by the modkit (below), not taken from the zip.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// Upstream dxwrapper releases.
const DXWRAPPER_REPO: &str = "elishacloud/dxwrapper";

/// Which DLL dxwrapper masquerades as. Mercenaries 2 is a Direct3D 9 title, so it
/// always loads `d3d9.dll` (and it's not a KnownDLL, so a local copy wins the
/// search) — the one stub guaranteed to load. It's also dxwrapper's primary
/// purpose (a D3D9 wrapper), and it's the proxy verified working on the reference
/// install. `dsound.dll` isn't even present there, so it was never a valid choice.
const PROXY_DLL: &str = "d3d9.dll";

/// Patch the release's bundled `dxwrapper.ini` (already preconfigured for D3D9 in
/// dx9.games.zip) with just the two keys the modkit owns:
///   - `LoadCustomDllPath = pmc_bb.dll` — side-load the logging bridge;
///   - `LoadPlugins = 0` — pmc_bb is the sole loader, dxwrapper's scanner stays off.
/// Every other setting the release ships is preserved verbatim. Used in preference
/// to the hand-written `dxwrapper_ini()` fallback, whose sparse shape isn't proven.
fn patch_ini(bundled: &str) -> String {
    let mut out = String::with_capacity(bundled.len() + 32);
    for line in bundled.lines() {
        let key = line.trim_start().to_ascii_lowercase();
        if key.starts_with("loadcustomdllpath") && line.contains('=') {
            out.push_str("LoadCustomDllPath = pmc_bb.dll");
        } else if key.starts_with("loadplugins") && line.contains('=') {
            out.push_str("LoadPlugins = 0");
        } else {
            out.push_str(line);
        }
        out.push_str("\r\n");
    }
    out
}

/// The dxwrapper config the modkit writes (dxwrapper always reads `dxwrapper.ini`
/// regardless of the stub's name). Modeled on the config verified working on the
/// reference install, with the one fix that made the mod chain work:
/// `LoadCustomDllPath = pmc_bb.dll` (not `scripts/`, which dxwrapper rejects as
/// "Cannot load custom library").
///
/// **Single loader:** `LoadPlugins = 0` keeps dxwrapper's own ASI scanner off, so
/// pmc_bb — side-loaded via `LoadCustomDllPath` — is the one loader that scans
/// `scripts/*.asi`. `AUTO` lets dxwrapper detect the `d3d9.dll` stub; the
/// D3D9 wrapper is enabled to match the proven setup.
fn dxwrapper_ini() -> String {
    "; Written by mercs2-modkit — licensed (non-destructive) mod path.\r\n\
     ; The exe is NOT modified. dxwrapper wraps d3d9 (the game is D3D9) and\r\n\
     ; side-loads pmc_bb.dll. dxwrapper's own ASI loader is OFF (LoadPlugins=0);\r\n\
     ; pmc_bb is the single loader and scans scripts/*.asi.\r\n\
     \r\n\
     [General]\r\n\
     RealDllPath       = AUTO\r\n\
     WrapperMode       = AUTO\r\n\
     ; The mod bridge + loader. MUST be the DLL, not a folder.\r\n\
     LoadCustomDllPath = pmc_bb.dll\r\n\
     DisableLogging    = 0\r\n\
     \r\n\
     [Plugins]\r\n\
     ; OFF on purpose: pmc_bb is the ASI loader, not dxwrapper (avoids two loaders).\r\n\
     LoadPlugins         = 0\r\n\
     LoadFromScriptsOnly = 0\r\n\
     \r\n\
     [Compatibility]\r\n\
     EnableD3d9Wrapper = 1\r\n"
        .to_string()
}

/// Outcome of a dxwrapper install.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DxwrapperResult {
    pub ok: bool,
    /// Release tag of the dxwrapper build that was installed.
    pub version: String,
    /// Absolute path of the stub proxy DLL written (e.g. `…/dsound.dll`).
    pub proxy_path: String,
    /// Absolute path of `dxwrapper.dll`.
    pub dxwrapper_path: String,
    /// Absolute path of `dxwrapper.ini`.
    pub ini_path: String,
    /// Non-fatal notes (e.g. what was backed up).
    pub notes: Vec<String>,
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("mercs2-modkit")
        .build()
        .map_err(|e| e.to_string())
}

/// `(tag, zip_bytes)` for the dxwrapper release zip in the latest release.
async fn fetch_dxwrapper_zip(client: &reqwest::Client) -> Result<(String, Vec<u8>), String> {
    let api = format!("https://api.github.com/repos/{DXWRAPPER_REPO}/releases/latest");
    let v: serde_json::Value = client
        .get(&api)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| format!("dxwrapper release lookup failed: {e}"))?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let tag = v["tag_name"].as_str().unwrap_or("latest").to_string();
    let assets = v["assets"].as_array().ok_or("dxwrapper release has no assets")?;

    // A dxwrapper release ships SEVERAL zips: dx7/dx8/dx9.games.zip (per-API
    // preconfigured bundles), dxwrapper.zip (full release build),
    // dxwrapper.debug.zip (a DEBUG build — different init flow that does NOT enable
    // the wrapper from a minimal ini, so LoadCustomDllPath never runs), plus
    // symbols.*.zip. Mercs2 is a Direct3D 9 title, so the correct package is
    // dx9.games.zip (release dxwrapper.dll + the d3d9 stub). Pick it explicitly,
    // then the plain release dxwrapper.zip, and NEVER a debug/symbols build — a
    // "contains dxwrapper" match grabbed dxwrapper.debug.zip (it sorts first).
    let lname = |a: &serde_json::Value| a["name"].as_str().map(|n| n.to_ascii_lowercase());
    let pick = assets
        .iter()
        .find(|a| lname(a).as_deref() == Some("dx9.games.zip"))
        .or_else(|| assets.iter().find(|a| lname(a).as_deref() == Some("dxwrapper.zip")))
        .or_else(|| {
            assets.iter().find(|a| {
                lname(a).is_some_and(|n| {
                    n.ends_with(".zip")
                        && n.contains("dxwrapper")
                        && !n.contains("debug")
                        && !n.contains("symbol")
                })
            })
        })
        .and_then(|a| a["browser_download_url"].as_str())
        .ok_or("No suitable dxwrapper zip (dx9.games.zip / dxwrapper.zip) in the latest release")?;

    let bytes = client
        .get(pick)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?
        .to_vec();

    Ok((tag, bytes))
}

/// Read one zip entry whose path ends with `suffix` (case-insensitive), matched
/// on the full archive path so we can target `Stub/dsound.dll` specifically.
fn read_zip_entry(zip: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>, suffix: &str) -> Option<Vec<u8>> {
    let suffix_lc = suffix.to_ascii_lowercase().replace('\\', "/");
    // Collect matching names first (immutable), then read (mutable borrow).
    let name = (0..zip.len()).find_map(|i| {
        let f = zip.by_index(i).ok()?;
        let n = f.name().replace('\\', "/").to_ascii_lowercase();
        if n.ends_with(&suffix_lc) {
            Some(f.name().to_string())
        } else {
            None
        }
    })?;
    let mut f = zip.by_name(&name).ok()?;
    let mut buf = Vec::with_capacity(f.size() as usize);
    f.read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Move an existing file aside to `<name>.bak` (best-effort), noting it.
fn backup(path: &Path, notes: &mut Vec<String>) {
    if path.exists() {
        let bak = path.with_extension(format!(
            "{}.bak",
            path.extension().and_then(|e| e.to_str()).unwrap_or("")
        ));
        if std::fs::rename(path, &bak).is_ok() {
            notes.push(format!("Backed up existing {} → {}", disp(path), disp(&bak)));
        }
    }
}

fn disp(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| p.to_string_lossy().to_string())
}

/// Download dxwrapper and install it as the non-destructive loader in `game_root`.
/// Does NOT modify the exe. Pair with `install_pmc_bb_log` for the logging bridge.
#[tauri::command]
pub async fn install_dxwrapper(game_root: String) -> Result<DxwrapperResult, String> {
    let root = PathBuf::from(&game_root);
    if !root.is_dir() {
        return Err(format!("Game folder not found: {game_root}"));
    }

    let client = client()?;
    let (tag, zip_bytes) = fetch_dxwrapper_zip(&client).await?;

    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))
        .map_err(|e| format!("Bad dxwrapper zip: {e}"))?;

    let dxwrapper_dll = read_zip_entry(&mut zip, "dxwrapper.dll")
        .ok_or("dxwrapper zip has no dxwrapper.dll")?;
    // In dx9.games.zip the stub is `d3d9.dll` at the root; the full dxwrapper.zip
    // keeps stubs under `Stub/`. Try both so either package works.
    let stub_dll = read_zip_entry(&mut zip, &format!("stub/{PROXY_DLL}"))
        .or_else(|| read_zip_entry(&mut zip, PROXY_DLL))
        .ok_or_else(|| format!("dxwrapper zip has no {PROXY_DLL} (root or Stub/)"))?;
    // Prefer the release's own preconfigured ini (patched), else our fallback.
    let ini_text = read_zip_entry(&mut zip, "dxwrapper.ini")
        .and_then(|b| String::from_utf8(b).ok())
        .filter(|s| s.to_ascii_lowercase().contains("loadcustomdllpath"))
        .map(|s| patch_ini(&s))
        .unwrap_or_else(dxwrapper_ini);

    let mut notes = Vec::new();

    let proxy_path = root.join(PROXY_DLL);
    let dxwrapper_path = root.join("dxwrapper.dll");
    let ini_path = root.join("dxwrapper.ini");

    backup(&proxy_path, &mut notes);
    backup(&dxwrapper_path, &mut notes);
    backup(&ini_path, &mut notes);

    std::fs::write(&proxy_path, &stub_dll)
        .map_err(|e| format!("Failed to write {PROXY_DLL}: {e}"))?;
    std::fs::write(&dxwrapper_path, &dxwrapper_dll)
        .map_err(|e| format!("Failed to write dxwrapper.dll: {e}"))?;
    std::fs::write(&ini_path, ini_text)
        .map_err(|e| format!("Failed to write dxwrapper.ini: {e}"))?;

    Ok(DxwrapperResult {
        ok: true,
        version: tag,
        proxy_path: proxy_path.to_string_lossy().to_string(),
        dxwrapper_path: dxwrapper_path.to_string_lossy().to_string(),
        ini_path: ini_path.to_string_lossy().to_string(),
        notes,
    })
}
