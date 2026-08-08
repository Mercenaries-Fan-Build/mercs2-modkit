//! What modkit installed, recorded on the side that did the installing.
//!
//! # Why this is not `localStorage`
//!
//! `pmc_bb`, `dxwrapper` and `apply_crack` recorded their installed version in the
//! **browser**, written by the Vue store after a successful `invoke`. Four things
//! follow from that, all of them bugs:
//!
//! * A cleared profile, a new machine, or a hand-installed file leaves `current`
//!   null — and the update check reads `available: !!current && semverGt(…)`, so it
//!   silently never fires. "Version unknown" was the most common state.
//! * It was never checked against the disk. It recorded what modkit *believed* it
//!   installed, which stays believed after somebody replaces the file by hand.
//! * It could not represent which **variant** was installed. `install_pmc_bb` and
//!   `install_pmc_bb_log` write the same destination and stamped the same key, so
//!   the two were indistinguishable after the fact — and now that pmc-blackbox
//!   publishes six feature variants under one install name, the on-disk filename
//!   cannot answer it either.
//! * `apply_crack` had no record at all, so it was re-downloaded on every crack.
//!
//! The toolset already did this correctly, in `installed.json` beside its binaries.
//! This generalizes that sidecar to every managed artifact, and adds the per-file
//! digest that makes "installed, but modified since" a state modkit can *see*
//! rather than assume away.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::place::{sha256_of_file, Placed};

/// Bumped only for a change old modkits cannot read. A ledger from the future is
/// treated as absent rather than misread — see [`Ledger::read_at`].
const FORMAT: u32 = 1;

/// One file modkit put somewhere.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledFile {
    pub abs_path: String,
    pub sha256: String,
    pub size: u64,
    /// Where the file it displaced was banked, if any.
    #[serde(default)]
    pub backup: Option<String>,
}

impl From<Placed> for InstalledFile {
    fn from(p: Placed) -> Self {
        Self {
            abs_path: p.abs_path,
            sha256: p.sha256,
            size: p.size,
            backup: p.backup,
        }
    }
}

/// One managed artifact, as installed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    /// Stable key: `pmc_bb`, `dxwrapper`, `apply_crack`, `toolset`.
    pub key: String,
    /// Release tag this came from.
    pub tag: String,
    /// The release asset actually downloaded, e.g. `pmc_bb_asi_log.dll`.
    ///
    /// Recorded because the install name is modkit's choice, not the artifact's:
    /// every pmc_bb variant is installed as `pmc_bb.dll` so the exe's import table
    /// and dxwrapper's `LoadCustomDllPath` resolve. Without this field the six
    /// variants are indistinguishable on disk.
    pub asset: String,
    /// Resolved feature bits, so the UI can explain *why* this variant.
    #[serde(default)]
    pub features: Vec<String>,
    /// `owner/repo` it came from.
    pub source: String,
    pub installed_at: u64,
    pub files: Vec<InstalledFile>,
}

impl Component {
    /// Whether every recorded file is still on disk with the bytes modkit wrote.
    ///
    /// A `false` here is not an error — replacing a plugin by hand is an ordinary
    /// thing to do. It is a state the UI should be able to *show*, instead of
    /// reporting a version that stopped being true.
    pub fn is_intact(&self) -> bool {
        self.files.iter().all(|f| {
            let p = Path::new(&f.abs_path);
            p.is_file() && sha256_of_file(p).map(|h| h == f.sha256).unwrap_or(false)
        })
    }

    /// Whether any recorded file has gone missing entirely.
    pub fn is_present(&self) -> bool {
        !self.files.is_empty() && self.files.iter().all(|f| Path::new(&f.abs_path).is_file())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Document {
    format: u32,
    #[serde(default)]
    components: BTreeMap<String, Component>,
}

/// The on-disk record. Holds its own path so it can be pointed at a temp dir.
pub struct Ledger {
    path: PathBuf,
}

impl Ledger {
    /// The real one, under the app's managed area.
    pub fn app() -> Result<Self, String> {
        Ok(Self {
            path: super::managed_dir()?.join("installed.json"),
        })
    }

    pub fn at(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }

    /// Read the record. Absent, unreadable, corrupt, or written by a newer modkit
    /// all read as **empty**.
    ///
    /// Refusing to install because previous bookkeeping is unparseable helps
    /// nobody: the next install rewrites it, and the files it describes are still
    /// verifiable from disk. What it must never do is claim files exist that do
    /// not, which is why every entry is re-checked before it is believed.
    pub fn read(&self) -> BTreeMap<String, Component> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return BTreeMap::new();
        };
        match serde_json::from_str::<Document>(&text) {
            Ok(doc) if doc.format <= FORMAT => doc.components,
            _ => BTreeMap::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<Component> {
        self.read().remove(key)
    }

    /// Write via a temp file and rename, so an interrupted write cannot leave a
    /// half-parsed record pointing at files that do not exist.
    fn write_all(&self, components: BTreeMap<String, Component>) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(&Document {
            format: FORMAT,
            components,
        })
        .map_err(|e| format!("Could not describe the installed components: {e}"))?;

        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text)
            .map_err(|e| format!("Could not write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| format!("Could not update {}: {e}", self.path.display()))
    }

    /// Record `component`, replacing any previous entry under its key.
    pub fn record(&self, component: Component) -> Result<(), String> {
        let mut all = self.read();
        all.insert(component.key.clone(), component);
        self.write_all(all)
    }

    /// Drop the entry for `key`. Returns what was there.
    pub fn forget(&self, key: &str) -> Result<Option<Component>, String> {
        let mut all = self.read();
        let gone = all.remove(key);
        if gone.is_some() {
            self.write_all(all)?;
        }
        Ok(gone)
    }
}

/// Seconds since the epoch, for `installed_at`.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component(key: &str, tag: &str, asset: &str, files: Vec<InstalledFile>) -> Component {
        Component {
            key: key.into(),
            tag: tag.into(),
            asset: asset.into(),
            features: vec!["log".into()],
            source: "Mercenaries-Fan-Build/pmc-blackbox".into(),
            installed_at: 1_700_000_000,
            files,
        }
    }

