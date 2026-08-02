//! WAD validation: run `wad_simulator` against a built patch WAD.
//!
//! Fetching and versioning the binary is [`super::toolchain`]'s job — this module
//! only runs it. The download/cache logic that used to live here was a
//! single-tool, x86_64-only copy of that machinery, and its cache never updated
//! (`if dest.exists() { return }`), which pinned every user to whichever build
//! they happened to fetch first.
//!
//! The simulator can also be installed with `cargo install wad_simulator`; in
//! that case [`validate_wad`] finds it on `PATH` (the default binary name).

use std::process::Command;

use serde::Serialize;
use tauri::Window;

use super::proc::NoWindow;
use super::toolchain::ensure_tool;

/// Outcome of running the simulator against a WAD.
#[derive(Debug, Serialize)]
pub struct ValidationResult {
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Path to the managed `wad_simulator`, installing it from the latest toolset
/// release if it is not there yet. Kept under its original name because the
/// export flow calls it directly.
#[tauri::command]
pub async fn fetch_wad_simulator(window: Window) -> Result<String, String> {
    let path = ensure_tool(window, "wad_simulator").await?;
    Ok(path.to_string_lossy().to_string())
}

/// Run the simulator against a WAD. `simulator_path` defaults to `wad_simulator`
/// on `PATH` when omitted (e.g. after `cargo install wad_simulator`).
#[tauri::command(async)]
pub fn validate_wad(
    wad_path: String,
    simulator_path: Option<String>,
) -> Result<ValidationResult, String> {
    let bin = simulator_path.unwrap_or_else(|| "wad_simulator".to_string());
    let output = Command::new(&bin)
        .arg("--wad")
        .arg(&wad_path)
        .no_window()
        .output()
        .map_err(|e| {
            format!(
                "Failed to run '{bin}': {e}. Install it from the Workshop Tools page, \
                 or with `cargo install wad_simulator`."
            )
        })?;

    Ok(ValidationResult {
        ok: output.status.success(),
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}
