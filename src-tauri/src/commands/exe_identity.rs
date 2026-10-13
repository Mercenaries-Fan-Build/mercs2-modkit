//! Which executables an install holds, identified by content.
//!
//! # Identity is the sha256, never the size
//!
//! Three catalogued builds weigh exactly 53,482,288 bytes: the `cruise.dll` crack,
//! the `pmc_bb.dll` crack, and the DRM-free rebuild of the patched exe. Size
//! classification called all three "cracked", and modkit used to badge, launch,
//! and pick a pmc_bb build from that. Every exe is now hashed and matched against
//! the catalogue ([`super::verify::exe_catalog`]). An exe whose digest is not
//! catalogued is reported as unrecognised; nothing is guessed from its size.
//!
//! # Hashed once per change
//!
//! An exe is ~50 MB, and detection runs on every refresh and before every launch.
//! Digests are cached in `<app-data>/exe-hashes.json`, keyed by absolute path, and
//! reused only while the file's size and modification time are unchanged. The
//! cache is written via a temp file and a rename, so a file that does not parse
//! was not written by modkit, and is reported as an error naming it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::commands::managed::place::sha256_of_file;
use crate::commands::managed::pmc_bb::ExeKind;
use crate::commands::verify::ExeEntry;

/// Bumped only for a change an older modkit cannot read.
const FORMAT: u32 = 1;

/// One executable, identified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExeIdentity {
    pub path: String,
    /// File name as it is on disk.
    pub name: String,
    pub size: u64,
    pub sha256: String,
    /// Catalogue id ([`crate::commands::verify::EXE_IDS`]); `None` when the digest
    /// is not catalogued.
    pub id: Option<String>,
    pub version: Option<String>,
    pub variant: Option<String>,
    pub label: Option<String>,
    /// Sidecar DLL the exe imports and needs present to start.
    pub requires: Option<String>,
    pub kind: ExeKind,
}

impl ExeIdentity {
    /// Whether the catalogue knows this build.
    pub fn recognised(&self) -> bool {
        self.id.is_some()
    }
}

