//! How the game is run on a non-Windows host: which Wine/Proton, which prefix, and
//! every knob passed to it. One file, read by the launcher itself.
//!
//! # Why the backend owns this
//!
//! The launcher used to take a `LaunchOverrides` argument that the frontend always
//! sent as `null`, so the Proton override the backend supported was unreachable.
//! Keeping the selection here means `launch_game` cannot run a configuration other
//! than the one the UI shows — the same reasoning as the managed-component ledger.
//!
//! # The file is the full surface; the UI is a subset
//!
//! `<app-data>/runtime.json` can set the runner, the prefix, environment variables,
//! DLL overrides, registry values written into the prefix, `WINEDEBUG`, and extra
//! game arguments. The UI exposes only the runner selection and the environment
//! list; everything else is an "I know what I'm doing" edit of this file.
//!
//! Because the file is hand-edited, it is parsed strictly: an unknown field, a bad
//! value, or a format from a newer modkit **fails the launch** with the file's path
//! and the problem. A typo that silently drops a setting would leave someone
//! debugging a Wine option that was never applied.
//!
//! # What modkit owns
//!
//! A few variables are set by the launcher itself (`WINEPREFIX`, `PMC_VERBOSE_LOG`,
//! Proton's `STEAM_COMPAT_*`). Setting one of them in `env` is refused rather than
//! letting one side silently win; the prefix has its own field. `WINEDLLOVERRIDES`
//! and `WINEDEBUG` may come from `env` *or* from their structured fields, never
//! both.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Bumped only for a change an older modkit cannot read.
const FORMAT: u32 = 1;

/// The settings file's name under the app-data dir.
const FILE: &str = "runtime.json";

/// One environment variable passed to Wine/Proton. A list, not a map, so the UI
/// keeps the order the user entered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

/// Data for one registry value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "lowercase")]
pub enum RegData {
    /// `REG_SZ`.
    String(String),
    /// `REG_DWORD`.
    Dword(u32),
    /// Remove the value. Removing an entry from this file does not undo a value
    /// already written into the prefix; this does.
    Delete,
}

/// One registry value written into the prefix before launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistryValue {
    /// Full key path, e.g. `HKEY_CURRENT_USER\Software\Wine\Mac Driver`.
    pub key: String,
    /// Value name; empty for the key's default value.
    pub name: String,
    pub value: RegData,
}

/// `<app-data>/runtime.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct RuntimeSettings {
    pub format: u32,

    /// macOS: release tag of the modkit-managed Wine build to run.
    pub wine_tag: Option<String>,
    /// Linux: the Proton to run (its directory or the `proton` script).
    pub proton: Option<String>,
    /// Linux: the Steam root (the dir holding `steamapps/`).
    pub steam_root: Option<String>,
    /// Linux: the Steam Linux Runtime `_v2-entry-point`.
    pub sniper: Option<String>,
    /// Linux: run inside the sniper container (default true).
    pub use_container: Option<bool>,
    /// The Wine prefix (macOS) or Proton compat-data dir (Linux). Unset means the
    /// modkit-managed one under app-data.
    pub prefix: Option<String>,

    /// Environment variables for Wine/Proton. The only tuning the UI edits.
    pub env: Vec<EnvVar>,
    /// DLL → load order (`n`, `b`, `n,b`, `b,n`, `d`, or empty to disable),
    /// assembled into `WINEDLLOVERRIDES`.
    pub dll_overrides: BTreeMap<String, String>,
    /// Registry values imported into the prefix before every launch (macOS).
    pub registry: Vec<RegistryValue>,
    /// `WINEDEBUG` channels, e.g. `+seh,+loaddll`.
    pub winedebug: Option<String>,
    /// Extra arguments after the game exe.
    pub exe_args: Vec<String>,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            format: FORMAT,
            wine_tag: None,
            proton: None,
            steam_root: None,
            sniper: None,
            use_container: None,
            prefix: None,
            env: Vec::new(),
            dll_overrides: BTreeMap::new(),
            registry: Vec::new(),
            winedebug: None,
            exe_args: Vec::new(),
        }
    }
}

