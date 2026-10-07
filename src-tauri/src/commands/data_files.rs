//! Game data files a Shipment edits in place: the shader stores `data/shader3.bin` and
//! `data/shader3Low.bin`.
//!
//! # What qm emits
//!
//! A shader kind lowers to a `data_file` placement: a whole replacement store, built from the
//! original store qm read through `--original-data`, with `base_sha256` naming that original.
//! `qm link` emits one store per `relative` for the whole set and the plan lists them in
//! `link_file_paths`; the per-Shipment stores are dropped for link's. Each plan item's
//! `data_files` names the stores that Shipment edits, which is how Modkit knows before the first
//! build that a set edits one.
//!
//! # The bank and the ledger
//!
//! A store is replaced whole, so the original is the only way back. Before the first build of a
//! set that edits a store, the store in the game folder is the original: a copy of it is banked
//! (content-addressed, [`crate::commands::managed::trash::bank`]) and a row is written to
//! `deployed/data-files.json`:
//!
//! ```json
//! { "relative": "data/shader3.bin", "original_sha256": "…", "bank_path": "…", "deployed_sha256": null }
//! ```
//!
//! The ledger is strict: a file that is there and does not read is an error. An absent file is
//! the statement "no store is banked".
//!
//! # Deploy and uninstall
//!
//! Deploy places link's store when link's `base_sha256` is the banked original and the store on
//! disk is either the original or the one Modkit deployed last. The bytes go to a temporary
//! sibling and are renamed into place, then re-hashed, and the row records `deployed_sha256`.
//!
//! Uninstall, and a deploy whose set edits the store no more, restores the original from the bank
//! when the store on disk is the one Modkit deployed, checks the restored bytes against
//! `original_sha256`, and clears the row. The bank copy is kept. Every mismatch is a hard failure
//! that leaves the store and its row as they were.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::load_plan::nullable;
use super::managed::place::sha256_of_file;
use super::managed::trash;
use super::paths::deployed_dir;
use super::placement::StagedDataFile;

/// The ledger's file name inside `deployed/`.
pub const LEDGER_FILE: &str = "data-files.json";

/// The bank's directory name inside `deployed/`.
const BANK_DIR: &str = "data-file-bank";

/// The ledger's own format number.
const LEDGER_FORMAT: u32 = 1;

/// A game data file Modkit deploys. A closed set: any other path is a parse error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum DataFileRel {
    /// `data/shader3.bin`.
    Shader3,
    /// `data/shader3Low.bin`.
    Shader3Low,
}

impl DataFileRel {
    /// Every member of the set.
    pub const ALL: [DataFileRel; 2] = [DataFileRel::Shader3, DataFileRel::Shader3Low];

    /// The path under the game folder, forward-slashed, exactly as qm writes it.
    pub fn relative(self) -> &'static str {
        match self {
            DataFileRel::Shader3 => "data/shader3.bin",
            DataFileRel::Shader3Low => "data/shader3Low.bin",
        }
    }

    /// The file name qm reads from `--original-data`.
    pub fn file_name(self) -> &'static str {
        match self {
            DataFileRel::Shader3 => "shader3.bin",
            DataFileRel::Shader3Low => "shader3Low.bin",
        }
    }

    /// Parse a path from a qm record. Anything outside the set is an error.
    pub fn parse(s: &str) -> Result<Self, String> {
        Self::ALL.into_iter().find(|r| r.relative() == s).ok_or_else(|| {
            format!(
                "{s:?} is not a data file Modkit deploys; the data files are {}",
                Self::ALL.map(|r| r.relative()).join(" and ")
            )
        })
    }
}

impl TryFrom<String> for DataFileRel {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        Self::parse(&s)
    }
}

impl From<DataFileRel> for String {
    fn from(r: DataFileRel) -> String {
        r.relative().to_string()
    }
}

impl std::fmt::Display for DataFileRel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.relative())
    }
}

/// Refuse a sha256 that is not 64 lowercase hex digits. `what` names the field for the message.
pub fn check_sha256(value: &str, what: &str) -> Result<(), String> {
    if value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        Ok(())
    } else {
        Err(format!("{what} {value:?} is not a lowercase hex sha256"))
    }
}

