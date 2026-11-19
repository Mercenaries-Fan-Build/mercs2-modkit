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
//! On Windows, reads use `reg query` (no elevation), mirroring `region.rs`. On
//! macOS and Linux the game runs in a Wine/Proton prefix, so the same keys are
//! read from that prefix's registry files (`system.reg` holds HKLM, `user.reg`
//! HKCU) and the same rule decides. This never *writes* anything and never
//! asserts a copy is unlicensed — absence only means "we couldn't confirm
//! activation here", which just leaves the crack path available.

use serde::Serialize;

#[cfg(any(not(target_os = "windows"), test))]
use std::collections::HashMap;
#[cfg(any(not(target_os = "windows"), test))]
use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
use super::proc::NoWindow;

/// The SecuROM subkey holding a title's per-machine activation blobs. Value
/// names look like `securom_v7_01.dat`; we treat any `securom_v7` value here as
/// activation data.
const SECUROM_USERDATA: &str = "UserData";
/// Prefix of the activation value names written under `UserData`. SecuROM v7
/// writes `securom_v7_<NN>.dat` (the license data), `.bak` (backup) and `.tmp`
/// (transient) — confirmed both by the forensic decomp analysis
/// (docs/securom_forensic_analysis.md, "User Data Storage") and the live registry
/// on an activated install. Matching the PREFIX, not the full `.dat` name, so it
/// catches every suffix (and the per-title index `_01`, `_02`, …).
const SECUROM_VALUE_PREFIX: &str = "securom_v7";
/// The activation key some SecuROM SKUs write instead of / alongside `UserData`.
/// The bang and spaces are part of the actual key name (from decomp `FUN_01c861d9`).
const SECUROM_LICENSE_KEY: &str = "License information - Do not delete!";

/// The EA "Mercenaries 2" install key. Its `Registration` value points at where
/// the CD-Key (ergc) lives — the Mercs2-specific ownership signal. 32-bit
/// installer → WOW6432Node on x64; the native view is tried too.
const INSTALL_KEYS: &[&str] = &[
    r"HKLM\SOFTWARE\WOW6432Node\EA Games\Mercenaries 2 World in Flames",
    r"HKLM\SOFTWARE\EA Games\Mercenaries 2 World in Flames",
];

/// Where the game assembles the CD-Key path itself when the install key carries
/// no `Registration` pointer (decomp `FUN_0074b760` / SettingsSerializer::Init:
/// `SOFTWARE\Electronic Arts\EA Games\Mercenaries 2 World in Flames\ergc`, HKLM,
/// default value). Used as the fallback target.
const ERGC_FALLBACK: &str = r"Software\Electronic Arts\EA Games\Mercenaries 2 World in Flames\ergc";

/// The engine's own "no key" sentinel (FUN_0074b760) — a present-but-unregistered
/// value we must NOT count as a CD-Key.
const NO_CD_KEY: &str = "No CD Key Found";

/// Registry roots + `SecuROM` base paths to probe. A 32-bit installer/game
/// writes under `WOW6432Node` on 64-bit Windows, but SecuROM has historically
/// used both HKLM and HKCU and both the redirected and native views, so we check
/// all of them and treat any hit as confirmation.
const SECUROM_BASES: &[&str] = &[
    r"HKLM\SOFTWARE\WOW6432Node\SecuROM",
    r"HKLM\SOFTWARE\SecuROM",
    r"HKCU\SOFTWARE\SecuROM",
];

/// What license detection found, for the UI to branch Setup on.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenseStatus {
    /// Whether the check ran on this host: Windows reads its registry, macOS and
    /// Linux read the registry files of the Wine/Proton prefix the game runs in.
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

/// Detect whether this machine holds a SecuROM activation for the game. On macOS
/// and Linux, a prefix registry file that can't be read or parsed is an error
/// naming the file.
#[tauri::command(async)]
pub fn detect_license() -> Result<LicenseStatus, String> {
    #[cfg(target_os = "windows")]
    {
        Ok(detect_license_windows())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let settings = super::runtime_settings::read()?;
        let prefix = super::runtime_settings::resolve_prefix(&settings, None)?;
        let layout = if cfg!(target_os = "linux") {
            PrefixLayout::Proton
        } else {
            PrefixLayout::Wine
        };
        detect_license_in_prefix(&prefix, layout)
    }
}

/// Read-only access to the registry values the licence rule looks at. Keys are
/// full paths starting with their hive (`HKLM\…` or `HKCU\…`); key and value
/// names match case-insensitively, as in the registry.
trait RegistryView {
    /// A named REG_SZ value's data; `None` if the key or value is absent or the
    /// value is not REG_SZ.
    fn string_value(&self, key: &str, name: &str) -> Option<String>;
    /// A key's `(Default)` REG_SZ value.
    fn default_value(&self, key: &str) -> Option<String>;
    /// The key exists (with or without values).
    fn key_exists(&self, key: &str) -> bool;
    /// The key holds at least one value whose name starts with `prefix`.
    fn has_value_with_prefix(&self, key: &str, prefix: &str) -> bool;
}

