//! The PMC wardrobe: let the player wear any humanoid model the game already ships.
//!
//! # How the wardrobe works in the game
//!
//! It is **pure Lua**. The outfit list is a plain global table in `wifpmcinterior`:
//!
//! ```lua
//! _tOutfits = {                       -- line 155, a GLOBAL (no `local`)
//!   mattias = { { Name=..., Model="pmc_hum_mattias_v3", PlayerVisibleName=... }, ... },
//!   chris   = { ... },
//!   jennifer= { ... },
//! }
//! ```
//!
//! Selecting an entry calls `Player.SetOutfit(guid, sModelName)`, which hashes that
//! **name string** with `pandemic_hash_m2`, looks it up in the ASET table, and loads the
//! model. So any model in the WAD is wearable as long as we know its name.
//!
//! # Every outfit is built by Quartermaster
//!
//! This module picks and checks outfits; it compiles nothing. The game saves the worn row by
//! its position and restores it through the player script's own character map, so a row
//! appended to `_tOutfits` by hand restores as a different model on the next load. `qm link`
//! generates each hero's list and that map together for the whole load order, so every build
//! hands the picks to qm as `add_outfit` contributions
//! ([`super::shipment::synthesize_wardrobe_shipment`]), with or without other Shipments.
//!
//! qm builds an outfit for whatever model name it is given, so [`validate_outfits`] refuses a
//! name the player's own `vz.wad` does not hold before qm runs.

use std::path::{Path, PathBuf};

use mercs2_formats::ffcs::load_ffcs_archive;
use mercs2_formats::hash::pandemic_hash_m2;
use mercs2_formats::texture::extract_model;
use serde::{Deserialize, Serialize};

/// The bundled decompilation of `wifpmcinterior`, read for the outfits the base game's
/// wardrobe already lists ([`base_wardrobe_models`]).
const INTERIOR_SOURCE: &str = include_str!("../../lua/wifpmcinterior.lua");

/// The three player characters. These are the `_tOutfits` keys — note the table uses
/// `jennifer` while the model names use `jen`.
pub const HEROES: [&str; 3] = ["mattias", "chris", "jennifer"];

/// Friendly labels for known models.
///
/// This is *only* a label table now — the picker finds wearable skins by rig match against
/// the heroes (see [`crate::commands::human_skins`]), then looks a nicer label up here,
/// falling back to [`pretty`] for anything not listed. Names come from the community's
/// `WardrobeUnlocker`, whose skin list is verified-working in-game, plus the heroes' own
/// tiers. Anything not present in the player's install simply isn't offered.
const CANDIDATES: &[(&str, &str)] = &[
    // Heroes and their unlockable tiers.
    ("pmc_hum_mattias", "Mattias"),
    ("pmc_hum_mattias_v2", "Mattias — Suit"),
    ("pmc_hum_mattias_v3", "Mattias — MetalHead"),
    ("pmc_hum_mattias_v4", "Mattias — Jacket"),
    ("pmc_hum_mattias_chickensuit", "Mattias — Chicken Suit"),
    ("pmc_hum_chris", "Chris"),
    ("pmc_hum_chris_v2", "Chris — Suit"),
    ("pmc_hum_chris_v3", "Chris — Commando"),
    ("pmc_hum_chris_v4", "Chris — Tier 4"),
    ("pmc_hum_chris_chickensuit", "Chris — Chicken Suit"),
    ("pmc_hum_jen", "Jennifer"),
    ("pmc_hum_jen_v2", "Jennifer — Suit"),
    ("pmc_hum_jen_v3", "Jennifer — Commando"),
    ("pmc_hum_jen_v4", "Jennifer — Tier 4"),
    ("pmc_hum_jen_v5", "Jennifer — Tier 5"),
    ("pmc_hum_jen_chickensuit", "Jennifer — Chicken Suit"),
    // PMC & allies.
    ("pmc_hum_fiona", "Fiona"),
    ("pmc_hum_eva", "Eva"),
    ("pmc_hum_diablo", "Diablo"),
    ("pmc_hum_hoang", "Hoang"),
    ("pmc_hum_stealth", "Stealth"),
    ("pmc_hum_mechanic", "PMC Mechanic"),
    ("pmc_hum_blanco", "Blanco (PMC)"),
    ("pmc_hum_helipilot", "Helicopter Pilot"),
    ("pmc_hum_proppilot", "Prop Pilot"),
    ("pmc_hum_fire", "MOPP Suit"),
    ("pmc_hum_obama", "Obama"),
    // Venezuela.
    ("vz_hum_solano", "Solano"),
    ("vz_hum_carmona", "Carmona"),
    ("vz_hum_blanco", "Blanco (VZ)"),
    ("vz_hum_captain", "VZ Captain"),
    ("vz_hum_deathsquad_a", "VZ Deathsquad"),
    ("vz_hum_soldierelite_a", "VZ Elite"),
    // Allied Nations.
    ("al_hum_boss", "Allied Boss"),
    ("al_hum_officer_a", "Allied Officer"),
    ("al_hum_pilot", "Allied Pilot"),
    ("al_hum_starter01", "Allied Recruit 1"),
    ("al_hum_starter02", "Allied Recruit 2"),
    // China.
    ("ch_hum_boss", "Chinese Boss"),
    ("ch_hum_prisoner", "Chinese Prisoner"),
    // Guerrillas.
    ("gr_hum_boss", "Guerrilla Boss"),
    ("gr_hum_boss_fake", "Guerrilla Boss (Disguise)"),
    ("gr_hum_advisor", "Guerrilla Advisor"),
    ("gr_hum_elite", "Guerrilla Elite"),
    // Pirates.
    ("pr_hum_boss", "Pirate Boss"),
    ("pr_hum_worker", "Pirate Worker"),
    // Universal Petroleum.
    ("oc_hum_boss", "UP Boss"),
    ("oc_hum_executive", "UP Executive"),
    ("oc_hum_boardmember", "UP Board Member"),
    ("oc_hum_mercenary_a", "UP Mercenary"),
    ("oc_hum_pilot", "UP Pilot"),
    ("oc_hum_fireman", "Fireman"),
    // Civilian / misc.
    ("civ_hum_doctorfemale", "Doctor"),
    ("police_hum_officer_b", "Police Officer"),
    ("civ_hum_beachfemale_a", "Beach Girl A"),
    ("civ_hum_beachfemale_b", "Beach Girl B"),
    ("civ_hum_beachfemale_c", "Beach Girl C"),
    ("civ_hum_beachfemale_d", "Beach Girl D"),
];

