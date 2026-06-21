//! Game setup: install the pmc_bb.dll ASI loader and crack/update the exe.
//!
//! Both pull prebuilt artifacts from the project's GitHub releases so the user
//! never needs a compiler or Python.

use std::path::PathBuf;
use std::process::Command;

use serde::Serialize;

use crate::commands::paths::app_data_dir;

/// Repo publishing `pmc_bb.dll` (ASI loader + SecuROM spoof).
const PMC_BB_REPO: &str = "Mercenaries-Fan-Build/pmc-blackbox";
/// Repo publishing the `apply_crack` SecuROM-bypass / updater tool.
const CRACK_REPO: &str = "Mercenaries-Fan-Build/mercs2-securom-bypass";

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("mercs2-modkit")
        .build()
        .map_err(|e| e.to_string())
}

/// Download an asset of a repo's latest release, trying each predicate in
/// `picks` in priority order (first predicate that any asset matches wins).
/// Returns `(release_tag, asset_name, bytes)`.
async fn download_latest_asset(
    client: &reqwest::Client,
    repo: &str,
    picks: &[&(dyn Fn(&str) -> bool + Sync)],
) -> Result<(String, String, Vec<u8>), String> {
    let api = format!("https://api.github.com/repos/{repo}/releases/latest");
    let v: serde_json::Value = client
        .get(&api)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| format!("Release lookup failed for {repo}: {e}"))?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let tag = v["tag_name"].as_str().unwrap_or("latest").to_string();
    let assets = v["assets"].as_array().ok_or("Latest release has no assets")?;
    let (name, url) = picks
        .iter()
        .find_map(|pick| {
            assets.iter().find_map(|a| {
                let n = a["name"].as_str()?;
                if pick(n) {
                    Some((n.to_string(), a["browser_download_url"].as_str()?.to_string()))
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| format!("No matching asset in the latest release of {repo}"))?;

    let bytes = client
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?
        .to_vec();

    Ok((tag, name, bytes))
}

/// OS token used to pick the right `apply_crack` build.
fn platform_token() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// CPU-arch token (paired with the OS token) to pick the matching `apply_crack`
/// build when a release ships more than one arch (e.g. windows i686 + x86_64).
fn arch_token() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "x86") {
        "i686"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        ""
    }
}

#[derive(Debug, Serialize)]
pub struct InstallDllResult {
    pub path: String,
    pub version: String,
}

/// Download the latest `pmc_bb.dll` and place it in the game root (our ASI
/// loader). Any existing copy is backed up to `pmc_bb.dll.bak` first.
#[tauri::command]
pub async fn install_pmc_bb(game_root: String) -> Result<InstallDllResult, String> {
    let root = PathBuf::from(&game_root);
    if !root.is_dir() {
        return Err(format!("Game folder not found: {game_root}"));
    }

    let client = client()?;
    // pmc_bb.dll is the injected Windows ASI loader — one platform-independent
    // asset, matched by exact name regardless of the host the modkit runs on.
    let pick_dll = |n: &str| n.eq_ignore_ascii_case("pmc_bb.dll");
    let picks: [&(dyn Fn(&str) -> bool + Sync); 1] = [&pick_dll];
    let (tag, _name, bytes) = download_latest_asset(&client, PMC_BB_REPO, &picks).await?;

    let dest = root.join("pmc_bb.dll");
    if dest.exists() {
        let backup = dest.with_extension("dll.bak");
        let _ = std::fs::rename(&dest, &backup);
    }
    std::fs::write(&dest, &bytes).map_err(|e| format!("Failed to write pmc_bb.dll: {e}"))?;

    Ok(InstallDllResult {
        path: dest.to_string_lossy().to_string(),
        version: tag,
    })
}

#[derive(Debug, Serialize)]
pub struct CrackResult {
    pub ok: bool,
    pub output_path: String,
    pub stdout: String,
    pub stderr: String,
}

/// Download `apply_crack` and run it on the exe to apply the SecuROM bypass,
/// optionally first updating v1.0 → v1.1. Writes a new cracked exe.
#[tauri::command]
pub async fn crack_game(
    exe_path: String,
    output_path: Option<String>,
    update_to_v11: bool,
) -> Result<CrackResult, String> {
    let exe = PathBuf::from(&exe_path);
    if !exe.is_file() {
        return Err(format!("Game exe not found: {exe_path}"));
    }

    let client = client()?;
    let os = platform_token();
    let arch = arch_token();
    // Prefer the build matching our exact OS+arch (releases now ship e.g. both
    // windows-i686 and windows-x86_64); fall back to any build for this OS so a
    // single-arch release still resolves. apply_crack only byte-patches the exe,
    // so the patched output is identical across arches — this is host-compat only.
    let exact = |n: &str| n.starts_with("apply_crack") && n.contains(os) && n.contains(arch);
    let os_only = |n: &str| n.starts_with("apply_crack") && n.contains(os);
    let picks: [&(dyn Fn(&str) -> bool + Sync); 2] = [&exact, &os_only];
    let (_tag, name, bytes) = download_latest_asset(&client, CRACK_REPO, &picks)
        .await
        .map_err(|e| format!("{e}. No apply_crack build for {os}/{arch}."))?;

    // Cache the tool binary and make it executable.
    let bindir = app_data_dir()?.join("bin");
    std::fs::create_dir_all(&bindir).map_err(|e| e.to_string())?;
    let bin = bindir.join(&name);
    std::fs::write(&bin, &bytes).map_err(|e| format!("Failed to save apply_crack: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&bin).map_err(|e| e.to_string())?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin, perms).map_err(|e| e.to_string())?;
    }

    let out = output_path
        .unwrap_or_else(|| exe.with_file_name("Mercenaries2.cracked.exe").to_string_lossy().to_string());

    let mut cmd = Command::new(&bin);
    cmd.arg(&exe_path).arg("--output").arg(&out);
    if !update_to_v11 {
        cmd.arg("--skip-update");
    }
    let output = cmd
        .output()
        .map_err(|e| format!("Failed to run apply_crack: {e}"))?;

    Ok(CrackResult {
        ok: output.status.success(),
        output_path: out,
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}
