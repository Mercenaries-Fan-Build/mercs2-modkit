//! Language manager — keep one language's content and move the rest to the
//! recoverable trash.
//!
//! Each Mercenaries 2 locale ships its own VO/text as a `<Lang>.wad` plus an
//! `Audios/vo_stream.<lang>.pws` audio stream, and a stock install excludes the
//! other languages (see `docs/mercs2_install_registry_contract.md` §4). A copy
//! assembled from several SKUs can end up with multiple language sets — wasted
//! disk and ambiguous VO. This lets the user pick one language to keep; every
//! other language's `.wad`/`.pws` is moved to the modkit trash (recoverable),
//! never hard-deleted.
//!
//! We can only keep/drop content that's already on disk — we have no source for
//! a language the install never shipped.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::commands::paths::trash_dir;

/// One supported language and the on-disk names of its content files.
struct LangSpec {
    /// Display name, also the WAD basename stem (e.g. `English` → `English.wad`).
    name: &'static str,
    /// Lowercase token in the audio stream name (`vo_stream.<token>.pws`).
    token: &'static str,
    /// Locales that select this language (informational, from §4).
    locales: &'static [&'static str],
}

const LANGUAGES: &[LangSpec] = &[
    LangSpec { name: "English", token: "english", locales: &["en_US", "en_GB"] },
    LangSpec { name: "German", token: "german", locales: &["de_DE"] },
    LangSpec { name: "Spanish", token: "spanish", locales: &["es_ES"] },
    LangSpec { name: "French", token: "french", locales: &["fr_FR"] },
    LangSpec { name: "Italian", token: "italian", locales: &["it_IT"] },
    LangSpec { name: "Russian", token: "russian", locales: &["ru_RU"] },
];

impl LangSpec {
    fn wad_name(&self) -> String {
        format!("{}.wad", self.name)
    }
    fn pws_name(&self) -> String {
        format!("vo_stream.{}.pws", self.token)
    }
}

/// Presence/size of one language's content in the install.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguagePresence {
    pub language: String,
    pub locales: Vec<String>,
    /// Expected WAD filename (`<Lang>.wad`).
    pub wad_name: String,
    pub wad_present: bool,
    pub wad_size: u64,
    /// Expected audio stream filename (`vo_stream.<lang>.pws`).
    pub pws_name: String,
    pub pws_present: bool,
    pub pws_size: u64,
}

impl LanguagePresence {
    /// Any content for this language is on disk.
    fn present(&self) -> bool {
        self.wad_present || self.pws_present
    }
}

/// What languages the install currently carries.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageStatus {
    /// Folder holding the language WADs (`data/`, or the root as a fallback).
    pub data_dir: Option<String>,
    /// Folder holding the `.pws` streams (`data/Audios/`), if it exists.
    pub audio_dir: Option<String>,
    pub languages: Vec<LanguagePresence>,
    /// How many languages have any content present.
    pub present_count: usize,
    /// NOVEL languages installed into `data/<name>.wad` that the base game never shipped — added by
    /// a language Shipment, selectable via the `mercs2_language` plugin.
    pub added: Vec<AddedLanguage>,
    /// State of the language-selector plugin that switches the game into an added language.
    pub selector: SelectorStatus,
}

/// A NOVEL language installed as `data/<name>.wad` — one the base game never shipped, placed by an
/// `add_language` Shipment. Distinct from the shipped languages above, which the game already carries.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddedLanguage {
    /// The language token — the WAD basename (`polski`), which is also what the selector forces the
    /// game's language index to resolve to at boot.
    pub name: String,
    /// A friendlier label for the UI (the token, title-cased).
    pub display: String,
    pub wad_name: String,
    pub wad_size: u64,
    /// True when the selector is enabled AND currently names this language.
    pub active: bool,
}

