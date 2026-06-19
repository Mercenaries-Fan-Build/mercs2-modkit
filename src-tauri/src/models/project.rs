//! In-memory project model: a loaded mod and its detected assets.

use serde::{Deserialize, Serialize};

use super::manifest::Manifest;

/// An asset after type detection and target-patch resolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedAsset {
    /// Path relative to the mod root (as declared in the manifest).
    pub path: String,
    /// Absolute path on disk.
    pub abs_path: String,
    /// Logical asset name (manifest `name`).
    pub name: String,
    /// `pandemic_hash_m2(name)` — the engine's lookup key and our conflict key.
    pub asset_hash: u32,
    /// Resolved asset type (after `"auto"` detection or explicit override).
    pub detected_type: String,
    /// Resolved target patch group (after `"auto"` resolution or override).
    pub target_patch: String,
}

/// A fully loaded mod: its manifest plus the detected assets it ships.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadedMod {
    /// Stable identifier derived from the mod name (kebab-case slug).
    pub id: String,
    /// Absolute path to the mod's root directory.
    pub root: String,
    pub manifest: Manifest,
    pub assets: Vec<DetectedAsset>,
}
