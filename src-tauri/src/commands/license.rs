//! Detect a legitimately licensed copy of the game, so Setup can offer the
//! non-destructive dxwrapper path instead of cracking the exe.
//!
//! The signal is a **SecuROM activation record** in the registry. When a v7 title
//! is activated legitimately, SecuROM writes its per-machine DRM/activation state
//! as `securom_v7_*` values under `…\SecuROM\UserData` (`.dat` is the live blob,
//! `.tmp`/`.bak` are transient/backup), and some SKUs also write a `License
//! information - Do not delete!` key. Either footprint means the stock exe will
//! pass SecuROM on its own — so the modkit leaves the exe untouched and loads
//! mods via dxwrapper + the logging-only `pmc_bb.dll` (no DRM spoof).
//!
//! **Verified against a real activated install:** the footprint was
//! `HKCU\SOFTWARE\SecuROM\UserData` holding `securom_v7_01.dat/.tmp/.bak` — and
//! *no* `License information` key. So `UserData` carrying `securom_v7_*` data is
//! the load-bearing signal here; keying only on `License information` (the first
//! cut) reported an activated copy as unlicensed. The base key can sit under HKLM
//! (`WOW6432Node` for the 32-bit game) or HKCU; HKCU is not WOW-redirected, so the
//! 64-bit modkit reads the same path the 32-bit game wrote.
//!
//! Reads use `reg query` (no elevation), mirroring `region.rs`. This never
//! *writes* anything and never asserts a copy is unlicensed — absence only means
//! "we couldn't confirm activation here", which just leaves the crack path available.

use serde::Serialize;

#[cfg(target_os = "windows")]
use super::proc::NoWindow;

/// The SecuROM subkey holding a title's per-machine activation blobs. Value
/// names look like `securom_v7_01.dat`; we treat any `securom_v7` value here as
/// activation data.
#[cfg(target_os = "windows")]
const SECUROM_USERDATA: &str = "UserData";
/// Prefix of the activation value names written under `UserData`. SecuROM v7
/// writes `securom_v7_<NN>.dat` (the license data), `.bak` (backup) and `.tmp`
/// (transient) — confirmed both by the forensic decomp analysis
/// (docs/securom_forensic_analysis.md, "User Data Storage") and the live registry
/// on an activated install. Matching the PREFIX, not the full `.dat` name, so it
/// catches every suffix (and the per-title index `_01`, `_02`, …).
#[cfg(target_os = "windows")]
const SECUROM_VALUE_PREFIX: &str = "securom_v7";
/// The activation key some SecuROM SKUs write instead of / alongside `UserData`.
/// The bang and spaces are part of the actual key name (from decomp `FUN_01c861d9`).
#[cfg(target_os = "windows")]
const SECUROM_LICENSE_KEY: &str = "License information - Do not delete!";

/// The EA "Mercenaries 2" install key. Its `Registration` value points at where
/// the CD-Key (ergc) lives — the Mercs2-specific ownership signal. 32-bit
/// installer → WOW6432Node on x64; the native view is tried too.
#[cfg(target_os = "windows")]
const INSTALL_KEYS: &[&str] = &[
    r"HKLM\SOFTWARE\WOW6432Node\EA Games\Mercenaries 2 World in Flames",
    r"HKLM\SOFTWARE\EA Games\Mercenaries 2 World in Flames",
];

/// Where the game assembles the CD-Key path itself when the install key carries
/// no `Registration` pointer (decomp `FUN_0074b760` / SettingsSerializer::Init:
/// `SOFTWARE\Electronic Arts\EA Games\Mercenaries 2 World in Flames\ergc`, HKLM,
/// default value). Used as the fallback target.
#[cfg(target_os = "windows")]
const ERGC_FALLBACK: &str = r"Software\Electronic Arts\EA Games\Mercenaries 2 World in Flames\ergc";

/// The engine's own "no key" sentinel (FUN_0074b760) — a present-but-unregistered
/// value we must NOT count as a CD-Key.
#[cfg(target_os = "windows")]
const NO_CD_KEY: &str = "No CD Key Found";

