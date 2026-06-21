//! WAD validation: fetch the `wad_simulator` release binary and run it.
//!
//! The simulator can also be installed with `cargo install wad_simulator`; in
//! that case [`validate_wad`] finds it on `PATH` (the default binary name).

use std::path::PathBuf;
use std::process::Command;

use serde::Serialize;

/// GitHub repo that publishes the `wad_simulator` release binaries.
const REPO: &str = "Mercenaries-Fan-Build/mercs2-wad-simulator";

/// Outcome of running the simulator against a WAD.
#[derive(Debug, Serialize)]
pub struct ValidationResult {
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Per-OS cache directory for the downloaded binary (no extra crates).
fn cache_dir() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "LOCALAPPDATA not set".to_string())?;
    #[cfg(not(target_os = "windows"))]
    let base = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".cache"))
        .ok_or_else(|| "HOME not set".to_string())?;

    let dir = base.join("mercs2-modkit").join("bin");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create cache dir: {e}"))?;
    Ok(dir)
}

/// Release asset name for the current platform (matches the release workflow).
fn platform_asset_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "wad_simulator-windows-x86_64.exe"
    } else if cfg!(target_os = "macos") {
        "wad_simulator-macos-x86_64"
    } else {
        "wad_simulator-linux-x86_64"
    }
}

/// Download (and cache) the latest `wad_simulator` release binary.
/// Returns the path to the cached executable.
#[tauri::command]
pub async fn fetch_wad_simulator() -> Result<String, String> {
    let dir = cache_dir()?;
    let asset = platform_asset_name();
    let dest = dir.join(asset);
    if dest.exists() {
        return Ok(dest.to_string_lossy().to_string());
    }

    let client = reqwest::Client::builder()
        .user_agent("mercs2-modkit")
        .build()
        .map_err(|e| format!("HTTP client: {e}"))?;

    let api = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let release: serde_json::Value = client
        .get(&api)
        .send()
        .await
        .map_err(|e| format!("Failed to query latest release: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Failed to parse release JSON: {e}"))?;

    let url = release["assets"]
        .as_array()
        .ok_or("Release has no assets")?
        .iter()
        .find(|a| a["name"].as_str() == Some(asset))
        .and_then(|a| a["browser_download_url"].as_str())
        .ok_or_else(|| format!("Release has no asset named '{asset}'"))?;

    let bytes = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("Download read failed: {e}"))?;

    std::fs::write(&dest, &bytes).map_err(|e| format!("Failed to save binary: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest)
            .map_err(|e| e.to_string())?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms).map_err(|e| e.to_string())?;
    }

    Ok(dest.to_string_lossy().to_string())
}

/// Run the simulator against a WAD. `simulator_path` defaults to `wad_simulator`
/// on `PATH` when omitted (e.g. after `cargo install wad_simulator`).
#[tauri::command]
pub fn validate_wad(
    wad_path: String,
    simulator_path: Option<String>,
) -> Result<ValidationResult, String> {
    let bin = simulator_path.unwrap_or_else(|| "wad_simulator".to_string());
    let output = Command::new(&bin)
        .arg("--wad")
        .arg(&wad_path)
        .output()
        .map_err(|e| {
            format!(
                "Failed to run '{bin}': {e}. Install with `cargo install wad_simulator`, \
                 or fetch the release binary first."
            )
        })?;

    Ok(ValidationResult {
        ok: output.status.success(),
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}
