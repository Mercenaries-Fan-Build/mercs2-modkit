//! Deploy staged `.asi` plugins into the game install for the ASI loader,
//! and remove (trash) deployed files back out of it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::commands::paths::trash_dir;

/// Folders the ASI loader scans; the deploy target must be one of these.
const VALID_TARGETS: &[&str] = &[".", "scripts", "plugins", "update"];

#[derive(Debug, Deserialize)]
pub struct DeployAsiArgs {
    /// Staging directory holding the plugin files.
    pub mod_root: String,
    /// `.asi` files to deploy, relative to `mod_root`.
    pub asi_files: Vec<String>,
    /// The game install root.
    pub game_root: String,
    /// One of `.`, `scripts`, `plugins`, `update`.
    pub target: String,
}

#[derive(Debug, Serialize)]
pub struct DeployResult {
    /// Absolute destination folder the plugins were copied into.
    pub target_dir: String,
    /// File names deployed (basename at the target).
    pub deployed: Vec<String>,
    /// Existing files that were backed up to `<name>.bak` first (an older `.bak` goes to the trash).
    pub backed_up: Vec<String>,
}

fn basename(rel: &str) -> &str {
    rel.rsplit(['/', '\\']).next().unwrap_or(rel)
}

/// Copy a mod's staged `.asi` plugins into the chosen ASI loader folder,
/// backing up any file already present at the destination.
#[tauri::command(async)]
pub fn deploy_asi(args: DeployAsiArgs) -> Result<DeployResult, String> {
    deploy_asi_with_trash(args, None)
}

/// [`deploy_asi`], with the trash an old `<name>.asi.bak` is moved to (`None` = the app's trash).
fn deploy_asi_with_trash(args: DeployAsiArgs, trash: Option<&Path>) -> Result<DeployResult, String> {
    if !VALID_TARGETS.contains(&args.target.as_str()) {
        return Err(format!(
            "Invalid ASI target '{}': expected one of {:?}",
            args.target, VALID_TARGETS
        ));
    }

    let game_root = PathBuf::from(&args.game_root);
    if !game_root.is_dir() {
        return Err(format!("Game folder not found: {}", args.game_root));
    }
    let mod_root = PathBuf::from(&args.mod_root);

    let target_dir: PathBuf = if args.target == "." {
        game_root
    } else {
        game_root.join(&args.target)
    };
    std::fs::create_dir_all(&target_dir)
        .map_err(|e| format!("Failed to create {}: {e}", target_dir.display()))?;

    let mut deployed = Vec::new();
    let mut backed_up = Vec::new();

    for rel in &args.asi_files {
        let src = mod_root.join(rel);
        if !src.is_file() {
            return Err(format!("Staged plugin missing: {}", src.display()));
        }
        let name = basename(rel);
        let dest = target_dir.join(name);

        // Back up an existing file before overwriting it. An older backup in the way goes to the
        // recoverable trash first, so it is kept and never stops the deploy.
        if dest.exists() {
            let backup = dest.with_extension("asi.bak");
            if backup.exists() {
                crate::commands::managed::trash::discard(&backup, trash).map_err(|e| {
                    format!("Failed to move the old {} to the trash: {e}", backup.display())
                })?;
            }
            std::fs::rename(&dest, &backup)
                .map_err(|e| format!("Failed to back up {}: {e}", dest.display()))?;
            backed_up.push(name.to_string());
        }

        std::fs::copy(&src, &dest)
            .map_err(|e| format!("Failed to deploy {name}: {e}"))?;
        deployed.push(name.to_string());
    }

    deployed.sort();
    Ok(DeployResult {
        target_dir: dir_string(&target_dir),
        deployed,
        backed_up,
    })
}

fn dir_string(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

#[derive(Debug, Serialize)]
pub struct TrashResult {
    /// Basenames successfully removed.
    pub trashed: Vec<String>,
    /// Requested paths that didn't exist (already gone).
    pub missing: Vec<String>,
    /// Where files were moved, or `None` when permanently deleted.
    pub trash_dir: Option<String>,
}

/// Force-remove deployed plugin files. By default each file is **moved to the
/// recoverable trash dir** (timestamped, so re-removing the same name never
/// clobbers a prior copy); with `permanent` it is deleted outright. Operates on
/// absolute paths (typically from the detected `deployed_asi` list), so it can
/// remove any plugin in the game folder — managed or orphaned.
#[tauri::command(async)]
pub fn trash_paths(paths: Vec<String>, permanent: bool) -> Result<TrashResult, String> {
    let dir = if permanent { None } else { Some(trash_dir()?) };

    let mut trashed = Vec::new();
    let mut missing = Vec::new();

    for p in paths.iter() {
        let src = Path::new(p);
        let name = src
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();

        if !src.exists() {
            missing.push(name);
            continue;
        }

        match &dir {
            Some(d) => {
                crate::commands::managed::trash::discard(src, Some(d))?;
            }
            None => {
                std::fs::remove_file(src)
                    .map_err(|e| format!("Failed to delete {name}: {e}"))?;
            }
        }
        trashed.push(name);
    }

    trashed.sort();
    Ok(TrashResult {
        trashed,
        missing,
        trash_dir: dir.map(|d| dir_string(&d)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files_in(dir: &Path) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let p = e.unwrap().path();
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                (name, std::fs::read_to_string(&p).unwrap())
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn an_old_asi_backup_in_the_way_is_trashed_and_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        let scripts = game.join("scripts");
        let staged = tmp.path().join("staged");
        let trash = tmp.path().join("trash");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("hook.asi"), "ours").unwrap();
        std::fs::write(scripts.join("hook.asi"), "foreign v2").unwrap();
        std::fs::write(scripts.join("hook.asi.bak"), "foreign v1").unwrap();

        let result = deploy_asi_with_trash(
            DeployAsiArgs {
                mod_root: staged.to_string_lossy().to_string(),
                asi_files: vec!["hook.asi".into()],
                game_root: game.to_string_lossy().to_string(),
                target: "scripts".into(),
            },
            Some(&trash),
        )
        .unwrap();

        assert_eq!(result.deployed, vec!["hook.asi".to_string()]);
        assert_eq!(result.backed_up, vec!["hook.asi".to_string()]);
        assert_eq!(
            files_in(&scripts),
            vec![
                ("hook.asi".to_string(), "ours".to_string()),
                ("hook.asi.bak".to_string(), "foreign v2".to_string()),
            ]
        );
        let trashed: Vec<String> = files_in(&trash).into_iter().map(|(_, body)| body).collect();
        assert_eq!(trashed, vec!["foreign v1".to_string()]);
    }
}
