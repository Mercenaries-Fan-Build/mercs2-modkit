//! Install / uninstall a built `vz-patch.wad` into the game, with a recoverable backup.
//!
//! This is the real guard rail. The failure modes a bad patch WAD can produce are ugly and
//! *look* unrecoverable to a player: a mis-sized texture body hangs the world load (a
//! livelock, not a crash), a duplicate material hash crashes only when you look at the
//! object. None of that matters much if the previous WAD is one click away — but it
//! matters enormously if we overwrote it with no copy.
//!
//! So: every deploy snapshots the WAD it is about to replace into the app's trash dir, and
//! `restore_patch_wad` puts it back. Nothing here ever hard-deletes.
//!
//! Everything is verified by **sha256**, never by size or mtime.
//!
//! # The deploy ledger
//!
//! Snapshots answer "what did we displace"; they cannot answer "what is deployed **now**".
//! Nothing did: the [`DeployWadResult`] carrying the installed hash is returned to the
//! frontend and discarded, the build's own `sha256` lives only in unpersisted store state, and
//! `GameInfo.deployed_patches` is filenames. So after a restart modkit knew a patch WAD existed
//! and nothing about which one.
//!
//! That gap makes it impossible to tell whether a `pmc_blackbox.log` came from the setup the
//! user currently has — the log records the WAD attribution it loaded, but there was nothing on
//! this side to compare it against. [`DeployedWadRecord`] is that missing left-hand side: one
//! durable row, rewritten on every deploy and restore, deleted when the patch is uninstalled.
//! **Absent is a meaningful state** ("no patch is deployed"), which is why it is a file that
//! gets removed rather than a row that gets blanked.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::commands::paths::deployed_dir;

const PATCH_NAME: &str = "vz-patch.wad";

/// A snapshot of a previously-installed `vz-patch.wad`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WadBackup {
    /// File name of the snapshot inside the backup dir.
    pub file: String,
    /// Absolute path to the snapshot.
    pub path: String,
    pub byte_size: u64,
    pub sha256: String,
}

#[derive(Debug, Serialize)]
pub struct DeployWadResult {
    /// Where the WAD now lives in the game install.
    pub installed_at: String,
    pub sha256: String,
    pub byte_size: u64,
    /// The WAD we displaced, if any — restorable via `restore_patch_wad`.
    pub backed_up: Option<WadBackup>,
}

/// A durable record of the `vz-patch.wad` modkit currently has installed.
///
/// Distinct from [`WadBackup`], which describes a **displaced** WAD. This describes the live
/// one, and it survives a restart — that is the whole point of it.
///
/// It states what modkit last wrote and where. It is not a live reading of the game folder: a
/// user can delete or replace `vz-patch.wad` behind modkit's back, so a caller that needs
/// certainty re-hashes the file at `installed_at` and treats a mismatch as "not ours".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployedWadRecord {
    /// Absolute path the WAD was installed to, inside the game's data dir.
    pub installed_at: String,
    /// sha256 of the deployed bytes — what a log's build attribution is compared against.
    pub sha256: String,
    pub byte_size: u64,
    /// Unix epoch seconds at deploy time. Ordering/staleness only; never an identifier.
    pub deployed_at: u64,
}

/// Snapshots of replaced patch WADs.
fn backups_dir() -> Result<PathBuf, String> {
    let dir = deployed_dir()?.join("wad-backups");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create WAD backup dir: {e}"))?;
    Ok(dir)
}

/// The single-row ledger file.
fn ledger_path() -> Result<PathBuf, String> {
    Ok(deployed_dir()?.join("deployed-wad.json"))
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// The three ledger operations are written against an explicit path so they are unit-testable
// without reaching for the process-wide env vars `app_data_dir` resolves.

fn write_ledger_at(path: &Path, rec: &DeployedWadRecord) -> Result<(), String> {
    let json = serde_json::to_string_pretty(rec)
        .map_err(|e| format!("serializing the deploy record: {e}"))?;
    std::fs::write(path, json)
        .map_err(|e| format!("Failed to record the deployed WAD at {}: {e}", path.display()))
}

fn clear_ledger_at(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Failed to clear the deploy record: {e}")),
    }
}

fn read_ledger_at(path: &Path) -> Result<Option<DeployedWadRecord>, String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("The deploy record at {} is unreadable: {e}", path.display()))
}

/// Record what we just installed. A deploy that succeeded but whose ledger write failed would
/// leave modkit unable to say what is deployed, so this error is propagated rather than logged.
fn write_ledger(rec: &DeployedWadRecord) -> Result<(), String> {
    write_ledger_at(&ledger_path()?, rec)
}

