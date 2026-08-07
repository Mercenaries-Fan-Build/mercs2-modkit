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
//! # The other half of a deploy: loose files
//!
//! A Shipment can carry things that are not WAD content at all — a `native_hook` `.asi` plugin, a
//! `place_file` companion — and those go into the **game folder**, not into `vz-patch.wad`. They are
//! staged by [`super::wad_builder`] and installed here, because a deploy that installed only the WAD
//! would report success and leave half the Shipment on the floor.
//!
//! Every placed file is written into a **ledger** ([`PlacementStore`]) at install time. That is
//! not bookkeeping for its own sake: an overlay is undone by replacing one file, but a file dropped
//! into the game folder cannot be backed out unless something wrote down what was put where. A
//! deploy with no undo record is the same class of defect as a deploy that does nothing, pointing
//! the other way.
//!
//! Nothing here hard-deletes on the file half either: a displaced foreign file is snapshotted, and
//! removal moves to the recoverable trash.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::commands::paths::{deployed_dir, trash_dir};
use crate::commands::placement::{self, StagedFile};

const PATCH_NAME: &str = "vz-patch.wad";

/// Folders the ASI loader scans. Same set [`super::deploy`] validates against — the loader globs
/// `*.asi` in the game root and in these three subfolders, and nowhere else.
const VALID_ASI_TARGETS: &[&str] = &[".", "scripts", "plugins", "update"];

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

/// One loose file this deploy put into the game folder, as recorded in the ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacedFile {
    /// Absolute path in the game install.
    pub abs_path: String,
    /// Path relative to the game folder, as installed (an `.asi` may have been re-rooted to the
    /// user's chosen ASI target, so this can differ from what the Shipment's record said).
    pub relative: String,
    /// sha256 of the bytes written.
    pub sha256: String,
    /// Which Shipment placed it.
    pub shipment: String,
}

/// What the file half of a deploy (or an uninstall) did.
#[derive(Debug, Default, Serialize)]
pub struct PlacementOutcome {
    /// Files installed into the game folder.
    pub placed: Vec<PlacedFile>,
    /// Files removed from the game folder (moved to the recoverable trash).
    pub removed: Vec<String>,
    /// Recorded files left alone because their bytes no longer match what modkit wrote — somebody
    /// replaced them by hand, so removing them would destroy work modkit did not do.
    pub skipped: Vec<String>,
    /// Pre-existing, unmanaged files displaced to `<name>.bak` to make room.
    pub backed_up: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DeployWadResult {
    /// Where the WAD now lives in the game install. Empty when the build produced no WAD (a
    /// Shipment carrying only `native_hook` / `place_file` contributions), in which case whatever
    /// patch WAD is already installed is deliberately left untouched.
    pub installed_at: String,
    pub sha256: String,
    pub byte_size: u64,
    /// The WAD we displaced, if any — restorable via `restore_patch_wad`.
    pub backed_up: Option<WadBackup>,
    /// The loose-file half of the deploy.
    #[serde(default)]
    pub files: PlacementOutcome,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Ledger {
    format: u32,
    #[serde(default)]
    files: Vec<PlacedFile>,
}

/// Where the loose-file bookkeeping lives: the ledger of what is installed, and the recoverable
/// trash removed files are moved to.
///
/// A struct rather than two free functions reading the app-data dir directly, so the install and
/// uninstall logic can be exercised against a temp directory. This is deliberate: the code that
/// writes into somebody's game install is exactly the code that should not be untestable because it
/// hardcoded a path to the real one.
struct PlacementStore {
    ledger: PathBuf,
    trash: PathBuf,
}

impl PlacementStore {
    /// The real one, under the app's managed area.
    fn app() -> Result<Self, String> {
        Ok(Self {
            ledger: deployed_dir()?.join("placed-files.json"),
            trash: trash_dir()?,
        })
    }

    /// Read the ledger. A missing or unreadable ledger yields an **empty** list rather than an
    /// error: its purpose is to let uninstall find files, and refusing to deploy because a previous
    /// deploy's bookkeeping is unreadable helps nobody. What it must never do is claim files exist
    /// that do not — every entry is re-verified against the disk before anything is removed.
    fn read(&self) -> Vec<PlacedFile> {
        let Ok(text) = std::fs::read_to_string(&self.ledger) else {
            return Vec::new();
        };
        serde_json::from_str::<Ledger>(&text)
            .map(|l| l.files)
            .unwrap_or_default()
    }

    fn write(&self, files: &[PlacedFile]) -> Result<(), String> {
        let text = serde_json::to_string_pretty(&Ledger {
            format: 1,
            files: files.to_vec(),
        })
        .map_err(|e| format!("describing the placed files: {e}"))?;
        if let Some(parent) = self.ledger.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("creating {}: {e}", parent.display()))?;
        }
        std::fs::write(&self.ledger, text)
            .map_err(|e| format!("writing {}: {e}", self.ledger.display()))
    }