/// Registry roots + `SecuROM` base paths to probe. A 32-bit installer/game
/// writes under `WOW6432Node` on 64-bit Windows, but SecuROM has historically
/// used both HKLM and HKCU and both the redirected and native views, so we check
/// all of them and treat any hit as confirmation.
#[cfg(target_os = "windows")]
const SECUROM_BASES: &[&str] = &[
    r"HKLM\SOFTWARE\WOW6432Node\SecuROM",
    r"HKLM\SOFTWARE\SecuROM",
    r"HKCU\SOFTWARE\SecuROM",
];

/// What license detection found, for the UI to branch Setup on.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenseStatus {
    /// Whether this check is meaningful on the current host. SecuROM activation
    /// is a Windows registry concept; on Linux it would live inside the
    /// Proton/Wine prefix, which the modkit doesn't manage here.
    pub applicable: bool,
    /// This copy is legally owned — treat it as licensed and prefer the
    /// non-destructive (dxwrapper) setup path. True when the EA CD-Key (ergc) is
    /// registered OR a SecuROM activation record is present.
    pub licensed: bool,
    /// The EA registration CD-Key (`ergc`, pointed at by the install key's
    /// `Registration` value) is present and not the "No CD Key Found" sentinel.
    /// Mercs2-specific — the strongest "legally purchased" signal.
    pub cd_key_present: bool,
    /// The `License information - Do not delete!` activation key was present.
    pub license_key_present: bool,
    /// `UserData` held `securom_v7_*` activation data (DRM activated).
    pub user_data_present: bool,
    /// The exact registry keys that matched, for transparency in the UI.
    pub keys_found: Vec<String>,
    /// Human-readable summary of what was found (or why the check doesn't apply).
    pub detail: String,
}

/// Detect whether this machine holds a SecuROM activation for the game.
#[tauri::command(async)]
pub fn detect_license() -> LicenseStatus {
    #[cfg(target_os = "windows")]
    {
        detect_license_windows()
    }
    #[cfg(not(target_os = "windows"))]
    {
        LicenseStatus {
            applicable: false,
            licensed: false,
            cd_key_present: false,
            license_key_present: false,
            user_data_present: false,
            keys_found: Vec::new(),
            detail: "Not applicable: a SecuROM activation is a Windows registry record. On \
                     Linux it lives inside the Proton/Wine prefix, which the modkit doesn't \
                     inspect here — use the crack path or point the prefix's registry checker \
                     at it manually."
                .into(),
        }
    }
}