    fn file_at(path: &Path, body: &[u8]) -> InstalledFile {
        std::fs::write(path, body).unwrap();
        InstalledFile {
            abs_path: path.to_string_lossy().to_string(),
            sha256: super::super::place::sha256_hex(body),
            size: body.len() as u64,
            backup: None,
        }
    }

    #[test]
    fn a_component_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::at(&dir.path().join("installed.json"));

        let c = component("pmc_bb", "v0.6.0", "pmc_bb_asi_log.dll", vec![]);
        ledger.record(c).unwrap();

        let got = ledger.get("pmc_bb").expect("recorded");
        assert_eq!(got.tag, "v0.6.0");
        assert_eq!(
            got.asset, "pmc_bb_asi_log.dll",
            "the variant is the point — the install name cannot carry it"
        );
        assert_eq!(got.features, vec!["log".to_string()]);
    }

    #[test]
    fn recording_replaces_rather_than_accumulates() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::at(&dir.path().join("installed.json"));

        ledger
            .record(component("pmc_bb", "v0.6.0", "pmc_bb_asi_log.dll", vec![]))
            .unwrap();
        ledger
            .record(component("pmc_bb", "v0.7.0", "pmc_bb_fully_loaded.dll", vec![]))
            .unwrap();

        assert_eq!(ledger.read().len(), 1);
        let got = ledger.get("pmc_bb").unwrap();
        assert_eq!(got.tag, "v0.7.0");
        assert_eq!(got.asset, "pmc_bb_fully_loaded.dll");
    }

    #[test]
    fn components_are_independent() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::at(&dir.path().join("installed.json"));

        ledger.record(component("pmc_bb", "v0.6.0", "a.dll", vec![])).unwrap();
        ledger.record(component("dxwrapper", "v2.0", "dx9.zip", vec![])).unwrap();

        assert_eq!(ledger.read().len(), 2);
        ledger.forget("pmc_bb").unwrap();
        assert!(ledger.get("pmc_bb").is_none());
        assert!(ledger.get("dxwrapper").is_some(), "an unrelated entry survived");
    }

    /// Unreadable bookkeeping must not be able to block an install.
    #[test]
    fn a_corrupt_ledger_reads_as_empty_and_is_recoverable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("installed.json");
        std::fs::write(&path, b"{ this is not json").unwrap();

        let ledger = Ledger::at(&path);
        assert!(ledger.read().is_empty());

        ledger.record(component("pmc_bb", "v0.6.0", "a.dll", vec![])).unwrap();
        assert_eq!(ledger.get("pmc_bb").unwrap().tag, "v0.6.0");
    }

    /// A record written by a newer modkit is not guessed at.
    #[test]
    fn a_future_format_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("installed.json");
        std::fs::write(&path, br#"{"format":99,"components":{"pmc_bb":{}}}"#).unwrap();
        assert!(Ledger::at(&path).read().is_empty());
    }

    #[test]
    fn a_missing_ledger_is_simply_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Ledger::at(&dir.path().join("nope.json")).read().is_empty());
    }

    #[test]
    fn intactness_tracks_the_bytes_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let dll = dir.path().join("pmc_bb.dll");
        let c = component("pmc_bb", "v0.6.0", "x.dll", vec![file_at(&dll, b"installed")]);

        assert!(c.is_intact());
        assert!(c.is_present());

        // Somebody swaps the DLL by hand — an ordinary thing to do, and previously
        // invisible: the recorded version kept claiming to describe this file.
        std::fs::write(&dll, b"something else").unwrap();
        assert!(!c.is_intact(), "a hand-modified file must be detectable");
        assert!(c.is_present(), "it is still there, just not ours");

        std::fs::remove_file(&dll).unwrap();
        assert!(!c.is_present());
    }

    #[test]
    fn a_component_with_no_files_is_not_present() {
        let c = component("pmc_bb", "v0.6.0", "x.dll", vec![]);
        assert!(!c.is_present(), "nothing recorded means nothing installed");
    }

    #[test]
    fn no_tmp_file_is_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::at(&dir.path().join("installed.json"));
        ledger.record(component("pmc_bb", "v1", "a.dll", vec![])).unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "left behind {leftovers:?}");
    }
}
