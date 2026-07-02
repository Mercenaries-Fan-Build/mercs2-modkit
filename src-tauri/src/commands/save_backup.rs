//! Player save-game backups.
//!
//! Mercenaries 2 writes `<Character>_<HEXID>.profile` files (autosaves:
//! `auto_<HEXID>.profile`) under `Documents\My Games\Mercenaries 2\SaveGames`
//! and overwrites the autosave slot constantly from mission Lua, so a bad exit
//! or a misbehaving mod can clobber a playthrough. Modkit snapshots the whole
//! folder into its app-data dir — automatically before every launch, and on
//! demand — with content dedup and a bounded history, and can restore any
//! snapshot (taking a safety snapshot of the current saves first).
//!
//! Save header facts (offsets from the community EXE/format reverse
//! engineering, validated against retail saves): u32 checksum @0x00, u32
//! play-seconds @0x14, u32 cash @0x18, u32 unix timestamp @0x24, ASCII
//! last-mission @0x2C..0x3C. The zlib-Lua payload starts at 0x468, so anything
//! shorter is not a valid save. The character name is NOT in the fixed header
//! (0x20A holds an internal autosave-slot reference) — it comes from the file
//! name, `<Character>_<HEXID>.profile`.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};

use crate::commands::paths;

/// Minimum valid `.profile` size: the fixed header before the zlib payload.
const HEADER_LEN: usize = 0x468;
/// Snapshots kept before the oldest are pruned (~13 KB per save, so cheap).
const MAX_BACKUPS: usize = 30;
/// Metadata file written inside each snapshot directory.
const META_FILE: &str = "meta.json";

/// One `.profile` file, with what its header says about the playthrough.
#[derive(Debug, Serialize)]
pub struct SaveFileInfo {
    pub file_name: String,
    pub size: u64,
    /// Filesystem mtime (unix seconds), 0 if unavailable.
    pub modified_unix: u64,
    /// True for the game's rolling `auto_*` slot.
    pub autosave: bool,
    /// Character name from the file name (`<Character>_<HEXID>.profile`);
    /// None for autosaves, whose file names carry no character.
    pub character: Option<String>,
    /// Header fields; all None when the file is too short to be a real save.
    pub cash: Option<u32>,
    pub playtime_seconds: Option<u32>,
    pub saved_at_unix: Option<u32>,
    pub last_mission: Option<String>,
}

/// The live SaveGames folder and its contents.
#[derive(Debug, Serialize)]
pub struct SavesInfo {
    /// Resolved SaveGames path (present even when it doesn't exist yet).
    pub dir: Option<String>,
    /// True when `dir` comes from the user's saved override, not autodetection.
    pub overridden: bool,
    pub exists: bool,
    pub saves: Vec<SaveFileInfo>,
}

/// Per-file entry in a snapshot's metadata.
#[derive(Debug, Serialize, Deserialize)]
struct MetaFileEntry {
    name: String,
    size: u64,
    md5: String,
}

/// Snapshot metadata persisted as `meta.json` in the snapshot directory.
#[derive(Debug, Serialize, Deserialize)]
struct BackupMeta {
    reason: String,
    created_unix: u64,
    /// Digest over the sorted (name, md5) pairs — equal digests mean the
    /// snapshot would be byte-identical, so it is skipped.
    digest: String,
    files: Vec<MetaFileEntry>,
}

/// One stored snapshot, summarized for the UI.
#[derive(Debug, Serialize)]
pub struct SaveBackupInfo {
    /// Snapshot directory name; the id used by restore/delete.
    pub id: String,
    pub reason: String,
    pub created_unix: u64,
    pub file_count: usize,
    pub total_bytes: u64,
    /// Character names parsed from the snapshotted saves (deduped).
    pub characters: Vec<String>,
}

/// Result of a snapshot attempt.
#[derive(Debug, Serialize)]
pub struct BackupResult {
    /// New snapshot id, or None if nothing was written.
    pub id: Option<String>,
    /// Why nothing was written ("no saves to back up", "identical to latest").
    pub skipped: Option<String>,
    pub file_count: usize,
}

