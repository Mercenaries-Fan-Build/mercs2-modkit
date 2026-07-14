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

/// Snapshots of replaced patch WADs.
fn backups_dir() -> Result<PathBuf, String> {
    let dir = deployed_dir()?.join("wad-backups");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create WAD backup dir: {e}"))?;
    Ok(dir)
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
#[tauri::command]
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

    Ok(DeployWadResult {
        installed_at: dest.to_string_lossy().to_string(),
        sha256: got,
        byte_size,
        backed_up,
    })
}

/// List restorable snapshots of previously-installed patch WADs, newest first.
#[tauri::command]
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
#[tauri::command]
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
            Ok(DeployWadResult {
                installed_at: dest.to_string_lossy().to_string(),
                byte_size: std::fs::metadata(&dest).map_err(|e| format!("stat: {e}"))?.len(),
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
            Ok(DeployWadResult {
                installed_at: String::new(),
                byte_size: 0,
                sha256: String::new(),
                backed_up,
            })
        }
    }
}
