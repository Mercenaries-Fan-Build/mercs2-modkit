//! Read `qm`'s `placement.json` — the record of what a build produced and where each artifact goes.
//!
//! # Why this module exists
//!
//! `qm build` emits **three** kinds of output into its `--out` directory: the overlay WAD, any
//! loose files a Shipment places into the game folder, and `placement.json` describing both. modkit
//! used to read only "the first WAD" and drop the rest on the floor, so a Shipment carrying
//! `native_hook` (an `.asi` plugin) or `place_file` (a companion) **built clean, reported success,
//! and deployed nothing but its WAD blocks**. Installed, and silently doing nothing.
//!
//! Silence is the worst failure mode, because it is indistinguishable from success. So the record is
//! read, every artifact it names is carried through to deploy, and everything placed is written down
//! so uninstall can take it back out again — a deploy with no undo record is the same bug pointing
//! the other way.
//!
//! # Two output shapes, not one
//!
//! * `qm build` writes `placement.json` next to its overlay, with one `overlay` entry for the WAD
//!   and one `game_folder` entry per loose file. The build directory **mirrors** the tree the files
//!   are copied into, so `destination.relative` names the file in both places.
//! * `qm link` (`link_installed`) emits `zz-quartermaster-link.wad` and, in releases before the
//!   record was added there, **no `placement.json` at all**.
//!
//! [`read_placement`] therefore returns `Ok(None)` for an absent record rather than failing, and
//! [`overlay_wad`] falls back to a **name-sorted** scan. That fallback is also the fix for the
//! second trap: the old `first_wad` took whatever `read_dir` yielded first, which is filesystem
//! order — arbitrary the moment a directory holds more than one WAD, and different on two machines
//! running the same build.
//!
//! A malformed record is a hard error. "Could not understand what qm said it produced" must not
//! degrade into "produced nothing".

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The record `qm build` writes into its output directory.
pub const PLACEMENT_FILE: &str = "placement.json";

/// Where one artifact belongs. Mirrors `mercs2_quartermaster::build::Destination`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Destination {
    /// A patch WAD, mounted by the deploy step rather than copied into the game folder.
    Overlay,
    /// A loose file, at `relative` under the game folder (forward slashes, game root = no prefix).
    GameFolder { relative: String },
}

/// One artifact in the record.
#[derive(Debug, Clone, Deserialize)]
pub struct PlacementEntry {
    /// The artifact's file name.
    pub name: String,
    /// Size of the bytes qm wrote.
    #[serde(default)]
    pub bytes: u64,
    /// sha256 of the bytes qm wrote, read back off disk by qm itself.
    pub sha256: String,
    pub destination: Destination,
}

/// A parsed `placement.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct PlacementRecord {
    /// Record format version. Unknown values are refused rather than guessed at.
    pub format: u32,
    #[serde(default)]
    pub placements: Vec<PlacementEntry>,
}

/// The only `placement.json` format modkit knows how to read.
const SUPPORTED_FORMAT: u32 = 1;

/// A loose file staged for the game folder, resolved to the bytes on disk that back it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagedFile {
    /// Absolute path to the built file in the qm output (or modkit's build staging) directory.
    pub source: String,
    /// Destination path relative to the game folder, forward-slashed. Never absolute, never `..`.
    pub relative: String,
    /// sha256 of the bytes, as qm recorded them.
    pub sha256: String,
    /// Which Shipment placed it, for attribution in the UI and in the deploy ledger.
    #[serde(default)]
    pub shipment: String,
}

impl StagedFile {
    /// The destination file name (the last path component of [`Self::relative`]).
    pub fn file_name(&self) -> &str {
        self.relative.rsplit('/').next().unwrap_or(&self.relative)
    }

    /// The destination directory relative to the game folder — `""` for the game root.
    pub fn dir(&self) -> &str {
        match self.relative.rfind('/') {
            Some(i) => &self.relative[..i],
            None => "",
        }
    }

    /// True for a `native_hook` plugin. `qm` guarantees the extension is exclusive to that kind:
    /// `native_hook` refuses anything that is *not* `.asi` (the loader globs `*.asi` and would
    /// otherwise ignore the file), and `place_file` refuses anything that *is*. So the extension
    /// identifies the kind exactly, without the record having to carry the contribution kind.
    pub fn is_asi(&self) -> bool {
        self.file_name().to_ascii_lowercase().ends_with(".asi")
    }
}

/// Read `dir/placement.json`.
///
/// `Ok(None)` means the file is not there — a legitimate state for `qm link` output. `Err` means it
/// is there and could not be understood, which is never treated as "nothing to place".
pub fn read_placement(dir: &Path) -> Result<Option<PlacementRecord>, String> {
    let path = dir.join(PLACEMENT_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("reading {}: {e}", path.display())),
    };
    let record: PlacementRecord = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not a placement record: {e}", path.display()))?;
    if record.format != SUPPORTED_FORMAT {
        return Err(format!(
            "{} is format {} — this modkit understands format {SUPPORTED_FORMAT}. Update modkit \
             rather than installing a build it cannot fully describe.",
            path.display(),
            record.format
        ));
    }
    Ok(Some(record))
}