/// The state of the `mercs2_language` selector plugin and its config, read from `scripts/`.
///
/// The game has no in-game language picker (it chooses at boot from OS-locale), so a novel language is
/// only reachable through this plugin. Its config (`mercs2_language.ini`) is what modkit rewrites to
/// switch languages.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectorStatus {
    /// `scripts/mercs2_language.asi` is installed.
    pub plugin_installed: bool,
    /// The config currently forces a language override on (`enabled = true`).
    pub enabled: bool,
    /// The language the config selects (`name =`), whether or not it is enabled.
    pub active: Option<String>,
    /// The config is in dry-run — the plugin logs its intent but applies nothing.
    pub dry_run: bool,
}

/// Result of selecting or clearing an added language.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetAddedLanguageResult {
    /// The language now selected, or `None` when the override was cleared.
    pub name: Option<String>,
    /// The config file that was written.
    pub ini_path: String,
    pub enabled: bool,
}

/// Result of keeping one language and trashing the others.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetLanguageResult {
    /// The language kept.
    pub kept: String,
    /// Basenames moved to the trash.
    pub removed: Vec<String>,
    /// Bytes reclaimed from the install.
    pub freed_bytes: u64,
    /// Where files were moved (the recoverable trash dir).
    pub trash_dir: Option<String>,
}

/// Pick the folder holding WADs: prefer `data/`, else the install root.
fn find_data_dir(root: &Path) -> Option<PathBuf> {
    let data = root.join("data");
    if data.is_dir() {
        Some(data)
    } else if root.is_dir() {
        Some(root.to_path_buf())
    } else {
        None
    }
}

/// The `Audios` subfolder under the data dir, case-insensitively, if present.
fn find_audio_dir(data_dir: &Path) -> Option<PathBuf> {
    find_child(data_dir, "audios").filter(|p| p.is_dir())
}

/// Resolve a child entry by case-insensitive name, returning its real path
/// (preserves on-disk casing on case-sensitive filesystems, e.g. Linux/Wine).
fn find_child(dir: &Path, name_lower: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        if e.file_name().to_string_lossy().eq_ignore_ascii_case(name_lower) {
            return Some(e.path());
        }
    }
    None
}

/// `(path, size)` for a file in `dir` matching `name` case-insensitively.
fn locate(dir: Option<&Path>, name: &str) -> (Option<PathBuf>, u64) {
    let Some(dir) = dir else { return (None, 0) };
    match find_child(dir, &name.to_ascii_lowercase()) {
        Some(p) if p.is_file() => {
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            (Some(p), size)
        }
        _ => (None, 0),
    }
}

/// Scan the install and report which languages' content is present.
#[tauri::command(async)]
pub fn scan_languages(game_root: String) -> Result<LanguageStatus, String> {
    let root = PathBuf::from(&game_root);
    if !root.is_dir() {
        return Err(format!("Game folder not found: {game_root}"));
    }
    let data_dir = find_data_dir(&root);
    let audio_dir = data_dir.as_deref().and_then(find_audio_dir);

    let languages: Vec<LanguagePresence> = LANGUAGES
        .iter()
        .map(|spec| {
            let (wad, wad_size) = locate(data_dir.as_deref(), &spec.wad_name());
            let (pws, pws_size) = locate(audio_dir.as_deref(), &spec.pws_name());
            LanguagePresence {
                language: spec.name.to_string(),
                locales: spec.locales.iter().map(|s| s.to_string()).collect(),
                wad_name: spec.wad_name(),
                wad_present: wad.is_some(),
                wad_size,
                pws_name: spec.pws_name(),
                pws_present: pws.is_some(),
                pws_size,
            }
        })
        .collect();

    let present_count = languages.iter().filter(|l| l.present()).count();

    // Added (novel) languages, plus the selector plugin's current state. An added language is `active`
    // only when the selector is enabled AND names it.
    let selector = read_selector(&root);
    let effective_active = if selector.enabled {
        selector.active.clone()
    } else {
        None
    };
    let added = scan_added_languages(data_dir.as_deref(), effective_active.as_deref());

    Ok(LanguageStatus {
        data_dir: data_dir.map(|d| d.to_string_lossy().to_string()),
        audio_dir: audio_dir.map(|d| d.to_string_lossy().to_string()),
        languages,
        present_count,
        added,
        selector,
    })
}

