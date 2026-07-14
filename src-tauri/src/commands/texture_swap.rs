//! Replace a texture in the game with your own image — safely.
//!
//! # The donor decides the shape; we publish a fully-resident replacement
//!
//! Most of the game's textures are **streamed**: their inline `BODY` holds only a small
//! resident *tail* (a 1024² map can arrive as a 32×32 stub) while the real mips live in
//! separate streaming blocks. 9,562 of the 13,339 retail textures are like this — including
//! the character and vehicle skins people actually want to reskin. So you cannot swap one
//! by overwriting its body in place: you would be painting the stub while the real pixels
//! stream in from somewhere else.
//!
//! What the engine *does* support — and what the shipped mattias_v5 / Obama skins actually
//! do — is publish a **fully-resident** container under the same asset hash:
//! `INFO[26..32] = 0` means "no streaming, the whole chain is inline", and the engine then
//! reads exactly `linear_mip_chain_size(w, h, fourcc, mips)` bytes from `BODY`.
//!
//! So: read the donor out of the player's own `vz.wad` to learn its **name, dimensions and
//! format**, re-encode the user's image to exactly those, and emit a fresh resident
//! container ([`mercs2_formats::texture::build_resident_texture`], a port of the
//! `dds_to_ucfx_texture.py` that produced those skins).
//!
//! # The invariant that must never break
//!
//! `BODY` must be **exactly** the dimension-derived mip chain. The engine reads the full
//! chain regardless of the header's mip count, so a body even slightly short makes the
//! streaming worker over-read → `STATUS_BUFFER_TOO_SMALL` → the page never becomes ready →
//! the **world load hangs**. That is a livelock, not a crash: no error, no stack trace, just
//! a game that never finishes loading. `build_resident_texture` refuses a body of the wrong
//! length, and we independently re-check our own encoder against `texsize` before calling it.
//!
//! # Consequence the user must be told
//!
//! The image is resized to the game's own resolution for that texture. **Upscaling is not
//! supported yet**: the dimensions are baked into the material/streaming setup, and raising
//! them is exactly the unproven path that ends in the hang above. Saying so is better than
//! shipping one.

use std::path::{Path, PathBuf};

use image::imageops::FilterType;
use mercs2_formats::ffcs::load_ffcs_archive;
use mercs2_formats::hash::pandemic_hash_m2;
use mercs2_formats::patch_wad::{AsetEntry, PatchBlock};
use mercs2_formats::texsize::{
    dxt_format, dxt_mip_count, info_is_fully_resident, linear_mip_chain_size,
};
use mercs2_formats::texture::{
    build_resident_texture, extract_container, parse_texture_container, TexFormat, TextureData,
};
use mercs2_formats::types::{TYPE_ID_TEXTURE, TYPE_HASH_TEXTURE};
use serde::{Deserialize, Serialize};
use texpresso::Format;

use crate::commands::texture_usage::ModelRef;

/// Every asset name we know, one per line.
///
/// The WAD's ASET table stores only 32-bit hashes, so a *browsable* texture list is only
/// possible for textures whose name we can recover. This list is the union of the project's
/// recovered-name tables (`docs/data/aset_*_names.json`), and we never trust it blindly: a
/// name earns its place in the catalog only when `pandemic_hash_m2(name)` matches a texture
/// actually present in the player's own WAD. Coverage on a stock install: **3,774 of 3,778**
/// primary textures (99.9%).
pub(crate) const ASSET_NAMES: &str = include_str!("../../data/asset_names.txt");

/// One texture in the browsable catalog. Deliberately cheap — no block is decompressed to
/// build this, so listing thousands is instant. Dimensions and pixels are fetched on demand
/// ([`inspect_texture`], [`texture_previews`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextureEntry {
    pub name: String,
    pub asset_hash: u32,
    /// Leading token of the name (`pmc`, `al`, `city`, …) — the game's own grouping.
    pub category: String,
    /// `diffuse` | `normal` | `specular` | `other`, from the name suffix.
    pub kind: String,
}

/// A decoded thumbnail for one texture.
#[derive(Debug, Clone, Serialize)]
pub struct TexturePreview {
    pub name: String,
    /// `data:image/png;base64,…` — ready to drop into an `<img src>`.
    pub data_url: String,
    /// The texture's real size in-game.
    pub width: u32,
    pub height: u32,
    /// Size of the image we could actually decode. Smaller than `width`×`height` when the
    /// texture is streamed: only its lowest mips are stored inline.
    pub preview_width: u32,
    pub preview_height: u32,
}