/// What the licence rule found in one registry.
#[derive(Debug)]
struct Signals {
    cd_key_present: bool,
    license_key_present: bool,
    user_data_present: bool,
    keys_found: Vec<String>,
}

impl Signals {
    fn securom_activated(&self) -> bool {
        self.license_key_present || self.user_data_present
    }

    /// Legally owned = a registered CD-Key OR a SecuROM activation. Either means we
    /// should not crack: the CD-Key is proof of purchase; the activation means the
    /// stock exe already passes DRM.
    fn licensed(&self) -> bool {
        self.cd_key_present || self.securom_activated()
    }
}

/// The licence rule, over any registry backend.
fn find_signals(reg: &impl RegistryView) -> Signals {
    let mut keys_found = Vec::new();
    let mut cd_key_present = false;
    let mut license_key_present = false;
    let mut user_data_present = false;

    // 1) Follow the install key's `Registration` pointer to the ergc CD-Key. This
    //    is the Mercs2-specific ownership signal (unlike the shared SecuROM keys).
    let pointer = INSTALL_KEYS
        .iter()
        .find_map(|k| reg.string_value(k, "Registration"))
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
        // `reg query` output arrives trimmed; trim here so every backend compares alike.
        match reg.default_value(cand) {
            Some(v) if !v.trim().is_empty() && v.trim() != NO_CD_KEY => {
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
        if reg.key_exists(&license_key) {
            license_key_present = true;
            keys_found.push(license_key);
        }
        // …but the load-bearing signal is `securom_v7_*` activation data in UserData.
        let userdata = format!(r"{base}\{SECUROM_USERDATA}");
        if reg.has_value_with_prefix(&userdata, SECUROM_VALUE_PREFIX) {
            user_data_present = true;
            keys_found.push(userdata);
        }
    }

    Signals {
        cd_key_present,
        license_key_present,
        user_data_present,
        keys_found,
    }
}

/// The live Windows registry, through `reg query`.
#[cfg(target_os = "windows")]
struct RegQuery;

#[cfg(target_os = "windows")]
impl RegistryView for RegQuery {
    fn string_value(&self, key: &str, name: &str) -> Option<String> {
        reg_query_value(key, name)
    }
    fn default_value(&self, key: &str) -> Option<String> {
        reg_default_value(key)
    }
    fn key_exists(&self, key: &str) -> bool {
        reg_key_exists(key)
    }
    fn has_value_with_prefix(&self, key: &str, prefix: &str) -> bool {
        reg_key_has_value_with_prefix(key, prefix)
    }
}

#[cfg(target_os = "windows")]
fn detect_license_windows() -> LicenseStatus {
    let signals = find_signals(&RegQuery);
    let cd_key_present = signals.cd_key_present;
    let securom_activated = signals.securom_activated();
    let licensed = signals.licensed();

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
        license_key_present: signals.license_key_present,
        user_data_present: signals.user_data_present,
        keys_found: signals.keys_found,
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

// ----------------------------------------------------------------------------
// macOS / Linux: the registry files of the prefix the game runs in
// ----------------------------------------------------------------------------

/// Where a prefix keeps its registry files.
#[cfg(any(not(target_os = "windows"), test))]
#[derive(Clone, Copy, Debug)]
enum PrefixLayout {
    /// A plain Wine prefix (macOS): `system.reg` and `user.reg` at its root.
    Wine,
    /// A Proton compat-data folder (Linux): the Wine prefix is its `pfx/`.
    Proton,
}

#[cfg(any(not(target_os = "windows"), test))]
impl PrefixLayout {
    /// The folder holding `system.reg` and `user.reg`.
    fn registry_dir(self, prefix: &Path) -> PathBuf {
        match self {
            PrefixLayout::Wine => prefix.to_path_buf(),
            PrefixLayout::Proton => prefix.join("pfx"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            PrefixLayout::Wine => "Wine prefix",
            PrefixLayout::Proton => "Proton prefix",
        }
    }
}

/// One value of a Wine registry key.
#[cfg(any(not(target_os = "windows"), test))]
#[derive(Debug)]
struct WineValue {
    /// Lowercased; `""` is the key's `(Default)` value (`@=` in the file).
    name: String,
    /// The data of a REG_SZ value (`"name"="data"`); `None` for every other type.
    sz: Option<String>,
}

/// One Wine registry file, parsed. Key paths (relative to the file's hive, with
/// single backslashes) are lowercased, so lookups are case-insensitive.
#[cfg(any(not(target_os = "windows"), test))]
#[derive(Debug, Default)]
struct WineHive {
    keys: HashMap<String, Vec<WineValue>>,
}

#[cfg(any(not(target_os = "windows"), test))]
impl WineHive {
    /// `key` is lowercased.
    fn values(&self, key: &str) -> Option<&[WineValue]> {
        self.keys.get(key).map(Vec::as_slice)
    }

    /// Wine writes no section for a key that holds only subkeys, so a key also
    /// exists when some section lies beneath it. `key` is lowercased.
    fn has_key(&self, key: &str) -> bool {
        let below = format!("{key}\\");
        self.keys.contains_key(key) || self.keys.keys().any(|k| k.starts_with(&below))
    }
}

/// Read and parse one registry file. `Ok(None)` when it does not exist; any
/// other read failure or unparseable content is an error naming the file.
#[cfg(any(not(target_os = "windows"), test))]
fn read_wine_hive(path: &Path) -> Result<Option<WineHive>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Could not read {}: {e}", path.display())),
    };
    parse_wine_hive(&text)
        .map(Some)
        .map_err(|e| format!("{} is not a Wine registry file Modkit can read: {e}", path.display()))
}

/// Parse the text of a Wine registry file (`WINE REGISTRY Version 2`):
/// `[key\\path] <timestamp>` sections, then `"name"=<data>` or `@=<data>` value
/// lines; hex data continues onto the next line after a trailing `\`. `;;`
/// comments and `#time`/`#arch`/`#class`/`#link` directives carry nothing the
/// licence rule reads. Errors name the line.
#[cfg(any(not(target_os = "windows"), test))]
fn parse_wine_hive(text: &str) -> Result<WineHive, String> {
    let mut lines = text.lines().enumerate().map(|(i, l)| (i + 1, l));
    match lines.next() {
        Some((_, "WINE REGISTRY Version 2")) => {}
        _ => return Err("line 1 is not `WINE REGISTRY Version 2`".into()),
    }
    let mut hive = WineHive::default();
    let mut current: Option<String> = None;
    let mut continued = false;
    for (n, line) in lines {
        if continued {
            continued = parse_hex_bytes(line.trim_start()).map_err(|e| format!("line {n}: {e}"))?;
            continue;
        }
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            let (key, after) = unescape(rest, ']').map_err(|e| format!("line {n}: {e}"))?;
            let stamp = after.trim();
            if !stamp.chars().all(|c| c.is_ascii_digit()) {
                return Err(format!("line {n}: unexpected text after the key name: {stamp}"));
            }
            let key = key.to_lowercase();
            hive.keys.entry(key.clone()).or_default();
            current = Some(key);
            continue;
        }
        let (name, data) = if let Some(rest) = line.strip_prefix('@') {
            (String::new(), rest)
        } else if let Some(rest) = line.strip_prefix('"') {
            let (name, after) = unescape(rest, '"').map_err(|e| format!("line {n}: {e}"))?;
            (name.to_lowercase(), after)
        } else {
            return Err(format!("line {n}: not a key, a value or a comment: {line}"));
        };
        let data = data
            .strip_prefix('=')
            .ok_or_else(|| format!("line {n}: no `=` after the value name"))?;
        let (sz, more) = parse_data(data).map_err(|e| format!("line {n}: {e}"))?;
        continued = more;
        let key = current
            .as_ref()
            .ok_or_else(|| format!("line {n}: a value before any key"))?;
        hive.keys.entry(key.clone()).or_default().push(WineValue { name, sz });
    }
    if continued {
        return Err("the file ends inside a value continued with `\\`".into());
    }
    Ok(hive)
}

