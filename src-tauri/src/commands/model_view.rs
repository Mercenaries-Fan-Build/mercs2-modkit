//! Geometry for the 3D texture viewer: *show me where this texture actually is.*
//!
//! A list of model names answers "what uses this texture" only in the weakest sense. What a
//! modder wants to know is **where on the model it lands** — is `pmc_hum_chris_ub` the
//! torso or the arms? So we hand the frontend the real mesh and let three.js draw it, with
//! the parts that sample the chosen texture lit up.
//!
//! # How the highlight is possible at all
//!
//! Because the geometry decoder already splits a model into **draw groups**, and each group
//! carries the texture hashes its material samples (`diffuse` / `specular` / `normal` — the
//! MTRL slots). So "which triangles use this texture" is not an inference: it is a direct
//! per-group comparison. The frontend gets one three.js group per draw group plus the flag
//! `uses_texture`, and simply makes those glow.
//!
//! # Why the decoder isn't in the engine crate
//!
//! It was — `mercs2_engine::mesh`. But `mercs2_engine` pulls in wgpu and winit, which a
//! Tauri app must not link. The decode is pure (no GPU, no windowing), so it now lives in
//! its own `mercs2_mesh` crate; the engine re-exports it, so nothing else changed.

use std::path::Path;

use mercs2_formats::ffcs::load_ffcs_archive;
use mercs2_formats::hash::pandemic_hash_m2;
use mercs2_formats::texture::extract_model;
use mercs2_mesh::{build_indexed_all, build_indexed_state, state_tiers, DrawGroup, ModelStats, Vertex};
use serde::Serialize;

/// One texture slot of a group's material.
#[derive(Debug, Clone, Serialize)]
pub struct SlotRef {
    /// `diffuse` | `specular` | `normal` | `map N`.
    pub slot: String,
    pub hash: u32,
    /// The texture's name, if we can recover it — the WAD stores only hashes.
    pub name: Option<String>,
    /// True when this is the texture the page is about.
    pub is_current: bool,
}

/// One draw call, ready to become a three.js group.
///
/// This is the real structure of a model: a "part" in the UI. A model is not one mesh — Chris
/// is 25 of these — and each one binds its own material. Exposing them is what turns "5 of 25
/// parts use this texture" into something you can actually inspect: which part, how big, what
/// else it wears, and where it sits on the sheet.
#[derive(Debug, Clone, Serialize)]
pub struct GeoGroup {
    /// Position in this model's group list — the id the UI selects by.
    pub id: usize,
    /// Offset into `indices`.
    pub index_start: u32,
    pub index_count: u32,
    pub triangles: usize,
    /// Does this group's material sample the texture we're inspecting?
    pub uses_texture: bool,
    /// Which slot it matched — `diffuse` | `specular` | `normal` | `map N`.
    pub slot: Option<String>,
    /// The group's diffuse map, so the viewer can texture the rest of the model normally.
    pub diffuse: Option<u32>,
    /// Every texture this group's material binds, named where possible. Lets the user hop
    /// from a part straight to the other maps it wears.
    pub textures: Vec<SlotRef>,
    /// The container's PRMG drawing-group index (several draws can share one — a PRMG often
    /// concatenates sub-strips with different materials).
    pub prmg: usize,
    /// `SEGM.state_mask` — which state/LOD bits this part is drawn in.
    pub lod_mask: u8,
    /// `SEGM.node` — the HIER node it hangs off (negative = none, always visible).
    pub node: i16,
}

/// A model, flattened for the browser.
#[derive(Debug, Clone, Serialize)]
pub struct ModelGeometry {
    pub model: String,
    pub model_hash: u32,
    /// Interleaved-free, three.js-friendly flat arrays.
    pub positions: Vec<f32>,
    pub normals: Vec<f32>,
    pub uvs: Vec<f32>,
    pub indices: Vec<u32>,
    pub groups: Vec<GeoGroup>,
    /// Bounding box, so the viewer can frame the model without guessing a camera distance.
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    /// How many draw groups actually use the texture. Zero means the texture isn't on any
    /// part of the variant we could draw.
    pub highlighted_groups: usize,
    /// The `SEGM` state/LOD bit this geometry was built from (`None` = unfiltered, every
    /// group). Models don't all share the engine's default `0x01`, and the bits are state
    /// masks rather than an ordered detail ladder — see `build_best_tier`.
    pub tier: Option<u8>,
}

/// Geometry for one state/LOD tier of a model.
type Built = (Vec<Vertex>, Vec<u32>, Vec<DrawGroup>, ModelStats);