/// What a texture swap will do, shown before the user commits to it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextureTarget {
    /// Engine texture name, e.g. `us_veh_abrams_dm`.
    pub name: String,
    pub asset_hash: u32,
    pub width: u32,
    pub height: u32,
    /// `DXT1` or `DXT5`.
    pub format: String,
    /// False => streamed world-cell texture; we refuse to swap it (see module docs).
    pub swappable: bool,
    pub reason: Option<String>,
}

/// One requested swap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextureSwap {
    /// Engine name of the texture to replace.
    pub name: String,
    /// The user's image (PNG/JPG) on disk.
    pub image_path: String,
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

/// Pull the donor container and read the fields that constrain the swap.
fn donor(game_path: &str, name: &str) -> Result<(Vec<u8>, u32, u32, u32, TexFormat, bool), String> {
    let wad = vz_wad(game_path)?;
    let mut f = std::fs::File::open(&wad).map_err(|e| format!("open vz.wad: {e}"))?;
    let size = f.metadata().map_err(|e| format!("stat: {e}"))?.len();
    let archive = load_ffcs_archive(&mut f, size).map_err(|e| format!("FFCS: {e}"))?;

    let hash = pandemic_hash_m2(name);
    let container = extract_container(&mut f, &archive, hash, TYPE_ID_TEXTURE, TYPE_HASH_TEXTURE)
        .map_err(|_| format!("Your game has no texture called \"{name}\"."))?;

    // INFO: u16 width@0, u16 height@2, .., fourcc@14. Read it straight so we also get the
    // residency bytes (INFO[26..32]) that decide whether this texture is streamed.
    let info = texture_info(&container).ok_or("texture container has no INFO chunk")?;
    if info.len() < 32 {
        return Err("texture INFO chunk is too short to inspect".into());
    }
    let width = u16::from_le_bytes([info[0], info[1]]) as u32;
    let height = u16::from_le_bytes([info[2], info[3]]) as u32;
    let mut fourcc = [0u8; 4];
    fourcc.copy_from_slice(&info[14..18]);
    // Almost every texture is DXT1/DXT5. A rare few are not: `cloud_noise` stores the D3D
    // format enum 21 (A8R8G8B8, uncompressed) where the fourcc would be. We can still *show*
    // those (see `decode_any`), but not replace them — the swap path re-encodes to DXT.
    let format = TexFormat::from_fourcc(&fourcc).ok_or_else(|| {
        format!(
            "\"{name}\" is stored in a format modkit can't edit (not DXT1/DXT5). \
             You can still view and export it, but not replace it."
        )
    })?;

    let resident = info_is_fully_resident(&info);
    Ok((container, hash, width, height, format, resident))
}

/// Raw INFO leaf bytes of a texture container.
///
/// Same shape as everywhere else: a 20-byte header, then 20-byte descriptor rows; a row
/// whose `u0 == 0xFFFFFFFF` is a container marker rather than a leaf, and a leaf's body
/// lives at `data_area + u0`.
fn texture_info(container: &[u8]) -> Option<Vec<u8>> {
    if container.len() < 20 || &container[0..4] != b"UCFX" {
        return None;
    }
    let rd = |o: usize| -> Option<usize> {
        container
            .get(o..o + 4)
            .and_then(|b| b.try_into().ok())
            .map(|b| u32::from_le_bytes(b) as usize)
    };

    let data_area = rd(4)?;
    let n_desc = rd(16)?;
    for i in 0..n_desc {
        let row = 20 + i * 20;
        if row + 20 > container.len() {
            return None;
        }
        if &container[row..row + 4] != b"INFO" {
            continue;
        }
        let u0 = rd(row + 4)?;
        if u0 == 0xFFFF_FFFF {
            continue; // a marker, not a leaf
        }
        let size = rd(row + 8)?;
        let start = if data_area > 0 { data_area + u0 } else { 8 + u0 };
        let end = start.checked_add(size)?;
        if end <= container.len() {
            return Some(container[start..end].to_vec());
        }
    }
    None
}

/// Classify a texture by its name suffix. The game's convention is `X` = the diffuse map,
/// `X_nm` = its normal map, `X_sm` = its specular map.
fn classify(name: &str) -> &'static str {
    if name.ends_with("_nm") {
        "normal"
    } else if name.ends_with("_sm") {
        "specular"
    } else if name.ends_with("_dm") || name.ends_with("_c") {
        "diffuse"
    } else {
        "other"
    }
}

