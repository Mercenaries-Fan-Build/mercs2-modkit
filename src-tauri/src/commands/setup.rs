//! Game setup: install the pmc_bb.dll ASI loader and crack/update the exe.
//!
//! Both pull prebuilt artifacts from the project's GitHub releases so the user
//! never needs a compiler or Python.

use std::path::PathBuf;
use std::process::Command;

use serde::Serialize;

use crate::commands::paths::app_data_dir;
use crate::commands::proc::NoWindow;

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
///
/// ARM is spelled **`arm64`**, not `aarch64`. That is the convention every repo
/// in the ecosystem publishes under — see
/// [`super::toolchain::platform_suffix`], which resolves `-macos-arm64`,
/// `-linux-arm64` and `-windows-arm64.exe`, and the assets the toolset release
/// actually carries. Spelling it `aarch64` here meant the `exact` predicate in
/// [`crack_game`] could never match on an ARM host, quietly demoting every ARM
/// user to the OS-only fallback and whichever asset GitHub happened to list
/// first — an x86_64 binary that will not even exec on ARM Linux.
/// [`arch_tokens_match_the_toolset_suffix`] pins the two modules together.
///
/// `None` for an arch the releases do not build (riscv, 32-bit ARM, …). That is
/// distinct from a guess: the previous `""` sentinel made `n.contains(arch)`
/// vacuously true, so `exact` silently collapsed into the fallback instead of
/// reporting that the host is unsupported.
fn arch_token() -> Option<&'static str> {
    Some(if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "x86") {
        "i686"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        return None;
    })
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

/// Download the logging-only `pmc_bb_log.dll` (the pmc-blackbox build with the
/// SecuROM event compiled out — it keeps the ASI loader) and install it into the
/// game root **as `pmc_bb.dll`** — the name dxwrapper's `LoadCustomDllPath` and
/// every ASI plugin (`GetModuleHandle("pmc_bb.dll")`) expect. On the licensed
/// path dxwrapper side-loads this DLL with its own `LoadPlugins = 0`, so pmc_bb
/// is the single loader that scans `scripts/*.asi` — same loader as the crack
/// path, minus the DRM spoof. Any existing `pmc_bb.dll` is backed up first.
#[tauri::command]
pub async fn install_pmc_bb_log(game_root: String) -> Result<InstallDllResult, String> {
    let root = PathBuf::from(&game_root);
    if !root.is_dir() {
        return Err(format!("Game folder not found: {game_root}"));
    }

    let client = client()?;
    // The logging-only build is published under its own asset name; fall back to
    // a case-insensitive match so a differently-cased release still resolves.
    let pick_log = |n: &str| n.eq_ignore_ascii_case("pmc_bb_log.dll");
    let picks: [&(dyn Fn(&str) -> bool + Sync); 1] = [&pick_log];
    let (tag, _name, bytes) = download_latest_asset(&client, PMC_BB_REPO, &picks)
        .await
        .map_err(|e| format!("{e}. The pmc-blackbox release must publish pmc_bb_log.dll (make log)."))?;

    // Install under the canonical on-disk name so plugins resolve pmc_log().
    let dest = root.join("pmc_bb.dll");
    if dest.exists() {
        let backup = dest.with_extension("dll.bak");
        let _ = std::fs::rename(&dest, &backup);
    }
    std::fs::write(&dest, &bytes)
        .map_err(|e| format!("Failed to write pmc_bb.dll (logging build): {e}"))?;

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
    /// Release tag of the `apply_crack` build that was downloaded and run, so the
    /// UI can persist it and later flag a newer release.
    pub tool_version: String,
}

/// Download the `apply_crack` build matching this host and cache it as an
/// executable in the app-data bin dir. Returns `(release_tag, binary_path)`.
/// Shared by [`crack_game`] and [`update_game`].
async fn cache_apply_crack(client: &reqwest::Client) -> Result<(String, PathBuf), String> {
    let os = platform_token();
    let arch = arch_token();
    // Prefer the build matching our exact OS+arch (the release ships all four
    // arches per OS); fall back to any build for this OS so a single-arch or
    // older release still resolves. apply_crack only byte-patches the exe, so the
    // patched output is identical across arches — this is host-compat only.
    //
    // The fallback is a last resort, not a convenience: on a host whose arch we
    // do not recognise it will hand back a binary for some *other* arch, which at
    // best runs under emulation and at worst refuses to exec. It stays because
    // failing to find anything is worse, and because `exact` now covers every
    // arch the release actually builds.
    let exact = |n: &str| {
        arch.is_some_and(|a| n.starts_with("apply_crack") && n.contains(os) && n.contains(a))
    };
    let os_only = |n: &str| n.starts_with("apply_crack") && n.contains(os);
    let picks: [&(dyn Fn(&str) -> bool + Sync); 2] = [&exact, &os_only];
    let (tag, name, bytes) = download_latest_asset(client, CRACK_REPO, &picks)
        .await
        .map_err(|e| {
            format!(
                "{e}. No apply_crack build for {os}/{}.",
                arch.unwrap_or(std::env::consts::ARCH)
            )
        })?;

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
    Ok((tag, bin))
}