/// Pick which state/LOD tier of the model to draw, and build it.
///
/// # Why this isn't just "tier 0x01"
///
/// A model's meshes are gated by a `SEGM` state mask, and the viewer originally hardcoded the
/// engine's default render bit `0x01`. That is wrong for two reasons found in the wild:
///
/// * **Some models don't have that tier at all.** `vz_hum_deathsquad_a` declares only `0x08`,
///   so asking for `0x01` yields zero groups and the build fails outright.
/// * **The bits are not an ordered LOD ladder.** `al_veh_tank_m1a2` declares
///   `[0x01,0x02,0x04,0x08,0x10,0x20,0x40]`, and the *higher* bits carry **more** geometry
///   (11 groups vs 8) — they are state masks (intact / damaged / …), not detail levels.
///
/// So: try the tiers the model actually declares. Prefer one that **contains the texture we
/// came here to look at** — otherwise clicking a texture that only appears on, say, the
/// damaged variant would show you a model with nothing lit up. Failing that, fall back to
/// `0x01` (what the engine shows by default), then any tier, then everything unfiltered.
fn build_best_tier(container: &[u8], tex_hash: u32) -> Result<(Built, Option<u8>), String> {
    // Match against EVERY slot of the group's material. A material binds up to 10 textures;
    // checking only diffuse/specular/normal misses the rest.
    let uses = |groups: &[DrawGroup]| groups.iter().any(|d| d.textures.contains(&tex_hash));

    let tiers = state_tiers(container);

    // 1. A tier that actually shows this texture. Prefer the engine's default bit on a tie.
    let mut ordered: Vec<u8> = tiers.clone();
    ordered.sort_by_key(|&t| (t != 0x01, t)); // 0x01 first, then ascending
    for t in &ordered {
        if let Ok(b) = build_indexed_state(container, *t) {
            if !b.2.is_empty() && uses(&b.2) {
                return Ok((b, Some(*t)));
            }
        }
    }

    // 2. No tier shows it — draw the model anyway, so the user still sees *something* and the
    //    UI can say "this texture isn't on any visible part".
    for t in &ordered {
        if let Ok(b) = build_indexed_state(container, *t) {
            if !b.2.is_empty() {
                return Ok((b, Some(*t)));
            }
        }
    }

    // 3. Nothing declared a usable tier: take every group, unfiltered.
    let b = build_indexed_all(container)?;
    if b.2.is_empty() {
        return Err("this model has no drawable geometry".into());
    }
    Ok((b, None))
}

/// Resolve a model reference to its asset hash.
///
/// The WAD addresses models **by hash**, not by name — the name is only ever an input to
/// `pandemic_hash_m2`. So a model whose name we never recovered is still perfectly loadable:
/// we already have its hash from the ASET table. Accepting a `0x…` reference here is what
/// lets the 3D view work for the ~thousands of models we can't name (an atlas texture like
/// `vz_angel_falls_tiny_tinygeometry_*` is used by 34 models, *all* of them unnamed — without
/// this, that page can't show a single one of them).
fn resolve_model(reference: &str) -> u32 {
    let r = reference.trim();
    if let Some(hex) = r.strip_prefix("0x").or_else(|| r.strip_prefix("0X")) {
        if let Ok(h) = u32::from_str_radix(hex, 16) {
            return h;
        }
    }
    pandemic_hash_m2(r)
}

