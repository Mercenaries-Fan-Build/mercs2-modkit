//! Matchmaking region — write the EA Games install registry key with a chosen
//! `Region` so a group of modkit users computes the same multiplayer version
//! string (`mercs2-pc_ver_<N>`) and can see each other in matchmaking.
//!
//! Mercenaries 2's online version is keyed off the installer-written `Region`
//! value under
//! `HKLM\SOFTWARE\WOW6432Node\EA Games\Mercenaries 2 World in Flames`. A loose
//! copy with no key falls back to one version; a key written by a regional
//! installer yields another — which is exactly what segregates lobbies. Every
//! player who wants to matchmake together must share ONE `Region` value (see
//! `docs/mercs2_install_registry_contract.md` §1–2). The modkit defaults to the
//! community pool value but lets the user pick any region the game recognizes,
//! e.g. to join a group that standardized on an EU value.
//!
//! Reads use `reg query` (no elevation). Writing under HKLM needs admin, so we
//! generate a `.reg` file and `reg import` it elevated via a UAC prompt — the
//! same `Start-Process -Verb RunAs` pattern used for the VC++ runtime install.

use serde::Serialize;

#[cfg(target_os = "windows")]
use super::proc::NoWindow;

/// The default `Region` for the community pool — what you get without picking
/// one. Matchmaking only works between installs that share a value, so the UI
/// must make clear that choosing a different region moves you to that region's
/// pool.
pub const POOL_REGION: &str = "mercenaries2_na";

/// Registry path of the EA Games install key (32-bit app → WOW6432Node on x64).
#[cfg(target_os = "windows")]
const KEY_PATH: &str = r"HKLM\SOFTWARE\WOW6432Node\EA Games\Mercenaries 2 World in Flames";

/// The constant `Product GUID` the game/patcher expect (see the contract §2).
#[cfg(target_os = "windows")]
const PRODUCT_GUID: &str = "{26FDF89A-FA65-4FA2-8522-37CC84DFDCEE}";

/// `Region` values the game's decomp recognizes — the only ones safe to write.
const KNOWN_REGIONS: &[&str] = &[
    "mercenaries2_na",
    "mercenaries2_enru",
    "mercenaries2_esit",
];

/// Snapshot of the matchmaking-relevant registry state.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionStatus {
    /// Whether this check is meaningful on the current host. The key is a
    /// Windows component; on Linux it lives inside the Proton/Wine prefix, which
    /// we don't manage here, so we report `applicable: false`.
    pub applicable: bool,
    /// The install key exists in the registry.
    pub key_present: bool,
    /// Current `Region` value, if the key/value is present.
    pub current_region: Option<String>,
    /// Current `Install Dir` value, if present.
    pub current_install_dir: Option<String>,
    /// The `Region` the user selected (the pool default if they never picked
    /// one) — what applying writes.
    pub expected_region: String,
    /// All `Region` values the game recognizes, for the UI's picker.
    pub known_regions: Vec<String>,
    /// The `Install Dir` value normalizing would write (the real game folder,
    /// with a trailing separator).
    pub install_dir: String,
    /// `Region` already matches the selected value — matchmaking is aligned.
    pub normalized: bool,
    /// Human-readable detail (what was found, or why the check doesn't apply).
    pub detail: String,
}

/// Outcome of a normalize attempt.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizeRegionResult {
    /// The key carries the pool `Region` after this call.
    pub ok: bool,
    /// The `Region` value written.
    pub region: String,
    pub message: String,
}

/// Game folder normalized to an `Install Dir` value: native separators with a
/// single trailing one (the installer always wrote a trailing backslash).
fn install_dir_value(game_root: &str) -> String {
    let trimmed = game_root.trim_end_matches(['/', '\\']);
    #[cfg(target_os = "windows")]
    let sep = '\\';
    #[cfg(not(target_os = "windows"))]
    let sep = '/';
    format!("{trimmed}{sep}")
}

/// The region status should be judged against: the user's pick if it's one the
/// game recognizes, else the pool default. Unknown picks (e.g. a stale stored
/// preference) fall back silently — `normalize_region` is the strict gate.
fn resolve_region(preferred: Option<&str>) -> &str {
    match preferred {
        Some(r) if KNOWN_REGIONS.contains(&r) => r,
        _ => POOL_REGION,
    }
}

fn known_regions_vec() -> Vec<String> {
    KNOWN_REGIONS.iter().map(|r| r.to_string()).collect()
}