/// The leading token of a name (`pmc_hum_chris_ub` → `pmc`) — the game's own grouping.
fn category(name: &str) -> String {
    name.split('_').next().unwrap_or("other").to_string()
}

/// Every texture in this install that we can name.
///
/// Built from the ASET table alone — no blocks are decompressed — so this is fast even
/// though it returns thousands of rows. Both primary and sub-entry rows are considered,
/// because `extract_container` resolves either (a shared texture may only exist as a
/// sub-entry of another asset's block).
#[tauri::command]
pub fn list_textures(game_path: String) -> Result<Vec<TextureEntry>, String> {
    let wad = vz_wad(&game_path)?;
    let mut f = std::fs::File::open(&wad).map_err(|e| format!("open vz.wad: {e}"))?;
    let size = f.metadata().map_err(|e| format!("stat: {e}"))?.len();
    let archive = load_ffcs_archive(&mut f, size).map_err(|e| format!("FFCS: {e}"))?;

    let present: std::collections::HashSet<u32> = archive
        .aset
        .iter()
        .filter(|e| e.type_id == TYPE_ID_TEXTURE)
        .map(|e| e.asset_hash)
        .collect();

    let mut out: Vec<TextureEntry> = ASSET_NAMES
        .lines()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .filter_map(|name| {
            let hash = pandemic_hash_m2(name);
            present.contains(&hash).then(|| TextureEntry {
                name: name.to_string(),
                asset_hash: hash,
                category: category(name),
                kind: classify(name).to_string(),
            })
        })
        .collect();

    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.asset_hash == b.asset_hash);
    Ok(out)
}

/// Decode thumbnails for a page of textures.
///
/// Batched because each one costs a block decompression — the UI asks for the ~60 rows it
/// is about to show, not all 3,700.
///
/// # What you actually get
///
/// A streamed texture stores only the **tail** of its mip chain inline (verified: a 512×512
/// DXT1 skin keeps mips 3-and-smaller, i.e. 64×64 down; a 512² DXT5 normal map keeps 32×32
/// down). The big mips live in separate streaming blocks we don't read here. So the preview
/// is the largest mip that is genuinely present, and `preview_width`/`preview_height` say
/// how big that was — we never upscale a 32×32 stub and pretend it is the 512×512 texture.
#[tauri::command]
pub fn texture_previews(
    game_path: String,
    names: Vec<String>,
    max_size: Option<u32>,
) -> Result<Vec<TexturePreview>, String> {
    // Thumbnails go into the DOM as base64 data URLs, so their size is not free: a raw
    // 512x512 PNG of a noisy game texture lands around 0.5-1 MB, and a grid of those would
    // crawl. Capping the long edge at 128 px takes a typical skin to ~25 KB.
    let cap = max_size.unwrap_or(128).max(16);

    let wad = vz_wad(&game_path)?;
    let mut f = std::fs::File::open(&wad).map_err(|e| format!("open vz.wad: {e}"))?;
    let size = f.metadata().map_err(|e| format!("stat: {e}"))?.len();
    let archive = load_ffcs_archive(&mut f, size).map_err(|e| format!("FFCS: {e}"))?;

    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let hash = pandemic_hash_m2(&name);
        let Ok(container) =
            extract_container(&mut f, &archive, hash, TYPE_ID_TEXTURE, TYPE_HASH_TEXTURE)
        else {
            continue; // skip quietly: a thumbnail is not worth failing a whole page over
        };
        if let Some(p) = decode_any(&name, &container, cap) {
            out.push(p);
        }
    }
    Ok(out)
}

/// Decode a texture container to a thumbnail, whatever shape it is in.
///
/// Tries the normal DXT path first, then falls back to an uncompressed decode. The fallback
/// exists because not every texture in the game is DXT: `cloud_noise` is 512×512 with a
/// "fourcc" of `0x00000015`, which is not four characters at all — it is the D3D format enum
/// **21 = `D3DFMT_A8R8G8B8`**, i.e. plain 32-bit BGRA (body = 512·512·4 bytes exactly).
///
/// It is one texture out of 13,422, so this is deliberately handled *here* rather than by
/// teaching `mercs2_formats::TexFormat` a new variant — the engine uploads through that enum,
/// and a preview thumbnail is not a good reason to widen its contract. But a format we don't
/// understand should degrade to "no picture", not to a silently blank tile with no
/// explanation, and any future/DLC format lands in this same path.
fn decode_any(name: &str, container: &[u8], cap: u32) -> Option<TexturePreview> {
    if let Ok(t) = parse_texture_container(container) {
        return decode_preview(name, &t, cap);
    }
    decode_uncompressed(name, container, cap)
}

