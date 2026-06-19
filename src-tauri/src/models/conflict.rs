//! Conflict model: assets claimed by more than one mod, and how to resolve them.

use serde::{Deserialize, Serialize};

/// One asset hash claimed by two or more mods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetConflict {
    /// The contested `pandemic_hash_m2` asset key.
    pub asset_hash: u32,
    /// Human-readable name if known (from a manifest or the type registry).
    pub asset_name: Option<String>,
    /// IDs of the mods that each declare this asset.
    pub mods: Vec<String>,
}

/// The full set of conflicts across a project's loaded mods.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConflictGraph {
    pub conflicts: Vec<AssetConflict>,
}