/// Result of restoring a snapshot over the live SaveGames folder.
#[derive(Debug, Serialize)]
pub struct RestoreResult {
    pub restored: Vec<String>,
    /// Safety snapshot of the pre-restore state, if one was taken.
    pub pre_restore_backup: Option<String>,
}

/// File persisting the user's SaveGames-folder override (absent = autodetect).
fn override_path() -> Result<PathBuf, String> {
    let dir = paths::app_data_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create app data dir: {e}"))?;
    Ok(dir.join("saves_dir_override.txt"))
}

/// The persisted user override, if any.
fn saves_dir_override() -> Option<PathBuf> {
    let text = std::fs::read_to_string(override_path().ok()?).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

/// Resolve the live SaveGames folder: the user's saved override when set,
/// otherwise the game's default location.
fn saves_dir(prefix: Option<&str>) -> Result<PathBuf, String> {
    match saves_dir_override() {
        Some(dir) => Ok(dir),
        None => default_saves_dir(prefix),
    }
}

/// The game's default SaveGames folder. The game hardcodes
/// `\My Games\Mercenaries 2\SaveGames\` under the user's Documents folder; on
/// Linux that lives inside the Proton prefix (same override → env → default
/// layering as the launcher).
fn default_saves_dir(prefix: Option<&str>) -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    {
        let _ = prefix;
        let home = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .ok_or("USERPROFILE is not set")?;
        Ok(home.join("Documents/My Games/Mercenaries 2/SaveGames"))
    }
    #[cfg(target_os = "linux")]
    {
        let compat = prefix
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("MERCS2_PREFIX").map(PathBuf::from))
            .map(Ok)
            .unwrap_or_else(|| paths::app_data_dir().map(|d| d.join("proton-prefix")))?;
        let user = compat.join("pfx/drive_c/users/steamuser");
        // Proton uses `Documents`; older Wine prefixes use `My Documents`.
        let documents = ["Documents", "My Documents"]
            .iter()
            .map(|d| user.join(d))
            .find(|p| p.is_dir())
            .unwrap_or_else(|| user.join("Documents"));
        Ok(documents.join("My Games/Mercenaries 2/SaveGames"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = prefix;
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("HOME is not set")?;
        Ok(home.join("Documents/My Games/Mercenaries 2/SaveGames"))
    }
}

/// Little-endian u32 at `off`, if in bounds.
fn u32_at(data: &[u8], off: usize) -> Option<u32> {
    data.get(off..off + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Parse the display fields out of a save header; None if `data` is too short
/// to be a real save. Returns (cash, playtime, saved_at, mission).
fn parse_header(data: &[u8]) -> Option<(u32, u32, u32, String)> {
    if data.len() < HEADER_LEN {
        return None;
    }
    let playtime = u32_at(data, 0x14)?;
    let cash = u32_at(data, 0x18)?;
    let saved_at = u32_at(data, 0x24)?;
    let mission = data[0x2C..0x3C]
        .split(|&b| b == 0)
        .next()
        .map(|b| String::from_utf8_lossy(b).trim().to_string())
        .unwrap_or_default();
    Some((cash, playtime, saved_at, mission))
}

/// Character name from a save's file name (`<Character>_<HEXID>.profile`) —
/// the stem minus the trailing `_<hex id>`. None for `auto_*` autosaves, whose
/// file names carry no character name.
fn character_from_file_name(file_name: &str) -> Option<String> {
    let stem = file_name.strip_suffix(".profile").unwrap_or(file_name);
    if stem.to_ascii_lowercase().starts_with("auto") {
        return None;
    }
    let Some((character, id)) = stem.rsplit_once('_') else {
        return Some(stem.trim().to_string()); // no hex-id suffix — use the stem
    };
    if character.is_empty() || id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(stem.to_string()); // unexpected shape — show the stem as-is
    }
    Some(character.trim().to_string())
}

/// All `.profile` files in `dir`, newest mtime first.
fn profile_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("profile"))
        })
        .collect();
    files.sort_by_key(|p| {
        std::cmp::Reverse(
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH),
        )
    });
    files
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn mtime_unix(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn file_info(path: &Path) -> SaveFileInfo {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let data = std::fs::read(path).unwrap_or_default();
    let header = parse_header(&data);
    SaveFileInfo {
        autosave: file_name.to_ascii_lowercase().starts_with("auto"),
        size: data.len() as u64,
        modified_unix: mtime_unix(path),
        character: character_from_file_name(&file_name),
        cash: header.as_ref().map(|h| h.0),
        playtime_seconds: header.as_ref().map(|h| h.1),
        saved_at_unix: header.as_ref().map(|h| h.2),
        last_mission: header.map(|h| h.3),
        file_name,
    }
}

/// Copy every `.profile` in `saves` into a new timestamped snapshot under
/// `backups`, unless there is nothing to copy or the content is identical to
/// the most recent snapshot. Prunes the oldest snapshots beyond `MAX_BACKUPS`.
fn backup_into(saves: &Path, backups: &Path, reason: &str) -> Result<BackupResult, String> {
    let files = profile_files(saves);
    if files.is_empty() {
        return Ok(BackupResult {
            id: None,
            skipped: Some("no saves to back up".into()),
            file_count: 0,
        });
    }

    // Fingerprint the current save set to skip byte-identical snapshots.
    let mut entries = Vec::new();
    for f in &files {
        let data = std::fs::read(f).map_err(|e| format!("Failed to read {}: {e}", f.display()))?;
        entries.push((
            f.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            data,
        ));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut set_hash = Md5::new();
    let meta_files: Vec<MetaFileEntry> = entries
        .iter()
        .map(|(name, data)| {
            let md5 = format!("{:x}", Md5::digest(data));
            set_hash.update(name.as_bytes());
            set_hash.update(md5.as_bytes());
            MetaFileEntry {
                name: name.clone(),
                size: data.len() as u64,
                md5,
            }
        })
        .collect();
    let digest = format!("{:x}", set_hash.finalize());

    if let Some(latest) = list_backups_in(backups)?.first() {
        let meta = read_meta(&backups.join(&latest.id));
        if meta.is_some_and(|m| m.digest == digest) {
            return Ok(BackupResult {
                id: None,
                skipped: Some("identical to the latest backup".into()),
                file_count: entries.len(),
            });
        }
    }

    // Zero-padded unix seconds keep lexicographic order == chronological order.
    let created = unix_now();
    let slug: String = reason
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(24)
        .collect();
    let id = format!("{created:012}_{slug}");
    let dir = backups.join(&id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create backup dir: {e}"))?;
    for (name, data) in &entries {
        std::fs::write(dir.join(name), data)
            .map_err(|e| format!("Failed to write backup of {name}: {e}"))?;
    }
    let meta = BackupMeta {
        reason: reason.to_string(),
        created_unix: created,
        digest,
        files: meta_files,
    };
    std::fs::write(
        dir.join(META_FILE),
        serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("Failed to write backup metadata: {e}"))?;

    // Prune the oldest snapshots beyond the cap (ids sort chronologically).
    let mut all = list_backups_in(backups)?;
    while all.len() > MAX_BACKUPS {
        let oldest = all.pop().unwrap();
        let _ = std::fs::remove_dir_all(backups.join(&oldest.id));
    }

    Ok(BackupResult {
        id: Some(id),
        skipped: None,
        file_count: entries.len(),
    })
}

fn read_meta(dir: &Path) -> Option<BackupMeta> {
    let data = std::fs::read(dir.join(META_FILE)).ok()?;
    serde_json::from_slice(&data).ok()
}

/// Snapshots under `backups`, newest first.
fn list_backups_in(backups: &Path) -> Result<Vec<SaveBackupInfo>, String> {
    let Ok(entries) = std::fs::read_dir(backups) else {
        return Ok(Vec::new()); // not created yet
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        let meta = read_meta(&dir);
        let saves = profile_files(&dir);
        let mut characters: Vec<String> = saves
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .filter_map(|n| character_from_file_name(&n))
            .filter(|c| !c.is_empty())
            .collect();
        characters.sort();
        characters.dedup();
        out.push(SaveBackupInfo {
            reason: meta.as_ref().map(|m| m.reason.clone()).unwrap_or_default(),
            created_unix: meta.map(|m| m.created_unix).unwrap_or_else(|| mtime_unix(&dir)),
            file_count: saves.len(),
            total_bytes: saves.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum(),
            characters,
            id,
        });
    }
    out.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(out)
}

/// Reject snapshot ids that could escape the backups directory.
fn checked_backup_dir(id: &str) -> Result<PathBuf, String> {
    if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
        return Err(format!("Invalid backup id: {id}"));
    }
    let dir = paths::save_backups_dir()?.join(id);
    if !dir.is_dir() {
        return Err(format!("Backup not found: {id}"));
    }
    Ok(dir)
}

/// Best-effort pre-launch snapshot, called from `launch_game`. Never blocks a
/// launch: all failures collapse into Err(reason) for optional logging.
pub fn backup_before_launch(prefix: Option<&str>) -> Result<BackupResult, String> {
    let saves = saves_dir(prefix)?;
    backup_into(&saves, &paths::save_backups_dir()?, "pre-launch")
}

/// The live SaveGames folder and per-save header details.
#[tauri::command]
pub fn list_saves(prefix: Option<String>) -> SavesInfo {
    let overridden = saves_dir_override().is_some();
    let Ok(dir) = saves_dir(prefix.as_deref()) else {
        return SavesInfo {
            dir: None,
            overridden,
            exists: false,
            saves: Vec::new(),
        };
    };
    SavesInfo {
        overridden,
        exists: dir.is_dir(),
        saves: profile_files(&dir).iter().map(|p| file_info(p)).collect(),
        dir: Some(dir.to_string_lossy().into_owned()),
    }
}

/// Set (or clear, with None) the SaveGames-folder override. The folder must
/// exist when setting — the picker only returns real directories, so anything
/// else is a stale path.
#[tauri::command]
pub fn set_saves_dir(dir: Option<String>) -> Result<(), String> {
    let path = override_path()?;
    match dir.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        Some(dir) => {
            if !Path::new(dir).is_dir() {
                return Err(format!("Not a folder: {dir}"));
            }
            std::fs::write(&path, dir)
                .map_err(|e| format!("Failed to save the folder override: {e}"))
        }
        None => match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("Failed to clear the folder override: {e}")),
        },
    }
}