/// One banked store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataFileRow {
    pub relative: DataFileRel,
    /// sha256 of the store that was in the game folder before Modkit replaced it.
    pub original_sha256: String,
    /// Absolute path of the banked copy of that store.
    pub bank_path: String,
    /// sha256 of the store Modkit placed, or `null` while Modkit has placed none.
    #[serde(deserialize_with = "nullable")]
    pub deployed_sha256: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LedgerFile {
    format: u32,
    rows: Vec<DataFileRow>,
}

/// What a deploy or an uninstall did to the data files.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct DataFileOutcome {
    /// Stores placed into the game folder.
    pub deployed: Vec<String>,
    /// Stores put back from the bank.
    pub restored: Vec<String>,
}

/// Where the ledger and the bank live. Injectable so the code that writes into a game install runs
/// against a temp directory in tests.
#[derive(Debug, Clone)]
pub struct DataFileStore {
    pub ledger: PathBuf,
    pub bank: PathBuf,
}

impl DataFileStore {
    /// The real one, under `deployed/`.
    pub fn app() -> Result<Self, String> {
        let dir = deployed_dir()?;
        Ok(Self { ledger: dir.join(LEDGER_FILE), bank: dir.join(BANK_DIR) })
    }

    /// Every row. An absent ledger has none; a ledger that does not read is an error.
    pub fn read(&self) -> Result<Vec<DataFileRow>, String> {
        let text = match std::fs::read_to_string(&self.ledger) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("reading {}: {e}", self.ledger.display())),
        };
        let file: LedgerFile = serde_json::from_str(&text).map_err(|e| {
            format!("The data-file ledger at {} is unreadable: {e}", self.ledger.display())
        })?;
        if file.format != LEDGER_FORMAT {
            return Err(format!(
                "The data-file ledger at {} is format {}; Modkit reads format {LEDGER_FORMAT} only",
                self.ledger.display(),
                file.format
            ));
        }
        let mut seen = BTreeSet::new();
        for row in &file.rows {
            if !seen.insert(row.relative) {
                return Err(format!(
                    "The data-file ledger at {} has two rows for {}",
                    self.ledger.display(),
                    row.relative
                ));
            }
            check_sha256(&row.original_sha256, &format!("{}'s original_sha256", row.relative))?;
            if let Some(d) = &row.deployed_sha256 {
                check_sha256(d, &format!("{}'s deployed_sha256", row.relative))?;
            }
        }
        Ok(file.rows)
    }

    fn write(&self, rows: &[DataFileRow]) -> Result<(), String> {
        let text = serde_json::to_string_pretty(&LedgerFile { format: LEDGER_FORMAT, rows: rows.to_vec() })
            .map_err(|e| format!("describing the data-file ledger: {e}"))?;
        if let Some(parent) = self.ledger.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
        }
        std::fs::write(&self.ledger, text).map_err(|e| format!("writing {}: {e}", self.ledger.display()))
    }
}

/// `relative` under `game_root`, one path component per `/`-separated part.
fn game_path(game_root: &Path, rel: DataFileRel) -> PathBuf {
    let mut path = game_root.to_path_buf();
    for part in rel.relative().split('/') {
        path.push(part);
    }
    path
}

/// Bank the original of every store in `wanted` that has no row, and write its row.
///
/// The store in the game folder stays where it is: a copy is banked. A store with a row is banked
/// already, and its original is the banked copy.
pub fn bank_originals(
    store: &DataFileStore,
    game_root: &Path,
    wanted: &BTreeSet<DataFileRel>,
) -> Result<Vec<DataFileRow>, String> {
    let mut rows = store.read()?;
    for &rel in wanted {
        if rows.iter().any(|r| r.relative == rel) {
            continue;
        }
        let path = game_path(game_root, rel);
        if !path.is_file() {
            return Err(format!(
                "A Shipment in this set edits {rel}, and there is no {} to bank as the original.",
                path.display()
            ));
        }
        let original_sha256 = sha256_of_file(&path)?;
        let incoming_dir = store.bank.join("incoming");
        std::fs::create_dir_all(&incoming_dir)
            .map_err(|e| format!("creating {}: {e}", incoming_dir.display()))?;
        let incoming = incoming_dir.join(rel.file_name());
        std::fs::copy(&path, &incoming)
            .map_err(|e| format!("copying {} to the bank: {e}", path.display()))?;
        let banked = trash::bank(&incoming, Some(&store.bank))?;
        let banked_sha256 = sha256_of_file(&banked)?;
        if banked_sha256 != original_sha256 {
            return Err(format!(
                "The banked copy of {rel} at {} has sha256 {banked_sha256}; the game's store has \
                 {original_sha256}.",
                banked.display()
            ));
        }
        rows.push(DataFileRow {
            relative: rel,
            original_sha256,
            bank_path: banked.to_string_lossy().to_string(),
            deployed_sha256: None,
        });
        store.write(&rows)?;
    }
    Ok(rows)
}

