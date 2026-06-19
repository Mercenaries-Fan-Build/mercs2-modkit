//! Conflict detection: find asset hashes claimed by more than one mod.

use std::collections::HashMap;

use mercs2_formats::types::type_name_from_hash;

use crate::models::conflict::{AssetConflict, ConflictGraph};
use crate::models::project::LoadedMod;

/// Build the conflict graph for a set of loaded mods.
///
/// Two mods conflict on an asset when they both declare the same
/// `pandemic_hash_m2` key — this mirrors the engine's last-write-wins overlay,
/// where the later patch silently shadows the earlier one.
#[tauri::command]
pub fn build_conflict_graph(mods: Vec<LoadedMod>) -> ConflictGraph {
    // asset_hash -> (mod ids that claim it, a representative human name)
    let mut by_hash: HashMap<u32, (Vec<String>, Option<String>)> = HashMap::new();

    for m in &mods {
        for a in &m.assets {
            let entry = by_hash.entry(a.asset_hash).or_insert_with(|| (Vec::new(), None));
            if !entry.0.contains(&m.id) {
                entry.0.push(m.id.clone());
            }
            if entry.1.is_none() {
                entry.1 = Some(a.name.clone());
            }
        }
    }

    let mut conflicts: Vec<AssetConflict> = by_hash
        .into_iter()
        .filter(|(_, (mods, _))| mods.len() > 1)
        .map(|(hash, (mods, name))| AssetConflict {
            asset_hash: hash,
            asset_name: name.or_else(|| {
                let n = type_name_from_hash(hash);
                if n == "unknown" {
                    None
                } else {
                    Some(n.to_string())
                }
            }),
            mods,
        })
        .collect();

    conflicts.sort_by_key(|c| c.asset_hash);
    ConflictGraph { conflicts }
}