/// Snapshot the current saves now (manual "Back up now" button).
#[tauri::command]
pub fn backup_saves(prefix: Option<String>, reason: Option<String>) -> Result<BackupResult, String> {
    let saves = saves_dir(prefix.as_deref())?;
    backup_into(
        &saves,
        &paths::save_backups_dir()?,
        reason.as_deref().unwrap_or("manual"),
    )
}

/// Stored snapshots, newest first.
#[tauri::command]
pub fn list_save_backups() -> Result<Vec<SaveBackupInfo>, String> {
    list_backups_in(&paths::save_backups_dir()?)
}

/// Copy a snapshot's saves back over the live SaveGames folder, snapshotting
/// the current state first so the restore itself is undoable.
#[tauri::command]
pub fn restore_save_backup(id: String, prefix: Option<String>) -> Result<RestoreResult, String> {
    let backup = checked_backup_dir(&id)?;
    let saves = saves_dir(prefix.as_deref())?;
    std::fs::create_dir_all(&saves).map_err(|e| format!("Failed to create SaveGames: {e}"))?;

    let pre = backup_into(&saves, &paths::save_backups_dir()?, "pre-restore")?;

    let mut restored = Vec::new();
    for file in profile_files(&backup) {
        let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
        std::fs::copy(&file, saves.join(&name))
            .map_err(|e| format!("Failed to restore {name}: {e}"))?;
        restored.push(name);
    }
    if restored.is_empty() {
        return Err(format!("Backup {id} contains no save files"));
    }
    Ok(RestoreResult {
        restored,
        pre_restore_backup: pre.id,
    })
}

