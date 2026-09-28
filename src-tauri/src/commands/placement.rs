//! Read `qm`'s `placement.json` — the record of what a build produced and where each artifact goes.
//!
//! # Why this module exists
//!
//! `qm build` and `qm link` emit more than an overlay WAD into their `--out` directory: loose files
//! for the game folder, new base WADs for `data/`, per-language and front-end patch WADs, and copies
//! of game data the deploy step makes inside the install. `placement.json` describes all of it. A
//! Shipment whose output is only partly read builds clean, reports success, and deploys a fraction
//! of itself, which is indistinguishable from success until the game runs.
//!
//! So the record is read whole, every artifact it names is carried through to deploy, and
//! everything placed is written down so uninstall can take it back out again.
//!
//! # One output shape
//!
//! qm writes `placement.json` into every output directory, `qm build` and `qm link` alike, including
//! an empty `placements` list when a step produced nothing. The record is required: a directory
//! without one, a record whose `format` is not 2, an unknown destination `kind`, or a missing field
//! is a hard error. "Could not understand what qm said it produced" never degrades into "produced
//! nothing".
//!
//! The destinations (`destination.kind`):
//!
//! | kind | artifact in the output dir | deploy |
//! |---|---|---|
//! | `overlay` | `<name>`, a patch WAD | merged into `data/vz-patch.wad` |
//! | `game_folder` | `<relative>` | copied to `<game>/<relative>` |
//! | `data_wad` | `<relative>` (`data/<name>.wad`) | copied to `<game>/<relative>`; the ledger records `display` |
//! | `language_patch` | `<relative>`, a patch WAD | merged per `language` into `data/<language>-patch.wad` |
//! | `shell_patch` | `<name>`, a patch WAD | merged into `data/shell-patch.wad` |
//! | `stream_copy` | none | `<game>/<from>` copied to `<game>/<to>` once its digest matches `sha256` |

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The record `qm build` and `qm link` write into their output directory, and the staged-build
/// record modkit writes beside `vz-patch.wad`.
pub const PLACEMENT_FILE: &str = "placement.json";

/// The only `placement.json` format modkit reads, for qm's record and for its own staged record.
const SUPPORTED_FORMAT: u32 = 2;

/// Where one artifact belongs. Mirrors `mercs2_quartermaster::build::Destination`.
///
/// Internally tagged by `kind`; an unknown kind is a parse error.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Destination {
    /// A patch WAD whose blocks the build step merges into `vz-patch.wad`.
    Overlay,
    /// A loose file, at `relative` under the game folder (forward slashes, game root = no prefix).
    GameFolder { relative: String },
    /// A new base WAD at `relative` under the game folder (`data/<name>.wad`) — an `add_language`
    /// language WAD the engine opens by name. Deployed like a loose file. `display` is the language's
    /// display name, which the deploy ledger keeps for the language screen.
    DataWad { relative: String, display: String },
    /// A patch WAD at `relative` in the output dir whose blocks are merged, with every other
    /// Shipment's `language_patch` for the same `language`, into `data/<language>-patch.wad`.
    LanguagePatch { language: String, relative: String },
    /// A patch WAD named by the entry's `name`, merged with every other Shipment's `shell_patch`
    /// into `data/shell-patch.wad`.
    ShellPatch,
    /// A copy of game data inside the install: `<game>/<from>` to `<game>/<to>`. The entry's `bytes`
    /// and `sha256` describe `<game>/<from>` as qm read it at build time.
    StreamCopy { from: String, to: String },
}

/// One artifact in the record.
#[derive(Debug, Clone, Deserialize)]
pub struct PlacementEntry {
    /// The artifact's file name.
    pub name: String,
    /// Size of the bytes qm wrote (for `stream_copy`, of the source qm read).
    pub bytes: u64,
    /// sha256 of those bytes.
    pub sha256: String,
    pub destination: Destination,
}

/// A parsed `placement.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct PlacementRecord {
    /// Record format version. Anything but [`SUPPORTED_FORMAT`] is refused.
    pub format: u32,
    pub placements: Vec<PlacementEntry>,
}