/// Report the matchmaking-relevant registry state for `game_root`, judged
/// against `preferred_region` (or the pool default when unset/unknown).
#[tauri::command(async)]
pub fn read_region(game_root: String, preferred_region: Option<String>) -> RegionStatus {
    let install_dir = install_dir_value(&game_root);
    let expected = resolve_region(preferred_region.as_deref());
    #[cfg(target_os = "windows")]
    {
        read_region_windows(install_dir, expected)
    }
    #[cfg(not(target_os = "windows"))]
    {
        RegionStatus {
            applicable: false,
            key_present: false,
            current_region: None,
            current_install_dir: None,
            expected_region: expected.to_string(),
            known_regions: known_regions_vec(),
            install_dir,
            normalized: false,
            detail: "Not applicable: the EA Games install key is a Windows registry value. \
                     On Linux it lives inside the Proton/Wine prefix, which the modkit \
                     doesn't manage here."
                .into(),
        }
    }
}

/// Write the install key with the given `Region` (the pool default when unset)
/// and the real `Install Dir`. Elevated; raises a UAC prompt.
#[tauri::command]
pub async fn normalize_region(
    game_root: String,
    region: Option<String>,
) -> Result<NormalizeRegionResult, String> {
    let region = region.unwrap_or_else(|| POOL_REGION.to_string());
    if !KNOWN_REGIONS.contains(&region.as_str()) {
        return Err(format!(
            "Refusing to write an unrecognized Region '{region}'. Use one of: {}",
            KNOWN_REGIONS.join(", ")
        ));
    }

    #[cfg(target_os = "windows")]
    {
        normalize_region_windows(&game_root, &region)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = &game_root;
        Err("Normalizing the Region is only supported on Windows. On Linux the \
             EA Games key lives inside the Proton/Wine prefix."
            .into())
    }
}

// ----------------------------------------------------------------------------
// Windows implementation
// ----------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn read_region_windows(install_dir: String, expected: &str) -> RegionStatus {
    let current_region = reg_query_value(KEY_PATH, "Region");
    let current_install_dir = reg_query_value(KEY_PATH, "Install Dir");
    let key_present = current_region.is_some() || current_install_dir.is_some();
    let normalized = current_region.as_deref() == Some(expected);

    let detail = if !key_present {
        "No EA Games install key found — this loose copy falls back to the default \
         version and is segregated from installs that have a Region set. Apply a \
         region to fix."
            .to_string()
    } else if normalized {
        format!(
            "Region is '{expected}' — matchmaking is aligned with everyone else \
             using this region."
        )
    } else {
        match &current_region {
            Some(r) => format!(
                "Region is '{r}', not your selected '{expected}'. Different regions \
                 compute different multiplayer versions and can't see each other. \
                 Apply to fix."
            ),
            None => format!(
                "The key exists but has no Region value — it falls back to the default \
                 version. Apply to write '{expected}'."
            ),
        }
    };

    RegionStatus {
        applicable: true,
        key_present,
        current_region,
        current_install_dir,
        expected_region: expected.to_string(),
        known_regions: known_regions_vec(),
        install_dir,
        normalized,
        detail,
    }
}

#[cfg(target_os = "windows")]
fn normalize_region_windows(game_root: &str, region: &str) -> Result<NormalizeRegionResult, String> {
    let install_dir = install_dir_value(game_root);

    // Generate the .reg file (substituting the real Install Dir) and import it
    // elevated — writing under HKLM\…\WOW6432Node requires admin.
    let reg_text = build_reg_file(region, &install_dir);
    let dir = crate::commands::paths::app_data_dir()?.join("bin");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let reg_path = dir.join("mercs2_region.reg");
    std::fs::write(&reg_path, reg_text)
        .map_err(|e| format!("Failed to write the .reg file: {e}"))?;

    import_reg_elevated(&reg_path)?;

    // Confirm the value landed.
    match reg_query_value(KEY_PATH, "Region").as_deref() {
        Some(r) if r == region => Ok(NormalizeRegionResult {
            ok: true,
            region: region.to_string(),
            message: format!(
                "Region set to '{region}'. You'll matchmake with everyone whose \
                 install shares this region."
            ),
        }),
        Some(other) => Err(format!(
            "The import ran but Region reads '{other}', not '{region}' — the write may \
             have been cancelled at the UAC prompt."
        )),
        None => Err("The import ran but no Region value is present — the write may have \
                     been cancelled at the UAC prompt."
            .into()),
    }
}

