//! `manifest.json` schema — the contract a mod author ships with their assets.

use serde::{Deserialize, Serialize};

fn default_auto() -> String {
    "auto".to_string()
}

/// One asset declared in a mod's `manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestAsset {
    /// Path to the asset file, relative to the mod root.
    pub path: String,
    /// Logical asset name used to compute the `pandemic_hash_m2`
    /// (e.g. `"models/vehicle_01"`). This is the key the engine resolves by.
    pub name: String,
    /// Asset type: `"auto"` (detect from contents/extension) or an explicit
    /// type (`"model"`, `"texture"`, `"script"`, `"stringdb"`, `"animation"`, `"sound"`).
    #[serde(default = "default_auto", rename = "type")]
    pub asset_type: String,
    /// Target patch group: `"auto"` (tool decides) or a named group.
    #[serde(default = "default_auto")]
    pub target_patch: String,
}

/// Engine/game requirements a mod declares.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ManifestRequirements {
    /// Required base game version, e.g. `"1.1"`.
    #[serde(default)]
    pub game_version: Option<String>,
}

/// Top-level `manifest.json` document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub requirements: ManifestRequirements,
    /// Other mods this one depends on, as `"name@semver-range"` strings.
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub assets: Vec<ManifestAsset>,
}
