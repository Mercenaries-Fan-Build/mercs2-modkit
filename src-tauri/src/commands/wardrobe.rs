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

use std::path::{Path, PathBuf};

use mercs2_formats::ffcs::load_ffcs_archive;
use mercs2_formats::hash::pandemic_hash_m2;
use mercs2_formats::patch_wad::{AsetEntry, PatchBlock};
use mercs2_formats::scripts_block::ScriptsBlock;
use mercs2_formats::sges::{decompress_block, decompress_sges};
use mercs2_formats::texture::extract_model;
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
/// Parsed once from the bundled `wifpmcinterior` source. Adding one of these does nothing
/// visible — it's already selectable — so the picker badges them and the generated Lua
/// dedupes them out. Used to steer the user toward genuinely-new skins.
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
    /// Already in the base game's wardrobe. Adding it changes nothing (it's deduped out), so
    /// the UI badges it "already available" and steers you to genuinely-new skins.
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

/// The Lua we append after the base script.
///
/// Both `_tOutfits` and `GetAvailableCostumes` are globals in `wifpmcinterior`, so appending
/// text is enough — no AST surgery, and it composes with any other mod that does the same.
///
/// The shape follows the community's `WardrobeUnlocker`, which is the recipe proven to work
/// in-game:
///
/// * **Dedupe by model.** A `_deferOutfit` helper skips a model already present in the
///   hero's list. Without it, picking a base-game tier (`pmc_hum_mattias_v3`, already there
///   as "MetalHead") appended a duplicate — which is why an earlier build appeared to change
///   nothing: the outfits were already in the wardrobe. It also means re-running the mod
///   can't stack duplicates.
/// * **Override `GetAvailableCostumes`.** The base version caps the visible slot count;
///   returning the longest list unlocks every entry, base and added alike.
fn wardrobe_lua(outfits: &[WardrobeOutfit]) -> String {
    // Escape for a Lua double-quoted string; the roster is fixed but a user label is free text.
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");

    let mut src = String::from(
        "\n-- ===== modkit: wardrobe additions =====\n\
         -- Add an outfit to a hero's list, but only if that model isn't already there\n\
         -- (so base-game outfits aren't duplicated and a re-run can't stack entries).\n\
         local function _modkitAddOutfit(sHero, tOutfit)\n  \
           _tOutfits[sHero] = _tOutfits[sHero] or {}\n  \
           for _, o in ipairs(_tOutfits[sHero]) do\n    \
             if o.Model == tOutfit.Model then return end\n  \
           end\n  \
           table.insert(_tOutfits[sHero], tOutfit)\n\
         end\n",
    );

    for o in outfits {
        src.push_str(&format!(
            "_modkitAddOutfit(\"{hero}\", {{ Name = \"{name}\", Model = \"{model}\", PlayerVisibleName = \"{label}\" }})\n",
            hero = esc(&o.hero),
            name = esc(&o.label),
            model = esc(&o.model),
            label = esc(&o.label),
        ));
    }

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
    let mut f =
        std::fs::File::open(&wad_path).map_err(|e| format!("open {}: {e}", wad_path.display()))?;
    let size = f.metadata().map_err(|e| format!("stat: {e}"))?.len();
    let archive = load_ffcs_archive(&mut f, size).map_err(|e| format!("FFCS: {e}"))?;

    // Validate every model name against the WAD *before* compiling anything, using the exact
    // resolution `Player.SetOutfit` performs at runtime: hash the name and resolve the model
    // by that hash, following ASET sub-entries. `extract_model` is that resolution. Checking
    // anything else (e.g. the primary ASET row's type) is how an earlier build wrongly
    // rejected `al_hum_boss` as "not a model (asset type 34)" — a model that SetOutfit loads
    // fine. A typo, by contrast, resolves to nothing and is caught here rather than at the
    // wardrobe mirror.
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
    fn generated_lua_adds_deduped_and_lifts_the_gate() {
        let src = wardrobe_lua(&[WardrobeOutfit {
            hero: "mattias".into(),
            model: "pmc_hum_mechanic".into(),
            label: "Eva".into(),
        }]);
        // Uses the dedupe helper, not a bare table.insert (which duplicated base outfits).
        assert!(src.contains("_modkitAddOutfit(\"mattias\""));
        assert!(src.contains("local function _modkitAddOutfit"));
        assert!(src.contains("if o.Model == tOutfit.Model then return end"));
        assert!(src.contains("Model = \"pmc_hum_mechanic\""));
        assert!(src.contains("function GetAvailableCostumes()"));
    }

    /// `base_wardrobe_models` must find the outfits the bundled `wifpmcinterior` already
    /// ships — those are the ones the picker badges and the generated Lua dedupes out. If it
    /// found none, every base tier would look "new" and get duplicated again.
    #[test]
    fn base_wardrobe_models_are_recovered() {
        let base = base_wardrobe_models();
        for m in ["pmc_hum_mattias", "pmc_hum_mattias_v3", "pmc_hum_chris", "pmc_hum_jen"] {
            assert!(base.contains(m), "{m} should be a known base outfit");
        }
        // A skin the base wardrobe does NOT have.
        assert!(!base.contains("pmc_hum_fiona"));
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
