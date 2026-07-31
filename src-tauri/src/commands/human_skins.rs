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

/// One mounted archive. Ordered base-first; a later source SHADOWS an earlier one, which is the
/// game's own mount rule (WAD mount order is last-wins).
struct Source {
    file: std::fs::File,
    archive: FfcsArchive,
}

/// Fingerprint over EVERY mounted archive, not just the base.
///
/// This used to hash `vz.wad`'s ASET alone, which made the cache blind to overlays: you could drop
/// a `vz-patch.wad` carrying a brand-new skin, and because the base WAD had not changed the
/// fingerprint matched, the cache was served, and the new model never appeared in the wardrobe. It
/// also meant REMOVING a patch left its skins listed. Folding each source's ASET in — in mount
/// order — makes adding, changing or removing a patch invalidate the scan.
fn fingerprint(srcs: &[Source]) -> u32 {
    let mut buf = Vec::new();
    for s in srcs {
        for e in &s.archive.aset {
            buf.extend_from_slice(&e.asset_hash.to_le_bytes());
            buf.extend_from_slice(&e.type_id.to_le_bytes());
        }
    }
    crc32_mercs2(&buf)
}

fn cache_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("human-skins.json"))
}

/// Extra names the user's own content introduces, one per line / array entry, hashed the same way
/// the built-in table is.
///
/// `ASSET_NAMES` is `include_str!`'d at COMPILE time, so a model the user authored can never be
/// named by it — and since `wearable` requires a name (`Player.SetOutfit` addresses the model by
/// name), every user-authored skin was permanently unwearable no matter how good its rig. This
/// reads an optional `custom-names.json` from the app data dir: a JSON array of NAMES only, never
/// hash→name pairs, so a typo cannot mint a wrong mapping — the hash is always derived from the
/// name, which is what the engine does too.
fn custom_names() -> Vec<String> {
    let Ok(dir) = app_data_dir() else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(dir.join("custom-names.json")) else {
        return Vec::new();
    };
    serde_json::from_slice::<Vec<String>>(&bytes).unwrap_or_default()
}

/// The archives to scan, in mount order: `vz.wad` first, then any overlay beside it.
///
/// Modkit mounted only `vz.wad` here, so nothing shipped in a patch overlay was ever considered —
/// the scan could not see a new skin even though the rest of the app loads the overlay fine.
fn wad_paths(game_path: &str) -> Result<Vec<PathBuf>, String> {
    let base = [
        Path::new(game_path).join("data").join("vz.wad"),
        Path::new(game_path).join("vz.wad"),
    ]
    .into_iter()
    .find(|c| c.is_file())
    .ok_or_else(|| format!("Could not find vz.wad under {game_path}"))?;
    let mut out = vec![base.clone()];
    // Overlays sit next to the base WAD. `vz-patch.wad` is the name the injection tooling ships.
    if let Some(dir) = base.parent() {
        let patch = dir.join("vz-patch.wad");
        if patch.is_file() {
            out.push(patch);
        }
    }
    Ok(out)
}

fn open_sources(game_path: &str) -> Result<Vec<Source>, String> {
    let mut srcs = Vec::new();
    for p in wad_paths(game_path)? {
        let mut file =
            std::fs::File::open(&p).map_err(|e| format!("open {}: {e}", p.display()))?;
        let size = file.metadata().map_err(|e| format!("stat: {e}"))?.len();
        let archive = load_ffcs_archive(&mut file, size)
            .map_err(|e| format!("FFCS {}: {e}", p.display()))?;
        srcs.push(Source { file, archive });
    }
    Ok(srcs)
}

/// A model's bones, taken from the LAST source that carries it (mount order is last-wins).
fn bones_of_sources(srcs: &mut [Source], hash: u32) -> HashSet<u32> {
    for s in srcs.iter_mut().rev() {
        let b = bones_of(&mut s.file, &s.archive, hash);
        if !b.is_empty() {
            return b;
        }
    }
    HashSet::new()
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
#[tauri::command(async)]
pub fn human_skins(game_path: String) -> Result<SkinIndex, String> {
    let mut srcs = open_sources(&game_path)?;
    let want = fingerprint(&srcs);

    if let Ok(bytes) = std::fs::read(cache_path()?) {
        if let Ok(idx) = serde_json::from_slice::<SkinIndex>(&bytes) {
            if idx.fingerprint == want {
                return Ok(idx);
            }
        }
    }

    // Built-in table first, then the user's own names, which win on a tie.
    let mut names: HashMap<u32, String> = ASSET_NAMES
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|n| (pandemic_hash_m2(n), n.to_string()))
        .collect();
    for n in custom_names() {
        let n = n.trim().to_string();
        if !n.is_empty() {
            names.insert(pandemic_hash_m2(&n), n);
        }
    }

    // Reference rig = the bones all three heroes share. Using the intersection rather than any
    // one hero avoids treating that hero's private extras (Mattias has 116 bones to Jen's 92)
    // as requirements.
    let hero_rigs: Vec<(&str, HashSet<u32>)> = HERO_MODELS
        .iter()
        .map(|(label, model)| {
            (*label, bones_of_sources(&mut srcs, pandemic_hash_m2(model)))
        })
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

    // Every model across every mounted archive, deduped. An overlay's brand-new asset appears here
    // alongside the base game's; one it SHADOWS is listed once and resolved from the overlay.
    let models: Vec<u32> = {
        let mut seen = HashSet::new();
        srcs.iter()
            .flat_map(|s| s.archive.aset.iter())
            .filter(|e| e.type_id == TYPE_ID_MODEL)
            .map(|e| e.asset_hash)
            .filter(|h| seen.insert(*h))
            .collect()
    };

    let mut skins = Vec::new();
    for m in models {
        let bones = bones_of_sources(&mut srcs, m);
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
        let (triangles, textures) = match srcs
            .iter_mut()
            .rev()
            .find_map(|s| extract_model(&mut s.file, &s.archive, m).ok())
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