/// D3D format enums we can read directly (both are 32-bit little-endian BGRA/BGRX).
const D3DFMT_A8R8G8B8: u32 = 21;
const D3DFMT_X8R8G8B8: u32 = 22;

fn decode_uncompressed(name: &str, container: &[u8], cap: u32) -> Option<TexturePreview> {
    let info = texture_info(container)?;
    if info.len() < 18 {
        return None;
    }
    let width = u16::from_le_bytes([info[0], info[1]]) as u32;
    let height = u16::from_le_bytes([info[2], info[3]]) as u32;
    let fmt = u32::from_le_bytes([info[14], info[15], info[16], info[17]]);
    if !matches!(fmt, D3DFMT_A8R8G8B8 | D3DFMT_X8R8G8B8) {
        return None;
    }

    let body = mercs2_formats::texture::texture_body(container)?;
    let needed = (width as usize) * (height as usize) * 4;
    if width == 0 || height == 0 || body.len() < needed {
        return None;
    }

    // D3D stores these as BGRA in memory order; PNG wants RGBA.
    let mut rgba = Vec::with_capacity(needed);
    for px in body[..needed].chunks_exact(4) {
        rgba.extend_from_slice(&[px[2], px[1], px[0], if fmt == D3DFMT_X8R8G8B8 { 255 } else { px[3] }]);
    }
    let img = image::RgbaImage::from_raw(width, height, rgba)?;
    Some(finish_preview(name, img, width, height, width, height, cap))
}

/// Shrink to the thumbnail cap and encode as a PNG data URL.
fn finish_preview(
    name: &str,
    img: image::RgbaImage,
    pw: u32,
    ph: u32,
    full_w: u32,
    full_h: u32,
    cap: u32,
) -> TexturePreview {
    // Never enlarge: a 32x32 stub stays 32x32 rather than being blown up to look like detail
    // it doesn't have.
    let long = pw.max(ph);
    let thumb = if long > cap {
        let (tw, th) = ((pw * cap / long).max(1), (ph * cap / long).max(1));
        image::imageops::resize(&img, tw, th, FilterType::Triangle)
    } else {
        img
    };

    let mut png: Vec<u8> = Vec::new();
    let _ = image::DynamicImage::ImageRgba8(thumb)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png);

    TexturePreview {
        name: name.to_string(),
        data_url: format!("data:image/png;base64,{}", b64(&png)),
        width: full_w,
        height: full_h,
        preview_width: pw,
        preview_height: ph,
    }
}

/// Decode the largest mip actually present inline, as a PNG data URL (long edge <= `cap`).
///
/// # The inline body comes in two shapes, and only one of them is a mip *tail*
///
/// * **Streamed** (most world/character art): the body holds the *tail* of the chain — the
///   small levels — while the big ones are paged in from other blocks. A 512² DXT1 skin can
///   keep only mips 3-and-down, i.e. 64² inline.
/// * **Prefix / single level** (HUD icons, UI art): the body simply *starts* at mip 0. A
///   `HUD_*` icon is 64×64 DXT5 with `mips = 1` and a body of exactly 4096 bytes — one level,
///   no chain at all.
///
/// This originally only understood the tail case, so every HUD texture matched nothing and
/// silently produced no thumbnail. Decide by size instead of assuming: if the body is at
/// least as big as mip 0, it *begins* with mip 0 and we decode that at full size; only a body
/// too small to hold mip 0 can be a resident tail.
fn decode_preview(name: &str, t: &TextureData, cap: u32) -> Option<TexturePreview> {
    let (bpx, pitch, _) = dxt_format(t.format.fourcc())?;
    let mips = dxt_mip_count(t.width as usize, t.height as usize);
    let body = t.all_mips.len();

    let level_bytes = |i: usize| -> usize {
        let w = (t.width as usize >> i).max(1);
        let h = (t.height as usize >> i).max(1);
        w.div_ceil(bpx).max(1) * h.div_ceil(bpx).max(1) * pitch
    };

    let first = if body >= level_bytes(0) {
        // Body starts at mip 0 — a full chain, a partial one, or a lone level. Full res.
        0
    } else {
        // Too small for mip 0, so it's a resident tail. Find where that tail begins.
        let tail_from = |k: usize| -> usize { (k..mips).map(level_bytes).sum() };
        (1..mips).find(|&k| tail_from(k) == body)?
    };

    let pw = (t.width >> first).max(1);
    let ph = (t.height >> first).max(1);
    let n = level_bytes(first);
    let src = t.all_mips.get(..n)?;

    let bc = match t.format {
        TexFormat::Bc1 => Format::Bc1,
        TexFormat::Bc3 => Format::Bc3,
    };
    let mut rgba = vec![0u8; (pw * ph * 4) as usize];
    bc.decompress(src, pw as usize, ph as usize, &mut rgba);

    let img = image::RgbaImage::from_raw(pw, ph, rgba)?;
    Some(finish_preview(name, img, pw, ph, t.width, t.height, cap))
}