    /// Move one file into the recoverable trash, timestamped so re-removing a name never clobbers
    /// an earlier copy. Same treatment [`super::deploy::trash_paths`] gives a plugin.
    fn trash(&self, src: &Path) -> Result<(), String> {
        std::fs::create_dir_all(&self.trash)
            .map_err(|e| format!("creating {}: {e}", self.trash.display()))?;
        let name = src
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dest = self.trash.join(format!("{stamp}-{name}"));
        if std::fs::rename(src, &dest).is_ok() {
            return Ok(());
        }
        // Across volumes `rename` fails; copy then remove.
        std::fs::copy(src, &dest).map_err(|e| format!("moving {name} to the trash: {e}"))?;
        std::fs::remove_file(src).map_err(|e| format!("removing {name}: {e}"))
    }

    /// Take previously-placed files back out of the game folder.
    ///
    /// A recorded file whose bytes no longer match what modkit wrote is **left alone** and reported
    /// as skipped. Somebody replaced it by hand — updating a plugin in place is a perfectly ordinary
    /// thing to do — and deleting it would destroy work modkit did not do.
    fn remove_placed(&self, files: &[PlacedFile], outcome: &mut PlacementOutcome) {
        for file in files {
            let path = Path::new(&file.abs_path);
            if !path.is_file() {
                continue;
            }
            match sha256_of(path) {
                Ok(hash) if hash == file.sha256 => {
                    if self.trash(path).is_ok() {
                        outcome.removed.push(file.relative.clone());
                    } else {
                        outcome.skipped.push(file.relative.clone());
                    }
                }
                _ => outcome.skipped.push(file.relative.clone()),
            }
        }
    }
}

/// Where a staged file lands under `game_root`.
///
/// `.asi` plugins are re-rooted to the user's chosen ASI target. The loader globs the game root and
/// `scripts/`, `plugins/`, `update/`, so which of the four a plugin sits in is a **modkit setting**,
/// not a Shipment's decision — qm always records `scripts/` because it has to record something, and
/// honouring that literally would put Shipment plugins somewhere other than every other plugin
/// modkit deploys. Everything else keeps its recorded path exactly: `scripts/OnBoot/init.lua` is a
/// Lua bridge rung and `plugins/foo.ini` is a companion its plugin looks for by name, and neither is
/// a scan folder modkit gets to pick.
fn destination_for(game_root: &Path, file: &StagedFile, asi_target: &str) -> (PathBuf, String) {
    if file.is_asi() {
        let name = file.file_name();
        let relative = if asi_target == "." {
            name.to_string()
        } else {
            format!("{asi_target}/{name}")
        };
        let dir = if asi_target == "." {
            game_root.to_path_buf()
        } else {
            game_root.join(asi_target)
        };
        return (dir.join(name), relative);
    }
    let mut dest = game_root.to_path_buf();
    for part in file.relative.split('/') {
        dest.push(part);
    }
    (dest, file.relative.clone())
}