/// Build the `.reg` payload. String values escape backslashes (each `\` → `\\`),
/// per .reg syntax. Mirrors `docs/mercs2_modkit_register.reg.template`.
#[cfg(target_os = "windows")]
fn build_reg_file(region: &str, install_dir: &str) -> String {
    let esc = |s: &str| s.replace('\\', "\\\\");
    let install = esc(install_dir);
    let registration = esc(r"Software\Electronic Arts\EA Games\Mercenaries 2 World in Flames\ergc");
    let program_group = esc(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\EA Games\");

    // The key path is doubled here too because it sits inside a [bracketed] line.
    format!(
        "Windows Registry Editor Version 5.00\r\n\
         \r\n\
         ; Written by mercs2-modkit \"Normalize Region\". One fixed Region per pool so\r\n\
         ; every install computes the same mercs2-pc_ver_<N> and can matchmake together.\r\n\
         \r\n\
         [HKEY_LOCAL_MACHINE\\SOFTWARE\\WOW6432Node\\EA Games\\Mercenaries 2 World in Flames]\r\n\
         \"Region\"=\"{region}\"\r\n\
         \"Install Dir\"=\"{install}\"\r\n\
         \"Locale\"=\"en_US\"\r\n\
         \"Language\"=\"English (US)\"\r\n\
         \"DisplayName\"=\"Mercenaries 2: World in Flames(tm)\"\r\n\
         \"ProductName\"=\"Mercenaries 2: World in Flames(tm)\"\r\n\
         \"Product GUID\"=\"{PRODUCT_GUID}\"\r\n\
         \"Registration\"=\"{registration}\"\r\n\
         \"Patch URL\"=\"http://www.mercs2.com/patch\"\r\n\
         \"Suppression Exe\"=\"\"\r\n\
         \r\n\
         [HKEY_LOCAL_MACHINE\\SOFTWARE\\WOW6432Node\\EA Games\\Mercenaries 2 World in Flames\\1.0]\r\n\
         \"DisplayName\"=\"Mercenaries 2 World in Flames\"\r\n\
         \"ProgramGroup\"=\"{program_group}\"\r\n"
    )
}

/// Read one REG_SZ value with `reg query`. Returns `None` if the key/value is
/// absent (reg exits non-zero) or the output can't be parsed.
#[cfg(target_os = "windows")]
fn reg_query_value(key: &str, name: &str) -> Option<String> {
    let out = std::process::Command::new("reg")
        .args(["query", key, "/v", name])
        .no_window()
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // The value line looks like: "    Region    REG_SZ    mercenaries2_na".
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(name) {
            if let Some(idx) = line.find("REG_SZ") {
                let value = line[idx + "REG_SZ".len()..].trim();
                return Some(value.to_string());
            }
        }
    }
    None
}

/// `reg import <file>` elevated, via a UAC prompt, waiting for completion.
#[cfg(target_os = "windows")]
fn import_reg_elevated(path: &std::path::Path) -> Result<(), String> {
    const SCRIPT: &str = "$ErrorActionPreference='Stop'; \
         $p = Start-Process -FilePath 'reg.exe' \
              -ArgumentList @('import', $env:MERCS2_REG_FILE) \
              -Verb RunAs -PassThru -Wait; \
         exit $p.ExitCode";

    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            SCRIPT,
        ])
        .env("MERCS2_REG_FILE", path)
        .no_window()
        .output()
        .map_err(|e| format!("Failed to run PowerShell: {e}"))?;

    match out.status.code() {
        Some(0) => Ok(()),
        Some(code) => Err(format!(
            "Writing the registry key failed (exit {code}). {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        None => Err("The registry write was terminated before finishing (UAC declined?).".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_dir_gets_one_trailing_separator() {
        #[cfg(target_os = "windows")]
        {
            assert_eq!(install_dir_value(r"C:\Games\Mercs2"), r"C:\Games\Mercs2\");
            assert_eq!(install_dir_value(r"C:\Games\Mercs2\"), r"C:\Games\Mercs2\");
            assert_eq!(install_dir_value(r"C:\Games\Mercs2\\"), r"C:\Games\Mercs2\");
        }
        #[cfg(not(target_os = "windows"))]
        {
            assert_eq!(install_dir_value("/games/mercs2"), "/games/mercs2/");
            assert_eq!(install_dir_value("/games/mercs2/"), "/games/mercs2/");
        }
    }

    #[test]
    fn pool_region_is_a_known_region() {
        assert!(KNOWN_REGIONS.contains(&POOL_REGION));
    }

    #[test]
    fn resolve_region_honors_known_picks_and_falls_back() {
        assert_eq!(resolve_region(None), POOL_REGION);
        assert_eq!(resolve_region(Some("mercenaries2_esit")), "mercenaries2_esit");
        // A stale/garbage stored preference must not poison the status check.
        assert_eq!(resolve_region(Some("mercenaries2_xx")), POOL_REGION);
        assert_eq!(resolve_region(Some("")), POOL_REGION);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn reg_file_escapes_backslashes_and_carries_region() {
        let reg = build_reg_file("mercenaries2_na", r"C:\Games\Mercs2\");
        assert!(reg.contains(r#""Region"="mercenaries2_na""#));
        // The install dir's backslashes are doubled for .reg syntax.
        assert!(reg.contains(r#""Install Dir"="C:\\Games\\Mercs2\\""#));
        assert!(reg.starts_with("Windows Registry Editor Version 5.00"));
    }
}
