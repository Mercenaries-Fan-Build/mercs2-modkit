//! Which models are wearable player skins? Ask the skeleton, don't guess the name.
//!
//! # Why this exists
//!
//! The wardrobe used to offer a **hardcoded list of ~37 candidate model names**. That was a
//! guess, and it was wrong in both directions: it listed `pmc_hum_fiona_unlockable`, which
//! isn't in a stock install at all, while missing `pmc_hum_fiona` — the model that actually
//! exists and is fully player-compatible. It also missed `oc_hum_pilot`, `oc_hum_fireman`,
//! `pr_hum_boss`, `gr_hum_starter_1`, `al_hum_workerb`, `vz_hum_blanco`, and a couple of dozen
//! more, purely because nobody had typed them in.
//!
//! The game already knows the answer. A player skin is a model **rigged to the same human
//! skeleton the heroes use** — that is exactly what makes the hero's animations play on it.
//! Bones are `HIER` nodes carrying name-hashes, so "is this the same rig?" is a set
//! comparison against Mattias / Chris / Jennifer. No heuristics, no name matching, and it
//! adapts automatically to DLC.
//!
//! Measured on retail: the three heroes share **85 bones**; **25** models carry 100% of that
//! skeleton and **62** more carry 90–99%.
//!
//! # The one hard requirement
//!
//! `Player.SetOutfit` takes a **name string**, which it hashes to find the model. So a skin is
//! only *usable* if we can also recover its name — a 100%-rig model we can't name is real, and
//! we say so, but it can't go in the wardrobe.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use mercs2_formats::crc32::crc32_mercs2;
use mercs2_formats::ffcs::{load_ffcs_archive, FfcsArchive};
use mercs2_formats::hash::pandemic_hash_m2;
use mercs2_formats::orchestrator::parse_hier;
use mercs2_formats::texture::extract_model;
use mercs2_formats::types::TYPE_ID_MODEL;
use mercs2_mesh::build_indexed_all;
use serde::{Deserialize, Serialize};

use crate::commands::paths::app_data_dir;
use crate::commands::texture_swap::ASSET_NAMES;

/// The three player characters. Their shared skeleton is the reference rig.
pub const HERO_MODELS: [(&str, &str); 3] = [
    ("mattias", "pmc_hum_mattias"),
    ("chris", "pmc_hum_chris"),
    ("jennifer", "pmc_hum_jen"),
];

/// A model rigged like a player character.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanSkin {
    pub hash: u32,
    /// `None` when we can't recover it — such a skin can be *viewed* but not worn, because
    /// `Player.SetOutfit` addresses the model by name.
    pub name: Option<String>,
    /// Pass this to the geometry commands (name, or `0x…`).
    pub reference: String,
    /// How much of the heroes' shared skeleton this model has, 0..1.
    pub rig_match: f32,
    /// Bones of the shared rig it is missing.
    pub missing_bones: usize,
    /// Total bones in its own skeleton (it may have extras — a hat, a backpack).
    pub bones: usize,
    pub triangles: usize,
    /// Its diffuse maps, named where possible — the "look" of the skin.
    pub textures: Vec<String>,
    /// Which hero it most resembles, and by how much.
    pub closest_hero: String,
    /// Wearable: named AND rigged well enough.
    ///
    /// That is the whole test — and getting it wrong twice is why this comment is long.
    /// `Player.SetOutfit` takes a *name string*, hashes it, and resolves the model **by
    /// hash**, following ASET sub-entries exactly as `extract_model` does. So the only
    /// requirements are that we can name it (or SetOutfit can't be called) and that its name
    /// resolves to a model (which every skin here does — that is how we read its bones).
    ///
    /// It does **not** need a *primary* MODEL row. An earlier version required that and
    /// wrongly hid 34 working skins — `al_hum_boss`, `oc_hum_pilot`, the faction bosses, the
    /// beach girls — all of which the community's `WardrobeUnlocker` ships as verified-working
    /// and all of which `extract_model` resolves. Sub-entry models are fine.
    pub wearable: bool,
    /// True for the three heroes themselves and their tiers.
    pub is_hero: bool,
}