/// Fill a fresh `dir` with the banked original of every store in `wanted`, each under the file
/// name qm reads, and return `dir` for `--original-data`. Each copy must hash to its row's
/// `original_sha256`.
pub fn stage_original_data(
    store: &DataFileStore,
    wanted: &BTreeSet<DataFileRel>,
    dir: &Path,
) -> Result<PathBuf, String> {
    let rows = store.read()?;
    std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    for &rel in wanted {
        let row = rows.iter().find(|r| r.relative == rel).ok_or_else(|| {
            format!("{rel} has no row in the data-file ledger, so its original is not banked.")
        })?;
        let dest = dir.join(rel.file_name());
        std::fs::copy(&row.bank_path, &dest)
            .map_err(|e| format!("copying the banked {rel} from {}: {e}", row.bank_path))?;
        let got = sha256_of_file(&dest)?;
        if got != row.original_sha256 {
            return Err(format!(
                "The banked original of {rel} at {} has sha256 {got}; the ledger records {}.",
                row.bank_path, row.original_sha256
            ));
        }
    }
    Ok(dir.to_path_buf())
}

/// Copy `source` to a temporary sibling of `dest`, check it hashes to `want`, rename it over
/// `dest`, and check `dest` hashes to `want`.
fn place_atomically(source: &Path, dest: &Path, want: &str, rel: DataFileRel) -> Result<(), String> {
    let temp = dest.with_file_name(format!("{}.modkit-incoming", rel.file_name()));
    std::fs::copy(source, &temp)
        .map_err(|e| format!("writing {}: {e} — is the game still running?", temp.display()))?;
    let staged = sha256_of_file(&temp)?;
    if staged != want {
        let _ = std::fs::remove_file(&temp);
        return Err(format!(
            "The copy of {rel} written to {} has sha256 {staged}; it must be {want}.",
            temp.display()
        ));
    }
    std::fs::rename(&temp, dest)
        .map_err(|e| format!("moving {} into place: {e} — is the game still running?", temp.display()))?;
    let landed = sha256_of_file(dest)?;
    if landed != want {
        return Err(format!(
            "{} has sha256 {landed} after it was placed; it must be {want}.",
            dest.display()
        ));
    }
    Ok(())
}

/// The sha256 of the store at `path`, which must exist.
fn on_disk_sha256(path: &Path, rel: DataFileRel) -> Result<String, String> {
    if !path.is_file() {
        return Err(format!("{rel} is not in the game folder at {}.", path.display()));
    }
    sha256_of_file(path)
}

/// Put `row`'s original back and drop the row from `rows`.
///
/// A row with a deployed store needs that store on disk; a row with none needs the original on
/// disk, and nothing is written.
fn restore_row(
    store: &DataFileStore,
    rows: &mut Vec<DataFileRow>,
    rel: DataFileRel,
    game_root: &Path,
    outcome: &mut DataFileOutcome,
) -> Result<(), String> {
    let row = rows
        .iter()
        .find(|r| r.relative == rel)
        .cloned()
        .ok_or_else(|| format!("{rel} has no row in the data-file ledger."))?;
    let path = game_path(game_root, rel);
    let found = on_disk_sha256(&path, rel)?;
    match &row.deployed_sha256 {
        Some(deployed) => {
            if &found != deployed {
                return Err(format!(
                    "{rel} in the game folder has sha256 {found}, and Modkit deployed {deployed}. \
                     Something else changed it, so Modkit leaves it and its banked original \
                     ({}) where they are.",
                    row.bank_path
                ));
            }
            place_atomically(Path::new(&row.bank_path), &path, &row.original_sha256, rel)?;
            outcome.restored.push(rel.relative().to_string());
        }
        None => {
            if found != row.original_sha256 {
                return Err(format!(
                    "{rel} in the game folder has sha256 {found}; Modkit deployed none, and the \
                     banked original ({}) has {}.",
                    row.bank_path, row.original_sha256
                ));
            }
        }
    }
    rows.retain(|r| r.relative != rel);
    store.write(rows)
}