/// Permanently delete one stored snapshot.
#[tauri::command]
pub fn delete_save_backup(id: String) -> Result<(), String> {
    let dir = checked_backup_dir(&id)?;
    std::fs::remove_dir_all(&dir).map_err(|e| format!("Failed to delete backup {id}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid save: header-sized buffer with known field values.
    fn fake_save(cash: u32, ts: u32) -> Vec<u8> {
        let mut data = vec![0u8; HEADER_LEN + 16];
        data[0x14..0x18].copy_from_slice(&1234u32.to_le_bytes());
        data[0x18..0x1C].copy_from_slice(&cash.to_le_bytes());
        data[0x24..0x28].copy_from_slice(&ts.to_le_bytes());
        data[0x2C..0x33].copy_from_slice(b"AllCon1");
        data
    }

    #[test]
    fn parses_header_fields() {
        let data = fake_save(250_000, 1_600_000_000);
        let (cash, playtime, saved_at, mission) = parse_header(&data).unwrap();
        assert_eq!(cash, 250_000);
        assert_eq!(playtime, 1234);
        assert_eq!(saved_at, 1_600_000_000);
        assert_eq!(mission, "AllCon1");
    }

    #[test]
    fn character_comes_from_file_name() {
        // Real-world shapes from retail saves.
        assert_eq!(
            character_from_file_name("Mattias Nilsson_63430745.profile").as_deref(),
            Some("Mattias Nilsson")
        );
        assert_eq!(character_from_file_name("auto_634304EA.profile"), None);
        // Underscores in the name: only the trailing hex id is stripped.
        assert_eq!(
            character_from_file_name("_______ ________48EFABFB.profile").as_deref(),
            Some("_______ _______")
        );
        // No hex-id suffix — fall back to the stem.
        assert_eq!(
            character_from_file_name("weird name.profile").as_deref(),
            Some("weird name")
        );
    }

    #[test]
    fn rejects_short_files() {
        assert!(parse_header(&[0u8; 64]).is_none());
    }

    #[test]
    fn backup_dedups_and_restores() {
        let tmp = tempfile::tempdir().unwrap();
        let saves = tmp.path().join("SaveGames");
        let backups = tmp.path().join("backups");
        std::fs::create_dir_all(&saves).unwrap();
        std::fs::write(saves.join("Mattias_01.profile"), fake_save(5, 1)).unwrap();

        // First snapshot copies the save; an unchanged second one is skipped.
        let first = backup_into(&saves, &backups, "pre-launch").unwrap();
        assert_eq!(first.file_count, 1);
        let id = first.id.expect("first backup should be written");
        assert!(backups.join(&id).join("Mattias_01.profile").is_file());
        let second = backup_into(&saves, &backups, "pre-launch").unwrap();
        assert!(second.id.is_none(), "identical content should be skipped");

        // Changed content snapshots again.
        std::fs::write(saves.join("Mattias_01.profile"), fake_save(99, 2)).unwrap();
        let third = backup_into(&saves, &backups, "manual").unwrap();
        assert!(third.id.is_some());

        let listed = list_backups_in(&backups).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].characters, vec!["Mattias".to_string()]);
        assert!(listed[0].id > listed[1].id, "newest first");
    }

    #[test]
    fn backup_skips_empty_saves_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let saves = tmp.path().join("SaveGames");
        std::fs::create_dir_all(&saves).unwrap();
        let res = backup_into(&saves, &tmp.path().join("backups"), "pre-launch").unwrap();
        assert!(res.id.is_none());
        assert_eq!(res.skipped.as_deref(), Some("no saves to back up"));
    }
}