/// Every `*.wad` in `dir`, sorted by file name (case-insensitively, then exactly, so the order is
/// total and identical on a case-sensitive and a case-preserving filesystem).
fn sorted_wads(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
    let mut wads: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("wad")))
        .collect();
    wads.sort_by(|a, b| {
        let (an, bn) = (
            a.file_name().unwrap_or_default().to_string_lossy().to_string(),
            b.file_name().unwrap_or_default().to_string_lossy().to_string(),
        );
        an.to_lowercase().cmp(&bn.to_lowercase()).then(an.cmp(&bn))
    });
    Ok(wads)
}

/// The overlay WAD a qm run produced, chosen **deterministically**.
///
/// The record names it, so that is the first source of truth. Without a record (the `qm link` path)
/// the name-sorted scan decides — never `read_dir` order, which is the filesystem's and differs
/// between machines running the identical build.
pub fn overlay_wad(dir: &Path, record: Option<&PlacementRecord>) -> Result<Option<PathBuf>, String> {
    if let Some(record) = record {
        let named: Vec<&PlacementEntry> = record
            .placements
            .iter()
            .filter(|p| matches!(p.destination, Destination::Overlay))
            .collect();
        if let Some(first) = named.first() {
            if named.len() > 1 {
                return Err(format!(
                    "{} names {} overlay WADs; modkit collapses qm's stack into one vz-patch.wad \
                     and cannot tell which of them to read",
                    dir.join(PLACEMENT_FILE).display(),
                    named.len()
                ));
            }
            let path = dir.join(&first.name);
            if !path.is_file() {
                return Err(format!(
                    "{} names the overlay {} but it is not in {}",
                    PLACEMENT_FILE,
                    first.name,
                    dir.display()
                ));
            }
            return Ok(Some(path));
        }
        // A record with no overlay entry is a scripts-only or files-only build; there is genuinely
        // no WAD, and falling through to a directory scan could only find a stale one.
        return Ok(None);
    }
    Ok(sorted_wads(dir)?.into_iter().next())
}

/// Reject a `relative` that would escape the game folder, or that is not a plain relative path.
///
/// qm already refuses these at build time, but this code writes into the user's game install from a
/// file on disk: the check belongs on **both** sides of that boundary, because the build machine and
/// the deploying machine are not the same machine and need not even be the same OS. `scripts\..\..`
/// is an escape on Windows and an ordinary filename on macOS.
fn refuse_unsafe_relative(relative: &str) -> Option<String> {
    if relative.is_empty() {
        return Some("it is empty".into());
    }
    if relative.contains('\\') {
        return Some("it contains a backslash; placement paths are forward-slashed".into());
    }
    if relative.starts_with('/') {
        return Some("it is absolute".into());
    }
    // `C:` / `\\host\share` — absolute on Windows even though `Path::is_absolute` says otherwise on
    // the Unix machine that may be running this check.
    if relative.len() >= 2 && relative.as_bytes()[1] == b':' {
        return Some("it names a drive".into());
    }
    for part in relative.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Some(format!("the component {part:?} escapes the game folder"));
        }
    }
    None
}

/// Resolve a record's `game_folder` entries into [`StagedFile`]s rooted at `dir`.
///
/// Every entry must exist on disk: qm writes the file and the record together, so a record naming a
/// file that is not there means the output directory was tampered with or a write failed, and
/// installing the rest as if it were complete is exactly the silent partial success this module
/// exists to remove.
pub fn staged_files(
    dir: &Path,
    record: &PlacementRecord,
    shipment: &str,
) -> Result<Vec<StagedFile>, String> {
    let mut out = Vec::new();
    for entry in &record.placements {
        let Destination::GameFolder { relative } = &entry.destination else {
            continue;
        };
        if let Some(why) = refuse_unsafe_relative(relative) {
            return Err(format!(
                "{shipment} places {relative:?}, which modkit refuses: {why}."
            ));
        }
        let source = dir.join(relative);
        if !source.is_file() {
            return Err(format!(
                "{shipment}'s build recorded {relative} but did not write it to {}",
                dir.display()
            ));
        }
        out.push(StagedFile {
            source: source.to_string_lossy().to_string(),
            relative: relative.clone(),
            sha256: entry.sha256.clone(),
            shipment: shipment.to_string(),
        });
    }
    Ok(out)
}