/// Models already present in the base game's wardrobe (`_tOutfits`).
///
/// Parsed once from the bundled `wifpmcinterior` source. The picker badges them, to steer the
/// user toward genuinely-new skins. qm lists an added outfit as its own row whatever its model,
/// so adding one of these gives the hero a second row for that model.
fn base_wardrobe_models() -> std::collections::HashSet<String> {
    INTERIOR_SOURCE
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("Model")?.trim_start().strip_prefix('=')?.trim();
            let inner = rest.strip_prefix('"')?;
            inner.split('"').next().map(|s| s.to_string())
        })
        .collect()
}

/// A model the player can actually wear — verified present in their `vz.wad`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WardrobeModel {
    /// The engine name. This exact string is the outfit's `Model`.
    pub model: String,
    /// Friendly label for the picker.
    pub label: String,
    pub asset_hash: u32,
    /// How much of the player characters' skeleton this skin has (0..1). 100% = certain to
    /// animate exactly like a hero; below that, the missing bones' animation tracks simply
    /// do nothing.
    pub rig_match: f32,
    /// Which of the three heroes this skin is built most like.
    pub closest_hero: String,
    pub triangles: usize,
    /// One of the three player characters (or one of their unlock tiers).
    pub is_hero: bool,
    /// Already in the base game's wardrobe, so the UI badges it "already available" and steers
    /// you to genuinely-new skins.
    pub in_base_wardrobe: bool,
}

/// One outfit a user wants added to the wardrobe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WardrobeOutfit {
    /// `mattias` | `chris` | `jennifer`.
    pub hero: String,
    /// Engine model name (must be one of [`list_wardrobe_models`]).
    pub model: String,
    /// What the in-game menu shows.
    pub label: String,
}

fn vz_wad(game_path: &str) -> Result<PathBuf, String> {
    for candidate in [
        Path::new(game_path).join("data").join("vz.wad"),
        Path::new(game_path).join("vz.wad"),
    ] {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!("Could not find vz.wad under {game_path}"))
}

