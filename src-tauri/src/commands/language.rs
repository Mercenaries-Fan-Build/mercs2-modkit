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
use std::time::{SystemTime, UNIX_EPOCH};

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

    Ok(LanguageStatus {
        data_dir: data_dir.map(|d| d.to_string_lossy().to_string()),
        audio_dir: audio_dir.map(|d| d.to_string_lossy().to_string()),
        languages,
        present_count,
    })
}

/// Keep `language` and move every other language's `.wad`/`.pws` to the trash.
/// Refuses if the chosen language has no content on disk (that would leave the
/// game with no VO/text).
#[tauri::command]
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

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Move `src` into the trash dir under a timestamped name (so re-runs never
/// clobber a prior copy). Falls back to copy+remove across volumes.
fn move_to_trash(src: &Path, trash: &Path, seq: usize) -> Result<(), String> {
    let name = src
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let dest = trash.join(format!("{}-{}-{}", now_millis(), seq, name));
    if std::fs::rename(src, &dest).is_err() {
        std::fs::copy(src, &dest)
            .map_err(|e| format!("Failed to move {name} to trash: {e}"))?;
        std::fs::remove_file(src).map_err(|e| format!("Failed to remove {name}: {e}"))?;
    }
    Ok(())
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
