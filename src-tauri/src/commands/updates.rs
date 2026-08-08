//! Latest-release lookup, used both to self-update modkit and to check mods for
//! newer versions. GitHub only (the catalog repos and modkit live there).

use serde::Serialize;

/// Summary of a repository's latest GitHub release.
#[derive(Debug, Serialize)]
pub struct ReleaseInfo {
    /// Release tag, e.g. `v0.2.0`.
    pub tag: String,
    /// Release title (falls back to the tag).
    pub name: String,
    /// Browser URL of the release page.
    pub url: String,
    /// Release notes (may be empty).
    pub body: String,
    /// Whether the release has finished publishing its downloads.
    ///
    /// False while CI is still uploading — a release exists from the moment it is
    /// created, so `tag_name` goes new before anything is downloadable from it.
    /// Callers must not announce an update on a release that answers `false`
    /// here; that is the window in which people are told about a build, follow
    /// the link, and find an empty release page.
    pub assets_ready: bool,
}

/// Whether this binary was installed in a form the Tauri updater can replace
/// in-place. Windows NSIS installs and Linux AppImages qualify; the portable
/// Windows exe and deb/rpm/flatpak installs update out-of-band, so the UI
/// should link to the release page instead of offering an in-app install.
#[tauri::command]
pub fn updater_supported() -> bool {
    #[cfg(target_os = "windows")]
    {
        // The NSIS bundle writes uninstall.exe next to the app binary; the
        // portable zip is a loose exe the updater cannot replace.
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("uninstall.exe").exists()))
            .unwrap_or(false)
    }
    #[cfg(target_os = "linux")]
    {
        // AppImage runtimes export APPIMAGE; deb/rpm/flatpak go through the
        // system package manager.
        std::env::var_os("APPIMAGE").is_some()
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        true
    }
}

/// Fetch the latest release of a GitHub repository.
///
/// A thin wrapper over [`crate::commands::net::release`] now. It kept its own copy
/// of the lookup, and the copies had drifted: this one read a missing tag as the
/// empty string and then checked for it, `setup.rs` defaulted to `"latest"`, and
/// `toolchain.rs` errored. Same request, three answers.
#[tauri::command]
pub async fn latest_release(repo: String) -> Result<ReleaseInfo, String> {
    use crate::commands::net;

    let owner_repo = net::release::github_owner_repo(&repo)
        .ok_or_else(|| format!("Not a GitHub repository: {repo}"))?;

    let client = net::client()?;
    let release = net::latest_release(&client, net::ReleaseHost::GitHub, &owner_repo).await?;

    Ok(ReleaseInfo {
        assets_ready: !release.is_still_publishing(),
        tag: release.tag,
        name: release.name,
        url: release.url,
        body: release.body,
    })
}
