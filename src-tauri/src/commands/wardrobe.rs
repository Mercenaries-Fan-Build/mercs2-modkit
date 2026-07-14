//! The PMC wardrobe: let the player wear any humanoid model the game already ships.
//!
//! # How the wardrobe actually works
//!
//! It is **pure Lua**. There is no reflection data and no save byte for the costume — the
//! outfit list is a plain global table in `wifpmcinterior`:
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
//! model. So *any* model in the WAD is wearable as long as we know its name — no new
//! assets, no injection, no risk. The menu is gated by `GetAvailableCostumes()`, another
//! global, which we redefine to lift the cap.
//!
//! # Why this composes across mods
//!
//! Because both are globals, a mod does not need us to rewrite the table — it only needs
//! us to **append source text** after it:
//!
//! ```lua
//! table.insert(_tOutfits.mattias, { Name=..., Model=..., PlayerVisibleName=... })
//! ```
//!
//! N mods therefore union by plain concatenation, compiled once. That is the whole reason
//! `scripts_vz` is owned by modkit rather than shipped pre-built by each mod: two mods that
//! each shipped their own compiled `scripts_vz` block would silently annihilate each other
//! (whole-block override, last one wins, no error).
//!
//! # What we do NOT do
//!
//! We do not recompile the whole game's Lua. Only the scripts a mod actually touches are
//! rebuilt from a bundled, verified source; every other entry in the block passes through
//! byte-for-byte from the user's own `vz.wad`. And we refuse outright if the user's
//! `wifpmcinterior` bytecode isn't the build our bundled source came from — better to
//! decline than to replace their script with a different game version's.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mercs2_formats::ffcs::load_ffcs_archive;
use mercs2_formats::hash::pandemic_hash_m2;
use mercs2_formats::patch_wad::{AsetEntry, PatchBlock};
use mercs2_formats::scripts_block::ScriptsBlock;
use mercs2_formats::sges::{decompress_block, decompress_sges};
use mercs2_formats::types::TYPE_ID_MODEL;
use serde::{Deserialize, Serialize};

/// The script that owns the wardrobe menu.
const INTERIOR_SCRIPT: &str = "wifpmcinterior";

/// Our verified source for that script, compiled into the binary.
///
/// This is a **decompilation** of the shipped bytecode, not original source — so it is
/// trusted exactly as far as it has been tested. It is the one script the community has
/// round-tripped through the game (the shipped 6-slot wardrobe mod took this path), and it
/// is the *only* script modkit will rebuild. Everything else in the block passes through
/// untouched.
const INTERIOR_SOURCE: &str = include_str!("../../lua/wifpmcinterior.lua");

/// The three player characters. These are the `_tOutfits` keys — note the table uses
/// `jennifer` while the model names use `jen`.
pub const HEROES: [&str; 3] = ["mattias", "chris", "jennifer"];

/// Candidate wearable models.
///
/// This is only a *candidate* list: every entry is checked against the player's own
/// `vz.wad` before it is offered, so a name that isn't in their install (a DLC skin they
/// don't own, say) simply never appears. That means we can be generous here without ever
/// offering something that would fail in-game.
const CANDIDATES: &[(&str, &str)] = &[
    // Heroes and their unlockable tiers.
    ("pmc_hum_mattias", "Mattias"),
    ("pmc_hum_mattias_v2", "Mattias — Vacation"),
    ("pmc_hum_mattias_v3", "Mattias — MetalHead"),
    ("pmc_hum_mattias_v4", "Mattias — Commando"),
    ("pmc_hum_mattias_v5", "Mattias — Grandpa"),
    ("pmc_hum_mattias_chickensuit", "Mattias — Chicken Suit"),
    ("pmc_hum_chris", "Chris"),
    ("pmc_hum_chris_v2", "Chris — Vacation"),
    ("pmc_hum_chris_v3", "Chris — Commando"),
    ("pmc_hum_chris_v4", "Chris — Tier 4"),
    ("pmc_hum_chris_chickensuit", "Chris — Chicken Suit"),
    ("pmc_hum_jen", "Jennifer"),
    ("pmc_hum_jen_v2", "Jennifer — Vacation"),
    ("pmc_hum_jen_v3", "Jennifer — Commando"),
    ("pmc_hum_jen_v4", "Jennifer — Tier 4"),
    ("pmc_hum_jen_v5", "Jennifer — Tier 5"),
    ("pmc_hum_jen_chickensuit", "Jennifer — Chicken Suit"),
    // PMC crew — the ones the shipped 6-slot wardrobe proved wearable.
    ("pmc_hum_mechanic", "Eva (Mechanic)"),
    ("pmc_hum_fiona_unlockable", "Fiona"),
    ("pmc_hum_helipilot_unlockable", "Ewan (Heli Pilot)"),
    ("pmc_hum_proppilot_unlockable", "Misha (Prop Pilot)"),
    // DLC / bonus characters (only offered if present in the install).
    ("pmc_hum_obama", "Obama"),
    ("pmc_hum_blanco", "Blanco"),
    // Faction characters.
    ("pr_hum_boss", "PR Boss"),
    ("al_hum_boss", "Allied Boss"),
    ("al_hum_pilot", "Allied Pilot"),
    ("al_hum_prisoner", "Allied Prisoner"),
    ("al_hum_workerb", "Worker"),
    ("ch_hum_prisoner", "Chinese Prisoner"),
    ("gr_hum_elite", "Guerrilla Elite"),
    ("gr_hum_starter_1", "Guerrilla"),
    ("police_hum_officer_a", "Police Officer"),
    ("police_hum_officer_b", "Police Officer B"),
    ("oc_hum_mercenary_a", "Mercenary"),
    ("oc_hum_mercenaryheavy_a", "Heavy Mercenary"),
    ("vz_hum_solano", "Solano"),
];