/// One value's data. Returns the REG_SZ text (`None` for other types) and
/// whether the data continues on the next line.
#[cfg(any(not(target_os = "windows"), test))]
fn parse_data(data: &str) -> Result<(Option<String>, bool), String> {
    if let Some(rest) = data.strip_prefix('"') {
        return Ok((Some(quoted_to_end(rest)?), false));
    }
    if let Some(rest) = data.strip_prefix("str(") {
        // REG_EXPAND_SZ, REG_MULTI_SZ, …: `str(<type in hex>):"…"`.
        let rest = after_hex_type(rest)?;
        let rest = rest
            .strip_prefix('"')
            .ok_or_else(|| format!("a str value without a quoted string: {data}"))?;
        quoted_to_end(rest)?;
        return Ok((None, false));
    }
    if let Some(rest) = data.strip_prefix("dword:") {
        if rest.len() == 8 && rest.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok((None, false));
        }
        return Err(format!("a dword that is not 8 hex digits: {rest}"));
    }
    if let Some(rest) = data.strip_prefix("hex:") {
        return Ok((None, parse_hex_bytes(rest)?));
    }
    if let Some(rest) = data.strip_prefix("hex(") {
        return Ok((None, parse_hex_bytes(after_hex_type(rest)?)?));
    }
    Err(format!("unrecognised value data: {data}"))
}

/// Past the `<type in hex>):` of `str(…):` / `hex(…):`.
#[cfg(any(not(target_os = "windows"), test))]
fn after_hex_type(rest: &str) -> Result<&str, String> {
    let (ty, after) = rest
        .split_once("):")
        .ok_or_else(|| format!("a value type without `):`: {rest}"))?;
    if ty.is_empty() || !ty.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("a value type that is not hex: {ty}"));
    }
    Ok(after)
}

/// A quoted string that must end the line.
#[cfg(any(not(target_os = "windows"), test))]
fn quoted_to_end(rest: &str) -> Result<String, String> {
    let (s, after) = unescape(rest, '"')?;
    if !after.is_empty() {
        return Err(format!("unexpected text after the closing quote: {after}"));
    }
    Ok(s)
}