/// Keep `language` and move every other language's `.wad`/`.pws` to the trash.
/// Refuses if the chosen language has no content on disk (that would leave the
/// game with no VO/text).
#[tauri::command(async)]
pub fn set_language(game_root: String, language: String) -> Result<SetLanguageResult, String> {
    let root = PathBuf::from(&game_root);
    if !root.is_dir() {
        return Err(format!("Game folder not found: {game_root}"));
    }

    let keep = LANGUAGES
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(&language))
        .ok_or_else(|| format!("Unknown language '{language}'"))?;

    let data_dir = find_data_dir(&root);
    let audio_dir = data_dir.as_deref().and_then(find_audio_dir);

    // Guard: don't strip everything if the language we're keeping isn't here.
    let (keep_wad, _) = locate(data_dir.as_deref(), &keep.wad_name());
    let (keep_pws, _) = locate(audio_dir.as_deref(), &keep.pws_name());
    if keep_wad.is_none() && keep_pws.is_none() {
        return Err(format!(
            "{} isn't installed (no {} or {} found) — refusing to remove the other \
             languages, which would leave the game with no language content.",
            keep.name,
            keep.wad_name(),
            keep.pws_name()
        ));
    }

    let trash = trash_dir()?;
    let mut removed = Vec::new();
    let mut freed_bytes = 0u64;

    for spec in LANGUAGES {
        if spec.name == keep.name {
            continue;
        }
        for (dir, name) in [
            (data_dir.as_deref(), spec.wad_name()),
            (audio_dir.as_deref(), spec.pws_name()),
        ] {
            let (path, size) = locate(dir, &name);
            if let Some(p) = path {
                move_to_trash(&p, &trash, removed.len())?;
                freed_bytes += size;
                removed.push(name);
            }
        }
    }

    removed.sort();
    Ok(SetLanguageResult {
        kept: keep.name.to_string(),
        removed,
        freed_bytes,
        trash_dir: Some(trash.to_string_lossy().to_string()),
    })
}

/// Move `src` into the trash dir under a timestamped name.
///
/// Timestamped rather than content-addressed on purpose: a language pack is a
/// `.wad` plus a VO stream, and hashing hundreds of megabytes on the way to the
/// trash would buy only a dedupe nobody needs here — the user drops each language
/// once. See [`crate::commands::managed::trash`].
fn move_to_trash(src: &Path, trash: &Path, _seq: usize) -> Result<(), String> {
    crate::commands::managed::trash::discard(src, Some(trash)).map(|_| ())
}

// ---------------------------------------------------------------------------
// Added (novel) languages + the mercs2_language selector
// ---------------------------------------------------------------------------

/// WAD basenames that are NEVER an added language: the base WADs and the shipped-language WADs. A
/// `<name>.wad` in `data/` that is none of these (and not a `-patch`) is a novel language a Shipment
/// added.
const RESERVED_WAD_STEMS: &[&str] = &[
    "vz", "shell", "loading", "english", "german", "spanish", "french", "italian", "russian",
    "japanese",
];

fn is_reserved_wad_stem(stem_lower: &str) -> bool {
    RESERVED_WAD_STEMS.contains(&stem_lower)
        || stem_lower.contains("-patch")
        || stem_lower.contains(" - copy")
}

/// Title-case a single language token for display (`polski` -> `Polski`).
fn title_case(token: &str) -> String {
    let mut chars = token.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `"1"/"true"/"on"/"yes"` -> true, anything else false.
fn ini_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "on" | "yes"
    )
}

/// The `scripts/` folder under the game root, case-insensitively.
fn find_scripts_dir(root: &Path) -> Option<PathBuf> {
    find_child(root, "scripts").filter(|p| p.is_dir())
}