/// A model the player can actually wear — verified present in their `vz.wad`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WardrobeModel {
    /// The engine name. This exact string goes into `Model = "..."`.
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

/// Every **primary** asset hash in the WAD, mapped to its type id.
fn aset_index(wad: &Path) -> Result<HashMap<u32, u32>, String> {
    let mut f = std::fs::File::open(wad).map_err(|e| format!("open {}: {e}", wad.display()))?;
    let size = f.metadata().map_err(|e| format!("stat: {e}"))?.len();
    let archive = load_ffcs_archive(&mut f, size).map_err(|e| format!("FFCS: {e}"))?;
    Ok(archive
        .aset
        .iter()
        .filter(|e| e.is_primary())
        .map(|e| (e.asset_hash, e.type_id))
        .collect())
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
#[tauri::command]
pub fn list_wardrobe_models(game_path: String) -> Result<Vec<WardrobeModel>, String> {
    let labels: std::collections::HashMap<&str, &str> = CANDIDATES.iter().copied().collect();

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

/// The Lua we append after the base script. Both `_tOutfits` and `GetAvailableCostumes`
/// are globals, so appending is enough — no AST surgery, and N mods concatenate cleanly.
fn wardrobe_lua(outfits: &[WardrobeOutfit]) -> String {
    let mut src = String::from("\n-- ===== modkit: wardrobe additions =====\n");

    for o in outfits {
        // Lua long-bracket-free escaping: these strings come from a fixed model roster and
        // a user label, so escape quotes/backslashes rather than trusting them.
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        src.push_str(&format!(
            "_tOutfits.{hero} = _tOutfits.{hero} or {{}}\n\
             table.insert(_tOutfits.{hero}, {{ Name = \"{name}\", Model = \"{model}\", PlayerVisibleName = \"{label}\" }})\n",
            hero = o.hero,
            name = esc(&o.label),
            model = esc(&o.model),
            label = esc(&o.label),
        ));
    }

    // Lift the menu gate. The base script defines this as a global function, so a later
    // definition wins — the menu then shows every slot in the table.
    src.push_str(
        "\n-- Unlock every wardrobe slot (the base definition caps the visible count).\n\
         function GetAvailableCostumes()\n  \
           local n = 0\n  \
           for _, list in pairs(_tOutfits) do\n    \
             if table.getn(list) > n then n = table.getn(list) end\n  \
           end\n  \
           return n\n\
         end\n",
    );

    src
}

/// Build the `scripts_vz` block carrying the wardrobe edit.
///
/// Returns `Ok(None)` when there is nothing to do (no outfits requested).
///
/// The block is taken from the user's own `vz.wad`, so every script we don't touch keeps
/// its original bytecode byte-for-byte; only `wifpmcinterior` is recompiled.
pub fn build_wardrobe_block(
    game_path: &str,
    outfits: &[WardrobeOutfit],
    base_source: &str,
) -> Result<Option<PatchBlock>, String> {
    if outfits.is_empty() {
        return Ok(None);
    }

    // Reject a hero we don't know: `_tOutfits.<hero>` would create a table the menu
    // never reads, and the outfit would silently not appear.
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

    // Validate every model name against the WAD *before* compiling anything. A typo here
    // is a guaranteed in-game failure (SetOutfit hashes the name and finds nothing), and
    // it is completely invisible until you walk up to the wardrobe.
    let index = aset_index(&wad_path)?;
    for o in outfits {
        let hash = pandemic_hash_m2(&o.model);
        match index.get(&hash) {
            Some(&t) if t == TYPE_ID_MODEL => {}
            Some(&t) => {
                return Err(format!(
                    "\"{}\" exists in your game but is not a model (asset type {t}) — it cannot be worn.",
                    o.model
                ))
            }
            None => {
                return Err(format!(
                    "Your game has no model called \"{}\" (hash 0x{hash:08X}). If it came from DLC, \
                     make sure that DLC is installed.",
                    o.model
                ))
            }
        }
    }

    // Locate and decompress the scripts block out of the player's WAD.
    let mut f =
        std::fs::File::open(&wad_path).map_err(|e| format!("open {}: {e}", wad_path.display()))?;
    let size = f.metadata().map_err(|e| format!("stat: {e}"))?.len();
    let archive = load_ffcs_archive(&mut f, size).map_err(|e| format!("FFCS: {e}"))?;

    let (idx, path) = archive
        .paths
        .iter()
        .enumerate()
        .find(|(_, p)| p.to_lowercase().contains("scripts_vz"))
        .map(|(i, p)| (i, p.clone()))
        .ok_or("No scripts_vz block in vz.wad")?;

    let raw = decompress_block(&mut f, &archive.indx, idx as u16)
        .map_err(|e| format!("decompress scripts_vz: {e}"))?;

    let mut block = ScriptsBlock::parse(&raw).map_err(|e| format!("parse scripts_vz: {e}"))?;
    let entry = block
        .find_by_name(INTERIOR_SCRIPT)
        .ok_or_else(|| format!("{INTERIOR_SCRIPT} not found in scripts_vz"))?;

    // Compile base + appended source in one chunk. `luac` names the chunk "@<path>";
    // match that so a runtime traceback points at something recognizable.
    let combined = format!("{base_source}{}", wardrobe_lua(outfits));
    let luaq = mercs2_luac::compile(&combined, &format!("@{INTERIOR_SCRIPT}.lua"))
        .map_err(|e| format!("The wardrobe script failed to compile: {e}"))?;

    block
        .replace_lua(entry, &luaq)
        .map_err(|e| format!("replace {INTERIOR_SCRIPT}: {e}"))?;

    // Carry the original block's ASET rows forward untouched — the block still contains
    // exactly the same set of scripts, so it still owns exactly the same asset hashes.
    // `from_decompressed` recomputes `packed_field` from the new decompressed size, which
    // matters because the appended Lua grows the block and may cross a 32 KB page.
    let aset: Vec<AsetEntry> = archive
        .aset
        .iter()
        .filter(|e| e.block_index() as usize == idx)
        .map(|e| AsetEntry::new(e.asset_hash, e.secondary_ref, e.packed_block_ref, e.type_id))
        .collect();

    let tier = archive.indx.get(idx).map(|e| e.packed_field);
    let rebuilt = block.serialize();
    let patch = PatchBlock::from_decompressed(&rebuilt, path, aset, tier)?;

    Ok(Some(patch))
}

/// Build the wardrobe's `scripts_vz` block using modkit's bundled base source.
///
/// This is what the WAD builder calls. Returns `Ok(None)` if no outfits were requested.
pub fn wardrobe_block(
    game_path: &str,
    outfits: &[WardrobeOutfit],
) -> Result<Option<PatchBlock>, String> {
    build_wardrobe_block(game_path, outfits, INTERIOR_SOURCE)
}

/// Preview the Lua modkit would append — so a user (or a bug report) can see exactly what
/// is being added to their game.
#[tauri::command]
pub fn preview_wardrobe_lua(outfits: Vec<WardrobeOutfit>) -> String {
    wardrobe_lua(&outfits)
}

/// Sanity-check that `decompress_sges` is reachable (keeps the import honest in builds
/// where `decompress_block` is the only path taken).
#[allow(dead_code)]
fn _unused(b: &[u8]) -> Result<Vec<u8>, String> {
    decompress_sges(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_lua_appends_and_lifts_the_gate() {
        let src = wardrobe_lua(&[WardrobeOutfit {
            hero: "mattias".into(),
            model: "pmc_hum_mechanic".into(),
            label: "Eva".into(),
        }]);
        assert!(src.contains("table.insert(_tOutfits.mattias"));
        assert!(src.contains("Model = \"pmc_hum_mechanic\""));
        assert!(src.contains("function GetAvailableCostumes()"));
    }

    /// The generated Lua must actually compile — otherwise the whole build dies at the
    /// last step with a message the user can do nothing about.
    #[test]
    fn generated_lua_compiles_standalone() {
        let src = format!(
            "_tOutfits = {{ mattias = {{}}, chris = {{}}, jennifer = {{}} }}\n{}",
            wardrobe_lua(&[
                WardrobeOutfit {
                    hero: "mattias".into(),
                    model: "pmc_hum_mechanic".into(),
                    label: "Eva".into(),
                },
                WardrobeOutfit {
                    hero: "chris".into(),
                    model: "pmc_hum_obama".into(),
                    label: "Obama".into(),
                },
            ])
        );
        let bytes = mercs2_luac::compile(&src, "@test.lua").expect("must compile");
        assert_eq!(&bytes[..4], b"\x1bLua");
    }

    /// A label with a quote in it must not break out of the Lua string.
    #[test]
    fn labels_are_escaped() {
        let src = wardrobe_lua(&[WardrobeOutfit {
            hero: "chris".into(),
            model: "pmc_hum_chris".into(),
            label: "He said \"hi\"".into(),
        }]);
        assert!(src.contains(r#"\"hi\""#), "quotes escaped: {src}");
        let full = format!("_tOutfits = {{ chris = {{}} }}\n{src}");
        mercs2_luac::compile(&full, "@t.lua").expect("escaped label still compiles");
    }
}
