//! One answer to "what is installed, is it current, and is it still ours?"
//!
//! Three components each answered this differently, and two of them answered it
//! from the browser. `checkComponentUpdates` made one GitHub call per component
//! and compared each release tag against a `localStorage` string; the toolset made
//! its own call and compared with `!=`; `apply_crack` was special-cased out of the
//! UI entirely because, being re-downloaded on every use, "up to date" was not a
//! state it could be in.

use serde::Serialize;

use super::{is_newer, ledger::Component, pmc_bb, Ledger};
use crate::commands::net::{self, ReleaseHost};

/// A managed artifact modkit knows how to install and keep current.
pub struct Managed {
    pub key: &'static str,
    pub label: &'static str,
    pub repo: &'static str,
}

/// Everything under the ledger's care.
///
/// The Workshop toolset is deliberately absent: it is one release of eleven
/// binaries with its own version-directory layout, reported by `toolset_status`.
/// Folding it in here would mean either flattening eleven rows into one or
/// teaching this table about per-tool assets, and neither buys anything the Tools
/// page does not already show.
pub const MANAGED: &[Managed] = &[
    Managed {
        key: "pmc_bb",
        label: "pmc_bb.dll (ASI loader / logging bridge)",
        repo: pmc_bb::REPO,
    },
    Managed {
        key: "dxwrapper",
        label: "dxwrapper",
        repo: "elishacloud/dxwrapper",
    },
    Managed {
        key: "apply_crack",
        label: "apply_crack (SecuROM bypass)",
        repo: "Mercenaries-Fan-Build/mercs2-securom-bypass",
    },
];

/// One row for the UI.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentStatus {
    pub key: String,
    pub label: String,
    pub repo: String,
    /// Release tag modkit installed, or `None` when modkit did not install it.
    pub installed_tag: Option<String>,
    /// Release asset installed — which of six pmc_bb builds, for instance.
    pub installed_asset: Option<String>,
    pub features: Vec<String>,
    /// Latest published tag; `None` when the lookup was skipped or failed, which
    /// is not an error — the page still renders offline.
    pub latest_tag: Option<String>,
    pub update_available: bool,
    /// Every recorded file is still on disk.
    pub present: bool,
    /// Every recorded file still has the bytes modkit wrote. `false` with
    /// `present` means somebody replaced it by hand — an ordinary thing to do,
    /// and previously invisible.
    pub modified: bool,
    /// Release page, for "what changed".
    pub url: Option<String>,
}

fn row(m: &Managed, installed: Option<&Component>) -> ComponentStatus {
    ComponentStatus {
        key: m.key.to_string(),
        label: m.label.to_string(),
        repo: m.repo.to_string(),
        installed_tag: installed.map(|c| c.tag.clone()),
        installed_asset: installed.map(|c| c.asset.clone()),
        features: installed.map(|c| c.features.clone()).unwrap_or_default(),
        latest_tag: None,
        update_available: false,
        present: installed.is_some_and(|c| c.is_present()),
        modified: installed.is_some_and(|c| c.is_present() && !c.is_intact()),
        url: None,
    }
}