/// Load orders `WINEDLLOVERRIDES` accepts.
const DLL_MODES: &[&str] = &["n", "b", "n,b", "b,n", "d", ""];

impl RuntimeSettings {
    /// Reject anything the launcher could not apply exactly as written.
    pub fn validate(&self) -> Result<(), String> {
        if self.format > FORMAT {
            return Err(format!(
                "format {} was written by a newer modkit (this one reads up to {FORMAT})",
                self.format
            ));
        }

        let mut seen = std::collections::BTreeSet::new();
        for v in &self.env {
            let k = v.key.as_str();
            if k.is_empty() || k.contains('=') || k.contains('\0') || v.value.contains('\0') {
                return Err(format!("env key '{k}' is not a valid variable name"));
            }
            if !seen.insert(k) {
                return Err(format!("env sets '{k}' twice"));
            }
        }
        if !self.dll_overrides.is_empty() && seen.contains("WINEDLLOVERRIDES") {
            return Err(
                "WINEDLLOVERRIDES is set in both env and dllOverrides — keep one".into(),
            );
        }
        if self.winedebug.is_some() && seen.contains("WINEDEBUG") {
            return Err("WINEDEBUG is set in both env and winedebug — keep one".into());
        }

        for (dll, mode) in &self.dll_overrides {
            if dll.is_empty() || dll.contains(['=', ';', ',']) {
                return Err(format!("dllOverrides name '{dll}' is not a DLL name"));
            }
            if !DLL_MODES.contains(&mode.as_str()) {
                return Err(format!(
                    "dllOverrides '{dll}' = '{mode}' is not a load order (use one of n, b, n,b, b,n, d, or empty)"
                ));
            }
        }

        for r in &self.registry {
            if !(r.key.starts_with("HKEY_CURRENT_USER\\")
                || r.key.starts_with("HKEY_LOCAL_MACHINE\\"))
            {
                return Err(format!(
                    "registry key '{}' must start with HKEY_CURRENT_USER\\ or HKEY_LOCAL_MACHINE\\",
                    r.key
                ));
            }
            let mut text = vec![r.key.as_str(), r.name.as_str()];
            if let RegData::String(s) = &r.value {
                text.push(s);
            }
            if text.iter().any(|t| !t.is_ascii() || t.contains(['\r', '\n'])) {
                return Err(format!(
                    "registry value '{}\\{}' must be single-line ASCII",
                    r.key, r.name
                ));
            }
        }
        Ok(())
    }

    /// The environment to hand Wine/Proton: `env`, then `WINEDLLOVERRIDES` and
    /// `WINEDEBUG` from their structured fields. `reserved` are the variables the
    /// launcher sets itself on this host; setting one in `env` is an error.
    pub fn wine_env(&self, reserved: &[&str]) -> Result<Vec<(String, String)>, String> {
        self.validate()?;
        let mut out = Vec::with_capacity(self.env.len() + 2);
        for v in &self.env {
            if reserved.contains(&v.key.as_str()) {
                return Err(format!(
                    "env sets '{}', which modkit sets itself when launching{}",
                    v.key,
                    if v.key == "WINEPREFIX" || v.key == "STEAM_COMPAT_DATA_PATH" {
                        " — use the `prefix` field instead"
                    } else {
                        ""
                    }
                ));
            }
            out.push((v.key.clone(), v.value.clone()));
        }
        if !self.dll_overrides.is_empty() {
            let joined = self
                .dll_overrides
                .iter()
                .map(|(dll, mode)| format!("{dll}={mode}"))
                .collect::<Vec<_>>()
                .join(";");
            out.push(("WINEDLLOVERRIDES".into(), joined));
        }
        if let Some(d) = &self.winedebug {
            out.push(("WINEDEBUG".into(), d.clone()));
        }
        Ok(out)
    }