#[cfg(target_os = "windows")]
fn detect_license_windows() -> LicenseStatus {
    let mut keys_found = Vec::new();
    let mut cd_key_present = false;
    let mut license_key_present = false;
    let mut user_data_present = false;

    // 1) Follow the install key's `Registration` pointer to the ergc CD-Key. This
    //    is the Mercs2-specific ownership signal (unlike the shared SecuROM keys).
    let pointer = INSTALL_KEYS
        .iter()
        .find_map(|k| reg_query_value(k, "Registration"))
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| ERGC_FALLBACK.to_string());
    // The pointer is relative to a hive root (HKLM). Resolve it in the native view
    // and, for a 32-bit-written key, the WOW6432Node view.
    let rel = pointer.trim_start_matches('\\');
    let mut ergc_candidates = vec![format!(r"HKLM\{rel}")];
    if let Some(sub) = rel
        .strip_prefix("Software\\")
        .or_else(|| rel.strip_prefix("SOFTWARE\\"))
    {
        ergc_candidates.push(format!(r"HKLM\SOFTWARE\WOW6432Node\{sub}"));
    }
    for cand in &ergc_candidates {
        match reg_default_value(cand) {
            Some(v) if !v.trim().is_empty() && v != NO_CD_KEY => {
                cd_key_present = true;
                keys_found.push(cand.clone());
                break;
            }
            _ => {}
        }
    }

    // 2) SecuROM activation state (corroborates that the stock exe will run).
    for base in SECUROM_BASES {
        // Some SKUs write a named activation key…
        let license_key = format!(r"{base}\{SECUROM_LICENSE_KEY}");
        if reg_key_exists(&license_key) {
            license_key_present = true;
            keys_found.push(license_key);
        }
        // …but the load-bearing signal is `securom_v7_*` activation data in UserData.
        let userdata = format!(r"{base}\{SECUROM_USERDATA}");
        if reg_key_has_value_with_prefix(&userdata, SECUROM_VALUE_PREFIX) {
            user_data_present = true;
            keys_found.push(userdata);
        }
    }

    let securom_activated = license_key_present || user_data_present;
    // Legally owned = a registered CD-Key OR a SecuROM activation. Either means we
    // should not crack: the CD-Key is proof of purchase; the activation means the
    // stock exe already passes DRM.
    let licensed = cd_key_present || securom_activated;

    let detail = match (cd_key_present, securom_activated) {
        (true, true) => "Registered (EA CD-Key present) and SecuROM-activated — licensed. Setup \
             uses the dxwrapper path and leaves your exe untouched."
            .to_string(),
        (true, false) => "Registered (EA CD-Key present) — treated as licensed. The stock exe \
             may still need SecuROM activation to run, but Setup won't crack it; use the \
             dxwrapper path."
            .to_string(),
        (false, true) => "SecuROM activation present — licensed. Setup uses the dxwrapper path \
             and leaves your exe untouched."
            .to_string(),
        (false, false) => "No EA CD-Key and no SecuROM activation found — this looks like a \
             loose/unactivated copy, so the crack path applies. (A DRM-free build never needs \
             either; you can still choose the dxwrapper path manually.)"
            .to_string(),
    };

    LicenseStatus {
        applicable: true,
        licensed,
        cd_key_present,
        license_key_present,
        user_data_present,
        keys_found,
        detail,
    }
}

/// True if a registry key exists. `reg query <key>` exits 0 when the key is
/// present (even with no values) and non-zero when it's absent.
#[cfg(target_os = "windows")]
fn reg_key_exists(key: &str) -> bool {
    std::process::Command::new("reg")
        .args(["query", key])
        .no_window()
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Read a REG_SZ value's data. `selector` is the `reg query` value selector:
/// `["/v", name]` for a named value, `["/ve"]` for the key's `(Default)` value.
/// Returns the text after the type token (`REG_SZ`), or `None` if absent.
#[cfg(target_os = "windows")]
fn reg_read_value(key: &str, selector: &[&str]) -> Option<String> {
    let out = std::process::Command::new("reg")
        .arg("query")
        .arg(key)
        .args(selector)
        .no_window()
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Value line: "    <name>    REG_SZ    <data>". Take everything after the type.
    for line in text.lines() {
        if let Some(idx) = line.find("REG_SZ") {
            return Some(line[idx + "REG_SZ".len()..].trim().to_string());
        }
    }
    None
}

/// One named REG_SZ value (e.g. the install key's `Registration` pointer).
#[cfg(target_os = "windows")]
fn reg_query_value(key: &str, name: &str) -> Option<String> {
    reg_read_value(key, &["/v", name])
}

/// A key's `(Default)` REG_SZ value (e.g. the ergc CD-Key).
#[cfg(target_os = "windows")]
fn reg_default_value(key: &str) -> Option<String> {
    reg_read_value(key, &["/ve"])
}

/// True if `reg query <key>` succeeds AND lists at least one value whose name
/// starts with `prefix` (case-insensitive). `reg query` prints one line per value
/// as `    <name>    <TYPE>    <data>`, so a substring match on the name is enough
/// to tell `securom_v7_01.dat` apart from an empty/unrelated key.
#[cfg(target_os = "windows")]
fn reg_key_has_value_with_prefix(key: &str, prefix: &str) -> bool {
    let out = match std::process::Command::new("reg").args(["query", key]).no_window().output() {
        Ok(o) if o.status.success() => o,
        _ => return false,
    };
    let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
    let prefix = prefix.to_lowercase();
    text.lines().any(|line| {
        // Value lines are indented; the first whitespace-delimited token is the name.
        line.starts_with(char::is_whitespace)
            && line.split_whitespace().next().is_some_and(|name| name.starts_with(&prefix))
    })
}