/// Comma-separated hex bytes, optionally ending in `\` to continue on the next
/// line (Wine writes `…,59,\`). Returns whether it continues.
#[cfg(any(not(target_os = "windows"), test))]
fn parse_hex_bytes(s: &str) -> Result<bool, String> {
    let (body, continues) = match s.strip_suffix('\\') {
        Some(b) => (b, true),
        None => (s, false),
    };
    let body = body.strip_suffix(',').unwrap_or(body);
    if body.is_empty() {
        return Ok(continues);
    }
    for byte in body.split(',') {
        if byte.len() != 2 || !byte.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("not a hex byte: `{byte}`"));
        }
    }
    Ok(continues)
}

/// Decode a Wine-escaped string up to the unescaped `end`, returning it and the
/// text after `end`. Wine's escapes (`server/unicode.c`, `dump_strW` /
/// `parse_strW`): `\a \b \e \f \n \r \t \v`, `\x` + 1–4 hex digits (a UTF-16 code
/// unit, so pairs encode surrogates), `\` + 1–3 octal digits, and `\` before any
/// other character stands for that character (`\\`, `\"`, `\]`).
#[cfg(any(not(target_os = "windows"), test))]
fn unescape(s: &str, end: char) -> Result<(String, &str), String> {
    let mut units: Vec<u16> = Vec::new();
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == end {
            let text = String::from_utf16(&units)
                .map_err(|_| format!("an escaped string that is not valid UTF-16: {s}"))?;
            return Ok((text, &s[i + c.len_utf8()..]));
        }
        if c != '\\' {
            let mut buf = [0u16; 2];
            units.extend_from_slice(c.encode_utf16(&mut buf));
            continue;
        }
        let Some((_, e)) = chars.next() else {
            return Err(format!("a string ending in a lone `\\`: {s}"));
        };
        let unit = match e {
            'a' => 0x07,
            'b' => 0x08,
            'e' => 0x1b,
            'f' => 0x0c,
            'n' => 0x0a,
            'r' => 0x0d,
            't' => 0x09,
            'v' => 0x0b,
            'x' => {
                let mut v: u16 = 0;
                let mut digits = 0;
                while digits < 4 {
                    match chars.peek().and_then(|&(_, d)| d.to_digit(16)) {
                        Some(d) => {
                            v = v * 16 + d as u16;
                            digits += 1;
                            chars.next();
                        }
                        None => break,
                    }
                }
                if digits == 0 {
                    return Err(format!("`\\x` without hex digits: {s}"));
                }
                v
            }
            '0'..='7' => {
                let mut v = e as u16 - '0' as u16;
                for _ in 0..2 {
                    match chars.peek().and_then(|&(_, d)| d.to_digit(8)) {
                        Some(d) => {
                            v = v * 8 + d as u16;
                            chars.next();
                        }
                        None => break,
                    }
                }
                v
            }
            other => {
                let mut buf = [0u16; 2];
                units.extend_from_slice(other.encode_utf16(&mut buf));
                continue;
            }
        };
        units.push(unit);
    }
    Err(format!("no closing `{end}`: {s}"))
}

/// The two registry files of a prefix; `None` where the file does not exist.
#[cfg(any(not(target_os = "windows"), test))]
struct PrefixRegistry {
    /// `system.reg`: HKEY_LOCAL_MACHINE.
    system: Option<WineHive>,
    /// `user.reg`: HKEY_CURRENT_USER.
    user: Option<WineHive>,
}

#[cfg(any(not(target_os = "windows"), test))]
impl PrefixRegistry {
    /// The file a key lives in and its lowercased path within that file.
    fn locate(&self, key: &str) -> (Option<&WineHive>, String) {
        let lower = key.to_lowercase();
        if let Some(rest) = lower.strip_prefix("hklm\\") {
            (self.system.as_ref(), rest.to_string())
        } else if let Some(rest) = lower.strip_prefix("hkcu\\") {
            (self.user.as_ref(), rest.to_string())
        } else {
            unreachable!("licence keys are rooted at HKLM or HKCU, not {key}")
        }
    }
}

#[cfg(any(not(target_os = "windows"), test))]
impl RegistryView for PrefixRegistry {
    fn string_value(&self, key: &str, name: &str) -> Option<String> {
        let (hive, path) = self.locate(key);
        let name = name.to_lowercase();
        hive?.values(&path)?.iter().find(|v| v.name == name)?.sz.clone()
    }
    fn default_value(&self, key: &str) -> Option<String> {
        self.string_value(key, "")
    }
    fn key_exists(&self, key: &str) -> bool {
        let (hive, path) = self.locate(key);
        hive.is_some_and(|h| h.has_key(&path))
    }
    fn has_value_with_prefix(&self, key: &str, prefix: &str) -> bool {
        let (hive, path) = self.locate(key);
        let prefix = prefix.to_lowercase();
        hive.and_then(|h| h.values(&path))
            .is_some_and(|vs| vs.iter().any(|v| v.name.starts_with(&prefix)))
    }
}