/// Read a qm output directory whole: its overlay WAD and its loose files, in one pass.
pub fn read_output(dir: &Path, shipment: &str) -> Result<(Option<PathBuf>, Vec<StagedFile>), String> {
    let record = read_placement(dir)?;
    let wad = overlay_wad(dir, record.as_ref())?;
    let files = match &record {
        Some(r) => staged_files(dir, r, shipment)?,
        None => Vec::new(),
    };
    Ok((wad, files))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn record_json(entries: &str) -> String {
        format!("{{\"format\":1,\"placements\":[{entries}]}}")
    }

    /// The `qm link` shape: a WAD and no record at all. It must not error and must not panic — the
    /// second output path the deploy step cannot assume away.
    #[test]
    fn a_missing_placement_json_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "zz-quartermaster-link.wad", "wad bytes");
        assert!(read_placement(dir.path()).unwrap().is_none());
        let (wad, files) = read_output(dir.path(), "link").unwrap();
        assert_eq!(wad.unwrap().file_name().unwrap(), "zz-quartermaster-link.wad");
        assert!(files.is_empty());
    }

    /// An empty directory with no record is not a failure either — `qm link` emits no WAD when no
    /// Shipment touches scripts.
    #[test]
    fn an_empty_output_dir_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (wad, files) = read_output(dir.path(), "link").unwrap();
        assert!(wad.is_none());
        assert!(files.is_empty());
    }

    /// WAD selection must not depend on `read_dir` order. With a record, the record decides.
    #[test]
    fn the_record_names_the_overlay() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "aaa-decoy.wad", "decoy");
        write(dir.path(), "my-shipment.wad", "real");
        write(
            dir.path(),
            PLACEMENT_FILE,
            &record_json(
                r#"{"name":"my-shipment.wad","bytes":4,"sha256":"ab","destination":{"kind":"overlay"}}"#,
            ),
        );
        let (wad, _) = read_output(dir.path(), "s").unwrap();
        assert_eq!(wad.unwrap().file_name().unwrap(), "my-shipment.wad");
    }

    /// Without a record the scan is name-sorted, so the same directory always yields the same WAD.
    #[test]
    fn the_fallback_scan_is_name_sorted() {
        let dir = tempfile::tempdir().unwrap();
        for n in ["zz-link.wad", "aa-first.wad", "mm-middle.wad"] {
            write(dir.path(), n, "x");
        }
        let chosen = overlay_wad(dir.path(), None).unwrap().unwrap();
        assert_eq!(chosen.file_name().unwrap(), "aa-first.wad");
        // And repeated reads agree, whatever the filesystem enumerates.
        for _ in 0..5 {
            assert_eq!(overlay_wad(dir.path(), None).unwrap().unwrap(), chosen);
        }
    }

    /// A files-only Shipment has no overlay, and the scan must not substitute a stale WAD for one.
    #[test]
    fn a_record_with_no_overlay_yields_no_wad() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "stale.wad", "left over");
        write(dir.path(), "scripts/hook.asi", "MZ");
        write(
            dir.path(),
            PLACEMENT_FILE,
            &record_json(
                r#"{"name":"hook.asi","bytes":2,"sha256":"cd","destination":{"kind":"game_folder","relative":"scripts/hook.asi"}}"#,
            ),
        );
        let (wad, files) = read_output(dir.path(), "s").unwrap();
        assert!(wad.is_none(), "no overlay entry means no overlay");
        assert_eq!(files.len(), 1);
        assert!(files[0].is_asi());
        assert_eq!(files[0].dir(), "scripts");
        assert_eq!(files[0].file_name(), "hook.asi");
    }

    /// A game-root placement has an empty directory half, and is not an `.asi`.
    #[test]
    fn a_game_root_companion_reports_no_directory() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "config.ini", "[x]");
        write(
            dir.path(),
            PLACEMENT_FILE,
            &record_json(
                r#"{"name":"config.ini","bytes":3,"sha256":"ef","destination":{"kind":"game_folder","relative":"config.ini"}}"#,
            ),
        );
        let (_, files) = read_output(dir.path(), "s").unwrap();
        assert_eq!(files[0].dir(), "");
        assert!(!files[0].is_asi());
    }

    /// A record that cannot be parsed is a refusal, never an empty list.
    #[test]
    fn a_malformed_record_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), PLACEMENT_FILE, "{ not json");
        assert!(read_placement(dir.path()).unwrap_err().contains("not a placement record"));

        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), PLACEMENT_FILE, r#"{"format":99,"placements":[]}"#);
        assert!(read_placement(dir.path()).unwrap_err().contains("format 99"));
    }

    /// A record naming a file qm did not write is a refusal — installing the rest would be a
    /// partial success reported as a whole one.
    #[test]
    fn a_record_naming_a_missing_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            PLACEMENT_FILE,
            &record_json(
                r#"{"name":"gone.asi","bytes":1,"sha256":"aa","destination":{"kind":"game_folder","relative":"scripts/gone.asi"}}"#,
            ),
        );
        let err = read_output(dir.path(), "s").unwrap_err();
        assert!(err.contains("did not write it"), "got: {err}");
    }

    /// Traversal is refused here as well as in qm: the two ends of the pipe are different machines
    /// and need not be the same OS.
    #[test]
    fn traversal_and_absolute_destinations_are_refused() {
        for bad in [
            "../Mercenaries2.exe",
            "/etc/passwd",
            "C:/Windows/system32/x.dll",
            "scripts\\..\\..\\x.exe",
            "scripts//x.ini",
            "",
        ] {
            assert!(
                refuse_unsafe_relative(bad).is_some(),
                "{bad:?} should be refused"
            );
        }
        for good in ["hook.asi", "scripts/hook.asi", "scripts/OnBoot/init.lua"] {
            assert!(refuse_unsafe_relative(good).is_none(), "{good:?} is fine");
        }
    }
}