/// Minimal base64 (standard alphabet, padded) — not worth a dependency.
fn b64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        s.push(A[(n >> 18 & 63) as usize] as char);
        s.push(A[(n >> 12 & 63) as usize] as char);
        s.push(if c.len() > 1 { A[(n >> 6 & 63) as usize] as char } else { '=' });
        s.push(if c.len() > 2 { A[(n & 63) as usize] as char } else { '=' });
    }
    s
}

/// Everything the details page shows for one texture.
#[derive(Debug, Clone, Serialize)]
pub struct TextureDetails {
    pub name: String,
    pub asset_hash: u32,
    pub category: String,
    pub kind: String,
    pub width: u32,
    pub height: u32,
    pub format: String,
    /// Bytes of the mip chain the engine reads for this texture.
    pub chain_bytes: usize,
    pub mip_count: usize,
    /// False => the game streams its detail from separate blocks.
    pub fully_resident: bool,
    /// A bigger preview than the grid thumbnail.
    pub preview: Option<TexturePreview>,
    /// Models that actually **paint** this texture on a surface. These are the ones the 3D
    /// view can show, and a hit here always highlights.
    pub used_by: Vec<ModelRef>,
    /// Models that reference it in a material but never bind it to a drawable group — its
    /// geometry lives in the wreck variant, a separate sub-model, or a LOD the model doesn't
    /// carry. Surfaced separately rather than pretending we can show them.
    pub declared_only_by: Vec<ModelRef>,
    /// True when more than one model paints it — replacing it changes all of them.
    pub shared: bool,
    /// The other maps of the same surface: `X` / `X_nm` / `X_sm`.
    pub siblings: Vec<TextureEntry>,
    /// Other textures worn by the models above — the rest of that character/vehicle.
    pub seen_with: Vec<TextureEntry>,
}

/// Strip a known map suffix to get the base surface name (`chris_ub_nm` -> `chris_ub`).
fn base_name(name: &str) -> &str {
    for suf in ["_nm", "_sm", "_dm"] {
        if let Some(stem) = name.strip_suffix(suf) {
            return stem;
        }
    }
    name
}

/// Full detail for one texture: what it is, what uses it, and what it sits alongside.
///
/// The "used by" relation does not exist in the WAD — it is inverted out of every model's
/// MTRL slots (see [`crate::commands::texture_usage`]). The first call on an install builds
/// that index (~10s in release); later calls read it from cache.
#[tauri::command]
pub fn texture_details(game_path: String, name: String) -> Result<TextureDetails, String> {
    let wad = vz_wad(&game_path)?;
    let (container, hash, width, height, format, resident) = donor(&game_path, &name)?;

    let t = parse_texture_container(&container)
        .map_err(|e| format!("{name}: {e}"))?;
    let mips = dxt_mip_count(width as usize, height as usize);
    let chain_bytes = linear_mip_chain_size(width as usize, height as usize, format.fourcc(), mips);

    // Who uses it.
    let index = crate::commands::texture_usage::load_or_build(&wad)?;
    let names = crate::commands::texture_usage::name_table(ASSET_NAMES);
    let user_hashes = index.painted_by(hash).to_vec();
    let used_by = crate::commands::texture_usage::name_models(&user_hashes, &names);
    let declared_only_by =
        crate::commands::texture_usage::name_models(&index.declared_only_by(hash), &names);

    // The catalog, so siblings/companions can be reported as real, browsable entries.
    let catalog = list_textures(game_path.clone())?;
    let by_hash: std::collections::HashMap<u32, &TextureEntry> =
        catalog.iter().map(|e| (e.asset_hash, e)).collect();

    // Siblings: the other maps of the same surface (diffuse / normal / specular).
    let base = base_name(&name);
    let siblings: Vec<TextureEntry> = catalog
        .iter()
        .filter(|e| e.name != name && base_name(&e.name) == base)
        .cloned()
        .collect();

    // Companions: every other texture the same models paint — i.e. the rest of that
    // character or vehicle. This is the useful part of "how do textures relate".
    let mut seen_with: Vec<TextureEntry> = Vec::new();
    let mut added: std::collections::HashSet<u32> = [hash].into_iter().collect();
    for m in &user_hashes {
        for (tex_key, users) in &index.painted {
            if !users.contains(m) {
                continue;
            }
            let Ok(tex) = tex_key.parse::<u32>() else { continue };
            if !added.insert(tex) {
                continue;
            }
            if let Some(e) = by_hash.get(&tex) {
                seen_with.push((*e).clone());
            }
        }
    }
    seen_with.sort_by(|a, b| a.name.cmp(&b.name));

    let preview = decode_preview(&name, &t, 256);

    Ok(TextureDetails {
        name: name.clone(),
        asset_hash: hash,
        category: category(&name),
        kind: classify(&name).to_string(),
        width,
        height,
        format: String::from_utf8_lossy(format.fourcc()).to_string(),
        chain_bytes,
        mip_count: mips,
        fully_resident: resident,
        preview,
        shared: used_by.len() > 1,
        used_by,
        declared_only_by,
        siblings,
        seen_with,
    })
}

