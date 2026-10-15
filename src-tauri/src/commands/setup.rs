//! Game setup: install the right pmc_bb build, and crack/update the exe.
//!
//! Both pull prebuilt artifacts from the project's GitHub releases so the user
//! never needs a compiler or Python. Transport is [`super::net`]; which artifact,
//! where it lands, and what gets recorded is [`super::managed`].
//!
//! # There is no longer an "install pmc_bb" and an "install the logging pmc_bb"
//!
//! There were two commands, each hardcoding a release asset name — `pmc_bb.dll`
//! and `pmc_bb_log.dll`. Neither name exists upstream any more: the DLL is now six
//! feature-named builds, and which one belongs on a given install is a decision,
//! not a constant. [`install_pmc_bb`] is that decision applied — see
//! [`super::managed::pmc_bb`] for the policy and the trap in the obvious repair.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use tauri::Window;

use crate::commands::managed::pmc_bb;
use crate::commands::{dxwrapper, launch_plan};
use crate::commands::managed::{self, place, Component, Ledger, PlaceOpts};
use crate::commands::net::{self, AssetRule, ReleaseHost};
use crate::commands::paths::downloading_dir;
use crate::commands::proc::NoWindow;

/// Repo publishing the `apply_crack` SecuROM-bypass / updater tool.
const CRACK_REPO: &str = "Mercenaries-Fan-Build/mercs2-securom-bypass";

/// Download-progress key for the `apply_crack` fetch.
const CRACK_KEY: &str = "apply_crack";

/// An `apply_crack` build smaller than this is not a binary.
const CRACK_MIN_SIZE: u64 = 64 * 1024;

/// What was installed, and which build it was.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallDllResult {
    pub path: String,
    pub version: String,
    /// Release asset installed, e.g. `pmc_bb_asi_log.dll`. The on-disk name is
    /// always `pmc_bb.dll`, so this is the only thing that says which build.
    pub asset: String,
    pub features: pmc_bb::Features,
    /// Why this build was chosen, for the UI to show verbatim.
    pub reason: String,
    /// True when the caller forced a specific build.
    pub overridden: bool,
}

/// Every build the release publishes, for the advanced picker.
#[tauri::command(async)]
pub fn pmc_bb_variants() -> Vec<serde_json::Value> {
    pmc_bb::VARIANTS
        .iter()
        .map(|v| {
            serde_json::json!({
                "asset": v.asset,
                "features": v.features,
                "blurb": v.blurb,
            })
        })
        .collect()
}

/// Install the launch plan's wanted pmc_bb build: the pinned one
/// (`overrides.pmcBb`), else the one the launched exe calls for.
#[tauri::command]
pub async fn install_pmc_bb(window: Window, game_root: String) -> Result<InstallDllResult, String> {
    let root = PathBuf::from(&game_root);
    if !root.is_dir() {
        return Err(format!("Game folder not found: {game_root}"));
    }
    let plan = {
        let (window, root) = (window.clone(), root.clone());
        tauri::async_runtime::spawn_blocking(move || launch_plan::for_root(&window, &root))
            .await
            .map_err(|e| e.to_string())??
    };
    install_pmc_bb_build(&window, &root, &plan.pmc_bb_wanted).await
}