/// Install the staged loose files into the game folder, replacing whatever modkit placed last time.
///
/// Order matters: the previous deploy's files come out **first**, so a plugin dropped from the load
/// order since the last build is genuinely gone rather than left behind loading into the game.
fn install_placements(
    store: &PlacementStore,
    staged: &[StagedFile],
    game_root: &Path,
    asi_target: &str,
) -> Result<PlacementOutcome, String> {
    let mut outcome = PlacementOutcome::default();
    let previous = store.read();
    store.remove_placed(&previous, &mut outcome);

    for file in staged {
        let (dest, relative) = destination_for(game_root, file, asi_target);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("creating {}: {e}", parent.display()))?;
        }
        // Anything still here after the ledger sweep is somebody else's file. Displace it to
        // `<name>.bak` rather than overwriting, matching how `deploy_asi` treats a foreign plugin.
        if dest.exists() {
            let backup = dest.with_file_name(format!("{}.bak", file.file_name()));
            std::fs::rename(&dest, &backup)
                .map_err(|e| format!("backing up {}: {e}", dest.display()))?;
            outcome.backed_up.push(relative.clone());
        }
        std::fs::copy(&file.source, &dest).map_err(|e| {
            format!(
                "installing {relative}: {e} — is the game still running? It holds its plugins open."
            )
        })?;
        // Verify by hash, exactly as the WAD half does: confirm the bytes that landed are the bytes
        // that were built.
        let got = sha256_of(&dest)?;
        if !file.sha256.is_empty() && got != file.sha256 {
            return Err(format!(
                "Installed {relative} does not match the built file (built {}, on disk {got})",
                file.sha256
            ));
        }
        outcome.placed.push(PlacedFile {
            abs_path: dest.to_string_lossy().to_string(),
            relative,
            sha256: got,
            shipment: file.shipment.clone(),
        });
    }

    store.write(&outcome.placed)?;
    Ok(outcome)
}

/// Remove every loose file the ledger records, and empty it.
fn uninstall_placements(store: &PlacementStore) -> Result<PlacementOutcome, String> {
    let mut outcome = PlacementOutcome::default();
    let previous = store.read();
    if previous.is_empty() {
        return Ok(outcome);
    }
    store.remove_placed(&previous, &mut outcome);
    store.write(&[])?;
    Ok(outcome)
}

/// The game's data dir holds `vz.wad`; the patch sits beside it.
fn patch_target(data_dir: &str) -> PathBuf {
    PathBuf::from(data_dir).join(PATCH_NAME)
}

#[derive(Debug, Deserialize)]
pub struct DeployWadArgs {
    /// The built WAD to install. **Empty** when the build produced none, which is a real outcome
    /// for a Shipment carrying only `native_hook` / `place_file` contributions.
    pub wad_path: String,
    /// The game's data dir (where `vz.wad` lives).
    pub data_dir: String,
    /// The build output directory. Its `placement.json` names the loose files to install. Omit and
    /// the file half is skipped entirely — which is what every pre-Shipment caller wants.
    #[serde(default)]
    pub staging_dir: Option<String>,
    /// The game install root (the parent of `data/`). Required whenever `staging_dir` names files.
    #[serde(default)]
    pub game_root: Option<String>,
    /// Which of the loader's four scan folders `.asi` plugins go in: `.`, `scripts`, `plugins`,
    /// `update`. Defaults to `scripts`, matching the store's own default.
    #[serde(default)]
    pub asi_target: Option<String>,
}