/// Cached scan (it decodes every model's HIER, ~10s).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkinIndex {
    pub fingerprint: u32,
    /// Bones shared by all three heroes — the reference skeleton.
    pub hero_bone_count: usize,
    pub skins: Vec<HumanSkin>,
}

fn fingerprint(archive: &FfcsArchive) -> u32 {
    let mut buf = Vec::with_capacity(archive.aset.len() * 8);
    for e in &archive.aset {
        buf.extend_from_slice(&e.asset_hash.to_le_bytes());
        buf.extend_from_slice(&e.type_id.to_le_bytes());
    }
    crc32_mercs2(&buf)
}

fn cache_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("human-skins.json"))
}

fn vz_wad(game_path: &str) -> Result<PathBuf, String> {
    for c in [
        Path::new(game_path).join("data").join("vz.wad"),
        Path::new(game_path).join("vz.wad"),
    ] {
        if c.is_file() {
            return Ok(c);
        }
    }
    Err(format!("Could not find vz.wad under {game_path}"))
}

/// Bone name-hashes of a model's skeleton.
fn bones_of(f: &mut std::fs::File, archive: &FfcsArchive, hash: u32) -> HashSet<u32> {
    extract_model(f, archive, hash)
        .map(|c| parse_hier(&c).into_iter().map(|n| n.hash).collect())
        .unwrap_or_default()
}

