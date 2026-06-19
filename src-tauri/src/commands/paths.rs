//! App-managed directories for the mod lifecycle: downloading → staging → deployed.

use std::path::PathBuf;

/// Root app-data directory (`mercs2-modkit`) under the OS-appropriate base.
pub fn app_data_dir() -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Library/Application Support"));
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));

    let base = base.ok_or("Could not resolve a home/app-data directory")?;
    Ok(base.join("mercs2-modkit"))
}

/// Create (if needed) and return a named lifecycle stage directory.
fn stage(name: &str) -> Result<PathBuf, String> {
    let dir = app_data_dir()?.join(name);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create {name} dir: {e}"))?;
    Ok(dir)
}

/// Raw release artifacts land here before unpacking.
pub fn downloading_dir() -> Result<PathBuf, String> {
    stage("downloading")
}

/// Unpacked, loadable mod sources live here.
pub fn staging_dir() -> Result<PathBuf, String> {
    stage("staging")
}

/// Built/deployed outputs are tracked here.
#[allow(dead_code)]
pub fn deployed_dir() -> Result<PathBuf, String> {
    stage("deployed")
}