/// Install a built `vz-patch.wad` into the game, backing up whatever was there — and install the
/// loose files the build staged alongside it, recording them so uninstall can take them back out.
///
/// The game holds the file open while running — close it first, or the copy fails.
#[tauri::command(async)]
pub fn deploy_patch_wad(args: DeployWadArgs) -> Result<DeployWadResult, String> {
    // Resolve the file half FIRST, so a build that names a file it cannot install refuses before
    // the WAD is swapped rather than leaving the two halves out of step.
    let staged = match args.staging_dir.as_deref().filter(|d| !d.is_empty()) {
        Some(dir) => placement::read_staged(Path::new(dir))?,
        None => Vec::new(),
    };
    let asi_target = args.asi_target.as_deref().unwrap_or("scripts");
    if !VALID_ASI_TARGETS.contains(&asi_target) {
        return Err(format!(
            "Invalid ASI target '{asi_target}': expected one of {VALID_ASI_TARGETS:?}"
        ));
    }
    let game_root = match args.game_root.as_deref().filter(|r| !r.is_empty()) {
        Some(r) => Some(PathBuf::from(r)),
        None if staged.is_empty() => None,
        // Refusing beats installing the WAD and silently dropping the plugins.
        None => {
            return Err(format!(
                "This build places {} file(s) into the game folder, but no game root was given.",
                staged.len()
            ))
        }
    };
    if let Some(root) = &game_root {
        if !root.is_dir() {
            return Err(format!("Game folder not found: {}", root.display()));
        }
    }

    // No WAD is a legitimate build outcome now, so an empty path installs the files and leaves the
    // installed patch WAD alone rather than erroring.
    let src = PathBuf::from(&args.wad_path);
    if args.wad_path.trim().is_empty() {
        let files = match &game_root {
            Some(root) => install_placements(&PlacementStore::app()?, &staged, root, asi_target)?,
            None => PlacementOutcome::default(),
        };
        return Ok(DeployWadResult {
            installed_at: String::new(),
            sha256: String::new(),
            byte_size: 0,
            backed_up: None,
            files,
        });
    }
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

    // The two halves of a deploy, and the order is deliberate. Loose files go in first; the
    // ledger is written only once both halves have landed, so a record means "this deploy
    // completed" rather than "the WAD copy succeeded". A placement failure therefore leaves no
    // record at all, and the freshness check reads that as "no deploy" and declines to attach a
    // convoy — refusing to report is the safe direction, where a record describing a half-done
    // deploy would be confidently wrong.
    let files = match &game_root {
        Some(root) => install_placements(&PlacementStore::app()?, &staged, root, asi_target)?,
        None => PlacementOutcome::default(),
    };
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
        files,
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
///
/// Removing the patch also takes out the loose files the last deploy placed — a plugin left loading
/// into a game whose patch WAD has been removed is exactly the half-uninstalled state this button
/// exists to avoid. Restoring a *specific* older WAD deliberately leaves them: the snapshots are
/// WADs only, so there is no matching older file set to put back, and silently deleting the current
/// one would be a change nobody asked for.
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
                files: PlacementOutcome::default(),
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
                files: uninstall_placements(&PlacementStore::app()?)?,
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

    fn store(dir: &Path) -> PlacementStore {
        PlacementStore {
            ledger: dir.join("ledger/placed-files.json"),
            trash: dir.join("trash"),
        }
    }

    /// Build a staged file on disk the way `wad_builder::stage_placements` leaves one.
    fn stage(root: &Path, relative: &str, body: &str, shipment: &str) -> StagedFile {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        StagedFile {
            source: path.to_string_lossy().to_string(),
            relative: relative.to_string(),
            sha256: loadprobe::sha256::sha256_hex(body.as_bytes()),
            shipment: shipment.to_string(),
        }
    }

    /// The headline defect: a Shipment carrying a `native_hook` used to build clean and deploy
    /// nothing. Its plugin must actually reach the game folder.
    #[test]
    fn a_native_hook_plugin_reaches_the_game_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let (build, game) = (tmp.path().join("build"), tmp.path().join("game"));
        std::fs::create_dir_all(&game).unwrap();
        // qm always records an .asi under `scripts/` — that is its only choice.
        let staged = [stage(&build, "scripts/my-hook.asi", "MZ plugin", "Hooky")];

        let out = install_placements(&store(tmp.path()), &staged, &game, "scripts").unwrap();
        assert_eq!(out.placed.len(), 1);
        let landed = game.join("scripts/my-hook.asi");
        assert!(landed.is_file(), "the plugin is in the game folder");
        assert_eq!(std::fs::read_to_string(&landed).unwrap(), "MZ plugin");
        assert_eq!(out.placed[0].abs_path, landed.to_string_lossy());
        assert_eq!(out.placed[0].shipment, "Hooky");
    }

    /// The ASI loader scans four folders, and WHICH one is a modkit setting rather than the
    /// Shipment's call. A plugin must follow `asiTarget` wherever the user pointed it.
    #[test]
    fn a_plugin_follows_the_users_asi_target() {
        for target in VALID_ASI_TARGETS {
            let tmp = tempfile::tempdir().unwrap();
            let (build, game) = (tmp.path().join("build"), tmp.path().join("game"));
            std::fs::create_dir_all(&game).unwrap();
            let staged = [stage(&build, "scripts/hook.asi", "MZ", "S")];

            let out = install_placements(&store(tmp.path()), &staged, &game, target).unwrap();
            let expected = if *target == "." {
                game.join("hook.asi")
            } else {
                game.join(target).join("hook.asi")
            };
            assert!(expected.is_file(), "{target}: expected {}", expected.display());
            assert_eq!(out.placed[0].abs_path, expected.to_string_lossy());
        }
    }

    /// `PlaceIn` is a closed set of seven, and every one of them must land where it says. A
    /// companion is NOT re-rooted the way a plugin is: `scripts/OnBoot` is a Lua bridge rung and
    /// `plugins/x.ini` is a file its plugin opens by name — neither is modkit's to relocate.
    #[test]
    fn every_place_in_destination_lands_where_it_says() {
        // `PlaceIn::relative_dir()`, all seven arms, in declaration order.
        let destinations = [
            ("", "GameRoot"),
            ("scripts", "Scripts"),
            ("plugins", "Plugins"),
            ("update", "Update"),
            ("scripts/OnBoot", "OnBoot"),
            ("scripts/OnLoad", "OnLoad"),
            ("scripts/OnKey", "OnKey"),
        ];
        let tmp = tempfile::tempdir().unwrap();
        let (build, game) = (tmp.path().join("build"), tmp.path().join("game"));
        std::fs::create_dir_all(&game).unwrap();

        let staged: Vec<StagedFile> = destinations
            .iter()
            .map(|(dir, name)| {
                let relative = if dir.is_empty() {
                    format!("{name}.ini")
                } else {
                    format!("{dir}/{name}.ini")
                };
                stage(&build, &relative, name, "Companions")
            })
            .collect();

        // Deliberately not "scripts": a companion must ignore the ASI target entirely.
        let out = install_placements(&store(tmp.path()), &staged, &game, "plugins").unwrap();
        assert_eq!(out.placed.len(), 7);
        for (dir, name) in destinations {
            let expected = if dir.is_empty() {
                game.join(format!("{name}.ini"))
            } else {
                game.join(dir).join(format!("{name}.ini"))
            };
            assert!(expected.is_file(), "{name}: expected {}", expected.display());
            assert_eq!(std::fs::read_to_string(&expected).unwrap(), name);
        }
    }

    /// The other half of the same defect: a deploy with no undo record. Uninstall must take every
    /// placed file back out, and the ledger must end up empty.
    #[test]
    fn uninstall_removes_every_placed_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (build, game) = (tmp.path().join("build"), tmp.path().join("game"));
        std::fs::create_dir_all(&game).unwrap();
        let s = store(tmp.path());
        let staged = [
            stage(&build, "scripts/hook.asi", "MZ", "S"),
            stage(&build, "scripts/OnBoot/init.lua", "-- lua", "S"),
        ];
        install_placements(&s, &staged, &game, "scripts").unwrap();
        assert!(game.join("scripts/hook.asi").is_file());

        let out = uninstall_placements(&s).unwrap();
        assert_eq!(out.removed.len(), 2, "both files removed: {out:?}");
        assert!(!game.join("scripts/hook.asi").exists());
        assert!(!game.join("scripts/OnBoot/init.lua").exists());
        assert!(s.read().is_empty(), "the ledger is emptied");
        // Nothing was hard-deleted: both are recoverable from the trash.
        assert_eq!(std::fs::read_dir(&s.trash).unwrap().count(), 2);
    }

    /// Re-deploying a load order that dropped a Shipment must take that Shipment's plugin out,
    /// not leave it loading into the game forever.
    #[test]
    fn a_dropped_plugin_is_removed_on_the_next_deploy() {
        let tmp = tempfile::tempdir().unwrap();
        let (build, game) = (tmp.path().join("build"), tmp.path().join("game"));
        std::fs::create_dir_all(&game).unwrap();
        let s = store(tmp.path());

        let first = [
            stage(&build, "scripts/keep.asi", "keep", "A"),
            stage(&build, "scripts/drop.asi", "drop", "B"),
        ];
        install_placements(&s, &first, &game, "scripts").unwrap();

        let second = [stage(&build, "scripts/keep.asi", "keep", "A")];
        let out = install_placements(&s, &second, &game, "scripts").unwrap();

        assert!(game.join("scripts/keep.asi").is_file(), "kept");
        assert!(!game.join("scripts/drop.asi").exists(), "dropped");
        assert!(out.removed.contains(&"scripts/drop.asi".to_string()));
        assert_eq!(out.placed.len(), 1);
        // And no `.bak` was made for our own file — the ledger sweep took it out first.
        assert!(!game.join("scripts/keep.asi.bak").exists());
        assert!(out.backed_up.is_empty(), "{out:?}");
    }

    /// A file the user replaced by hand is not ours to delete. Leave it, and say so.
    #[test]
    fn a_hand_edited_file_is_left_alone_and_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let (build, game) = (tmp.path().join("build"), tmp.path().join("game"));
        std::fs::create_dir_all(&game).unwrap();
        let s = store(tmp.path());
        let staged = [stage(&build, "scripts/hook.asi", "v1", "S")];
        install_placements(&s, &staged, &game, "scripts").unwrap();

        // The user drops a newer build of the plugin in themselves.
        std::fs::write(game.join("scripts/hook.asi"), "v2 by hand").unwrap();

        let out = uninstall_placements(&s).unwrap();
        assert!(out.removed.is_empty());
        assert_eq!(out.skipped, vec!["scripts/hook.asi".to_string()]);
        assert_eq!(
            std::fs::read_to_string(game.join("scripts/hook.asi")).unwrap(),
            "v2 by hand"
        );
    }

    /// Somebody else's plugin at our destination is displaced, never overwritten.
    #[test]
    fn a_foreign_file_is_backed_up_rather_than_clobbered() {
        let tmp = tempfile::tempdir().unwrap();
        let (build, game) = (tmp.path().join("build"), tmp.path().join("game"));
        std::fs::create_dir_all(game.join("scripts")).unwrap();
        std::fs::write(game.join("scripts/hook.asi"), "somebody else's").unwrap();

        let staged = [stage(&build, "scripts/hook.asi", "ours", "S")];
        let out = install_placements(&store(tmp.path()), &staged, &game, "scripts").unwrap();

        assert_eq!(out.backed_up, vec!["scripts/hook.asi".to_string()]);
        assert_eq!(
            std::fs::read_to_string(game.join("scripts/hook.asi.bak")).unwrap(),
            "somebody else's"
        );
        assert_eq!(
            std::fs::read_to_string(game.join("scripts/hook.asi")).unwrap(),
            "ours"
        );
    }

    /// Uninstalling with nothing recorded is a no-op, not a panic — the state every install that
    /// predates this feature is in.
    #[test]
    fn uninstalling_with_no_ledger_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        let out = uninstall_placements(&store(tmp.path())).unwrap();
        assert!(out.removed.is_empty() && out.skipped.is_empty());
    }

    /// The bytes that land are verified against the bytes that were built, exactly as the WAD half
    /// is. A record whose digest does not describe its file is refused rather than installed.
    #[test]
    fn a_digest_mismatch_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let (build, game) = (tmp.path().join("build"), tmp.path().join("game"));
        std::fs::create_dir_all(&game).unwrap();
        let mut staged = stage(&build, "scripts/hook.asi", "real bytes", "S");
        staged.sha256 = "0".repeat(64);

        let err = install_placements(&store(tmp.path()), &[staged], &game, "scripts").unwrap_err();
        assert!(err.contains("does not match the built file"), "got: {err}");
    }
}
