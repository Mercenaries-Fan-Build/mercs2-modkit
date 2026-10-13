//! Taking a managed component back out of the game folder.
//!
//! Only the files the ledger records are touched, and only while they still hold the bytes
//! modkit wrote: a recorded file changed since is the player's now, so it stays where it is and is
//! reported. Nothing is put back in place of a removed file; what an install displaced stays in
//! the trash, where [`super::trash::bank`] left it. Once the files are dealt with the ledger entry
//! is dropped, so status, updates and the launch plan stop treating the component as modkit's.
//!
//! A failure part-way stops with the entry still recorded. Running the removal again is safe:
//! the files already trashed read as gone.

use std::path::Path;

use serde::Serialize;

use super::ledger::Ledger;
use super::place::sha256_of_file;
use super::{pmc_bb, trash};
use crate::commands::dxwrapper;

/// The components the app offers to remove.
const REMOVABLE: [&str; 2] = [pmc_bb::LEDGER_KEY, dxwrapper::KEY];

/// What a removal did with each recorded file.
#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct ComponentRemoval {
    /// Moved to the trash.
    pub removed: Vec<String>,
    /// Changed since modkit wrote it, so left in place.
    pub kept_modified: Vec<String>,
    /// Already gone.
    pub missing: Vec<String>,
}

/// Remove `key`'s recorded files and forget it. See the module docs.
pub fn remove_component(
    ledger: &Ledger,
    key: &str,
    trash_into: Option<&Path>,
) -> Result<ComponentRemoval, String> {
    let component = ledger
        .get(key)
        .ok_or_else(|| format!("Modkit has no record of installing {key}, so there is nothing of its to remove."))?;
    let mut out = ComponentRemoval::default();
    for file in &component.files {
        let path = Path::new(&file.abs_path);
        if !path.is_file() {
            out.missing.push(file.abs_path.clone());
        } else if sha256_of_file(path)? != file.sha256 {
            out.kept_modified.push(file.abs_path.clone());
        } else {
            trash::discard(path, trash_into)?;
            out.removed.push(file.abs_path.clone());
        }
    }
    ledger.forget(key)?;
    Ok(out)
}

/// Remove pmc_bb or dxwrapper from the game folder.
#[tauri::command(async)]
pub fn remove_managed_component(key: String) -> Result<ComponentRemoval, String> {
    if !REMOVABLE.contains(&key.as_str()) {
        return Err(format!("{key} can't be removed from the app."));
    }
    remove_component(&Ledger::app()?, &key, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::managed::ledger::{Component, InstalledFile};
    use crate::commands::managed::place::sha256_hex;

    fn file(path: &Path, bytes: &[u8]) -> InstalledFile {
        std::fs::write(path, bytes).unwrap();
        InstalledFile {
            abs_path: path.to_string_lossy().into(),
            sha256: sha256_hex(bytes),
            size: bytes.len() as u64,
            backup: None,
        }
    }

    /// dxwrapper's three files: one untouched, one edited by hand, one already gone. Only the
    /// untouched one is trashed, the edited one stays, and the record is dropped.
    #[test]
    fn only_files_still_as_modkit_wrote_them_are_trashed() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        std::fs::create_dir_all(&game).unwrap();
        let bin = tmp.path().join("trash");
        let stub = file(&game.join("d3d9.dll"), b"stub");
        let ini = file(&game.join("dxwrapper.ini"), b"LoadPlugins = 1");
        let dll = file(&game.join("dxwrapper.dll"), b"dll");
        std::fs::write(game.join("dxwrapper.ini"), b"LoadPlugins = 0").unwrap();
        std::fs::remove_file(game.join("dxwrapper.dll")).unwrap();

        let ledger = Ledger::at(&tmp.path().join("installed.json"));
        ledger
            .record(Component {
                key: dxwrapper::KEY.into(),
                tag: "v1".into(),
                asset: "dxwrapper.zip".into(),
                features: vec![],
                source: "elishacloud/dxwrapper".into(),
                installed_at: 0,
                files: vec![stub.clone(), ini.clone(), dll.clone()],
            })
            .unwrap();

        let out = remove_component(&ledger, dxwrapper::KEY, Some(&bin)).unwrap();
        assert_eq!(
            out,
            ComponentRemoval {
                removed: vec![stub.abs_path],
                kept_modified: vec![ini.abs_path],
                missing: vec![dll.abs_path],
            }
        );
        assert!(!game.join("d3d9.dll").exists());
        assert_eq!(std::fs::read(game.join("dxwrapper.ini")).unwrap(), b"LoadPlugins = 0");
        assert_eq!(std::fs::read_dir(&bin).unwrap().count(), 1);
        assert!(ledger.get(dxwrapper::KEY).is_none(), "the record is dropped");
    }

    /// With no record there is nothing to remove, and that is said rather than reported as done.
    #[test]
    fn an_unrecorded_component_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = Ledger::at(&tmp.path().join("installed.json"));
        assert!(remove_component(&ledger, pmc_bb::LEDGER_KEY, None).is_err());
    }

    #[test]
    fn only_pmc_bb_and_dxwrapper_are_removable_from_the_app() {
        assert!(remove_managed_component("apply_crack".into()).is_err());
    }
}