/// Whether `name` is one of the game's executables: `Mercenaries2*.exe`.
pub fn is_game_exe_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("mercenaries2") && lower.ends_with(".exe")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CacheEntry {
    size: u64,
    mtime_secs: u64,
    mtime_nanos: u32,
    sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CacheFile {
    format: u32,
    entries: BTreeMap<String, CacheEntry>,
}

/// The digest cache, held for one scan and written back once.
pub struct HashCache {
    path: PathBuf,
    entries: BTreeMap<String, CacheEntry>,
    dirty: bool,
}

impl HashCache {
    /// The app's cache.
    pub fn app() -> Result<Self, String> {
        Self::open(&super::paths::app_data_dir()?.join("exe-hashes.json"))
    }

    /// Open the cache at `path`. Absent means empty.
    pub fn open(path: &Path) -> Result<Self, String> {
        let entries = match std::fs::read_to_string(path) {
            Ok(text) => {
                let file: CacheFile = serde_json::from_str(&text).map_err(|e| {
                    format!("{} is not valid: {e}. Delete it and modkit will rebuild it.", path.display())
                })?;
                if file.format > FORMAT {
                    return Err(format!(
                        "{} was written by a newer modkit (format {}, this one reads up to {FORMAT}). \
                         Delete it and modkit will rebuild it.",
                        path.display(),
                        file.format
                    ));
                }
                file.entries
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(format!("Could not read {}: {e}", path.display())),
        };
        Ok(Self { path: path.to_path_buf(), entries, dirty: false })
    }

    /// sha256 of `path`, from the cache while size and mtime are unchanged.
    fn sha256(&mut self, path: &Path) -> Result<(u64, String), String> {
        let meta = std::fs::metadata(path)
            .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        let modified = meta
            .modified()
            .map_err(|e| format!("Could not read the modification time of {}: {e}", path.display()))?
            .duration_since(UNIX_EPOCH)
            .map_err(|e| format!("{} has a modification time before 1970: {e}", path.display()))?;
        let key = path.to_string_lossy().to_string();
        let fresh = CacheEntry {
            size: meta.len(),
            mtime_secs: modified.as_secs(),
            mtime_nanos: modified.subsec_nanos(),
            sha256: String::new(),
        };
        if let Some(hit) = self.entries.get(&key) {
            if hit.size == fresh.size
                && hit.mtime_secs == fresh.mtime_secs
                && hit.mtime_nanos == fresh.mtime_nanos
            {
                return Ok((hit.size, hit.sha256.clone()));
            }
        }
        let sha256 = sha256_of_file(path)?;
        self.entries.insert(key, CacheEntry { sha256: sha256.clone(), ..fresh });
        self.dirty = true;
        Ok((meta.len(), sha256))
    }

    /// Write the cache back, dropping entries for files that no longer exist.
    pub fn save(mut self) -> Result<(), String> {
        let before = self.entries.len();
        self.entries.retain(|p, _| Path::new(p).is_file());
        if !self.dirty && self.entries.len() == before {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(&CacheFile { format: FORMAT, entries: self.entries })
            .map_err(|e| format!("Could not describe the exe digest cache: {e}"))?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("Could not write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| format!("Could not update {}: {e}", self.path.display()))
    }
}

/// Identify one executable against `catalog`.
pub fn identify(path: &Path, catalog: &[ExeEntry], cache: &mut HashCache) -> Result<ExeIdentity, String> {
    let (size, sha256) = cache.sha256(path)?;
    let entry = catalog.iter().find(|e| e.hash.eq_ignore_ascii_case(&sha256));
    let id = entry.and_then(|e| e.id.clone());
    Ok(ExeIdentity {
        path: path.to_string_lossy().to_string(),
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        size,
        sha256,
        kind: ExeKind::from_exe_id(id.as_deref()),
        id,
        version: entry.map(|e| e.version.clone()),
        variant: entry.map(|e| e.variant.clone()),
        label: entry.and_then(|e| e.label.clone()),
        requires: entry.and_then(|e| e.requires.clone()),
    })
}

/// Every `Mercenaries2*.exe` directly in `root`, identified, sorted by file name.
pub fn scan(root: &Path, catalog: &[ExeEntry], cache: &mut HashCache) -> Result<Vec<ExeIdentity>, String> {
    let entries = std::fs::read_dir(root)
        .map_err(|e| format!("Could not list {}: {e}", root.display()))?;
    let mut paths = Vec::new();
    for e in entries {
        let e = e.map_err(|e| format!("Could not list {}: {e}", root.display()))?;
        let p = e.path();
        let is_exe = p.file_name().and_then(|n| n.to_str()).is_some_and(is_game_exe_name);
        if is_exe && p.is_file() {
            paths.push(p);
        }
    }
    paths.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()));
    paths.iter().map(|p| identify(p, catalog, cache)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::managed::place::sha256_hex;

    fn entry(id: &str, body: &[u8]) -> ExeEntry {
        ExeEntry {
            id: Some(id.into()),
            version: "v1.1".into(),
            variant: "cracked".into(),
            label: None,
            requires: Some("pmc_bb.dll".into()),
            note: None,
            size: body.len() as u64,
            hash: sha256_hex(body),
        }
    }

    #[test]
    fn exes_are_identified_by_digest_and_listed_by_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Mercenaries2.exe"), b"stock").unwrap();
        std::fs::write(dir.path().join("Mercenaries2.cracked.exe"), b"pmcbb").unwrap();
        std::fs::write(dir.path().join("readme.exe"), b"other").unwrap();
        let catalog = [entry("v11-cracked-pmcbb", b"pmcbb")];
        let mut cache = HashCache::open(&dir.path().join("cache.json")).unwrap();

        let exes = scan(dir.path(), &catalog, &mut cache).unwrap();
        let names: Vec<_> = exes.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Mercenaries2.cracked.exe", "Mercenaries2.exe"]);
        assert_eq!(exes[0].id.as_deref(), Some("v11-cracked-pmcbb"));
        assert_eq!(exes[0].kind, ExeKind::CrackedImportingPmcBb);
        assert!(!exes[1].recognised());
        assert_eq!(exes[1].kind, ExeKind::Unknown);
    }

    /// Same size, different bytes: size must never decide identity.
    #[test]
    fn a_same_sized_exe_with_other_bytes_is_unrecognised() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("Mercenaries2.exe");
        std::fs::write(&exe, b"AAAAA").unwrap();
        let catalog = [entry("v11-cracked-pmcbb", b"BBBBB")];
        let mut cache = HashCache::open(&dir.path().join("cache.json")).unwrap();
        let id = identify(&exe, &catalog, &mut cache).unwrap();
        assert_eq!(id.size, 5);
        assert!(!id.recognised());
    }

    #[test]
    fn the_cache_is_reused_until_the_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("Mercenaries2.exe");
        let cache_path = dir.path().join("cache.json");
        std::fs::write(&exe, b"one").unwrap();

        let mut cache = HashCache::open(&cache_path).unwrap();
        let first = identify(&exe, &[], &mut cache).unwrap();
        cache.save().unwrap();

        // Poison the cached digest: a reread that hits the cache returns it.
        let text = std::fs::read_to_string(&cache_path).unwrap();
        std::fs::write(&cache_path, text.replace(&first.sha256, "cached")).unwrap();
        let mut cache = HashCache::open(&cache_path).unwrap();
        assert_eq!(identify(&exe, &[], &mut cache).unwrap().sha256, "cached");

        // A different size invalidates the entry.
        std::fs::write(&exe, b"three").unwrap();
        let mut cache = HashCache::open(&cache_path).unwrap();
        assert_eq!(identify(&exe, &[], &mut cache).unwrap().sha256, sha256_hex(b"three"));
    }

    #[test]
    fn entries_for_deleted_files_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("Mercenaries2.exe");
        let cache_path = dir.path().join("cache.json");
        std::fs::write(&exe, b"one").unwrap();
        let mut cache = HashCache::open(&cache_path).unwrap();
        identify(&exe, &[], &mut cache).unwrap();
        cache.save().unwrap();

        std::fs::remove_file(&exe).unwrap();
        HashCache::open(&cache_path).unwrap().save().unwrap();
        let text = std::fs::read_to_string(&cache_path).unwrap();
        assert!(!text.contains("Mercenaries2.exe"), "{text}");
    }

    #[test]
    fn a_corrupt_cache_is_an_error_naming_it() {
        let dir = tempfile::tempdir().unwrap();
        let cache_path = dir.path().join("cache.json");
        std::fs::write(&cache_path, b"{ nope").unwrap();
        let err = HashCache::open(&cache_path).err().expect("refused");
        assert!(err.contains("cache.json"), "{err}");
    }
}