    /// The registry values as a `REGEDIT4` file for `reg import`, keys in the
    /// order they first appear.
    pub fn registry_file(&self) -> String {
        let mut keys: Vec<&str> = Vec::new();
        for r in &self.registry {
            if !keys.contains(&r.key.as_str()) {
                keys.push(&r.key);
            }
        }
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let mut out = String::from("REGEDIT4\r\n");
        for key in keys {
            out.push_str(&format!("\r\n[{key}]\r\n"));
            for r in self.registry.iter().filter(|r| r.key == key) {
                let name = if r.name.is_empty() {
                    "@".to_string()
                } else {
                    format!("\"{}\"", esc(&r.name))
                };
                let data = match &r.value {
                    RegData::String(s) => format!("\"{}\"", esc(s)),
                    RegData::Dword(d) => format!("dword:{d:08x}"),
                    RegData::Delete => "-".to_string(),
                };
                out.push_str(&format!("{name}={data}\r\n"));
            }
        }
        out
    }
}

/// Where the settings live.
pub fn settings_path() -> Result<PathBuf, String> {
    Ok(super::paths::app_data_dir()?.join(FILE))
}

/// Read and validate the settings at `path`. Absent means defaults; anything that
/// does not parse or validate is an error naming the file.
pub fn read_at(path: &Path) -> Result<RuntimeSettings, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RuntimeSettings::default())
        }
        Err(e) => return Err(format!("Could not read {}: {e}", path.display())),
    };
    let settings: RuntimeSettings = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not valid: {e}. Fix or remove it.", path.display()))?;
    settings
        .validate()
        .map_err(|e| format!("{} is not valid: {e}. Fix or remove it.", path.display()))?;
    Ok(settings)
}

/// Validate and write via a temp file and rename.
pub fn write_at(path: &Path, settings: &RuntimeSettings) -> Result<(), String> {
    settings.validate()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Could not describe the runtime settings: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("Could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("Could not update {}: {e}", path.display()))
}

/// The app's settings.
pub fn read() -> Result<RuntimeSettings, String> {
    read_at(&settings_path()?)
}

/// Read-modify-write the app's settings, so a UI setter never clobbers fields
/// only the file sets.
pub fn update(f: impl FnOnce(&mut RuntimeSettings)) -> Result<RuntimeSettings, String> {
    let path = settings_path()?;
    let mut s = read_at(&path)?;
    f(&mut s);
    write_at(&path, &s)?;
    Ok(s)
}

/// The modkit-managed prefix for this host: a Proton compat-data dir on Linux, a
/// Wine prefix elsewhere.
pub fn managed_prefix() -> Result<PathBuf, String> {
    let name = if cfg!(target_os = "linux") {
        "proton-prefix"
    } else {
        "wine-prefix"
    };
    Ok(super::paths::app_data_dir()?.join(name))
}

/// The prefix a launch uses: an explicit `arg`, then the settings' `prefix`, then
/// `MERCS2_PREFIX`, then the managed one.
pub fn resolve_prefix(settings: &RuntimeSettings, arg: Option<&str>) -> Result<PathBuf, String> {
    if let Some(p) = arg.or(settings.prefix.as_deref()) {
        return Ok(PathBuf::from(p));
    }
    if let Some(p) = std::env::var_os("MERCS2_PREFIX") {
        return Ok(PathBuf::from(p));
    }
    managed_prefix()
}

/// What the UI shows: the settings and where the file is, so the "I know what
/// I'm doing" fields can be edited by hand.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSettingsView {
    pub path: String,
    pub settings: RuntimeSettings,
}

#[tauri::command(async)]
pub fn get_runtime_settings() -> Result<RuntimeSettingsView, String> {
    let path = settings_path()?;
    Ok(RuntimeSettingsView {
        settings: read_at(&path)?,
        path: path.to_string_lossy().into_owned(),
    })
}