/// What `export_texture` actually managed to write.
#[derive(Debug, Clone, Serialize)]
pub struct TextureExport {
    pub path: String,
    /// Size of the PNG we wrote.
    pub width: u32,
    pub height: u32,
    /// The texture's real in-game size.
    pub full_width: u32,
    pub full_height: u32,
    /// False when the game streams this texture's detail from elsewhere, so the largest
    /// version stored *with* it is smaller than its nominal size. We export what exists
    /// rather than upscaling a stub and calling it full-resolution.
    pub is_full_resolution: bool,
}

/// Save a texture out as a PNG the user can open and edit.
///
/// Exports the largest mip actually stored inline. For a fully-resident texture that is the
/// real thing at full size; for a streamed one it's the resident tail (a 512² skin may only
/// keep 64² inline), and `is_full_resolution` says so — better an honest small PNG than a
/// blurry upscale presented as the original.
#[tauri::command]
pub fn export_texture(
    game_path: String,
    name: String,
    dest: String,
) -> Result<TextureExport, String> {
    let (container, _hash, width, height, _fmt, _resident) = donor(&game_path, &name)?;

    // `u32::MAX` cap = never downscale; we want the real pixels here, not a thumbnail.
    // `decode_any`, so a non-DXT texture (the uncompressed `cloud_noise`) still exports.
    let p = decode_any(&name, &container, u32::MAX)
        .ok_or_else(|| format!("Couldn't decode the pixels of \"{name}\"."))?;

    // `decode_preview` hands back a data URL; we want the PNG bytes on disk.
    let b64 = p
        .data_url
        .split_once("base64,")
        .map(|(_, d)| d)
        .ok_or("internal: preview was not a base64 data URL")?;
    let png = b64_decode(b64).ok_or("internal: could not decode the preview PNG")?;

    std::fs::write(&dest, &png).map_err(|e| format!("Couldn't write {dest}: {e}"))?;

    Ok(TextureExport {
        path: dest,
        width: p.preview_width,
        height: p.preview_height,
        full_width: width,
        full_height: height,
        is_full_resolution: p.preview_width == width && p.preview_height == height,
    })
}