/// Deploy the staged stores, and restore every banked store the staged set does not deploy.
///
/// Every staged store is checked before anything is written: its `base_sha256` must be the
/// banked original, and the store on disk must be the original or the store Modkit deployed.
pub fn install_data_files(
    store: &DataFileStore,
    staged: &[StagedDataFile],
    game_root: &Path,
) -> Result<DataFileOutcome, String> {
    let mut rows = store.read()?;
    let mut seen = BTreeSet::new();
    for file in staged {
        if !seen.insert(file.relative) {
            return Err(format!("The staged build deploys {} twice.", file.relative));
        }
        let rel = file.relative;
        let row = rows.iter().find(|r| r.relative == rel).ok_or_else(|| {
            format!("{rel} has no row in the data-file ledger, so there is no banked original to deploy over.")
        })?;
        if file.base_sha256 != row.original_sha256 {
            return Err(format!(
                "{rel} was built from an original with sha256 {}; the banked original has {}. \
                 Rebuild against this game install.",
                file.base_sha256, row.original_sha256
            ));
        }
        let found = on_disk_sha256(&game_path(game_root, rel), rel)?;
        let expected_on_disk = found == row.original_sha256 || row.deployed_sha256.as_deref() == Some(found.as_str());
        if !expected_on_disk {
            return Err(format!(
                "{rel} in the game folder has sha256 {found}. It is neither the banked original ({}) \
                 nor the store Modkit deployed ({}), so Modkit does not replace it.",
                row.original_sha256,
                row.deployed_sha256.as_deref().unwrap_or("none")
            ));
        }
    }

    let mut outcome = DataFileOutcome::default();
    let leaving: Vec<DataFileRel> = rows
        .iter()
        .map(|r| r.relative)
        .filter(|rel| !seen.contains(rel))
        .collect();
    for rel in leaving {
        restore_row(store, &mut rows, rel, game_root, &mut outcome)?;
    }

    for file in staged {
        let rel = file.relative;
        place_atomically(Path::new(&file.source), &game_path(game_root, rel), &file.sha256, rel)?;
        let row = rows
            .iter_mut()
            .find(|r| r.relative == rel)
            .ok_or_else(|| format!("{rel} has no row in the data-file ledger."))?;
        row.deployed_sha256 = Some(file.sha256.clone());
        store.write(&rows)?;
        outcome.deployed.push(rel.relative().to_string());
    }
    Ok(outcome)
}