/// Status for every managed component.
///
/// With `check_remote`, each component's latest release is looked up; a failed
/// lookup degrades that row to `latest_tag: null` rather than failing the call, so
/// an offline launch still reports what is installed.
///
/// Note what is *not* gated here. The old check computed
/// `available: !!current && semverGt(latest, current)`, so an unknown installed
/// version made the update silently unavailable forever — the single most common
/// state, since the version lived in browser storage. An unknown installed version
/// now still reports the latest release; the UI can offer a reinstall, which is
/// the action that makes it knowable.
#[tauri::command]
pub async fn managed_status(check_remote: bool) -> Result<Vec<ComponentStatus>, String> {
    let ledger = Ledger::app()?;
    let installed = ledger.read();

    let mut rows: Vec<ComponentStatus> = MANAGED
        .iter()
        .map(|m| row(m, installed.get(m.key)))
        .collect();

    if !check_remote {
        return Ok(rows);
    }

    let Ok(client) = net::client() else {
        return Ok(rows);
    };
    for (m, r) in MANAGED.iter().zip(rows.iter_mut()) {
        if let Ok(release) = net::latest_release(&client, ReleaseHost::GitHub, m.repo).await {
            r.update_available = r
                .installed_tag
                .as_deref()
                .is_some_and(|cur| is_newer(cur, &release.tag));
            r.url = Some(release.url);
            r.latest_tag = Some(release.tag);
        }
    }

    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::managed::ledger::InstalledFile;
    use crate::commands::managed::place::sha256_hex;
    use std::path::Path;

    fn component(tag: &str, files: Vec<InstalledFile>) -> Component {
        Component {
            key: "pmc_bb".into(),
            tag: tag.into(),
            asset: "pmc_bb_log_only.dll".into(),
            features: vec!["log".into()],
            source: pmc_bb::REPO.into(),
            installed_at: 0,
            files,
        }
    }

    fn file_at(path: &Path, body: &[u8]) -> InstalledFile {
        std::fs::write(path, body).unwrap();
        InstalledFile {
            abs_path: path.to_string_lossy().to_string(),
            sha256: sha256_hex(body),
            size: body.len() as u64,
            backup: None,
        }
    }

    #[test]
    fn every_managed_component_has_a_distinct_key_and_a_repo() {
        let mut keys: Vec<_> = MANAGED.iter().map(|m| m.key).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len(), "two components share a key");

        for m in MANAGED {
            assert!(m.repo.contains('/'), "{} has no owner/repo", m.key);
            assert!(!m.label.is_empty(), "{} has no label", m.key);
        }
    }

    /// apply_crack used to be excluded from the update UI because nothing recorded
    /// a version for it, so "up to date" was not a state it could be in.
    #[test]
    fn apply_crack_is_an_ordinary_component_now() {
        assert!(
            MANAGED.iter().any(|m| m.key == "apply_crack"),
            "apply_crack must be reportable like anything else"
        );
    }

    #[test]
    fn an_uninstalled_component_reports_nothing_installed() {
        let m = &MANAGED[0];
        let r = row(m, None);
        assert_eq!(r.installed_tag, None);
        assert!(!r.present);
        assert!(!r.modified);
        assert!(r.features.is_empty());
    }

    #[test]
    fn an_intact_install_is_present_and_unmodified() {
        let dir = tempfile::tempdir().unwrap();
        let dll = dir.path().join("pmc_bb.dll");
        let c = component("v0.6.0", vec![file_at(&dll, b"the build")]);

        let r = row(&MANAGED[0], Some(&c));
        assert_eq!(r.installed_tag.as_deref(), Some("v0.6.0"));
        assert_eq!(r.installed_asset.as_deref(), Some("pmc_bb_log_only.dll"));
        assert!(r.present);
        assert!(!r.modified);
    }

    /// The state localStorage could never represent: still there, no longer ours.
    #[test]
    fn a_hand_replaced_file_reports_present_and_modified() {
        let dir = tempfile::tempdir().unwrap();
        let dll = dir.path().join("pmc_bb.dll");
        let c = component("v0.6.0", vec![file_at(&dll, b"the build")]);
        std::fs::write(&dll, b"somebody else's build").unwrap();

        let r = row(&MANAGED[0], Some(&c));
        assert!(r.present);
        assert!(r.modified, "a hand-swapped DLL must be visible as such");
    }

    #[test]
    fn a_deleted_file_reports_absent() {
        let dir = tempfile::tempdir().unwrap();
        let dll = dir.path().join("pmc_bb.dll");
        let c = component("v0.6.0", vec![file_at(&dll, b"the build")]);
        std::fs::remove_file(&dll).unwrap();

        let r = row(&MANAGED[0], Some(&c));
        assert!(!r.present);
        assert!(!r.modified, "gone is not modified");
    }
}