fn vz_wad(game_path: &str) -> Result<std::path::PathBuf, String> {
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

/// One state/LOD variant of a model, and whether the texture shows up in it.
///
/// This is the answer to "the texture isn't visible — under what conditions IS it?". Rather
/// than leaving the user to guess, we build every state bit the model declares and report
/// which ones paint the texture, so they can be toggled.
#[derive(Debug, Clone, Serialize)]
pub struct ModelVariant {
    /// The `SEGM` state/LOD bit (`None` = every group, unfiltered).
    pub tier: Option<u8>,
    pub groups: usize,
    pub triangles: usize,
    /// Draw groups in THIS variant that bind the texture.
    pub highlighted: usize,
    /// Convenience: `highlighted > 0`.
    pub shows_texture: bool,
}

/// Every variant of a model, flagged with whether the texture appears in it.
///
/// Cheap enough to call on page load: it decompresses the container once and rebuilds the
/// index/group tables per bit (a few ms), with no texture decode.
#[tauri::command]
pub fn model_variants(
    game_path: String,
    model: String,
    texture: String,
) -> Result<Vec<ModelVariant>, String> {
    let wad = vz_wad(&game_path)?;
    let mut f = std::fs::File::open(&wad).map_err(|e| format!("open vz.wad: {e}"))?;
    let size = f.metadata().map_err(|e| format!("stat: {e}"))?.len();
    let archive = load_ffcs_archive(&mut f, size).map_err(|e| format!("FFCS: {e}"))?;

    let container = extract_model(&mut f, &archive, resolve_model(&model))
        .map_err(|_| format!("Couldn't read the model \"{model}\" from your game."))?;
    let tex_hash = pandemic_hash_m2(&texture);

    let count =
        |groups: &[DrawGroup]| groups.iter().filter(|d| d.textures.contains(&tex_hash)).count();

    let mut out = Vec::new();
    for t in state_tiers(&container) {
        if let Ok((_, indices, groups, _)) = build_indexed_state(&container, t) {
            if groups.is_empty() {
                continue;
            }
            let highlighted = count(&groups);
            out.push(ModelVariant {
                tier: Some(t),
                groups: groups.len(),
                triangles: indices.len() / 3,
                highlighted,
                shows_texture: highlighted > 0,
            });
        }
    }

    // The unfiltered build, but only when it differs from every single-bit variant — for most
    // models it is identical to one of them and would just be a confusing duplicate row.
    if let Ok((_, indices, groups, _)) = build_indexed_all(&container) {
        if !groups.is_empty() && !out.iter().any(|v| v.groups == groups.len()) {
            let highlighted = count(&groups);
            out.push(ModelVariant {
                tier: None,
                groups: groups.len(),
                triangles: indices.len() / 3,
                highlighted,
                shows_texture: highlighted > 0,
            });
        }
    }

    // Most detail first — that's the version a user means by "the model".
    out.sort_by_key(|v| std::cmp::Reverse(v.triangles));
    Ok(out)
}

/// One part, somewhere in the game, that paints a given texture.
#[derive(Debug, Clone, Serialize)]
pub struct TexturePart {
    /// Model that owns it — pass `reference` to the geometry commands.
    pub model: String,
    pub model_name: Option<String>,
    pub model_hash: u32,
    /// Part id within that model's group list — **valid only for `tier`**.
    ///
    /// Part ids are an index into the built group list, and that list depends on which state
    /// bit was built. So a part id is meaningless without the tier it came from: pass both
    /// back to `model_geometry` or you will isolate a different part entirely.
    pub part: usize,
    /// The state bit this part's id belongs to (`None` = the auto-selected one, which
    /// `model_geometry` reproduces when given no tier).
    pub tier: Option<u8>,
    pub triangles: usize,
    /// Which MTRL slot binds the texture.
    pub slot: String,
    /// The state/LOD bits this part is drawn in.
    pub lod_mask: u8,
}

/// Every part, in every model, that paints this texture.
///
/// The per-model parts list answers "where is it on *this* model". This answers the broader
/// question — "what, in the whole game, is this texture actually on" — without making the user
/// click through each model in turn. For an atlas shared by 34 models that difference is the
/// whole point.
///
/// Uses the cached usage index to know which models to open, so it only decodes the handful
/// that genuinely paint it.
#[tauri::command]
pub fn texture_parts(game_path: String, texture: String) -> Result<Vec<TexturePart>, String> {
    let wad = vz_wad(&game_path)?;
    let mut f = std::fs::File::open(&wad).map_err(|e| format!("open vz.wad: {e}"))?;
    let size = f.metadata().map_err(|e| format!("stat: {e}"))?.len();
    let archive = load_ffcs_archive(&mut f, size).map_err(|e| format!("FFCS: {e}"))?;

    let tex_hash = pandemic_hash_m2(&texture);
    let index = crate::commands::texture_usage::load_or_build(&wad)?;
    let names =
        crate::commands::texture_usage::name_table(crate::commands::texture_swap::ASSET_NAMES);

    let mut out = Vec::new();
    for &m in index.painted_by(tex_hash) {
        let Ok(container) = extract_model(&mut f, &archive, m) else {
            continue;
        };
        // Build the SAME variant the viewer will show. Part ids index into the built group
        // list, so listing ids from an unfiltered build (Chris: 40 groups) while the viewer
        // renders a tier-filtered one (25 groups) would make every click isolate the wrong
        // part. Use `build_best_tier` — exactly what `model_geometry` does with no tier — and
        // hand the tier back so the caller can pin it.
        let Ok(((_, _, draws, _), tier)) = build_best_tier(&container, tex_hash) else {
            continue;
        };
        let model_name = names.get(&m).map(|s| s.to_string());

        for (id, d) in draws.iter().enumerate() {
            let Some(i) = d.textures.iter().position(|&t| t == tex_hash) else {
                continue;
            };
            out.push(TexturePart {
                model: model_name.clone().unwrap_or_else(|| format!("0x{m:08X}")),
                model_name: model_name.clone(),
                model_hash: m,
                part: id,
                tier,
                triangles: (d.index_count / 3) as usize,
                slot: match i {
                    0 => "diffuse".into(),
                    1 => "specular".into(),
                    2 => "normal".into(),
                    n => format!("map {}", n + 1),
                },
                lod_mask: d.lod_mask,
            });
        }
    }

    // Biggest parts first — that's where a reskin actually shows.
    out.sort_by_key(|p| std::cmp::Reverse(p.triangles));
    Ok(out)
}

/// Decode `model` and mark every draw group whose material samples `texture`.
///
/// `texture` is the *name*; it is hashed the same way the engine does, so a group matches
/// exactly when the engine would bind that asset to it.
///
/// `tier` forces a specific `SEGM` state bit (from [`model_variants`]); omit it to let
/// [`build_best_tier`] choose one that shows the texture.
#[tauri::command]
pub fn model_geometry(
    game_path: String,
    model: String,
    texture: String,
    tier: Option<u8>,
) -> Result<ModelGeometry, String> {
    let wad = vz_wad(&game_path)?;
    let mut f = std::fs::File::open(&wad).map_err(|e| format!("open vz.wad: {e}"))?;
    let size = f.metadata().map_err(|e| format!("stat: {e}"))?.len();
    let archive = load_ffcs_archive(&mut f, size).map_err(|e| format!("FFCS: {e}"))?;

    let model_hash = resolve_model(&model);
    let container = extract_model(&mut f, &archive, model_hash)
        .map_err(|_| format!("Couldn't read the model \"{model}\" from your game."))?;

    let tex_hash = pandemic_hash_m2(&texture);

    // An explicit tier (the user clicked a variant chip) wins; otherwise pick one that shows
    // the texture — see `build_best_tier`.
    let ((verts, indices, draws, stats), tier) = match tier {
        Some(bit) => {
            let b = build_indexed_state(&container, bit)
                .map_err(|e| format!("Couldn't show \"{model}\" at that detail level: {e}"))?;
            (b, Some(bit))
        }
        None => build_best_tier(&container, tex_hash)
            .map_err(|e| format!("Couldn't show \"{model}\": {e}"))?,
    };

    let mut positions = Vec::with_capacity(verts.len() * 3);
    let mut normals = Vec::with_capacity(verts.len() * 3);
    let mut uvs = Vec::with_capacity(verts.len() * 2);
    for v in &verts {
        positions.extend_from_slice(&v.pos);
        normals.extend_from_slice(&v.normal);
        uvs.extend_from_slice(&v.uv);
    }

    // Slots 0/1/2 are the named maps; anything beyond is a secondary map the material declares
    // but the renderer doesn't sample — worth naming rather than hiding.
    fn slot_name(i: usize) -> String {
        match i {
            0 => "diffuse".into(),
            1 => "specular".into(),
            2 => "normal".into(),
            n => format!("map {}", n + 1),
        }
    }

    let names = crate::commands::texture_usage::name_table(crate::commands::texture_swap::ASSET_NAMES);

    let groups: Vec<GeoGroup> = draws
        .iter()
        .enumerate()
        .map(|(id, d)| {
            let textures: Vec<SlotRef> = d
                .textures
                .iter()
                .enumerate()
                .filter(|(_, &t)| t != 0)
                .map(|(i, &t)| SlotRef {
                    slot: slot_name(i),
                    hash: t,
                    name: names.get(&t).map(|s| s.to_string()),
                    is_current: t == tex_hash,
                })
                .collect();

            let slot = textures.iter().find(|s| s.is_current).map(|s| s.slot.clone());

            GeoGroup {
                id,
                index_start: d.index_start,
                index_count: d.index_count,
                triangles: (d.index_count / 3) as usize,
                uses_texture: slot.is_some(),
                slot,
                diffuse: d.diffuse,
                textures,
                prmg: d.group_index,
                lod_mask: d.lod_mask,
                node: d.node,
            }
        })
        .collect();

    let highlighted_groups = groups.iter().filter(|g| g.uses_texture).count();

    Ok(ModelGeometry {
        model,
        model_hash,
        positions,
        normals,
        uvs,
        indices,
        groups,
        bbox_min: stats.bbox_min,
        bbox_max: stats.bbox_max,
        highlighted_groups,
        tier,
    })
}