/// Download `apply_crack` and run it on the exe to apply the SecuROM bypass,
/// writing a new cracked exe. apply_crack auto-detects the version and always
/// brings a v1.0 input up to v1.1 before cracking (a v1.1 input skips that
/// itself) — there is no skip-update, so this always yields cracked v1.1.
#[tauri::command]
pub async fn crack_game(
    exe_path: String,
    output_path: Option<String>,
) -> Result<CrackResult, String> {
    let exe = PathBuf::from(&exe_path);
    if !exe.is_file() {
        return Err(format!("Game exe not found: {exe_path}"));
    }

    let client = client()?;
    let (tag, bin) = cache_apply_crack(&client).await?;

    let out = output_path
        .unwrap_or_else(|| exe.with_file_name("Mercenaries2.cracked.exe").to_string_lossy().to_string());

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
/// installs it in place as the game's exe, backing the original up to `BACKUP/`
/// first. For LICENSED copies — SecuROM stays intact, the activation carries
/// over, and mods load via dxwrapper + pmc_bb. The exe is replaced (this is an
/// update), but the original is always recoverable from `BACKUP/`.
#[tauri::command]
pub async fn update_game(exe_path: String) -> Result<CrackResult, String> {
    let exe = PathBuf::from(&exe_path);
    if !exe.is_file() {
        return Err(format!("Game exe not found: {exe_path}"));
    }

    let client = client()?;
    let (tag, bin) = cache_apply_crack(&client).await?;

    // Stage the updated exe next to the original, then swap it in only on success.
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

    // Back up the original once (never clobber an existing backup — it holds the
    // true pre-update exe), mirroring the official patch's BACKUP/ convention.
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
    // Install the update in place (copy over, then drop the staged file). `copy`
    // overwrites the destination on every platform, unlike `rename` on Windows.
    std::fs::copy(&staged, &exe)
        .map_err(|e| format!("Failed to install updated exe: {e}"))?;
    let _ = std::fs::remove_file(&staged);

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

    /// The two download paths must spell an arch the same way.
    ///
    /// [`arch_token`] picks `apply_crack` assets from the securom-bypass release;
    /// [`super::super::toolchain::platform_suffix`] picks tool assets from the
    /// toolset release. Both repos publish under one naming scheme, but nothing
    /// tied the two constants together — so `aarch64` here drifted against
    /// `-…-arm64` there, and the mismatch was invisible until an ARM host tried
    /// to install. Asserting the token is a substring of the suffix catches that
    /// on any runner, whatever arch CI happens to use.
    #[test]
    fn arch_tokens_match_the_toolset_suffix() {
        let (Some(token), Some(suffix)) = (arch_token(), super::super::toolchain::platform_suffix())
        else {
            // An arch neither module publishes for. Both said so — consistent.
            return;
        };
        assert!(
            suffix.contains(token),
            "arch_token() is {token:?} but the toolset publishes {suffix:?} — an \
             `apply_crack-…-{token}` asset will never match on this host",
        );
    }

    /// The OS token has to appear in the suffix for the same reason.
    #[test]
    fn os_token_matches_the_toolset_suffix() {
        let Some(suffix) = super::super::toolchain::platform_suffix() else {
            return;
        };
        assert!(
            suffix.contains(platform_token()),
            "platform_token() is {:?} but the toolset publishes {suffix:?}",
            platform_token(),
        );
    }

    /// ARM is `arm64` in asset names, never `aarch64` — the spelling this whole
    /// pairing exists to keep straight. Pinned independently of the host so a
    /// regression fails on an x86_64 runner too, where the test above is silent
    /// about ARM.
    #[test]
    fn arm_is_spelled_arm64() {
        if cfg!(target_arch = "aarch64") {
            assert_eq!(arch_token(), Some("arm64"));
        }
        for suffix in ["-macos-arm64", "-linux-arm64", "-windows-arm64.exe"] {
            assert!(!suffix.contains("aarch64"), "{suffix} regressed to aarch64");
        }
    }
}