/// A file staged for the game folder, resolved to the bytes on disk that back it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagedFile {
    /// Absolute path to the built file in the qm output (or modkit's build staging) directory.
    pub source: String,
    /// Destination path relative to the game folder, forward-slashed. Never absolute, never `..`.
    pub relative: String,
    /// sha256 of the bytes, as qm recorded them (or as modkit computed them for a merged WAD).
    pub sha256: String,
    /// Which Shipment placed it (for a merged patch WAD, every contributing Shipment in load order),
    /// for attribution in the UI and in the deploy ledger.
    pub shipment: String,
    /// The language's display name, set exactly for a `data_wad` placement.
    pub display: Option<String>,
    /// The language token, set exactly for a merged `data/<language>-patch.wad`.
    pub language: Option<String>,
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

/// A `stream_copy` placement: copy `<game>/<from>` to `<game>/<to>` at deploy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamCopy {
    /// Source, relative to the game folder, forward-slashed.
    pub from: String,
    /// Destination, relative to the game folder, forward-slashed.
    pub to: String,
    /// Size of the source as qm read it.
    pub bytes: u64,
    /// sha256 of the source as qm read it. Deploy refuses the copy when the source differs.
    pub sha256: String,
    /// Which Shipment asked for the copy.
    pub shipment: String,
}

/// A patch WAD in a qm output that is merged per language into `data/<language>-patch.wad`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguagePatch {
    /// The engine's lowercase language token (`english`, `french`, …).
    pub language: String,
    /// Absolute path to the WAD in the output dir.
    pub path: PathBuf,
}

/// Everything one qm output directory produced, validated against the disk.
#[derive(Debug, Default)]
pub struct QmOutput {
    /// The overlay WAD, merged into `vz-patch.wad`.
    pub overlay: Option<PathBuf>,
    /// `game_folder` and `data_wad` files, copied into the game folder.
    pub files: Vec<StagedFile>,
    /// `language_patch` WADs, at most one per language.
    pub language_patches: Vec<LanguagePatch>,
    /// The `shell_patch` WAD.
    pub shell_patch: Option<PathBuf>,
    /// `stream_copy` placements.
    pub stream_copies: Vec<StreamCopy>,
}

impl QmOutput {
    /// True when the output carries nothing to merge, copy or place.
    pub fn is_empty(&self) -> bool {
        self.overlay.is_none()
            && self.files.is_empty()
            && self.language_patches.is_empty()
            && self.shell_patch.is_none()
            && self.stream_copies.is_empty()
    }
}

/// Read `dir/placement.json`. A missing, unparseable or non-format-2 record is an error.
pub fn read_placement(dir: &Path) -> Result<PlacementRecord, String> {
    let path = dir.join(PLACEMENT_FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "reading {}: {e}. qm writes this record into every output directory, so its absence \
             means the qm run did not complete.",
            path.display()
        )
    })?;
    let record: PlacementRecord = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not a placement record: {e}", path.display()))?;
    if record.format != SUPPORTED_FORMAT {
        return Err(format!(
            "{} is format {} — this modkit reads format {SUPPORTED_FORMAT} only. Use the qm \
             release this modkit pins.",
            path.display(),
            record.format
        ));
    }
    Ok(record)
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

/// Reject a language token that is not `[a-z0-9_]+`. The engine builds `.\Data\<token>-patch.wad`
/// from it, so it is a file-name component as well as a table key.
fn refuse_bad_language(language: &str) -> Option<String> {
    if language.is_empty() {
        return Some("it is empty".into());
    }
    if !language
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return Some("a language token is lowercase letters, digits and `_` only".into());
    }
    None
}

/// `relative` checked by [`refuse_unsafe_relative`], with the refusal naming who placed it.
fn checked_relative<'a>(relative: &'a str, what: &str, shipment: &str) -> Result<&'a str, String> {
    match refuse_unsafe_relative(relative) {
        Some(why) => Err(format!(
            "{shipment} records the {what} {relative:?}, which modkit refuses: {why}."
        )),
        None => Ok(relative),
    }
}