/// Inverse of [`b64`]. Standard alphabet, tolerant of padding.
fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 26,
            b'0'..=b'9' => (c - b'0') as u32 + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    };
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let bytes: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    for chunk in bytes.chunks(4) {
        let pad = chunk.iter().filter(|&&c| c == b'=').count();
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= if c == b'=' { 0 } else { val(c)? } << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// Describe what swapping `name` would do — and whether we'll allow it.
#[tauri::command]
pub fn inspect_texture(game_path: String, name: String) -> Result<TextureTarget, String> {
    let (_c, hash, width, height, format, resident) = donor(&game_path, &name)?;

    // A replacement is published as a fully-resident container regardless of how the
    // original was stored, so a streamed donor is fine — that is the normal case, and the
    // shape the proven character skins use.
    let reason = (!resident).then(|| {
        format!(
            "The game streams this texture, so your replacement will be stored in full \
             ({width}×{height}) instead. That's the same thing the existing character-skin \
             mods do."
        )
    });

    Ok(TextureTarget {
        name,
        asset_hash: hash,
        width,
        height,
        format: String::from_utf8_lossy(format.fourcc()).to_string(),
        swappable: true,
        reason,
    })
}

/// Build the override block for one texture swap.
pub fn swap_block(game_path: &str, swap: &TextureSwap) -> Result<PatchBlock, String> {
    let (_container, hash, width, height, format, _resident) = donor(game_path, &swap.name)?;

    // Decode the user's image and force it to the donor's exact dimensions. Resizing is not
    // a limitation we could avoid by trying harder — the body length is what keeps the
    // engine from over-reading, so it is fixed by definition.
    let img = image::open(&swap.image_path)
        .map_err(|e| format!("Couldn't read {}: {e}", swap.image_path))?;
    let rgba = image::DynamicImage::ImageRgba8(
        img.resize_exact(width, height, FilterType::Lanczos3).to_rgba8(),
    )
    .to_rgba8();

    // Re-encode into the donor's own format, generating the full mip chain the engine will
    // read (it reads the dimension-derived chain regardless of any header field).
    let mips = dxt_mip_count(width as usize, height as usize);
    let bc = match format {
        TexFormat::Bc1 => Format::Bc1, // DXT1
        TexFormat::Bc3 => Format::Bc3, // DXT5
    };

    let mut body = Vec::new();
    let mut level: image::RgbaImage = rgba;
    let (mut w, mut h) = (width, height);
    for _ in 0..mips {
        let mut buf = vec![0u8; bc.compressed_size(w as usize, h as usize)];
        bc.compress(
            level.as_raw(),
            w as usize,
            h as usize,
            texpresso::Params::default(),
            &mut buf,
        );
        body.extend_from_slice(&buf);

        w = (w / 2).max(1);
        h = (h / 2).max(1);
        level = image::imageops::resize(&level, w, h, FilterType::Lanczos3);
    }

    // Belt and braces: our encoder must agree with `texsize`, which is the size the engine
    // actually reads. `build_resident_texture` checks this too, but a mismatch here means
    // our mip loop drifted from the engine's chain — worth failing loudly and separately.
    let want = linear_mip_chain_size(width as usize, height as usize, format.fourcc(), mips);
    if body.len() != want {
        return Err(format!(
            "internal: encoded {} bytes but the engine expects a {want}-byte mip chain for \
             {width}x{height} {:?}",
            body.len(),
            format
        ));
    }

    // Publish a fully-resident container under the original's hash — the shape the shipped
    // character skins use, and the only one that works for a streamed donor.
    let container = build_resident_texture(&swap.name, width, height, format, &body)?;

    let aset = AsetEntry::new(hash, 0xFFFF_FFFF, 0x0000_FFFF, TYPE_ID_TEXTURE);
    PatchBlock::from_decompressed(
        &single_entry_block(hash, &container),
        format!("blocks\\modkit\\tex\\{}.block", swap.name),
        vec![aset],
        None,
    )
}

/// Wrap one UCFX container in the single-entry block table the loader expects:
/// `u32 count` + one 16-byte row + the container.
fn single_entry_block(hash: u32, container: &[u8]) -> Vec<u8> {
    const TEXTURE_TYPE_HASH: u32 = 0xF011_157A;
    let mut out = Vec::with_capacity(20 + container.len());
    out.extend_from_slice(&1u32.to_le_bytes()); // entry_count
    out.extend_from_slice(&hash.to_le_bytes()); // name_hash
    out.extend_from_slice(&TEXTURE_TYPE_HASH.to_le_bytes()); // type_hash
    out.extend_from_slice(&0u32.to_le_bytes()); // field_c
    out.extend_from_slice(&(container.len() as u32).to_le_bytes()); // chunk_size
    out.extend_from_slice(container);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The block table the loader reads must be exactly count + row + container.
    #[test]
    fn single_entry_block_layout() {
        let c = b"UCFX....payload";
        let b = single_entry_block(0xDEADBEEF, c);
        assert_eq!(u32::from_le_bytes(b[0..4].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(b[4..8].try_into().unwrap()), 0xDEADBEEF);
        assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()), 0xF011_157A);
        assert_eq!(
            u32::from_le_bytes(b[16..20].try_into().unwrap()) as usize,
            c.len()
        );
        assert_eq!(&b[20..], c);
    }

    /// The hand-rolled base64 must match the standard alphabet + padding, or every
    /// thumbnail silently fails to render in the webview.
    #[test]
    fn base64_matches_the_standard_encoding() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(b64(b"foob"), "Zm9vYg==");
        assert_eq!(b64(b"fooba"), "Zm9vYmE=");
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
        // A PNG signature, which is what we actually feed it.
        assert_eq!(b64(&[0x89, 0x50, 0x4E, 0x47]), "iVBORw==");
    }

    /// Export round-trips the preview PNG through base64, so the decoder must be the exact
    /// inverse of the encoder or the written file is corrupt.
    #[test]
    fn base64_round_trips() {
        for case in [
            &b""[..],
            b"f",
            b"fo",
            b"foo",
            b"foob",
            b"fooba",
            b"foobar",
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A], // a real PNG signature
        ] {
            assert_eq!(b64_decode(&b64(case)).as_deref(), Some(case), "round-trip {case:?}");
        }
    }

    /// A texture's inline body is either a mip *tail* (streamed) or *starts* at mip 0
    /// (a full chain, or a lone level as HUD icons ship). Assuming only the tail case made
    /// every `HUD_*` icon decode to nothing.
    #[test]
    fn decode_preview_handles_both_body_shapes() {
        let mk = |w: u32, h: u32, fmt: TexFormat, body_len: usize| TextureData {
            width: w,
            height: h,
            format: fmt,
            mip0: vec![],
            all_mips: vec![0x7Fu8; body_len],
            mip_count: 1,
        };

        // A HUD icon: 64x64 DXT5, ONE level, body = 4096. Was returning None.
        let hud = mk(64, 64, TexFormat::Bc3, 4096);
        let p = decode_preview("HUD_HQ_AN", &hud, 128).expect("a single-mip body must decode");
        assert_eq!((p.preview_width, p.preview_height), (64, 64), "at full size");

        // A fully-resident skin: the whole chain is inline, so it also starts at mip 0.
        let mips = dxt_mip_count(512, 512);
        let full = linear_mip_chain_size(512, 512, b"DXT1", mips);
        let skin = mk(512, 512, TexFormat::Bc1, full);
        let p = decode_preview("skin", &skin, 512).expect("full chain");
        assert_eq!((p.preview_width, p.preview_height), (512, 512));

        // A streamed skin: body too small for mip 0, so it is the tail. 512x512 DXT1 keeping
        // mips 3.. = 64x64 down.
        let tail: usize = (3..mips)
            .map(|i| {
                let (w, h) = ((512usize >> i).max(1), (512usize >> i).max(1));
                w.div_ceil(4) * h.div_ceil(4) * 8
            })
            .sum();
        let streamed = mk(512, 512, TexFormat::Bc1, tail);
        let p = decode_preview("streamed", &streamed, 512).expect("resident tail");
        assert_eq!(
            (p.preview_width, p.preview_height),
            (64, 64),
            "decode the largest level actually present, not a made-up 512"
        );
        assert_eq!((p.width, p.height), (512, 512), "but still report the real size");
    }

    #[test]
    fn names_are_classified_and_grouped() {
        assert_eq!(classify("pmc_hum_chris_ub_nm"), "normal");
        assert_eq!(classify("pmc_hum_chris_ub_sm"), "specular");
        assert_eq!(classify("us_veh_abrams_dm"), "diffuse");
        assert_eq!(classify("al_hum_boss_ub"), "other");
        assert_eq!(category("pmc_hum_chris_ub"), "pmc");
    }

    /// Our mip-chain encoder must agree with `texsize`, which is the size the engine reads.
    /// If these ever diverge the result is a world-load livelock, so pin it.
    #[test]
    fn encoded_chain_matches_the_engines_expected_size() {
        for (w, h, fmt, bc) in [
            (256u32, 256u32, b"DXT1", Format::Bc1),
            (512, 512, b"DXT5", Format::Bc3),
            (1024, 1024, b"DXT1", Format::Bc1),
        ] {
            let mips = dxt_mip_count(w as usize, h as usize);
            let mut total = 0usize;
            let (mut lw, mut lh) = (w, h);
            for _ in 0..mips {
                total += bc.compressed_size(lw as usize, lh as usize);
                lw = (lw / 2).max(1);
                lh = (lh / 2).max(1);
            }
            let want = linear_mip_chain_size(w as usize, h as usize, fmt, mips);
            assert_eq!(total, want, "{w}x{h} {}", String::from_utf8_lossy(fmt));
        }
    }
}
