//! Microsoft Visual C++ 2008 (MSVC 9.0) runtime detection and install.
//!
//! Mercenaries 2 and its `binkw32.dll` (the Bink video codec) are 32-bit and
//! link against the VC++ 2008 CRT. When that runtime is absent the Windows
//! loader can't resolve binkw32's *own* dependency and reports the misleading
//! "The code execution cannot proceed because binkw32.dll was not found.
//! Reinstalling the program may fix this problem" — even though binkw32.dll is
//! present. The real fix is installing the 32-bit VC++ 2008 redistributable.
//!
//! We never ship the runtime DLLs ourselves: we download the genuine,
//! Microsoft-signed `vcredist_x86.exe`, verify its Authenticode signature is
//! Microsoft's before running anything, and let Windows' own installer (with a
//! UAC prompt that shows the Microsoft publisher) put the assembly in place.

use serde::Serialize;

#[cfg(target_os = "windows")]
use std::path::{Path, PathBuf};

/// Official Microsoft permalink for the Visual C++ 2008 SP1 (x86) redistributable.
/// SP1 ships publisher-policy redirects, so manifests that reference the original
/// 9.0.21022 CRT (as game-shipped binkw32 builds do) resolve against it.
#[cfg(target_os = "windows")]
const VCREDIST_2008_SP1_X86_URL: &str =
    "https://download.microsoft.com/download/5/D/8/5D8C65CB-C849-4025-8E95-C3966CAFD8AE/vcredist_x86.exe";

/// Result of probing the host for the 32-bit VC++ 2008 runtime.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VcRedistStatus {
    /// Whether this check is meaningful on the current host. The redistributable
    /// is a Windows component; on Linux the runtime lives inside the Proton/Wine
    /// prefix, not on the host, so we report `applicable: false` there.
    pub applicable: bool,
    /// Whether the 32-bit MSVC 9.0 CRT assembly is installed.
    pub installed: bool,
    /// Human-readable detail (where it was found, or why the check doesn't apply).
    pub detail: String,
}

/// Outcome of an install attempt.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallVcRedistResult {
    /// The runtime is present after this call.
    pub installed: bool,
    /// It was already installed, so nothing was downloaded or run.
    pub already_present: bool,
    pub message: String,
}