/// The on-disk artifact at `relative` under `dir`, which qm must have written.
///
/// qm writes the file and the record together, so a record naming a file that is not there means
/// the output directory was tampered with or a write failed, and installing the rest as if it were
/// complete is the silent partial success this module exists to remove.
fn artifact(dir: &Path, relative: &str, shipment: &str) -> Result<PathBuf, String> {
    let mut path = dir.to_path_buf();
    for part in relative.split('/') {
        path.push(part);
    }
    if !path.is_file() {
        return Err(format!(
            "{shipment}'s qm output records {relative} but it is not in {}",
            dir.display()
        ));
    }
    Ok(path)
}

/// Read a qm output directory whole, validating every entry of its record against the disk.
pub fn read_output(dir: &Path, shipment: &str) -> Result<QmOutput, String> {
    let record = read_placement(dir)?;
    let mut out = QmOutput::default();
    for entry in &record.placements {
        match &entry.destination {
            Destination::Overlay => {
                if out.overlay.is_some() {
                    return Err(format!(
                        "{} names more than one overlay WAD; modkit merges one overlay per qm \
                         output and cannot tell which of them to read",
                        dir.join(PLACEMENT_FILE).display()
                    ));
                }
                let name = checked_relative(&entry.name, "overlay", shipment)?;
                out.overlay = Some(artifact(dir, name, shipment)?);
            }
            Destination::GameFolder { relative } => {
                let relative = checked_relative(relative, "game-folder file", shipment)?;
                out.files.push(StagedFile {
                    source: artifact(dir, relative, shipment)?.to_string_lossy().to_string(),
                    relative: relative.to_string(),
                    sha256: entry.sha256.clone(),
                    shipment: shipment.to_string(),
                    display: None,
                    language: None,
                });
            }
            Destination::DataWad { relative, display } => {
                let relative = checked_relative(relative, "data WAD", shipment)?;
                if display.trim().is_empty() {
                    return Err(format!(
                        "{shipment} records the data WAD {relative} with an empty display name"
                    ));
                }
                out.files.push(StagedFile {
                    source: artifact(dir, relative, shipment)?.to_string_lossy().to_string(),
                    relative: relative.to_string(),
                    sha256: entry.sha256.clone(),
                    shipment: shipment.to_string(),
                    display: Some(display.clone()),
                    language: None,
                });
            }
            Destination::LanguagePatch { language, relative } => {
                if let Some(why) = refuse_bad_language(language) {
                    return Err(format!(
                        "{shipment} records a language patch for {language:?}, which modkit \
                         refuses: {why}."
                    ));
                }
                if out.language_patches.iter().any(|p| &p.language == language) {
                    return Err(format!(
                        "{shipment} records more than one language patch for {language}; modkit \
                         merges one per language per qm output and cannot tell which to read"
                    ));
                }
                let relative = checked_relative(relative, "language patch", shipment)?;
                out.language_patches.push(LanguagePatch {
                    language: language.clone(),
                    path: artifact(dir, relative, shipment)?,
                });
            }
            Destination::ShellPatch => {
                if out.shell_patch.is_some() {
                    return Err(format!(
                        "{shipment} records more than one shell patch; modkit merges one per qm \
                         output and cannot tell which to read"
                    ));
                }
                let name = checked_relative(&entry.name, "shell patch", shipment)?;
                out.shell_patch = Some(artifact(dir, name, shipment)?);
            }
            Destination::StreamCopy { from, to } => {
                let from = checked_relative(from, "stream-copy source", shipment)?;
                let to = checked_relative(to, "stream-copy destination", shipment)?;
                out.stream_copies.push(StreamCopy {
                    from: from.to_string(),
                    to: to.to_string(),
                    bytes: entry.bytes,
                    sha256: entry.sha256.clone(),
                    shipment: shipment.to_string(),
                });
            }
        }
    }
    Ok(out)
}

