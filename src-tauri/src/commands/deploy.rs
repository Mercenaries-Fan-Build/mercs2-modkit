//! Deploy staged `.asi` plugins into the game install for the ASI loader.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
    /// Existing files that were backed up to `<name>.bak` first.
    pub backed_up: Vec<String>,
}

fn basename(rel: &str) -> &str {
    rel.rsplit(['/', '\\']).next().unwrap_or(rel)
}

/// Copy a mod's staged `.asi` plugins into the chosen ASI loader folder,
/// backing up any file already present at the destination.
#[tauri::command]
pub fn deploy_asi(args: DeployAsiArgs) -> Result<DeployResult, String> {
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

        // Back up an existing file before overwriting it.
        if dest.exists() {
            let backup = dest.with_extension("asi.bak");
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