/// Forget the deployed WAD — the patch is gone and the game is stock again.
fn clear_ledger() -> Result<(), String> {
    clear_ledger_at(&ledger_path()?)
}

/// What modkit currently has deployed, or `None` when no patch WAD is installed.
///
/// A corrupt ledger is an **error**, not a `None`. "No record" is a load-bearing answer — it is
/// what makes an ASI-only setup (which has never deployed a WAD) compare equal to a log with no
/// build attribution — so quietly returning it for an unreadable file would turn a broken ledger
/// into a false match.
#[tauri::command(async)]
pub fn deployed_wad_record() -> Result<Option<DeployedWadRecord>, String> {
    read_ledger_at(&ledger_path()?)
}

fn sha256_of(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok(loadprobe::sha256::sha256_hex(&bytes))
}

/// The game's data dir holds `vz.wad`; the patch sits beside it.
fn patch_target(data_dir: &str) -> PathBuf {
    PathBuf::from(data_dir).join(PATCH_NAME)
}

#[derive(Debug, Deserialize)]
pub struct DeployWadArgs {
    /// The built WAD to install.
    pub wad_path: String,
    /// The game's data dir (where `vz.wad` lives).
    pub data_dir: String,
}

/// Install a built `vz-patch.wad` into the game, backing up whatever was there.
///
/// The game holds the file open while running — close it first, or the copy fails.
#[tauri::command(async)]
pub fn deploy_patch_wad(args: DeployWadArgs) -> Result<DeployWadResult, String> {
    let src = PathBuf::from(&args.wad_path);
    if !src.is_file() {
        return Err(format!("No WAD at {}", src.display()));
    }
    let dest = patch_target(&args.data_dir);
    let dest_dir = dest
        .parent()
        .ok_or_else(|| format!("Bad data dir {}", args.data_dir))?;
    if !dest_dir.is_dir() {
        return Err(format!("Game data dir not found: {}", dest_dir.display()));
    }

    // Snapshot the WAD we're about to displace, keyed by its own content hash so
    // re-deploying the same WAD twice doesn't pile up identical copies.
    let backed_up = if dest.is_file() {
        let hash = sha256_of(&dest)?;
        let file = format!("vz-patch.{}.wad", &hash[..16]);
        let snap = backups_dir()?.join(&file);
        if !snap.exists() {
            std::fs::copy(&dest, &snap)
                .map_err(|e| format!("Failed to back up the existing WAD: {e}"))?;
        }
        let meta = std::fs::metadata(&snap).map_err(|e| format!("stat backup: {e}"))?;
        Some(WadBackup {
            file,
            path: snap.to_string_lossy().to_string(),
            byte_size: meta.len(),
            sha256: hash,
        })
    } else {
        None
    };

    std::fs::copy(&src, &dest).map_err(|e| {
        format!(
            "Failed to install {}: {e} — is the game still running? It holds the WAD open.",
            dest.display()
        )
    })?;

    // Verify by hash, not by size: confirm the bytes that landed are the bytes we built.
    let want = sha256_of(&src)?;
    let got = sha256_of(&dest)?;
    if want != got {
        return Err(format!(
            "Installed WAD does not match the built one (built {want}, on disk {got})"
        ));
    }
    let byte_size = std::fs::metadata(&dest)
        .map_err(|e| format!("stat installed WAD: {e}"))?
        .len();

    let installed_at = dest.to_string_lossy().to_string();
    write_ledger(&DeployedWadRecord {
        installed_at: installed_at.clone(),
        sha256: got.clone(),
        byte_size,
        deployed_at: now_unix(),
    })?;

    Ok(DeployWadResult {
        installed_at,
        sha256: got,
        byte_size,
        backed_up,
    })
}

/// List restorable snapshots of previously-installed patch WADs, newest first.
#[tauri::command(async)]
pub fn list_patch_wad_backups() -> Result<Vec<WadBackup>, String> {
    let dir = backups_dir()?;
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("read dir entry: {e}"))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("wad") {
            continue;
        }
        let meta = entry.metadata().map_err(|e| format!("stat: {e}"))?;
        out.push(WadBackup {
            file: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            path: path.to_string_lossy().to_string(),
            byte_size: meta.len(),
            sha256: sha256_of(&path)?,
        });
    }
    // Newest first.
    out.sort_by(|a, b| b.file.cmp(&a.file));
    Ok(out)
}

#[derive(Debug, Deserialize)]
pub struct RestoreWadArgs {
    /// Backup file name from `list_patch_wad_backups`; omit to just remove the patch.
    pub file: Option<String>,
    pub data_dir: String,
}