/// modkit's own staged-build record, written next to `vz-patch.wad` by
/// [`super::wad_builder::assemble_patch_wad`] and read by [`super::deploy_wad::deploy_patch_wad`].
///
/// Distinct from qm's record on purpose: qm describes ONE Shipment's output in ITS scratch
/// directory, while this describes the whole load order's staged files — the merged language and
/// shell patch WADs among them — with their sources already re-pointed into the build directory.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StagedBuild {
    /// Files to copy into the game folder.
    pub files: Vec<StagedFile>,
    /// Copies to make inside the game folder.
    pub stream_copies: Vec<StreamCopy>,
}

impl StagedBuild {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.stream_copies.is_empty()
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StagedRecord {
    format: u32,
    #[serde(flatten)]
    build: StagedBuild,
}

/// The staged-build record's JSON for `build`.
pub fn staged_record_json(build: &StagedBuild) -> Result<String, String> {
    serde_json::to_string_pretty(&StagedRecord {
        format: SUPPORTED_FORMAT,
        build: build.clone(),
    })
    .map_err(|e| format!("Failed to describe the staged files: {e}"))
}

/// Read the staged-build record from a build output directory.
///
/// An empty [`StagedBuild`] when there is no record: the build step removes it when a build places
/// nothing, so its absence is the statement "nothing to install". A record that is there and cannot
/// be read is refused.
pub fn read_staged(build_dir: &Path) -> Result<StagedBuild, String> {
    let path = build_dir.join(PLACEMENT_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(StagedBuild::default()),
        Err(e) => return Err(format!("reading {}: {e}", path.display())),
    };
    let record: StagedRecord = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not a staged-build record: {e}", path.display()))?;
    if record.format != SUPPORTED_FORMAT {
        return Err(format!(
            "{} is format {} — this modkit reads format {SUPPORTED_FORMAT} only.",
            path.display(),
            record.format
        ));
    }
    let refuse = |what: &str, value: &str, why: String| {
        format!(
            "{} names the {what} {value:?}, which modkit refuses: {why}.",
            path.display()
        )
    };
    for file in &record.build.files {
        if let Some(why) = refuse_unsafe_relative(&file.relative) {
            return Err(refuse("destination", &file.relative, why));
        }
        if let Some(language) = &file.language {
            if let Some(why) = refuse_bad_language(language) {
                return Err(refuse("language", language, why));
            }
        }
    }
    for copy in &record.build.stream_copies {
        if let Some(why) = refuse_unsafe_relative(&copy.from) {
            return Err(refuse("stream-copy source", &copy.from, why));
        }
        if let Some(why) = refuse_unsafe_relative(&copy.to) {
            return Err(refuse("stream-copy destination", &copy.to, why));
        }
    }
    Ok(record.build)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn write_record(dir: &Path, entries: &str) {
        write(
            dir,
            PLACEMENT_FILE,
            &format!("{{\"format\":2,\"placements\":[{entries}]}}"),
        );
    }

    /// qm writes a record into every output directory, so a directory without one is a qm run that
    /// did not complete, never "nothing produced".
    #[test]
    fn a_missing_placement_json_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "zz-quartermaster-link.wad", "wad bytes");
        let err = read_output(dir.path(), "link").unwrap_err();
        assert!(err.contains("did not complete"), "got: {err}");
    }

    /// An empty `placements` list is qm saying outright that the step produced nothing.
    #[test]
    fn an_empty_record_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write_record(dir.path(), "");
        assert!(read_output(dir.path(), "link").unwrap().is_empty());
    }

    /// WAD selection is the record's, never the directory's.
    #[test]
    fn the_record_names_the_overlay() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "aaa-decoy.wad", "decoy");
        write(dir.path(), "my-shipment.wad", "real");
        write_record(
            dir.path(),
            r#"{"name":"my-shipment.wad","bytes":4,"sha256":"ab","destination":{"kind":"overlay"}}"#,
        );
        let out = read_output(dir.path(), "s").unwrap();
        assert_eq!(out.overlay.unwrap().file_name().unwrap(), "my-shipment.wad");
    }

    /// A files-only Shipment has no overlay, and a stale WAD in the directory is not one.
    #[test]
    fn a_record_with_no_overlay_yields_no_wad() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "stale.wad", "left over");
        write(dir.path(), "scripts/hook.asi", "MZ");
        write_record(
            dir.path(),
            r#"{"name":"hook.asi","bytes":2,"sha256":"cd","destination":{"kind":"game_folder","relative":"scripts/hook.asi"}}"#,
        );
        let out = read_output(dir.path(), "s").unwrap();
        assert!(out.overlay.is_none(), "no overlay entry means no overlay");
        assert_eq!(out.files.len(), 1);
        assert!(out.files[0].is_asi());
        assert_eq!(out.files[0].dir(), "scripts");
        assert_eq!(out.files[0].file_name(), "hook.asi");
        assert_eq!(out.files[0].display, None);
    }

    /// A game-root placement has an empty directory half, and is not an `.asi`.
    #[test]
    fn a_game_root_companion_reports_no_directory() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "config.ini", "[x]");
        write_record(
            dir.path(),
            r#"{"name":"config.ini","bytes":3,"sha256":"ef","destination":{"kind":"game_folder","relative":"config.ini"}}"#,
        );
        let out = read_output(dir.path(), "s").unwrap();
        assert_eq!(out.files[0].dir(), "");
        assert!(!out.files[0].is_asi());
    }

    /// An `add_language` ships a new base WAD as a `data_wad` placement: staged like a companion,
    /// carrying the display name the deploy ledger keeps.
    #[test]
    fn a_data_wad_is_staged_with_its_display_name() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "data/polski.wad", "FFCS...");
        write_record(
            dir.path(),
            r#"{"name":"polski.wad","bytes":7,"sha256":"ab","destination":{"kind":"data_wad","relative":"data/polski.wad","display":"Polski (PL)"}}"#,
        );
        let out = read_output(dir.path(), "mercs2-language").unwrap();
        assert!(out.overlay.is_none(), "a data_wad is not an overlay");
        assert_eq!(out.files.len(), 1);
        assert_eq!(out.files[0].relative, "data/polski.wad");
        assert_eq!(out.files[0].display.as_deref(), Some("Polski (PL)"));
        assert_eq!(out.files[0].language, None);
    }

    /// `display` is required on a `data_wad`: a missing one is a parse error, an empty one a refusal.
    #[test]
    fn a_data_wad_without_a_display_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "data/polski.wad", "FFCS");
        write_record(
            dir.path(),
            r#"{"name":"polski.wad","bytes":4,"sha256":"ab","destination":{"kind":"data_wad","relative":"data/polski.wad"}}"#,
        );
        let err = read_output(dir.path(), "s").unwrap_err();
        assert!(err.contains("not a placement record") && err.contains("display"), "got: {err}");

        write_record(
            dir.path(),
            r#"{"name":"polski.wad","bytes":4,"sha256":"ab","destination":{"kind":"data_wad","relative":"data/polski.wad","display":"  "}}"#,
        );
        let err = read_output(dir.path(), "s").unwrap_err();
        assert!(err.contains("empty display name"), "got: {err}");
    }

    /// Every kind in one record: each lands in its own half of the output.
    #[test]
    fn every_kind_is_read_into_its_own_half() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "zz-quartermaster-link.wad", "overlay");
        write(dir.path(), "scripts/hook.asi", "MZ");
        write(dir.path(), "data/polski.wad", "FFCS");
        write(dir.path(), "language_patch/english.wad", "en patch");
        write(dir.path(), "language_patch/polski.wad", "pl patch");
        write(dir.path(), "shell-patch.wad", "shell");
        write_record(
            dir.path(),
            &[
                r#"{"name":"zz-quartermaster-link.wad","bytes":7,"sha256":"a1","destination":{"kind":"overlay"}}"#,
                r#"{"name":"hook.asi","bytes":2,"sha256":"a2","destination":{"kind":"game_folder","relative":"scripts/hook.asi"}}"#,
                r#"{"name":"polski.wad","bytes":4,"sha256":"a3","destination":{"kind":"data_wad","relative":"data/polski.wad","display":"Polski"}}"#,
                r#"{"name":"english.wad","bytes":8,"sha256":"a4","destination":{"kind":"language_patch","language":"english","relative":"language_patch/english.wad"}}"#,
                r#"{"name":"polski.wad","bytes":8,"sha256":"a5","destination":{"kind":"language_patch","language":"polski","relative":"language_patch/polski.wad"}}"#,
                r#"{"name":"shell-patch.wad","bytes":5,"sha256":"a6","destination":{"kind":"shell_patch"}}"#,
                r#"{"name":"vo_stream.polski.pws","bytes":1234,"sha256":"a7","destination":{"kind":"stream_copy","from":"data/Audios/vo_stream.english.pws","to":"data/Audios/vo_stream.polski.pws"}}"#,
            ]
            .join(","),
        );
        let out = read_output(dir.path(), "Lang").unwrap();
        assert_eq!(out.overlay.unwrap().file_name().unwrap(), "zz-quartermaster-link.wad");
        assert_eq!(
            out.files.iter().map(|f| f.relative.as_str()).collect::<Vec<_>>(),
            vec!["scripts/hook.asi", "data/polski.wad"]
        );
        assert_eq!(
            out.language_patches,
            vec![
                LanguagePatch {
                    language: "english".into(),
                    path: dir.path().join("language_patch").join("english.wad"),
                },
                LanguagePatch {
                    language: "polski".into(),
                    path: dir.path().join("language_patch").join("polski.wad"),
                },
            ]
        );
        assert_eq!(out.shell_patch.unwrap(), dir.path().join("shell-patch.wad"));
        assert_eq!(
            out.stream_copies,
            vec![StreamCopy {
                from: "data/Audios/vo_stream.english.pws".into(),
                to: "data/Audios/vo_stream.polski.pws".into(),
                bytes: 1234,
                sha256: "a7".into(),
                shipment: "Lang".into(),
            }]
        );
    }

    /// A language token is `[a-z0-9_]+`: it becomes a file name in the game's data folder.
    #[test]
    fn a_bad_language_token_is_refused() {
        for bad in ["English", "", "pl-PL", "../x", "polski wad"] {
            let dir = tempfile::tempdir().unwrap();
            write(dir.path(), "language_patch/x.wad", "x");
            write_record(
                dir.path(),
                &format!(
                    r#"{{"name":"x.wad","bytes":1,"sha256":"a","destination":{{"kind":"language_patch","language":{},"relative":"language_patch/x.wad"}}}}"#,
                    serde_json::to_string(bad).unwrap()
                ),
            );
            let err = read_output(dir.path(), "s").unwrap_err();
            assert!(err.contains("refuses"), "{bad:?}: {err}");
        }
        assert!(refuse_bad_language("chinese_2").is_none());
    }

    /// Two language patches for one language in one output are ambiguous, and refused.
    #[test]
    fn two_language_patches_for_one_language_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.wad", "a");
        write(dir.path(), "b.wad", "b");
        write_record(
            dir.path(),
            r#"{"name":"a.wad","bytes":1,"sha256":"a","destination":{"kind":"language_patch","language":"english","relative":"a.wad"}},
               {"name":"b.wad","bytes":1,"sha256":"b","destination":{"kind":"language_patch","language":"english","relative":"b.wad"}}"#,
        );
        let err = read_output(dir.path(), "s").unwrap_err();
        assert!(err.contains("more than one language patch for english"), "got: {err}");
    }

    /// `from` and `to` of a stream copy go through the same traversal check as every other path.
    #[test]
    fn unsafe_stream_copy_paths_are_refused() {
        for (from, to) in [
            ("../vo_stream.english.pws", "data/Audios/vo_stream.polski.pws"),
            ("data/Audios/vo_stream.english.pws", "/etc/vo_stream.polski.pws"),
            ("data\\Audios\\vo_stream.english.pws", "data/Audios/x.pws"),
            ("data/Audios/vo_stream.english.pws", "C:/x.pws"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            write_record(
                dir.path(),
                &format!(
                    r#"{{"name":"x.pws","bytes":1,"sha256":"a","destination":{{"kind":"stream_copy","from":{},"to":{}}}}}"#,
                    serde_json::to_string(from).unwrap(),
                    serde_json::to_string(to).unwrap()
                ),
            );
            let err = read_output(dir.path(), "s").unwrap_err();
            assert!(err.contains("which modkit refuses"), "{from} -> {to}: {err}");
        }
    }

    /// An unknown destination kind is a parse error, never a skipped entry.
    #[test]
    fn an_unknown_kind_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write_record(
            dir.path(),
            r#"{"name":"x","bytes":1,"sha256":"a","destination":{"kind":"data_file","relative":"x"}}"#,
        );
        let err = read_output(dir.path(), "s").unwrap_err();
        assert!(err.contains("not a placement record") && err.contains("data_file"), "got: {err}");
    }

    /// A record that cannot be parsed, or of any format but 2, is a refusal.
    #[test]
    fn a_malformed_or_format_1_record_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), PLACEMENT_FILE, "{ not json");
        assert!(read_placement(dir.path()).unwrap_err().contains("not a placement record"));

        for format in [1, 99] {
            let dir = tempfile::tempdir().unwrap();
            write(
                dir.path(),
                PLACEMENT_FILE,
                &format!(r#"{{"format":{format},"placements":[]}}"#),
            );
            let err = read_placement(dir.path()).unwrap_err();
            assert!(err.contains(&format!("format {format}")), "got: {err}");
        }
    }

    /// A record naming a file qm did not write is a refusal — installing the rest would be a
    /// partial success reported as a whole one.
    #[test]
    fn a_record_naming_a_missing_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write_record(
            dir.path(),
            r#"{"name":"gone.asi","bytes":1,"sha256":"aa","destination":{"kind":"game_folder","relative":"scripts/gone.asi"}}"#,
        );
        let err = read_output(dir.path(), "s").unwrap_err();
        assert!(err.contains("it is not in"), "got: {err}");

        let dir = tempfile::tempdir().unwrap();
        write_record(
            dir.path(),
            r#"{"name":"shell-patch.wad","bytes":1,"sha256":"aa","destination":{"kind":"shell_patch"}}"#,
        );
        let err = read_output(dir.path(), "s").unwrap_err();
        assert!(err.contains("it is not in"), "got: {err}");
    }

    /// The staged record round-trips, and its paths and language tokens are re-checked on read.
    #[test]
    fn the_staged_record_round_trips_and_is_rechecked() {
        let dir = tempfile::tempdir().unwrap();
        let build = StagedBuild {
            files: vec![StagedFile {
                source: "/b/files/data/english-patch.wad".into(),
                relative: "data/english-patch.wad".into(),
                sha256: "ab".into(),
                shipment: "A, B".into(),
                display: None,
                language: Some("english".into()),
            }],
            stream_copies: vec![StreamCopy {
                from: "data/Audios/vo_stream.english.pws".into(),
                to: "data/Audios/vo_stream.polski.pws".into(),
                bytes: 3,
                sha256: "cd".into(),
                shipment: "Lang".into(),
            }],
        };
        write(dir.path(), PLACEMENT_FILE, &staged_record_json(&build).unwrap());
        let back = read_staged(dir.path()).unwrap();
        assert_eq!(back.files[0].language.as_deref(), Some("english"));
        assert_eq!(back.stream_copies, build.stream_copies);

        let mut bad = build.clone();
        bad.stream_copies[0].to = "../escape.pws".into();
        write(dir.path(), PLACEMENT_FILE, &staged_record_json(&bad).unwrap());
        assert!(read_staged(dir.path()).unwrap_err().contains("stream-copy destination"));

        let mut bad = build;
        bad.files[0].language = Some("English".into());
        write(dir.path(), PLACEMENT_FILE, &staged_record_json(&bad).unwrap());
        assert!(read_staged(dir.path()).unwrap_err().contains("language"));

        write(dir.path(), PLACEMENT_FILE, r#"{"format":1,"files":[],"stream_copies":[]}"#);
        assert!(read_staged(dir.path()).unwrap_err().contains("format 1"));
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