/// Run the licence rule over the registry files of `prefix`. A prefix (or its
/// registry files) that does not exist yet is "not found", with a detail naming
/// the path; a registry file that can't be read or parsed is an error.
#[cfg(any(not(target_os = "windows"), test))]
fn detect_license_in_prefix(prefix: &Path, layout: PrefixLayout) -> Result<LicenseStatus, String> {
    let dir = layout.registry_dir(prefix);
    let kind = layout.name();
    let not_found = |detail: String| LicenseStatus {
        applicable: true,
        licensed: false,
        cd_key_present: false,
        license_key_present: false,
        user_data_present: false,
        keys_found: Vec::new(),
        detail,
    };
    match std::fs::metadata(&dir) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Err(format!("{} is not a folder, so it is not a {kind}.", dir.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(not_found(format!(
                "The {kind} Modkit launches the game in has not been created yet ({} does not \
                 exist), so there is no registry to check for an EA CD-Key or SecuROM \
                 activation, and the crack path applies.",
                dir.display()
            )))
        }
        Err(e) => return Err(format!("Could not read {}: {e}", dir.display())),
    }

    let system_path = dir.join("system.reg");
    let user_path = dir.join("user.reg");
    let registry = PrefixRegistry {
        system: read_wine_hive(&system_path)?,
        user: read_wine_hive(&user_path)?,
    };
    if registry.system.is_none() && registry.user.is_none() {
        return Ok(not_found(format!(
            "The {kind} Modkit launches the game in ({}) has no registry files yet ({} and {} do \
             not exist), so there is no registry to check for an EA CD-Key or SecuROM \
             activation, and the crack path applies.",
            dir.display(),
            system_path.display(),
            user_path.display()
        )));
    }

    let signals = find_signals(&registry);
    let place = format!(
        "in the registry of the {kind} Modkit launches the game in ({})",
        dir.display()
    );
    let mut detail = match (signals.cd_key_present, signals.securom_activated()) {
        (true, true) => format!(
            "Registered (EA CD-Key present) and SecuROM-activated {place} — licensed. Setup \
             uses the dxwrapper path and leaves your exe untouched."
        ),
        (true, false) => format!(
            "Registered (EA CD-Key present) {place} — treated as licensed. The stock exe may \
             still need SecuROM activation to run, but Setup won't crack it; use the dxwrapper \
             path."
        ),
        (false, true) => format!(
            "SecuROM activation present {place} — licensed. Setup uses the dxwrapper path and \
             leaves your exe untouched."
        ),
        (false, false) => format!(
            "No EA CD-Key and no SecuROM activation found {place}, so the crack path applies. \
             (A DRM-free build never needs either; you can still choose the dxwrapper path \
             manually.)"
        ),
    };
    for (path, hive) in [(&system_path, &registry.system), (&user_path, &registry.user)] {
        if hive.is_none() {
            detail.push_str(&format!(" {} does not exist, so it was not checked.", path.display()));
        }
    }

    Ok(LicenseStatus {
        applicable: true,
        licensed: signals.licensed(),
        cd_key_present: signals.cd_key_present,
        license_key_present: signals.license_key_present,
        user_data_present: signals.user_data_present,
        keys_found: signals.keys_found,
        detail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER_SYSTEM: &str =
        "WINE REGISTRY Version 2\n;; All keys relative to REGISTRY\\\\Machine\n\n#arch=win64\n\n";
    const HEADER_USER: &str = "WINE REGISTRY Version 2\n;; All keys relative to \
                               REGISTRY\\\\User\\\\S-1-5-21-0-0-0-1000\n\n#arch=win64\n\n";

    /// Filler modelled on a stock prefix: other keys, value types, escapes and a
    /// continued hex value, so every test parses more than the keys it checks.
    const STOCK_SYSTEM: &str = "[Software\\\\Classes\\\\.exe] 1791151695\n\
        #time=1dd544cdafcd7c6\n\
        @=\"exefile\"\n\
        \"Content Type\"=\"application/x-msdownload\"\n\n\
        [Software\\\\Microsoft\\\\Cryptography\\\\Providers] 1791151696\n\
        #time=1dd544cdb4070a8\n\
        \"Dll\"=str(7):\"cryptnet.dll\\0\"\n\
        \"Path\"=str(2):\"%SystemRoot%\\\\system32\"\n\
        \"Flags\"=dword:00000001\n\
        @=hex(ffff0012):5c,00,5c,00,2e,00,5c,00,44,00,49,00,53,00,50,00,4c,00,41,00,59,\\\n\
        \x20 00,00\n\
        \"Blob\"=hex:01,02,03\n\n\
        [Software\\\\Fonts] 1791151696\n\
        \"@Hiragino Sans W0 (TrueType)\"=\"Z:\\\\System\\\\Library\\\\Fonts\\\\\\x30d2\\x30e9 W0.ttc\"\n\
        @=\"\\\"C:\\\\Program Files\\\\iexplore.exe\\\" %1\"\n\n";
    const STOCK_USER: &str = "[Console] 1793552555\n\
        #time=1dd6a22cae458dc\n\
        \"ColorTable00\"=dword:00000000\n\
        \"FaceName\"=\"Courier New\"\n\n\
        [Control Panel\\\\International\\\\\\xd83c\\xdf0e\\xd83c\\xdf0f] 1791151689\n\
        @=\"\"\n\n";

    /// A prefix folder holding `system.reg` / `user.reg` with these extra
    /// sections (`None` leaves the file out).
    fn prefix_with(dir: &Path, system: Option<&str>, user: Option<&str>) {
        std::fs::create_dir_all(dir).unwrap();
        if let Some(s) = system {
            std::fs::write(dir.join("system.reg"), format!("{HEADER_SYSTEM}{STOCK_SYSTEM}{s}")).unwrap();
        }
        if let Some(u) = user {
            std::fs::write(dir.join("user.reg"), format!("{HEADER_USER}{STOCK_USER}{u}")).unwrap();
        }
    }

    fn detect_wine(system: Option<&str>, user: Option<&str>) -> LicenseStatus {
        let tmp = tempfile::tempdir().unwrap();
        prefix_with(tmp.path(), system, user);
        detect_license_in_prefix(tmp.path(), PrefixLayout::Wine).unwrap()
    }

    const INSTALL_WOW: &str = "[Software\\\\Wow6432Node\\\\EA Games\\\\Mercenaries 2 World in Flames] 1700000000\n\
        #time=1d0000000000000\n\
        \"Registration\"=\"Software\\\\Electronic Arts\\\\EA Games\\\\Mercenaries 2 World in Flames\\\\ergc\"\n\n";

    #[test]
    fn cd_key_through_the_registration_pointer_in_wow6432node() {
        let ergc = "[Software\\\\Wow6432Node\\\\Electronic Arts\\\\EA Games\\\\Mercenaries 2 World in Flames\\\\ergc] 1700000000\n\
            @=\"ANEEQ-1234-5678-9012-3456\"\n\n";
        let s = detect_wine(Some(&format!("{INSTALL_WOW}{ergc}")), Some(""));
        assert!(s.applicable);
        assert!(s.cd_key_present);
        assert!(s.licensed);
        assert!(!s.user_data_present && !s.license_key_present);
        assert_eq!(
            s.keys_found,
            [r"HKLM\SOFTWARE\WOW6432Node\Electronic Arts\EA Games\Mercenaries 2 World in Flames\ergc"]
        );
        assert!(s.detail.starts_with("Registered (EA CD-Key present) in the registry of the Wine prefix"));
    }

    #[test]
    fn cd_key_through_the_registration_pointer_in_the_native_view() {
        let install = "[Software\\\\EA Games\\\\Mercenaries 2 World in Flames] 1700000000\n\
            \"Registration\"=\"Software\\\\Electronic Arts\\\\EA Games\\\\Mercenaries 2 World in Flames\\\\ergc\"\n\n\
            [Software\\\\Electronic Arts\\\\EA Games\\\\Mercenaries 2 World in Flames\\\\ergc] 1700000000\n\
            @=\"ANEEQ-1234-5678-9012-3456\"\n\n";
        let s = detect_wine(Some(install), Some(""));
        assert!(s.cd_key_present && s.licensed);
        assert_eq!(
            s.keys_found,
            [r"HKLM\Software\Electronic Arts\EA Games\Mercenaries 2 World in Flames\ergc"]
        );
    }

    #[test]
    fn cd_key_at_the_fallback_path_without_a_registration_pointer() {
        let ergc = "[Software\\\\Wow6432Node\\\\Electronic Arts\\\\EA Games\\\\Mercenaries 2 World in Flames\\\\ergc] 1700000000\n\
            @=\"ANEEQ-1234-5678-9012-3456\"\n\n";
        let s = detect_wine(Some(ergc), Some(""));
        assert!(s.cd_key_present && s.licensed);
    }

    #[test]
    fn the_no_cd_key_sentinel_is_not_a_cd_key() {
        let ergc = "[Software\\\\Wow6432Node\\\\Electronic Arts\\\\EA Games\\\\Mercenaries 2 World in Flames\\\\ergc] 1700000000\n\
            @=\"No CD Key Found\"\n\n";
        let s = detect_wine(Some(&format!("{INSTALL_WOW}{ergc}")), Some(""));
        assert!(!s.cd_key_present);
        assert!(!s.licensed);
        assert!(s.keys_found.is_empty());
    }

    #[test]
    fn securom_v7_user_data_in_user_reg() {
        let user = "[Software\\\\SecuROM\\\\UserData] 1700000000\n\
            #time=1d0000000000000\n\
            \"securom_v7_01.dat\"=hex:01,02,03,04,05,06,07,08,09,0a,0b,0c,0d,0e,0f,10,11,\\\n\
            \x20 12,13\n\
            \"securom_v7_01.bak\"=hex:01,02\n\n";
        let s = detect_wine(Some(""), Some(user));
        assert!(s.user_data_present && s.licensed);
        assert!(!s.cd_key_present);
        assert_eq!(s.keys_found, [r"HKCU\SOFTWARE\SecuROM\UserData"]);
        assert!(s.detail.starts_with("SecuROM activation present in the registry of the Wine prefix"));
    }

    #[test]
    fn securom_v7_user_data_in_each_system_reg_base() {
        for (section, key) in [
            ("Software\\\\Wow6432Node\\\\SecuROM\\\\UserData", r"HKLM\SOFTWARE\WOW6432Node\SecuROM\UserData"),
            ("Software\\\\SecuROM\\\\UserData", r"HKLM\SOFTWARE\SecuROM\UserData"),
        ] {
            let system = format!("[{section}] 1700000000\n\"securom_v7_01.dat\"=hex:01,02\n\n");
            let s = detect_wine(Some(&system), Some(""));
            assert!(s.user_data_present && s.licensed, "{section}");
            assert_eq!(s.keys_found, [key]);
        }
    }

    #[test]
    fn the_license_information_key_counts_even_with_only_subkeys() {
        // Wine writes no section for a key with no values of its own.
        let system = "[Software\\\\Wow6432Node\\\\SecuROM\\\\License information - Do not delete!\\\\Sub] 1700000000\n\
            \"x\"=dword:00000001\n\n";
        let s = detect_wine(Some(system), Some(""));
        assert!(s.license_key_present && s.licensed);
        assert_eq!(
            s.keys_found,
            [r"HKLM\SOFTWARE\WOW6432Node\SecuROM\License information - Do not delete!"]
        );
    }

    #[test]
    fn names_match_case_insensitively() {
        let system = "[SOFTWARE\\\\wow6432node\\\\ea games\\\\MERCENARIES 2 WORLD IN FLAMES] 1700000000\n\
            \"REGISTRATION\"=\"Software\\\\Electronic Arts\\\\EA Games\\\\Mercenaries 2 World in Flames\\\\ergc\"\n\n\
            [software\\\\WOW6432NODE\\\\electronic arts\\\\ea games\\\\mercenaries 2 world in flames\\\\ERGC] 1700000000\n\
            @=\"ANEEQ-1234\"\n\n";
        let user = "[software\\\\securom\\\\userdata] 1700000000\n\"SECUROM_V7_01.DAT\"=hex:01\n\n";
        let s = detect_wine(Some(system), Some(user));
        assert!(s.cd_key_present && s.user_data_present && s.licensed);
        assert!(s.detail.starts_with("Registered (EA CD-Key present) and SecuROM-activated in the registry"));
    }

    #[test]
    fn nothing_found_is_unlicensed_and_names_the_prefix() {
        let unrelated = "[Software\\\\SecuROM\\\\UserData] 1700000000\n\"other\"=dword:00000000\n\n\
            [Software\\\\Wow6432Node\\\\EA Games\\\\Mercenaries 2 World in Flames] 1700000000\n\
            \"Registration\"=\"\"\n\n";
        let tmp = tempfile::tempdir().unwrap();
        prefix_with(tmp.path(), Some(unrelated), Some(""));
        let s = detect_license_in_prefix(tmp.path(), PrefixLayout::Wine).unwrap();
        assert!(s.applicable);
        assert!(!s.licensed && !s.cd_key_present && !s.user_data_present && !s.license_key_present);
        assert!(s.keys_found.is_empty());
        assert_eq!(
            s.detail,
            format!(
                "No EA CD-Key and no SecuROM activation found in the registry of the Wine prefix \
                 Modkit launches the game in ({}), so the crack path applies. (A DRM-free build \
                 never needs either; you can still choose the dxwrapper path manually.)",
                tmp.path().display()
            )
        );
    }

    #[test]
    fn a_proton_prefix_keeps_its_registry_under_pfx() {
        let user = "[Software\\\\SecuROM\\\\UserData] 1700000000\n\"securom_v7_01.dat\"=hex:01\n\n";
        let tmp = tempfile::tempdir().unwrap();
        prefix_with(&tmp.path().join("pfx"), Some(""), Some(user));
        let proton = detect_license_in_prefix(tmp.path(), PrefixLayout::Proton).unwrap();
        assert!(proton.user_data_present && proton.licensed);
        assert!(proton.detail.contains(&format!(
            "in the registry of the Proton prefix Modkit launches the game in ({})",
            tmp.path().join("pfx").display()
        )));
        // Read as a plain Wine prefix, the same folder has no registry files at its root.
        let wine = detect_license_in_prefix(tmp.path(), PrefixLayout::Wine).unwrap();
        assert!(!wine.licensed);
        assert!(wine.detail.contains("has no registry files yet"));
    }

    #[test]
    fn a_plain_wine_prefix_keeps_its_registry_at_the_root() {
        let user = "[Software\\\\SecuROM\\\\UserData] 1700000000\n\"securom_v7_01.dat\"=hex:01\n\n";
        let tmp = tempfile::tempdir().unwrap();
        prefix_with(tmp.path(), Some(""), Some(user));
        assert!(detect_license_in_prefix(tmp.path(), PrefixLayout::Wine).unwrap().licensed);
        // Read as Proton, there is no `pfx/` yet.
        let proton = detect_license_in_prefix(tmp.path(), PrefixLayout::Proton).unwrap();
        assert!(!proton.licensed);
        assert!(proton.detail.contains("has not been created yet"));
    }

    #[test]
    fn a_missing_prefix_is_not_found_and_names_the_path() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path().join("wine-prefix");
        let s = detect_license_in_prefix(&prefix, PrefixLayout::Wine).unwrap();
        assert!(s.applicable && !s.licensed);
        assert_eq!(
            s.detail,
            format!(
                "The Wine prefix Modkit launches the game in has not been created yet ({} does \
                 not exist), so there is no registry to check for an EA CD-Key or SecuROM \
                 activation, and the crack path applies.",
                prefix.display()
            )
        );
        let s = detect_license_in_prefix(&prefix, PrefixLayout::Proton).unwrap();
        assert!(s.detail.contains(&format!("({} does not exist)", prefix.join("pfx").display())));
    }

    #[test]
    fn a_prefix_without_registry_files_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let s = detect_license_in_prefix(tmp.path(), PrefixLayout::Wine).unwrap();
        assert!(s.applicable && !s.licensed);
        assert!(s.detail.contains(&format!(
            "({} and {} do not exist)",
            tmp.path().join("system.reg").display(),
            tmp.path().join("user.reg").display()
        )));
    }

    #[test]
    fn one_missing_registry_file_is_named_and_the_other_is_still_checked() {
        let user = "[Software\\\\SecuROM\\\\UserData] 1700000000\n\"securom_v7_01.dat\"=hex:01\n\n";
        let tmp = tempfile::tempdir().unwrap();
        prefix_with(tmp.path(), None, Some(user));
        let s = detect_license_in_prefix(tmp.path(), PrefixLayout::Wine).unwrap();
        assert!(s.user_data_present && s.licensed);
        assert!(s.detail.ends_with(&format!(
            " {} does not exist, so it was not checked.",
            tmp.path().join("system.reg").display()
        )));
    }

    #[test]
    fn an_unreadable_registry_file_fails_and_names_it() {
        let tmp = tempfile::tempdir().unwrap();
        prefix_with(tmp.path(), None, Some(""));
        // Not UTF-8, so it can't be read as text.
        std::fs::write(tmp.path().join("system.reg"), [0xff, 0xfe, 0x00, 0x57]).unwrap();
        let err = detect_license_in_prefix(tmp.path(), PrefixLayout::Wine).unwrap_err();
        assert!(
            err.starts_with(&format!("Could not read {}: ", tmp.path().join("system.reg").display())),
            "{err}"
        );
    }

    #[test]
    fn an_unparseable_registry_file_fails_and_names_it_and_the_line() {
        let tmp = tempfile::tempdir().unwrap();
        prefix_with(tmp.path(), Some(""), None);
        std::fs::write(
            tmp.path().join("user.reg"),
            format!("{HEADER_USER}[Software\\\\SecuROM\\\\UserData] 1700000000\n\"securom_v7_01.dat\n"),
        )
        .unwrap();
        let err = detect_license_in_prefix(tmp.path(), PrefixLayout::Wine).unwrap_err();
        assert!(
            err.starts_with(&format!(
                "{} is not a Wine registry file Modkit can read: line 7: no closing `\"`",
                tmp.path().join("user.reg").display()
            )),
            "{err}"
        );
    }

    #[test]
    fn a_file_without_the_wine_header_fails() {
        let err = parse_wine_hive("REGEDIT4\n[HKEY_LOCAL_MACHINE\\Software]\n").unwrap_err();
        assert_eq!(err, "line 1 is not `WINE REGISTRY Version 2`");
    }

    #[test]
    fn values_and_escapes_decode() {
        let hive = parse_wine_hive(&format!("{HEADER_SYSTEM}{STOCK_SYSTEM}")).unwrap();
        let fonts = hive.values("software\\fonts").unwrap();
        assert_eq!(fonts[0].name, "@hiragino sans w0 (truetype)");
        assert_eq!(fonts[0].sz.as_deref(), Some("Z:\\System\\Library\\Fonts\\\u{30d2}\u{30e9} W0.ttc"));
        assert_eq!(fonts[1].name, "");
        assert_eq!(fonts[1].sz.as_deref(), Some("\"C:\\Program Files\\iexplore.exe\" %1"));
        let providers = hive.values("software\\microsoft\\cryptography\\providers").unwrap();
        // Only plain strings are REG_SZ; str(N), dword and hex values carry no text.
        assert!(providers.iter().all(|v| v.sz.is_none()));
        assert_eq!(providers.len(), 5);
        let user = parse_wine_hive(&format!("{HEADER_USER}{STOCK_USER}")).unwrap();
        assert!(user.has_key("control panel\\international\\\u{1f30e}\u{1f30f}"));
        assert!(user.has_key("control panel"));
    }
}
