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

use std::path::PathBuf;
use std::process::Command;

use serde::Serialize;
use tauri::Window;

use crate::commands::managed::pmc_bb::{self, ExeKind};
use crate::commands::managed::{self, m2_sdk, place, Component, Ledger, PlaceOpts};
use crate::commands::net::{self, AssetRule, ReleaseHost};
use crate::commands::paths::app_data_dir;
use crate::commands::proc::NoWindow;

/// Repo publishing the `apply_crack` SecuROM-bypass / updater tool.
const CRACK_REPO: &str = "Mercenaries-Fan-Build/mercs2-securom-bypass";

/// Ledger key for the cached `apply_crack` build.
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

/// Which pmc_bb build this install wants, without downloading anything.
///
/// Exposed so the Game Info panel can explain the choice — and flag a mismatch —
/// before the user commits to an install.
#[tauri::command]
pub async fn resolve_pmc_bb(
    window: Window,
    game_root: String,
    variant: Option<String>,
) -> Result<pmc_bb::Choice, String> {
    let root = PathBuf::from(&game_root);
    if !root.is_dir() {
        return Err(format!("Game folder not found: {game_root}"));
    }
    pmc_bb::resolve(exe_kind(&window, &root).await, variant.as_deref())
}

/// The exe's catalogue identity, as far as the variant choice cares.
///
/// Identification hashes a ~20 MB executable and parses the bundled manifest, so
/// it goes to the blocking pool rather than parking an async runtime worker —
/// the discipline `commands::mod`'s header lays out, and what `verify_game`
/// already does for the same work.
///
/// A failure to identify is [`ExeKind::Unknown`], which resolves to the build that
/// changes nothing about how the game starts. That is the right direction to fail
/// in: the harmful mistake is installing a SecuROM spoof where none is wanted.
pub(crate) async fn exe_kind(window: &Window, root: &std::path::Path) -> ExeKind {
    let window = window.clone();
    let root = root.to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let report = crate::commands::verify::identify_main_exe(&window, &root);
        ExeKind::from_exe_id(report.as_ref().and_then(|r| r.identified_id.as_deref()))
    })
    .await
    .unwrap_or(ExeKind::Unknown)
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

/// Download the pmc_bb build this install wants and place it in the game root.
///
/// `variant` forces a specific release asset; omit it for the automatic choice.
/// Whatever is chosen is installed as `pmc_bb.dll` — the name the exe's import
/// table and dxwrapper's `LoadCustomDllPath` both resolve — and recorded in the
/// ledger under the asset it actually came from, because after the copy the
/// filename can no longer tell the six builds apart.
#[tauri::command]
pub async fn install_pmc_bb(
    window: Window,
    game_root: String,
    variant: Option<String>,
) -> Result<InstallDllResult, String> {
    let root = PathBuf::from(&game_root);
    if !root.is_dir() {
        return Err(format!("Game folder not found: {game_root}"));
    }

    let choice = pmc_bb::resolve(exe_kind(&window, &root).await, variant.as_deref())?;

    let client = net::client()?;
    let release = net::latest_release(&client, ReleaseHost::GitHub, pmc_bb::REPO).await?;
    let asset = release.require(
        &[AssetRule::Named(&choice.asset)],
        &format!("the {} build of pmc_bb", choice.features.names().join("+")),
    )?;

    let bytes = net::download(
        &client,
        &asset.url,
        net::DownloadOpts::new("pmc_bb", &choice.asset).with_window(Some(&window)),
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

    Ledger::app()?.record(Component {
        key: "pmc_bb".to_string(),
        tag: release.tag.clone(),
        asset: choice.asset.clone(),
        features: choice.features.names(),
        source: pmc_bb::REPO.to_string(),
        installed_at: managed::ledger::now_unix(),
        files: vec![placed.clone().into()],
    })?;

    Ok(InstallDllResult {
        path: placed.abs_path,
        version: release.tag,
        asset: choice.asset,
        features: choice.features,
        reason: choice.reason,
        overridden: choice.overridden,
    })
}

/// What installing the m2-sdk runtime produced.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallM2SdkResult {
    pub path: String,
    pub version: String,
}