/// Scan `data/` for novel-language WADs — a `<name>.wad` the base game never shipped. `active`
/// (already resolved to `enabled ? name : None` by the caller) marks the one the selector forces.
fn scan_added_languages(data_dir: Option<&Path>, active: Option<&str>) -> Vec<AddedLanguage> {
    let Some(dir) = data_dir else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        if !path.is_file() {
            continue;
        }
        let file = e.file_name().to_string_lossy().to_string();
        let lower = file.to_ascii_lowercase();
        let Some(stem) = lower.strip_suffix(".wad") else {
            continue;
        };
        if is_reserved_wad_stem(stem) {
            continue;
        }
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        out.push(AddedLanguage {
            display: title_case(stem),
            active: active.is_some_and(|a| a.eq_ignore_ascii_case(stem)),
            wad_name: format!("{stem}.wad"),
            wad_size: size,
            name: stem.to_string(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Read `scripts/mercs2_language.ini`, and whether the plugin itself is installed.
fn read_selector(root: &Path) -> SelectorStatus {
    let mut status = SelectorStatus::default();
    let Some(scripts) = find_scripts_dir(root) else {
        return status;
    };
    status.plugin_installed = find_child(&scripts, "mercs2_language.asi")
        .map(|p| p.is_file())
        .unwrap_or(false);
    if let Some(ini) = find_child(&scripts, "mercs2_language.ini").filter(|p| p.is_file()) {
        if let Ok(text) = std::fs::read_to_string(&ini) {
            for line in text.lines() {
                let t = line.trim();
                if t.is_empty() || t.starts_with('[') || t.starts_with(';') || t.starts_with('#') {
                    continue;
                }
                let Some((k, v)) = t.split_once('=') else {
                    continue;
                };
                match k.trim().to_ascii_lowercase().as_str() {
                    "enabled" => status.enabled = ini_bool(v),
                    "dry_run" => status.dry_run = ini_bool(v),
                    "name" => {
                        let v = v.trim();
                        if !v.is_empty() {
                            status.active = Some(v.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    status
}

/// Atomic write: stage a sibling `.part` and rename into place.
fn write_atomic(dest: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut tmp = dest.as_os_str().to_os_string();
    tmp.push(".part");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, dest).map_err(|e| format!("installing {}: {e}", dest.display()))?;
    Ok(())
}

/// The selector config the plugin reads at boot: force `name`, or a disabled default.
fn selector_ini(name: Option<&str>) -> String {
    match name {
        Some(name) => format!(
            "; mercs2_language.asi — written by Modkit's language selector.\n\
             [language]\n\
             enabled = true\n\
             dry_run = false\n\
             index = 7\n\
             name = {name}\n\
             script = latin\n"
        ),
        None => "; mercs2_language.asi — disabled by Modkit's language selector.\n\
                 [language]\n\
                 enabled = false\n\
                 dry_run = true\n\
                 index = 7\n\
                 script = latin\n"
            .to_string(),
    }
}

/// Switch the game into an added language: write the selector config so the plugin forces `name` at
/// the next launch. Refuses a language whose WAD is not installed, or when the plugin is absent
/// (nothing would read the config).
#[tauri::command(async)]
pub fn set_added_language(
    game_root: String,
    name: String,
) -> Result<SetAddedLanguageResult, String> {
    let root = PathBuf::from(&game_root);
    if !root.is_dir() {
        return Err(format!("Game folder not found: {game_root}"));
    }
    let scripts = find_scripts_dir(&root).ok_or_else(|| {
        "The language selector plugin is not installed (no scripts/ folder). Install the language \
         Shipment first."
            .to_string()
    })?;
    if !find_child(&scripts, "mercs2_language.asi")
        .map(|p| p.is_file())
        .unwrap_or(false)
    {
        return Err("The language selector plugin (mercs2_language.asi) is not installed, so \
                    nothing would read the selection."
            .to_string());
    }
    let token = name.trim().to_ascii_lowercase();
    let data_dir = find_data_dir(&root);
    let installed = data_dir
        .as_deref()
        .and_then(|d| find_child(d, &format!("{token}.wad")))
        .map(|p| p.is_file())
        .unwrap_or(false);
    if !installed {
        return Err(format!("{name} is not installed — no data/{token}.wad found."));
    }
    let ini = scripts.join("mercs2_language.ini");
    write_atomic(&ini, selector_ini(Some(&token)).as_bytes())?;
    Ok(SetAddedLanguageResult {
        name: Some(token),
        ini_path: ini.to_string_lossy().to_string(),
        enabled: true,
    })
}

/// Clear the override: disable the selector so the game uses its normal boot language.
#[tauri::command(async)]
pub fn clear_added_language(game_root: String) -> Result<SetAddedLanguageResult, String> {
    let root = PathBuf::from(&game_root);
    if !root.is_dir() {
        return Err(format!("Game folder not found: {game_root}"));
    }
    let Some(scripts) = find_scripts_dir(&root) else {
        return Ok(SetAddedLanguageResult {
            name: None,
            ini_path: String::new(),
            enabled: false,
        });
    };
    let ini = scripts.join("mercs2_language.ini");
    if ini.is_file() {
        write_atomic(&ini, selector_ini(None).as_bytes())?;
    }
    Ok(SetAddedLanguageResult {
        name: None,
        ini_path: ini.to_string_lossy().to_string(),
        enabled: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_file_names() {
        let en = &LANGUAGES[0];
        assert_eq!(en.wad_name(), "English.wad");
        assert_eq!(en.pws_name(), "vo_stream.english.pws");
        let ru = LANGUAGES.iter().find(|l| l.name == "Russian").unwrap();
        assert_eq!(ru.wad_name(), "Russian.wad");
        assert_eq!(ru.pws_name(), "vo_stream.russian.pws");
    }

    #[test]
    fn scan_finds_present_languages_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let data = root.join("data");
        let audio = data.join("Audios");
        std::fs::create_dir_all(&audio).unwrap();
        // English present (lowercase wad name to exercise case-insensitive match);
        // German wad present, no pws.
        std::fs::write(data.join("english.wad"), b"en").unwrap();
        std::fs::write(audio.join("vo_stream.english.pws"), b"envo").unwrap();
        std::fs::write(data.join("German.wad"), b"de").unwrap();

        let status = scan_languages(root.to_string_lossy().to_string()).unwrap();
        assert_eq!(status.present_count, 2);
        let en = status.languages.iter().find(|l| l.language == "English").unwrap();
        assert!(en.wad_present && en.pws_present);
        assert_eq!(en.wad_size, 2);
        let de = status.languages.iter().find(|l| l.language == "German").unwrap();
        assert!(de.wad_present && !de.pws_present);
        let fr = status.languages.iter().find(|l| l.language == "French").unwrap();
        assert!(!fr.wad_present && !fr.pws_present);
    }

    #[test]
    fn set_language_keeps_one_trashes_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let data = root.join("data");
        let audio = data.join("Audios");
        std::fs::create_dir_all(&audio).unwrap();
        std::fs::write(data.join("English.wad"), b"en").unwrap();
        std::fs::write(audio.join("vo_stream.english.pws"), b"envo").unwrap();
        std::fs::write(data.join("German.wad"), b"de").unwrap();
        std::fs::write(audio.join("vo_stream.german.pws"), b"devo").unwrap();

        let res = set_language(
            root.to_string_lossy().to_string(),
            "English".to_string(),
        )
        .unwrap();
        assert_eq!(res.kept, "English");
        assert_eq!(res.removed, vec!["German.wad", "vo_stream.german.pws"]);
        assert_eq!(res.freed_bytes, 6); // 2 + 4
        // Kept files stay, German files are gone from the install.
        assert!(data.join("English.wad").is_file());
        assert!(audio.join("vo_stream.english.pws").is_file());
        assert!(!data.join("German.wad").exists());
        assert!(!audio.join("vo_stream.german.pws").exists());
    }

    #[test]
    fn set_language_refuses_when_kept_language_absent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let data = root.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("German.wad"), b"de").unwrap();

        // Keeping French, which isn't installed, must error and remove nothing.
        let err = set_language(root.to_string_lossy().to_string(), "French".to_string())
            .unwrap_err();
        assert!(err.contains("French isn't installed"));
        assert!(data.join("German.wad").is_file());
    }
}