/// List the models this install can actually wear.
///
/// ## Detected, not guessed
///
/// This used to filter a hardcoded candidate list. That was wrong in both directions — it
/// named `pmc_hum_fiona_unlockable` (not in a stock install) while missing `pmc_hum_fiona`
/// (which is, and is perfectly wearable), and it omitted a couple of dozen skins nobody had
/// thought to type in.
///
/// Now the skins come from [`crate::commands::human_skins`], which asks the game: a wearable
/// skin is a model **rigged to the same skeleton the heroes use**, because that is precisely
/// what makes the hero's animations play on it. Every result is by definition present in the
/// user's own WAD, so a bad pick stays unrepresentable — and DLC skins appear automatically.
///
/// The curated names below are used only to give a nicer label than the raw model name.
/// `(async)` as with [`crate::commands::model_view::model_geometry`] — a sync
/// `#[tauri::command]` runs on the UI thread, and rig-matching every candidate skin against
/// the three heroes walks a lot of the WAD.
#[tauri::command(async)]
pub fn list_wardrobe_models(game_path: String) -> Result<Vec<WardrobeModel>, String> {
    let labels: std::collections::HashMap<&str, &str> = CANDIDATES.iter().copied().collect();
    let base = base_wardrobe_models();

    let index = crate::commands::human_skins::human_skins(game_path)?;

    Ok(index
        .skins
        .iter()
        .filter(|s| s.wearable)
        .filter_map(|s| {
            let model = s.name.clone()?;
            Some(WardrobeModel {
                label: labels
                    .get(model.as_str())
                    .map(|l| l.to_string())
                    .unwrap_or_else(|| pretty(&model)),
                asset_hash: s.hash,
                in_base_wardrobe: base.contains(&model),
                model,
                rig_match: s.rig_match,
                closest_hero: s.closest_hero.clone(),
                triangles: s.triangles,
                is_hero: s.is_hero,
            })
        })
        .collect())
}

/// Turn `oc_hum_mercenaryheavy_a` into `Mercenaryheavy A (OC)` — a readable fallback for the
/// skins the curated table doesn't name.
fn pretty(model: &str) -> String {
    let parts: Vec<&str> = model.split('_').collect();
    let faction = parts.first().copied().unwrap_or("").to_uppercase();
    let rest: Vec<String> = parts
        .iter()
        .skip(2) // drop the faction prefix and the `hum` marker
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect();
    let label = rest.join(" ");
    if label.is_empty() {
        model.to_string()
    } else {
        format!("{label} ({faction})")
    }
}

/// Refuse an outfit the game cannot wear, before anything is built.
///
/// A hero that is not a `_tOutfits` key is offered to no one. A model is checked with the exact
/// resolution `Player.SetOutfit` performs at runtime: hash the name and resolve the model by that
/// hash, following ASET sub-entries, which is what `extract_model` does. Checking the primary ASET
/// row's type instead rejects `al_hum_boss` (asset type 34), a model `SetOutfit` loads.
pub fn validate_outfits(game_path: &str, outfits: &[WardrobeOutfit]) -> Result<(), String> {
    for o in outfits {
        if !HEROES.contains(&o.hero.as_str()) {
            return Err(format!(
                "Unknown character \"{}\" — expected one of {}",
                o.hero,
                HEROES.join(", ")
            ));
        }
    }

    let wad_path = vz_wad(game_path)?;
    let mut f =
        std::fs::File::open(&wad_path).map_err(|e| format!("open {}: {e}", wad_path.display()))?;
    let size = f.metadata().map_err(|e| format!("stat: {e}"))?.len();
    let archive = load_ffcs_archive(&mut f, size).map_err(|e| format!("FFCS: {e}"))?;

    for o in outfits {
        let hash = pandemic_hash_m2(&o.model);
        if extract_model(&mut f, &archive, hash).is_err() {
            return Err(format!(
                "Your game has no wearable model called \"{}\" (hash 0x{hash:08X}). If it came \
                 from DLC, make sure that DLC is installed.",
                o.model
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `base_wardrobe_models` must find the outfits the bundled `wifpmcinterior` already
    /// ships — those are the ones the picker badges.
    #[test]
    fn base_wardrobe_models_are_recovered() {
        let base = base_wardrobe_models();
        for m in ["pmc_hum_mattias", "pmc_hum_mattias_v3", "pmc_hum_chris", "pmc_hum_jen"] {
            assert!(base.contains(m), "{m} should be a known base outfit");
        }
        // A skin the base wardrobe does NOT have.
        assert!(!base.contains("pmc_hum_fiona"));
    }

    /// A hero the wardrobe has no list for is refused by name, before the game is read.
    #[test]
    fn an_unknown_hero_is_refused() {
        let err = validate_outfits(
            "/no/such/game",
            &[WardrobeOutfit { hero: "eva".into(), model: "pmc_hum_eva".into(), label: "Eva".into() }],
        )
        .unwrap_err();
        assert!(err.contains("Unknown character \"eva\""), "{err}");
    }
}