/// Restore every banked store and clear its row.
pub fn uninstall_data_files(store: &DataFileStore, game_root: &Path) -> Result<DataFileOutcome, String> {
    let mut rows = store.read()?;
    let mut outcome = DataFileOutcome::default();
    let all: Vec<DataFileRel> = rows.iter().map(|r| r.relative).collect();
    for rel in all {
        restore_row(store, &mut rows, rel, game_root, &mut outcome)?;
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(bytes: &[u8]) -> String {
        loadprobe::sha256::sha256_hex(bytes)
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        store: DataFileStore,
        game: PathBuf,
        build: PathBuf,
    }

    /// A game folder whose `data/` holds both retail stores.
    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        std::fs::create_dir_all(game.join("data")).unwrap();
        std::fs::write(game.join("data/shader3.bin"), b"retail shader3").unwrap();
        std::fs::write(game.join("data/shader3Low.bin"), b"retail shader3Low").unwrap();
        let store = DataFileStore {
            ledger: tmp.path().join("deployed").join(LEDGER_FILE),
            bank: tmp.path().join("deployed").join(BANK_DIR),
        };
        let build = tmp.path().join("build");
        std::fs::create_dir_all(&build).unwrap();
        Fixture { store, game, build, _tmp: tmp }
    }

    fn only(rel: DataFileRel) -> BTreeSet<DataFileRel> {
        [rel].into_iter().collect()
    }

    /// A staged store built from `base`.
    fn staged(f: &Fixture, rel: DataFileRel, body: &[u8], base: &str) -> StagedDataFile {
        let source = f.build.join(format!("{}-{}", sha(body), rel.file_name()));
        std::fs::write(&source, body).unwrap();
        StagedDataFile {
            source: source.to_string_lossy().to_string(),
            relative: rel,
            bytes: body.len() as u64,
            sha256: sha(body),
            base_sha256: base.to_string(),
            shipment: "Quartermaster link".into(),
        }
    }

    fn on_disk(f: &Fixture, rel: DataFileRel) -> Vec<u8> {
        std::fs::read(game_path(&f.game, rel)).unwrap()
    }

    #[test]
    fn the_set_is_closed() {
        assert_eq!(DataFileRel::parse("data/shader3.bin").unwrap(), DataFileRel::Shader3);
        assert_eq!(DataFileRel::parse("data/shader3Low.bin").unwrap(), DataFileRel::Shader3Low);
        for bad in ["data/shader3low.bin", "data/Shader3.bin", "shader3.bin", "data/vz.wad", "../data/shader3.bin", ""] {
            let err = DataFileRel::parse(bad).unwrap_err();
            assert!(err.contains("is not a data file Modkit deploys"), "{bad:?}: {err}");
        }
        let json = serde_json::to_string(&DataFileRel::Shader3Low).unwrap();
        assert_eq!(json, "\"data/shader3Low.bin\"");
        assert!(serde_json::from_str::<DataFileRel>("\"data/shader4.bin\"").is_err());
    }

    /// Banking copies the store in the game folder into the bank, leaves the game's store in
    /// place, and writes a row with no deployed store.
    #[test]
    fn banking_records_the_original_and_leaves_it_in_the_game() {
        let f = fixture();
        let rows = bank_originals(&f.store, &f.game, &only(DataFileRel::Shader3)).unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.relative, DataFileRel::Shader3);
        assert_eq!(row.original_sha256, sha(b"retail shader3"));
        assert_eq!(row.deployed_sha256, None);
        assert_eq!(std::fs::read(&row.bank_path).unwrap(), b"retail shader3");
        assert_eq!(on_disk(&f, DataFileRel::Shader3), b"retail shader3", "the game keeps its store");
        assert_eq!(f.store.read().unwrap(), rows);

        let text = std::fs::read_to_string(&f.store.ledger).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["rows"][0]["relative"], "data/shader3.bin");
        assert!(v["rows"][0]["deployed_sha256"].is_null());
    }

    /// A store with a row is banked already: banking again leaves the row, even when the game's
    /// store has changed since.
    #[test]
    fn banking_twice_keeps_the_first_original() {
        let f = fixture();
        bank_originals(&f.store, &f.game, &only(DataFileRel::Shader3)).unwrap();
        std::fs::write(game_path(&f.game, DataFileRel::Shader3), b"something else").unwrap();
        let rows = bank_originals(&f.store, &f.game, &only(DataFileRel::Shader3)).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].original_sha256, sha(b"retail shader3"));
    }

    #[test]
    fn banking_a_store_the_game_lacks_is_refused() {
        let f = fixture();
        std::fs::remove_file(game_path(&f.game, DataFileRel::Shader3Low)).unwrap();
        let err = bank_originals(&f.store, &f.game, &only(DataFileRel::Shader3Low)).unwrap_err();
        assert!(err.contains("there is no") && err.contains("shader3Low.bin"), "{err}");
        assert!(f.store.read().unwrap().is_empty());
    }

    /// qm reads the originals by file name from one directory.
    #[test]
    fn the_original_data_dir_holds_the_banked_stores_by_name() {
        let f = fixture();
        let both: BTreeSet<_> = DataFileRel::ALL.into_iter().collect();
        bank_originals(&f.store, &f.game, &both).unwrap();
        std::fs::write(game_path(&f.game, DataFileRel::Shader3), b"deployed by modkit").unwrap();

        let dir = f.build.join("original-data");
        let got = stage_original_data(&f.store, &both, &dir).unwrap();
        assert_eq!(got, dir);
        assert_eq!(std::fs::read(dir.join("shader3.bin")).unwrap(), b"retail shader3");
        assert_eq!(std::fs::read(dir.join("shader3Low.bin")).unwrap(), b"retail shader3Low");
    }

    #[test]
    fn staging_original_data_without_a_row_is_refused() {
        let f = fixture();
        let err = stage_original_data(&f.store, &only(DataFileRel::Shader3), &f.build.join("o")).unwrap_err();
        assert!(err.contains("not banked"), "{err}");
    }

    #[test]
    fn an_unreadable_ledger_is_an_error() {
        let f = fixture();
        std::fs::create_dir_all(f.store.ledger.parent().unwrap()).unwrap();
        std::fs::write(&f.store.ledger, b"{ not json").unwrap();
        assert!(f.store.read().unwrap_err().contains("unreadable"));
        assert!(bank_originals(&f.store, &f.game, &only(DataFileRel::Shader3)).is_err());

        let row = r#"{"relative":"data/shader3.bin","original_sha256":"00","bank_path":"/b"}"#;
        std::fs::write(&f.store.ledger, format!(r#"{{"format":1,"rows":[{row}]}}"#)).unwrap();
        assert!(f.store.read().unwrap_err().contains("deployed_sha256"), "a missing key is refused");

        let row = format!(
            r#"{{"relative":"data/shader9.bin","original_sha256":"{}","bank_path":"/b","deployed_sha256":null}}"#,
            "a".repeat(64)
        );
        std::fs::write(&f.store.ledger, format!(r#"{{"format":1,"rows":[{row}]}}"#)).unwrap();
        assert!(f.store.read().unwrap_err().contains("is not a data file"));

        std::fs::write(&f.store.ledger, r#"{"format":2,"rows":[]}"#).unwrap();
        assert!(f.store.read().unwrap_err().contains("format 2"));
    }

    /// Deploy places link's store, records its sha256, and a redeploy over Modkit's own store is
    /// accepted.
    #[test]
    fn deploy_and_redeploy() {
        let f = fixture();
        let rows = bank_originals(&f.store, &f.game, &only(DataFileRel::Shader3)).unwrap();
        let base = rows[0].original_sha256.clone();

        let first = staged(&f, DataFileRel::Shader3, b"linked v1", &base);
        let out = install_data_files(&f.store, std::slice::from_ref(&first), &f.game).unwrap();
        assert_eq!(out.deployed, vec!["data/shader3.bin".to_string()]);
        assert_eq!(on_disk(&f, DataFileRel::Shader3), b"linked v1");
        assert_eq!(f.store.read().unwrap()[0].deployed_sha256, Some(sha(b"linked v1")));
        assert!(!f.game.join("data/shader3.bin.modkit-incoming").exists());

        let second = staged(&f, DataFileRel::Shader3, b"linked v2", &base);
        install_data_files(&f.store, &[second], &f.game).unwrap();
        assert_eq!(on_disk(&f, DataFileRel::Shader3), b"linked v2");
        let row = &f.store.read().unwrap()[0];
        assert_eq!(row.deployed_sha256, Some(sha(b"linked v2")));
        assert_eq!(row.original_sha256, base, "the original is still the retail store");
    }

    /// A store built from a different original is refused, and nothing is written.
    #[test]
    fn a_base_mismatch_is_refused() {
        let f = fixture();
        bank_originals(&f.store, &f.game, &only(DataFileRel::Shader3)).unwrap();
        let file = staged(&f, DataFileRel::Shader3, b"linked", &sha(b"another install's store"));
        let err = install_data_files(&f.store, &[file], &f.game).unwrap_err();
        assert!(err.contains("Rebuild against this game install"), "{err}");
        assert_eq!(on_disk(&f, DataFileRel::Shader3), b"retail shader3");
        assert_eq!(f.store.read().unwrap()[0].deployed_sha256, None);
    }

    /// A store on disk that is neither the original nor Modkit's is not replaced.
    #[test]
    fn a_hand_modified_store_fails_deploy() {
        let f = fixture();
        let rows = bank_originals(&f.store, &f.game, &only(DataFileRel::Shader3)).unwrap();
        let base = rows[0].original_sha256.clone();
        install_data_files(&f.store, &[staged(&f, DataFileRel::Shader3, b"linked v1", &base)], &f.game).unwrap();
        std::fs::write(game_path(&f.game, DataFileRel::Shader3), b"hand edited").unwrap();

        let err = install_data_files(&f.store, &[staged(&f, DataFileRel::Shader3, b"linked v2", &base)], &f.game)
            .unwrap_err();
        assert!(err.contains("neither the banked original"), "{err}");
        assert_eq!(on_disk(&f, DataFileRel::Shader3), b"hand edited");
        assert_eq!(f.store.read().unwrap()[0].deployed_sha256, Some(sha(b"linked v1")));
    }

    /// Deploying a store with no banked original is refused.
    #[test]
    fn a_store_without_a_row_is_refused() {
        let f = fixture();
        let file = staged(&f, DataFileRel::Shader3, b"linked", &sha(b"retail shader3"));
        let err = install_data_files(&f.store, &[file], &f.game).unwrap_err();
        assert!(err.contains("no banked original"), "{err}");
    }

    /// Uninstall puts the original back byte for byte, clears the row, and keeps the bank copy.
    #[test]
    fn uninstall_restores_the_original_byte_for_byte() {
        let f = fixture();
        let both: BTreeSet<_> = DataFileRel::ALL.into_iter().collect();
        let rows = bank_originals(&f.store, &f.game, &both).unwrap();
        let files: Vec<_> = rows
            .iter()
            .map(|r| staged(&f, r.relative, format!("linked {}", r.relative).as_bytes(), &r.original_sha256))
            .collect();
        install_data_files(&f.store, &files, &f.game).unwrap();

        let out = uninstall_data_files(&f.store, &f.game).unwrap();
        assert_eq!(out.restored, vec!["data/shader3.bin".to_string(), "data/shader3Low.bin".to_string()]);
        assert_eq!(on_disk(&f, DataFileRel::Shader3), b"retail shader3");
        assert_eq!(on_disk(&f, DataFileRel::Shader3Low), b"retail shader3Low");
        assert!(f.store.read().unwrap().is_empty());
        for row in rows {
            assert!(Path::new(&row.bank_path).is_file(), "the bank copy is kept");
        }
    }

    /// A store changed by hand after Modkit deployed it fails uninstall, and stays as it is with
    /// its row.
    #[test]
    fn a_hand_modified_store_fails_uninstall() {
        let f = fixture();
        let rows = bank_originals(&f.store, &f.game, &only(DataFileRel::Shader3)).unwrap();
        install_data_files(&f.store, &[staged(&f, DataFileRel::Shader3, b"linked", &rows[0].original_sha256)], &f.game)
            .unwrap();
        std::fs::write(game_path(&f.game, DataFileRel::Shader3), b"hand edited").unwrap();

        let err = uninstall_data_files(&f.store, &f.game).unwrap_err();
        assert!(err.contains("Something else changed it"), "{err}");
        assert_eq!(on_disk(&f, DataFileRel::Shader3), b"hand edited");
        assert_eq!(f.store.read().unwrap().len(), 1);
    }

    /// A deploy whose set edits one store no more restores that store and deploys the other.
    #[test]
    fn a_set_without_a_store_restores_it() {
        let f = fixture();
        let both: BTreeSet<_> = DataFileRel::ALL.into_iter().collect();
        let rows = bank_originals(&f.store, &f.game, &both).unwrap();
        let files: Vec<_> = rows
            .iter()
            .map(|r| staged(&f, r.relative, b"linked", &r.original_sha256))
            .collect();
        install_data_files(&f.store, &files, &f.game).unwrap();

        let out = install_data_files(&f.store, &files[..1], &f.game).unwrap();
        assert_eq!(out.restored, vec!["data/shader3Low.bin".to_string()]);
        assert_eq!(out.deployed, vec!["data/shader3.bin".to_string()]);
        assert_eq!(on_disk(&f, DataFileRel::Shader3Low), b"retail shader3Low");
        let left = f.store.read().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].relative, DataFileRel::Shader3);

        let out = install_data_files(&f.store, &[], &f.game).unwrap();
        assert_eq!(out.restored, vec!["data/shader3.bin".to_string()]);
        assert_eq!(on_disk(&f, DataFileRel::Shader3), b"retail shader3");
        assert!(f.store.read().unwrap().is_empty());
    }

    /// A row banked before a build that was never deployed is cleared when the original is still
    /// on disk, and refused when it is not.
    #[test]
    fn an_undeployed_row_is_cleared_only_over_the_original() {
        let f = fixture();
        bank_originals(&f.store, &f.game, &only(DataFileRel::Shader3)).unwrap();
        std::fs::write(game_path(&f.game, DataFileRel::Shader3), b"other").unwrap();
        let err = uninstall_data_files(&f.store, &f.game).unwrap_err();
        assert!(err.contains("Modkit deployed none"), "{err}");
        assert_eq!(f.store.read().unwrap().len(), 1);

        std::fs::write(game_path(&f.game, DataFileRel::Shader3), b"retail shader3").unwrap();
        let out = uninstall_data_files(&f.store, &f.game).unwrap();
        assert!(out.restored.is_empty());
        assert!(f.store.read().unwrap().is_empty());
    }
}