/// Download `choice` and place it as `pmc_bb.dll`, recording the asset in the
/// ledger (the file name can't tell the builds apart), then flip dxwrapper's
/// `LoadPlugins` to match the new build's loader.
pub(crate) async fn install_pmc_bb_build(
    window: &Window,
    root: &Path,
    choice: &pmc_bb::Choice,
) -> Result<InstallDllResult, String> {
    let client = net::client()?;
    let release = net::latest_release(&client, ReleaseHost::GitHub, pmc_bb::REPO).await?;
    let asset = release.require(
        &[AssetRule::Named(&choice.asset)],
        &format!("the {} build of pmc_bb", choice.features.names().join("+")),
    )?;

    let bytes = net::download(
        &client,
        &asset.url,
        net::DownloadOpts::new(pmc_bb::LEDGER_KEY, &choice.asset).with_window(Some(window)),
    )
    .await?;

    let dest = root.join(pmc_bb::INSTALL_NAME);
    let placed = place(
        &dest,
        &bytes,
        PlaceOpts::default()
            .expecting(asset.sha256())
            .sized(asset.size)
            .at_least(pmc_bb::MIN_SIZE),
    )?;

    let ledger = Ledger::app()?;
    ledger.record(Component {
        key: pmc_bb::LEDGER_KEY.to_string(),
        tag: release.tag.clone(),
        asset: choice.asset.clone(),
        features: choice.features.names(),
        source: pmc_bb::REPO.to_string(),
        installed_at: managed::ledger::now_unix(),
        files: vec![placed.clone().into()],
    })?;
    dxwrapper::sync_load_plugins(root, &ledger, !choice.features.asi, PlaceOpts::default())?;

    Ok(InstallDllResult {
        path: placed.abs_path,
        version: release.tag,
        asset: choice.asset.clone(),
        features: choice.features,
        reason: choice.reason.clone(),
        overridden: choice.overridden,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrackResult {
    pub ok: bool,
    pub output_path: String,
    pub stdout: String,
    pub stderr: String,
    /// Release tag of the `apply_crack` build that was run.
    pub tool_version: String,
}

/// Clear out the cache a previous version kept: an `apply_crack` ledger row and
/// the binary under `bin/` it points at. Nothing reads either any more, and a
/// recorded install of a tool modkit no longer installs misreports the ledger.
/// A no-op on every run after the first.
fn discard_cached_apply_crack() -> Result<(), String> {
    let ledger = Ledger::app()?;
    let Some(cached) = ledger.get(CRACK_KEY) else {
        return Ok(());
    };
    for file in &cached.files {
        let path = Path::new(&file.abs_path);
        if path.is_file() {
            std::fs::remove_file(path).map_err(|e| {
                format!("Could not clear the cached apply_crack at {}: {e}", path.display())
            })?;
        }
    }
    ledger.forget(CRACK_KEY)?;
    Ok(())
}

/// A freshly fetched `apply_crack`, deleted again when it goes out of scope.
struct CrackTool {
    /// Release tag that was fetched, reported as `CrackResult::toolVersion`.
    tag: String,
    bin: PathBuf,
    dir: PathBuf,
}

impl Drop for CrackTool {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Fetch the current `apply_crack` for this host into a scratch directory.
///
/// Nothing is kept. Cracking is a once-per-install action, so there is no copy to
/// amortise and a stale one is strictly worse: the latest release is resolved and
/// downloaded on every use, which is what makes the build that runs current by
/// construction. That is also why `managed::status::MANAGED` omits apply_crack —
/// with nothing retained there is no version to report and no update to announce.
async fn fetch_apply_crack(window: &Window) -> Result<CrackTool, String> {
    discard_cached_apply_crack()?;
    let client = net::client()?;
    let release = net::latest_release(&client, ReleaseHost::GitHub, CRACK_REPO).await?;

    // Prefer the build matching this host exactly; fall back to any build for this
    // OS so a single-arch or older release still resolves. apply_crack only
    // byte-patches the exe, so its output is identical across arches — the arch
    // only has to be one this machine can execute.
    let os = net::release::platform_token();
    let arch = net::release::arch_token();
    let exact = |n: &str| {
        arch.is_some_and(|a| n.starts_with("apply_crack") && n.contains(os) && n.contains(a))
    };
    let os_only = |n: &str| n.starts_with("apply_crack") && n.contains(os);
    let asset = release
        .require(
            &[AssetRule::Pred(&exact), AssetRule::Pred(&os_only)],
            &format!(
                "apply_crack on {os}/{}",
                arch.unwrap_or(std::env::consts::ARCH)
            ),
        )?
        .clone();

    let bytes = net::download(
        &client,
        &asset.url,
        net::DownloadOpts::new(CRACK_KEY, "apply_crack").with_window(Some(window)),
    )
    .await?;

    // Its own directory, named for the instant it was fetched, so two runs can
    // never land on one path and so the whole thing goes in a single remove.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("The system clock is before the epoch: {e}"))?
        .as_nanos();
    let dir = downloading_dir()?.join(format!("apply_crack-{stamp}"));
    let placed = place(
        &dir.join(&asset.name),
        &bytes,
        PlaceOpts::default()
            .executable()
            .expecting(asset.sha256())
            .sized(asset.size)
            .at_least(CRACK_MIN_SIZE)
            .keeping_no_bak(),
    )?;

    Ok(CrackTool {
        tag: release.tag,
        bin: PathBuf::from(placed.abs_path),
        dir,
    })
}

/// Download `apply_crack` and run it on the exe to apply the SecuROM bypass,
/// writing a new cracked exe. apply_crack auto-detects the version and always
/// brings a v1.0 input up to v1.1 before cracking (a v1.1 input skips that
/// itself) — there is no skip-update, so this always yields cracked v1.1.
#[tauri::command]
pub async fn crack_game(
    window: Window,
    exe_path: String,
    output_path: Option<String>,
) -> Result<CrackResult, String> {
    let exe = PathBuf::from(&exe_path);
    if !exe.is_file() {
        return Err(format!("Game exe not found: {exe_path}"));
    }

    let tool = fetch_apply_crack(&window).await?;

    let out = output_path.unwrap_or_else(|| {
        exe.with_file_name("Mercenaries2.cracked.exe")
            .to_string_lossy()
            .to_string()
    });

    let output = Command::new(&tool.bin)
        .arg(&exe_path)
        .arg("--output")
        .arg(&out)
        .no_window()
        .output()
        .map_err(|e| format!("Failed to run apply_crack: {e}"))?;

    Ok(CrackResult {
        ok: output.status.success(),
        output_path: out,
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        tool_version: tool.tag.clone(),
    })
}

/// Update the game exe to the official **v1.1 without cracking**
/// (`apply_crack --update-only`): produces the retail, still-SecuROM v1.1 and
/// installs it in place as the game's exe, backing the original up first. For
/// LICENSED copies — SecuROM stays intact, the activation carries over, and mods
/// load via dxwrapper + pmc_bb. The exe is replaced (this is an update), but the
/// original is always recoverable.
#[tauri::command]
pub async fn update_game(window: Window, exe_path: String) -> Result<CrackResult, String> {
    let exe = PathBuf::from(&exe_path);
    if !exe.is_file() {
        return Err(format!("Game exe not found: {exe_path}"));
    }

    let tool = fetch_apply_crack(&window).await?;

    // Produce the updated exe beside the original, then swap it in only on success.
    let staged = exe.with_file_name("Mercenaries2.v1.1.staged.exe");
    let output = Command::new(&tool.bin)
        .arg(&exe_path)
        .arg("--update-only")
        .arg("--output")
        .arg(&staged)
        .no_window()
        .output()
        .map_err(|e| format!("Failed to run apply_crack: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        let _ = std::fs::remove_file(&staged);
        return Ok(CrackResult {
            ok: false,
            output_path: exe_path,
            stdout,
            stderr,
            tool_version: tool.tag.clone(),
        });
    }

    // Keep mirroring the official patch's BACKUP/ convention — people know to look
    // there — but the recoverable copy `place` banks is what actually guarantees
    // the original survives. `BACKUP/` is written once and never clobbered.
    if let Some(root) = exe.parent() {
        let backup_dir = root.join("BACKUP");
        std::fs::create_dir_all(&backup_dir)
            .map_err(|e| format!("Failed to create BACKUP dir: {e}"))?;
        let backup = backup_dir.join(exe.file_name().unwrap_or_default());
        if !backup.exists() {
            std::fs::copy(&exe, &backup)
                .map_err(|e| format!("Failed to back up original exe: {e}"))?;
        }
    }

    let updated = std::fs::read(&staged)
        .map_err(|e| format!("Failed to read the updated exe: {e}"))?;
    let result = place(
        &exe,
        &updated,
        PlaceOpts {
            // The exe already has a BACKUP/ copy and a banked snapshot; a third
            // `Mercenaries2.exe.bak` beside it only invites launching the wrong one.
            keep_bak_sibling: false,
            ..PlaceOpts::default()
        },
    );
    let _ = std::fs::remove_file(&staged);
    result?;

    Ok(CrackResult {
        ok: true,
        output_path: exe_path,
        stdout,
        stderr,
        tool_version: tool.tag.clone(),
    })
}
