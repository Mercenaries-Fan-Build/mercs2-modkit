//! Curated mod catalog — a hand-maintained `registry.json` of available mods.
//!
//! Loaded from a remote URL the curator maintains, falling back to the copy
//! bundled with the app when offline.

use serde::{Deserialize, Serialize};

/// Raw `registry.json` the curator edits; fetched at runtime.
const REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/Mercenaries-Fan-Build/mercs2-modkit/main/registry.json";

/// Compiled-in fallback used when the remote fetch fails.
const BUNDLED: &str = include_str!("../../registry.json");

/// One curated catalog entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    /// Friendly display name.
    pub name: String,
    /// Short description of what the mod does.
    pub description: String,
    /// Git repository whose latest release provides the mod artifacts.
    pub repository: String,
}

/// The catalog plus where it came from (`"remote"` or `"bundled"`).
#[derive(Debug, Serialize)]
pub struct Catalog {
    pub entries: Vec<CatalogEntry>,
    pub source: String,
}

async fn fetch_remote() -> Result<Vec<CatalogEntry>, String> {
    let client = reqwest::Client::builder()
        .user_agent("mercs2-modkit")
        .build()
        .map_err(|e| e.to_string())?;
    let text = client
        .get(REGISTRY_URL)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("Invalid registry.json: {e}"))
}

/// Fetch the curated catalog, preferring the remote list and falling back to
/// the bundled copy when the network or remote file is unavailable.
#[tauri::command]
pub async fn fetch_catalog() -> Catalog {
    if let Ok(entries) = fetch_remote().await {
        return Catalog {
            entries,
            source: "remote".to_string(),
        };
    }
    let entries = serde_json::from_str::<Vec<CatalogEntry>>(BUNDLED).unwrap_or_default();
    Catalog {
        entries,
        source: "bundled".to_string(),
    }
}