/// Replace the environment list — the one tuning field the UI edits.
#[tauri::command(async)]
pub fn set_runtime_env(env: Vec<EnvVar>) -> Result<RuntimeSettings, String> {
    update(|s| s.env = env)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(k: &str, v: &str) -> EnvVar {
        EnvVar {
            key: k.into(),
            value: v.into(),
        }
    }

    #[test]
    fn a_missing_file_is_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_at(&dir.path().join("runtime.json")).unwrap(),
            RuntimeSettings::default()
        );
    }

    #[test]
    fn settings_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runtime.json");
        let mut s = RuntimeSettings {
            wine_tag: Some("23.7.1-1".into()),
            env: vec![env("WINE_LARGE_ADDRESS_AWARE", "1")],
            exe_args: vec!["-windowed".into()],
            ..Default::default()
        };
        s.dll_overrides.insert("winecoreaudio.drv".into(), "d".into());
        write_at(&path, &s).unwrap();
        assert_eq!(read_at(&path).unwrap(), s);
    }

    /// The file is hand-edited; a typo must fail loudly, not drop the setting.
    #[test]
    fn an_unknown_field_fails_naming_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runtime.json");
        std::fs::write(&path, r#"{ "envv": [] }"#).unwrap();
        let err = read_at(&path).unwrap_err();
        assert!(err.contains("runtime.json"), "{err}");
        assert!(err.contains("envv"), "{err}");
    }

    #[test]
    fn a_future_format_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runtime.json");
        std::fs::write(&path, r#"{ "format": 99 }"#).unwrap();
        assert!(read_at(&path).unwrap_err().contains("newer modkit"));
    }

    #[test]
    fn a_setter_keeps_file_only_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runtime.json");
        std::fs::write(&path, r#"{ "winedebug": "+seh", "exeArgs": ["-x"] }"#).unwrap();
        let mut s = read_at(&path).unwrap();
        s.env = vec![env("A", "1")];
        write_at(&path, &s).unwrap();
        let back = read_at(&path).unwrap();
        assert_eq!(back.winedebug.as_deref(), Some("+seh"));
        assert_eq!(back.exe_args, vec!["-x".to_string()]);
        assert_eq!(back.env, vec![env("A", "1")]);
    }

    #[test]
    fn wine_env_assembles_structured_fields() {
        let mut s = RuntimeSettings {
            env: vec![env("WINE_LARGE_ADDRESS_AWARE", "1")],
            winedebug: Some("+seh".into()),
            ..Default::default()
        };
        s.dll_overrides.insert("winecoreaudio.drv".into(), "d".into());
        s.dll_overrides.insert("d3d9".into(), "n,b".into());
        let got = s.wine_env(&["WINEPREFIX"]).unwrap();
        assert_eq!(
            got,
            vec![
                ("WINE_LARGE_ADDRESS_AWARE".into(), "1".into()),
                ("WINEDLLOVERRIDES".into(), "d3d9=n,b;winecoreaudio.drv=d".into()),
                ("WINEDEBUG".into(), "+seh".into()),
            ]
        );
    }

    #[test]
    fn a_reserved_variable_in_env_is_refused() {
        let s = RuntimeSettings {
            env: vec![env("WINEPREFIX", "/tmp/x")],
            ..Default::default()
        };
        let err = s.wine_env(&["WINEPREFIX", "PMC_VERBOSE_LOG"]).unwrap_err();
        assert!(err.contains("prefix"), "{err}");
    }

    #[test]
    fn the_same_setting_from_two_places_is_refused() {
        let mut s = RuntimeSettings {
            env: vec![env("WINEDLLOVERRIDES", "x=n")],
            ..Default::default()
        };
        s.dll_overrides.insert("y".into(), "b".into());
        assert!(s.validate().unwrap_err().contains("WINEDLLOVERRIDES"));

        let s = RuntimeSettings {
            env: vec![env("WINEDEBUG", "-all")],
            winedebug: Some("+seh".into()),
            ..Default::default()
        };
        assert!(s.validate().unwrap_err().contains("WINEDEBUG"));
    }

    #[test]
    fn bad_env_and_dll_entries_are_refused() {
        let dup = RuntimeSettings {
            env: vec![env("A", "1"), env("A", "2")],
            ..Default::default()
        };
        assert!(dup.validate().unwrap_err().contains("twice"));

        let bad_key = RuntimeSettings {
            env: vec![env("A=B", "1")],
            ..Default::default()
        };
        assert!(bad_key.validate().is_err());

        let mut bad_mode = RuntimeSettings::default();
        bad_mode.dll_overrides.insert("d3d9".into(), "native".into());
        assert!(bad_mode.validate().unwrap_err().contains("load order"));
    }

    #[test]
    fn the_registry_file_groups_by_key_and_escapes() {
        let s = RuntimeSettings {
            registry: vec![
                RegistryValue {
                    key: r"HKEY_CURRENT_USER\Software\Wine\Mac Driver".into(),
                    name: "CaptureDisplaysForFullscreen".into(),
                    value: RegData::String("y".into()),
                },
                RegistryValue {
                    key: r"HKEY_CURRENT_USER\Software\Wine\Direct3D".into(),
                    name: "MaxVersionGL".into(),
                    value: RegData::Dword(0x30002),
                },
                RegistryValue {
                    key: r"HKEY_CURRENT_USER\Software\Wine\Mac Driver".into(),
                    name: "Path".into(),
                    value: RegData::String(r#"C:\a "b""#.into()),
                },
                RegistryValue {
                    key: r"HKEY_CURRENT_USER\Software\Wine\Direct3D".into(),
                    name: "".into(),
                    value: RegData::Delete,
                },
            ],
            ..Default::default()
        };
        s.validate().unwrap();
        assert_eq!(
            s.registry_file(),
            "REGEDIT4\r\n\
             \r\n[HKEY_CURRENT_USER\\Software\\Wine\\Mac Driver]\r\n\
             \"CaptureDisplaysForFullscreen\"=\"y\"\r\n\
             \"Path\"=\"C:\\\\a \\\"b\\\"\"\r\n\
             \r\n[HKEY_CURRENT_USER\\Software\\Wine\\Direct3D]\r\n\
             \"MaxVersionGL\"=dword:00030002\r\n\
             @=-\r\n"
        );
    }

    #[test]
    fn a_registry_key_outside_hkcu_and_hklm_is_refused() {
        let s = RuntimeSettings {
            registry: vec![RegistryValue {
                key: r"Software\Wine".into(),
                name: "x".into(),
                value: RegData::Dword(1),
            }],
            ..Default::default()
        };
        assert!(s.validate().unwrap_err().contains("HKEY_CURRENT_USER"));
    }

    #[test]
    fn registry_values_parse_from_json() {
        let s: RuntimeSettings = serde_json::from_str(
            r#"{ "registry": [
                { "key": "HKEY_CURRENT_USER\\Software\\Wine", "name": "a", "value": { "type": "string", "data": "y" } },
                { "key": "HKEY_CURRENT_USER\\Software\\Wine", "name": "b", "value": { "type": "dword", "data": 1 } },
                { "key": "HKEY_CURRENT_USER\\Software\\Wine", "name": "c", "value": { "type": "delete" } }
            ] }"#,
        )
        .unwrap();
        assert_eq!(s.registry[1].value, RegData::Dword(1));
        assert_eq!(s.registry[2].value, RegData::Delete);
    }

    #[test]
    fn an_explicit_prefix_wins_over_the_setting() {
        let s = RuntimeSettings {
            prefix: Some("/from/settings".into()),
            ..Default::default()
        };
        assert_eq!(resolve_prefix(&s, Some("/arg")).unwrap(), PathBuf::from("/arg"));
        assert_eq!(
            resolve_prefix(&s, None).unwrap(),
            PathBuf::from("/from/settings")
        );
    }
}
