//! Load a mod from disk: parse `manifest.json`, detect asset types, compute hashes.

use std::path::PathBuf;

use mercs2_formats::hash::pandemic_hash_m2;
use serde::Serialize;

use crate::commands::asset_catalog::detect_type_for;
use crate::models::manifest::Manifest;
use crate::models::project::{DetectedAsset, LoadedMod};

/// Derive a stable kebab-case id from a mod name.
fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in name.chars() {
        if c.is_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Default patch group for an asset type when the manifest says `"auto"`.
pub fn default_patch_for(detected_type: &str) -> String {
    match detected_type {
        "script" | "stringdb" => "scripts".to_string(),
        _ => "assets".to_string(),
    }
}

/// Load a mod directory: reads `<path>/manifest.json` and resolves each asset.
#[tauri::command]
pub fn load_mod(path: String) -> Result<LoadedMod, String> {
    let root = PathBuf::from(&path);
    let manifest_path = root.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path).map_err(|e| {
        format!(
            "Failed to read manifest.json at {}: {e}",
            manifest_path.display()
        )
    })?;
    let manifest: Manifest =
        serde_json::from_str(&raw).map_err(|e| format!("Invalid manifest.json: {e}"))?;

    let mut assets = Vec::new();
    for a in &manifest.assets {
        let abs = root.join(&a.path);
        let detected_type = if a.asset_type == "auto" {
            detect_type_for(&abs)
        } else {
            a.asset_type.clone()
        };
        let target_patch = if a.target_patch == "auto" {
            default_patch_for(&detected_type)
        } else {
            a.target_patch.clone()
        };
        assets.push(DetectedAsset {
            path: a.path.clone(),
            abs_path: abs.to_string_lossy().to_string(),
            name: a.name.clone(),
            asset_hash: pandemic_hash_m2(&a.name),
            detected_type,
            target_patch,
        });
    }

    Ok(LoadedMod {
        id: slugify(&manifest.name),
        root: root.to_string_lossy().to_string(),
        manifest,
        assets,
    })
}

/// A single manifest validation problem.
#[derive(Serialize)]
pub struct ValidationError {
    pub field: String,
    pub message: String,
}

fn is_semverish(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() >= 2
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// Validate a manifest's required fields. Returns an empty vec when valid.
#[tauri::command]
pub fn validate_manifest(manifest: Manifest) -> Vec<ValidationError> {
    let mut errs = Vec::new();
    if manifest.name.trim().is_empty() {
        errs.push(ValidationError {
            field: "name".into(),
            message: "Mod name is required".into(),
        });
    }
    if !is_semverish(&manifest.version) {
        errs.push(ValidationError {
            field: "version".into(),
            message: format!("Version '{}' is not valid (expected x.y.z)", manifest.version),
        });
    }
    if manifest.assets.is_empty() {
        errs.push(ValidationError {
            field: "assets".into(),
            message: "Mod declares no assets".into(),
        });
    }
    errs
}