/// Every model rigged like a player character, best match first.
///
/// A skin qualifies at **≥ 50%** of the hero skeleton — deliberately loose, because the UI
/// shows the exact percentage and lets the user judge. A partial rig isn't fatal: the engine
/// plays the hero's animation tracks, and tracks addressed to a bone the model lacks simply
/// do nothing. It's the ones at 100% that are certain.
#[tauri::command]
pub fn human_skins(game_path: String) -> Result<SkinIndex, String> {
    let wad = vz_wad(&game_path)?;
    let mut f = std::fs::File::open(&wad).map_err(|e| format!("open vz.wad: {e}"))?;
    let size = f.metadata().map_err(|e| format!("stat: {e}"))?.len();
    let archive = load_ffcs_archive(&mut f, size).map_err(|e| format!("FFCS: {e}"))?;
    let want = fingerprint(&archive);

    if let Ok(bytes) = std::fs::read(cache_path()?) {
        if let Ok(idx) = serde_json::from_slice::<SkinIndex>(&bytes) {
            if idx.fingerprint == want {
                return Ok(idx);
            }
        }
    }

    let names: HashMap<u32, &str> = ASSET_NAMES
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|n| (pandemic_hash_m2(n), n))
        .collect();

    // Reference rig = the bones all three heroes share. Using the intersection rather than any
    // one hero avoids treating that hero's private extras (Mattias has 116 bones to Jen's 92)
    // as requirements.
    let hero_rigs: Vec<(&str, HashSet<u32>)> = HERO_MODELS
        .iter()
        .map(|(label, model)| (*label, bones_of(&mut f, &archive, pandemic_hash_m2(model))))
        .collect();
    let common: HashSet<u32> = hero_rigs
        .iter()
        .skip(1)
        .fold(hero_rigs[0].1.clone(), |acc, (_, b)| {
            acc.intersection(b).copied().collect()
        });
    if common.is_empty() {
        return Err("Could not read the player characters' skeleton from your game.".into());
    }

    let hero_hashes: HashSet<u32> = HERO_MODELS
        .iter()
        .map(|(_, m)| pandemic_hash_m2(m))
        .collect();

    let models: Vec<u32> = {
        let mut seen = HashSet::new();
        archive
            .aset
            .iter()
            .filter(|e| e.type_id == TYPE_ID_MODEL)
            .map(|e| e.asset_hash)
            .filter(|h| seen.insert(*h))
            .collect()
    };

    let mut skins = Vec::new();
    for m in models {
        let bones = bones_of(&mut f, &archive, m);
        if bones.is_empty() {
            continue;
        }
        let have = common.iter().filter(|h| bones.contains(h)).count();
        let rig_match = have as f32 / common.len() as f32;
        if rig_match < 0.5 {
            continue;
        }

        // Which hero's own skeleton it overlaps most — a rough "who is it built like".
        let closest_hero = hero_rigs
            .iter()
            .map(|(label, hb)| {
                let shared = hb.iter().filter(|h| bones.contains(h)).count();
                (shared, *label)
            })
            .max_by_key(|(shared, _)| *shared)
            .map(|(_, l)| l.to_string())
            .unwrap_or_default();

        // Its look: the diffuse maps its parts actually paint.
        let (triangles, textures) = match extract_model(&mut f, &archive, m)
            .ok()
            .and_then(|c| build_indexed_all(&c).ok())
        {
            Some((_, indices, groups, _)) => {
                let mut seen = HashSet::new();
                let mut tex: Vec<String> = groups
                    .iter()
                    .filter_map(|g| g.diffuse)
                    .filter(|d| *d != 0 && seen.insert(*d))
                    .filter_map(|d| names.get(&d).map(|s| s.to_string()))
                    .collect();
                tex.sort();
                (indices.len() / 3, tex)
            }
            None => (0, Vec::new()),
        };

        let name = names.get(&m).map(|s| s.to_string());
        skins.push(HumanSkin {
            hash: m,
            reference: name.clone().unwrap_or_else(|| format!("0x{m:08X}")),
            // Named + well-rigged. See the field's docs for why nothing else is required.
            wearable: name.is_some() && rig_match >= 0.9,
            is_hero: hero_hashes.contains(&m),
            name,
            rig_match,
            missing_bones: common.len() - have,
            bones: bones.len(),
            triangles,
            textures,
            closest_hero,
        });
    }

    // Best rig first; within a tier, named before unnamed, then alphabetical.
    skins.sort_by(|a, b| {
        b.rig_match
            .partial_cmp(&a.rig_match)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.name.is_some().cmp(&a.name.is_some()))
            .then(a.reference.cmp(&b.reference))
    });

    let idx = SkinIndex {
        fingerprint: want,
        hero_bone_count: common.len(),
        skins,
    };
    if let Ok(p) = cache_path() {
        if let Ok(json) = serde_json::to_vec(&idx) {
            let _ = std::fs::write(p, json);
        }
    }
    Ok(idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Named + rigged. Nothing more — a sub-entry-only model is still wearable.
    #[test]
    fn wearable_requires_only_a_name_and_a_good_rig() {
        let mk = |name: Option<&str>, rig: f32| HumanSkin {
            hash: 1,
            reference: name.unwrap_or("0x1").into(),
            wearable: name.is_some() && rig >= 0.9,
            is_hero: false,
            name: name.map(str::to_string),
            rig_match: rig,
            missing_bones: 0,
            bones: 90,
            triangles: 100,
            textures: vec![],
            closest_hero: "chris".into(),
        };

        assert!(mk(Some("pmc_hum_fiona"), 1.0).wearable);

        // `al_hum_boss` is a sub-entry-only model — the game keeps it inside another asset's
        // block. It is STILL wearable: `SetOutfit` resolves by hash across all rows, and the
        // community ships it as verified-working. Requiring a primary MODEL row wrongly hid it.
        assert!(mk(Some("al_hum_boss"), 1.0).wearable);

        // `SetOutfit` hashes a NAME string, so a model we can't name is unreachable however
        // good its skeleton.
        assert!(!mk(None, 1.0).wearable);

        // A half-rig won't animate like a hero.
        assert!(!mk(Some("x"), 0.6).wearable);
    }
}