/// Download `m2-sdk.dll` at the highest release satisfying `version_req`, and place it in the game
/// ROOT.
///
/// Internal to auto-on-deploy: the SDK is developer infrastructure — a shared library every
/// SDK-based plugin links against — installed only as a resolved dependency (see
/// [`resolve_shipment_dependencies`]), never as a user action. It is managed rather than shipped
/// (see [`super::managed::m2_sdk`]), and it lands in the game root — not `scripts/` — because
/// pmc_bb's loader resolves a plugin's imports from the exe's directory. A copy in `scripts/` beside
/// the `.asi` is invisible to it, and is the `0x7E` ERROR_MOD_NOT_FOUND load failure.
pub(crate) async fn install_m2_sdk(
    window: Window,
    game_root: String,
    // The semver range from the requiring Shipment's `load.requires` (e.g. `"^0.1"`).
    version_req: String,
) -> Result<InstallM2SdkResult, String> {
    let root = PathBuf::from(&game_root);
    if !root.is_dir() {
        return Err(format!("Game folder not found: {game_root}"));
    }

    let client = net::client()?;
    // Resolve the range to the highest release that satisfies it, so a Shipment gets the SDK its
    // `load.requires` declared — one shared, updatable copy, ledger-recorded below.
    let req = semver::VersionReq::parse(&version_req)
        .map_err(|e| format!("m2-sdk requirement {version_req:?} is not a valid semver range: {e}"))?;
    let releases = net::list_releases(&client, ReleaseHost::GitHub, m2_sdk::REPO).await?;
    let release = net::highest_satisfying(&releases, &req).cloned().ok_or_else(|| {
        format!(
            "no published m2-sdk release satisfies {version_req:?} — available: {}",
            releases
                .iter()
                .map(|r| r.tag.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    let asset = release.require(
        &[AssetRule::Named(m2_sdk::ASSET)],
        "the m2-sdk runtime (m2-sdk.dll)",
    )?;

    let bytes = net::download(
        &client,
        &asset.url,
        net::DownloadOpts::new(m2_sdk::KEY, m2_sdk::ASSET).with_window(Some(&window)),
    )
    .await?;

    let dest = root.join(m2_sdk::INSTALL_NAME);
    let placed = place(
        &dest,
        &bytes,
        PlaceOpts::default()
            .expecting(asset.sha256())
            .sized(asset.size)
            .at_least(m2_sdk::MIN_SIZE),
    )?;

    Ledger::app()?.record(Component {
        key: m2_sdk::KEY.to_string(),
        tag: release.tag.clone(),
        asset: m2_sdk::ASSET.to_string(),
        features: Vec::new(),
        source: m2_sdk::REPO.to_string(),
        installed_at: managed::ledger::now_unix(),
        files: vec![placed.clone().into()],
    })?;

    Ok(InstallM2SdkResult {
        path: placed.abs_path,
        version: release.tag,
    })
}

/// Map a Shipment's managed-dependency NAME (from `load.requires`) to the component repo that
/// satisfies it. `None` for a name modkit does not manage — not fatal: the dependency is reported
/// unresolved rather than blocking the deploy, since a future modkit may manage it.
fn managed_component_repo(name: &str) -> Option<&'static str> {
    match name {
        // Spelled `m2-sdk` in manifests; accept the ledger key spelling too.
        "m2-sdk" | m2_sdk::KEY => Some(m2_sdk::REPO),
        _ => None,
    }
}

/// One managed dependency after an auto-on-deploy resolution pass.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedDependency {
    pub name: String,
    pub version_req: String,
    /// The release tag installed, or `None` when the dependency was skipped (see `note`).
    pub installed_tag: Option<String>,
    /// Why it was skipped, when it was.
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveDepsResult {
    pub resolved: Vec<ResolvedDependency>,
}

/// Resolve and install a Shipment's managed dependencies — the auto-on-deploy step.
///
/// Reads the Shipment's `load.requires`, and for each MANAGED (`{name, version}`) requirement
/// installs the highest released version satisfying its semver range, through the same
/// ledger-tracked channel as a manual install. A range nothing satisfies is a hard error; a dep
/// naming a component modkit does not manage is reported unresolved but does not fail the deploy.
#[tauri::command]
pub async fn resolve_shipment_dependencies(
    window: Window,
    shipment_dir: String,
    game_root: String,
) -> Result<ResolveDepsResult, String> {
    let dir = PathBuf::from(&shipment_dir);
    // No manifest is not an error: a deploy may hand a directory that is not a Shipment source
    // tree (an already-built patch, say). There is simply nothing to resolve.
    let Some(manifest) = crate::commands::shipment::manifest_path(&dir) else {
        return Ok(ResolveDepsResult { resolved: Vec::new() });
    };

    let mut resolved = Vec::new();
    for (name, version_req) in crate::commands::shipment::read_managed_requirements(&manifest) {
        match managed_component_repo(&name) {
            Some(repo) if repo == m2_sdk::REPO => {
                let installed =
                    install_m2_sdk(window.clone(), game_root.clone(), version_req.clone()).await?;
                resolved.push(ResolvedDependency {
                    name,
                    version_req,
                    installed_tag: Some(installed.version),
                    note: None,
                });
            }
            Some(_) => resolved.push(ResolvedDependency {
                name,
                version_req,
                installed_tag: None,
                note: Some("recognised, but no installer is wired for it yet".into()),
            }),
            None => resolved.push(ResolvedDependency {
                name,
                version_req,
                installed_tag: None,
                note: Some("not a modkit-managed component; left for the mod to provide".into()),
            }),
        }
    }
    Ok(ResolveDepsResult { resolved })
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

/// Path to a current `apply_crack` for this host, downloading it only when the
/// cached copy is missing, altered, or out of date.
///
/// It used to be re-downloaded on every single crack, which is why the update
/// check had to special-case it as never-actionable: with nothing recording a
/// version, "up to date" was not a state it could be in. Now it is an ordinary
/// ledger component like any other.
async fn ensure_apply_crack(window: &Window) -> Result<(String, PathBuf), String> {
    let ledger = Ledger::app()?;
    let client = net::client()?;
    let release = net::latest_release(&client, ReleaseHost::GitHub, CRACK_REPO).await?;

    if let Some(existing) = ledger.get(CRACK_KEY) {
        if existing.tag == release.tag && existing.is_intact() {
            if let Some(f) = existing.files.first() {
                return Ok((existing.tag.clone(), PathBuf::from(&f.abs_path)));
            }
        }
    }

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

    let bindir = app_data_dir()?.join("bin");
    let bin = bindir.join(&asset.name);
    let placed = place(
        &bin,
        &bytes,
        PlaceOpts::default()
            .executable()
            .expecting(asset.sha256())
            .sized(asset.size)
            .at_least(CRACK_MIN_SIZE),
    )?;

    ledger.record(Component {
        key: CRACK_KEY.to_string(),
        tag: release.tag.clone(),
        asset: asset.name.clone(),
        features: Vec::new(),
        source: CRACK_REPO.to_string(),
        installed_at: managed::ledger::now_unix(),
        files: vec![placed.into()],
    })?;

    Ok((release.tag, bin))
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

    let (tag, bin) = ensure_apply_crack(&window).await?;

    let out = output_path.unwrap_or_else(|| {
        exe.with_file_name("Mercenaries2.cracked.exe")
            .to_string_lossy()
            .to_string()
    });

    let output = Command::new(&bin)
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
        tool_version: tag,
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

    let (tag, bin) = ensure_apply_crack(&window).await?;

    // Produce the updated exe beside the original, then swap it in only on success.
    let staged = exe.with_file_name("Mercenaries2.v1.1.staged.exe");
    let output = Command::new(&bin)
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
            tool_version: tag,
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
        tool_version: tag,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Shipment spells the dep `m2-sdk`; the ledger key is `m2_sdk`. Both must resolve to the
    /// same component, or a manifest and the ledger would disagree about what got installed.
    #[test]
    fn managed_component_repo_maps_both_spellings_of_m2_sdk() {
        assert_eq!(managed_component_repo("m2-sdk"), Some(m2_sdk::REPO));
        assert_eq!(managed_component_repo(m2_sdk::KEY), Some(m2_sdk::REPO));
    }

    /// A name modkit does not manage resolves to `None` — reported unresolved, not guessed at.
    #[test]
    fn managed_component_repo_is_none_for_an_unmanaged_name() {
        assert_eq!(managed_component_repo("some-random-lib"), None);
        assert_eq!(managed_component_repo(""), None);
    }
}