/// Restore a previous `vz-patch.wad` — or, with no `file`, remove the patch entirely
/// (reverting the game to stock `vz.wad`, which is always a safe state).
///
/// The current WAD is snapshotted first, so "restore" is itself undoable.
#[tauri::command(async)]
pub fn restore_patch_wad(args: RestoreWadArgs) -> Result<DeployWadResult, String> {
    let dest = patch_target(&args.data_dir);

    // Snapshot whatever is installed now, so restoring is not a one-way door.
    let backed_up = if dest.is_file() {
        let hash = sha256_of(&dest)?;
        let file = format!("vz-patch.{}.wad", &hash[..16]);
        let snap = backups_dir()?.join(&file);
        if !snap.exists() {
            std::fs::copy(&dest, &snap).map_err(|e| format!("Failed to snapshot current WAD: {e}"))?;
        }
        Some(WadBackup {
            file,
            path: snap.to_string_lossy().to_string(),
            byte_size: std::fs::metadata(&snap).map_err(|e| format!("stat: {e}"))?.len(),
            sha256: hash,
        })
    } else {
        None
    };

    match args.file {
        // Restore a specific snapshot.
        Some(file) => {
            let snap = backups_dir()?.join(&file);
            if !snap.is_file() {
                return Err(format!("No such backup: {file}"));
            }
            std::fs::copy(&snap, &dest).map_err(|e| {
                format!("Failed to restore {file}: {e} — is the game still running?")
            })?;
            let got = sha256_of(&dest)?;
            let byte_size = std::fs::metadata(&dest).map_err(|e| format!("stat: {e}"))?.len();
            let installed_at = dest.to_string_lossy().to_string();
            // A restore *is* a deploy as far as "what is running" is concerned.
            write_ledger(&DeployedWadRecord {
                installed_at: installed_at.clone(),
                sha256: got.clone(),
                byte_size,
                deployed_at: now_unix(),
            })?;
            Ok(DeployWadResult {
                installed_at,
                byte_size,
                sha256: got,
                backed_up,
            })
        }
        // No file: uninstall the patch entirely. Stock game.
        None => {
            if dest.is_file() {
                std::fs::remove_file(&dest)
                    .map_err(|e| format!("Failed to remove the patch WAD: {e} — is the game running?"))?;
            }
            // Nothing is deployed now, and that is a state to record by absence.
            clear_ledger()?;
            Ok(DeployWadResult {
                installed_at: String::new(),
                byte_size: 0,
                sha256: String::new(),
                backed_up,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(sha: &str) -> DeployedWadRecord {
        DeployedWadRecord {
            installed_at: "/game/data/vz-patch.wad".into(),
            sha256: sha.into(),
            byte_size: 4096,
            deployed_at: 1_754_400_000,
        }
    }

    /// The headline: what is deployed survives being written and read back by a later session.
    /// This is the left-hand side a log's build attribution gets compared against, and before
    /// the ledger existed there was none.
    #[test]
    fn the_deployed_hash_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deployed-wad.json");

        assert!(read_ledger_at(&path).unwrap().is_none(), "nothing deployed yet");

        write_ledger_at(&path, &rec("aa11")).unwrap();
        let got = read_ledger_at(&path).unwrap().expect("a record");
        assert_eq!(got.sha256, "aa11");
        assert_eq!(got.byte_size, 4096);
        assert_eq!(got.installed_at, "/game/data/vz-patch.wad");

        // A second deploy replaces the row rather than appending: there is only ever one live WAD.
        write_ledger_at(&path, &rec("bb22")).unwrap();
        assert_eq!(read_ledger_at(&path).unwrap().unwrap().sha256, "bb22");
    }

    /// Uninstalling the patch must leave *no* record, not a blank one — "no deploy record" is
    /// the state an ASI-only setup is legitimately in, and it has to be expressible.
    #[test]
    fn uninstall_clears_the_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deployed-wad.json");
        write_ledger_at(&path, &rec("aa11")).unwrap();

        clear_ledger_at(&path).unwrap();
        assert!(read_ledger_at(&path).unwrap().is_none());
        // Clearing an already-clear ledger is not an error (restore-to-stock is idempotent).
        clear_ledger_at(&path).unwrap();
    }

    /// A ledger we cannot parse is an error, never a silent "nothing is deployed" — that
    /// confusion would make a stale convoy look like a fresh one.
    #[test]
    fn a_corrupt_ledger_is_not_mistaken_for_an_empty_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deployed-wad.json");
        std::fs::write(&path, b"{ this is not json").unwrap();

        let err = read_ledger_at(&path).unwrap_err();
        assert!(err.contains("unreadable"), "got: {err}");
    }
}