/// Report whether the VC++ 2008 (x86) runtime is installed.
#[tauri::command]
pub fn check_vcredist() -> VcRedistStatus {
    #[cfg(target_os = "windows")]
    {
        match vc90_crt_assembly_dir() {
            Some(dir) => VcRedistStatus {
                applicable: true,
                installed: true,
                detail: format!("Found the 32-bit VC++ 2008 CRT at {}", dir.display()),
            },
            None => VcRedistStatus {
                applicable: true,
                installed: false,
                detail: "The 32-bit Microsoft Visual C++ 2008 runtime (MSVC 9.0 CRT) \
                         isn't installed — the game can't load binkw32.dll without it."
                    .into(),
            },
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        VcRedistStatus {
            applicable: false,
            installed: false,
            detail: "Not applicable: the VC++ 2008 redistributable is a Windows component. \
                     On Linux the runtime is provided inside the Proton/Wine prefix."
                .into(),
        }
    }
}

/// Download the Microsoft-signed VC++ 2008 (x86) redistributable, verify it's
/// genuinely Microsoft-signed, and run it (elevated, via a UAC prompt).
#[tauri::command]
pub async fn install_vcredist() -> Result<InstallVcRedistResult, String> {
    #[cfg(target_os = "windows")]
    {
        install_windows().await
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("Installing the VC++ 2008 redistributable is only supported on Windows. \
             On Linux the runtime is provided by the Proton/Wine prefix (e.g. \
             `protontricks <appid> vcrun2008`)."
            .into())
    }
}

// ----------------------------------------------------------------------------
// Windows implementation
// ----------------------------------------------------------------------------

/// Locate the installed 32-bit VC90 CRT side-by-side assembly, if any.
///
/// The 2008 redistributable installs the CRT as a WinSxS assembly; the 32-bit
/// variant lives in a folder named like
/// `x86_microsoft.vc90.crt_1fc8b3b9a1e18e3b_9.0.30729.x_none_…` containing
/// `msvcr90.dll`. Scanning WinSxS is more reliable than MSI product-code
/// bookkeeping, which differs between the RTM and SP1 packages.
#[cfg(target_os = "windows")]
fn vc90_crt_assembly_dir() -> Option<PathBuf> {
    let windir = std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("windir"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
    let winsxs = windir.join("WinSxS");

    for entry in std::fs::read_dir(&winsxs).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name.starts_with("x86_microsoft.vc90.crt_") && entry.path().join("msvcr90.dll").is_file()
        {
            return Some(entry.path());
        }
    }
    None
}

#[cfg(target_os = "windows")]
async fn install_windows() -> Result<InstallVcRedistResult, String> {
    // Already there? Don't download or prompt for elevation needlessly.
    if vc90_crt_assembly_dir().is_some() {
        return Ok(InstallVcRedistResult {
            installed: true,
            already_present: true,
            message: "The Microsoft Visual C++ 2008 runtime is already installed.".into(),
        });
    }

    // Download from Microsoft over HTTPS.
    let client = reqwest::Client::builder()
        .user_agent("mercs2-modkit")
        .build()
        .map_err(|e| e.to_string())?;
    let bytes = client
        .get(VCREDIST_2008_SP1_X86_URL)
        .send()
        .await
        .map_err(|e| format!("Failed to download the VC++ 2008 redistributable: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Microsoft download returned an error: {e}"))?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;

    let dir = crate::commands::paths::app_data_dir()?.join("bin");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let exe = dir.join("vcredist_2008sp1_x86.exe");
    std::fs::write(&exe, &bytes)
        .map_err(|e| format!("Failed to save the downloaded installer: {e}"))?;

    // Refuse to run anything that isn't a valid, Microsoft-signed binary.
    verify_microsoft_signature(&exe)?;

    // Run elevated (UAC shows the Microsoft publisher) and wait for it.
    run_installer_elevated(&exe)?;

    if vc90_crt_assembly_dir().is_some() {
        Ok(InstallVcRedistResult {
            installed: true,
            already_present: false,
            message: "Installed the Microsoft Visual C++ 2008 SP1 runtime.".into(),
        })
    } else {
        Err("The installer ran but the VC++ 2008 runtime still isn't detected — \
             the install may have been cancelled, or a reboot is required."
            .into())
    }
}

/// Verify `path` carries a valid Authenticode signature issued to Microsoft,
/// via PowerShell's `Get-AuthenticodeSignature`. The path is passed through an
/// env var to sidestep all command-line quoting concerns.
#[cfg(target_os = "windows")]
fn verify_microsoft_signature(path: &Path) -> Result<(), String> {
    const SCRIPT: &str = "$ErrorActionPreference='Stop'; \
         $s = Get-AuthenticodeSignature -FilePath $env:VCREDIST_PATH; \
         if ($s.Status -ne 'Valid') { Write-Error \"signature status is $($s.Status)\"; exit 2 }; \
         if ($s.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') { \
            Write-Error \"unexpected signer $($s.SignerCertificate.Subject)\"; exit 3 }";

    let out = powershell(SCRIPT, path)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Refusing to run the download — could not confirm it is Microsoft-signed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Launch the installer elevated and silently, waiting for completion. The UAC
/// prompt that `-Verb RunAs` raises shows the verified Microsoft publisher.
#[cfg(target_os = "windows")]
fn run_installer_elevated(path: &Path) -> Result<(), String> {
    const SCRIPT: &str = "$ErrorActionPreference='Stop'; \
         $p = Start-Process -FilePath $env:VCREDIST_PATH -ArgumentList '/q' \
              -Verb RunAs -PassThru -Wait; \
         exit $p.ExitCode";

    let out = powershell(SCRIPT, path)?;
    match out.status.code() {
        // 0 = success, 3010 = success + reboot required, 1638 = newer already present.
        Some(0) | Some(3010) | Some(1638) => Ok(()),
        Some(code) => Err(format!(
            "The VC++ 2008 installer exited with code {code}. {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        None => Err("The VC++ 2008 installer was terminated before finishing.".into()),
    }
}

/// Run a PowerShell `-Command` with `$env:VCREDIST_PATH` bound to `path`.
#[cfg(target_os = "windows")]
fn powershell(script: &str, path: &Path) -> Result<std::process::Output, String> {
    std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .env("VCREDIST_PATH", path)
        .output()
        .map_err(|e| format!("Failed to run PowerShell: {e}"))
}
