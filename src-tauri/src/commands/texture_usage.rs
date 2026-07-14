//! "Where is this texture actually used?"
//!
//! # There is no such index in the game
//!
//! The WAD maps *assets to blocks*. It has nothing that answers "which models use texture
//! X" — that relation only exists implicitly, inside each model's `MTRL` chunk, which lists
//! the texture hashes its materials sample. So to answer the question we invert it: walk
//! every model, read its MTRL slots, and build `texture -> [models]`.
//!
//! # Two things that are easy to get wrong
//!
//! **Walk every model ASET row, not just the primary ones.** Most models are shared or
//! aliased and exist only as *sub-entries* of another asset's block. Indexing primaries
//! alone finds 1,771 models and covers just **15.9%** of textures — it reports that nothing
//! uses `al_veh_tank_m1a1_dm`, which is plainly false. Including sub-entry rows finds
//! **3,007** models and lifts coverage to **88.8%**.
//!
//! **Cache it.** The scan decompresses every model container: ~10s in release, minutes in a
//! debug build. Doing that per click would be unusable, so it is built once and cached,
//! keyed by the *content* of the WAD's ASET table (see [`aset_fingerprint`]) rather than by
//! size/mtime — a different install (DLC, a patch) must not silently reuse a stale index.
//!
//! # Why this is not a vector database
//!
//! This is an exact, finite relation — a join, not a similarity search. A `HashMap` answers
//! it precisely and instantly. An embedding index would add fuzziness exactly where
//! certainty is wanted. (Semantic search — "textures that *look* rusty" — is a genuinely
//! different feature and would need image embeddings, not this.)

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use mercs2_formats::crc32::crc32_mercs2;
use mercs2_formats::ffcs::{load_ffcs_archive, FfcsArchive};
use mercs2_formats::hash::pandemic_hash_m2;
use mercs2_formats::texture::{extract_model, parse_mtrl};
use mercs2_formats::types::TYPE_ID_MODEL;
use mercs2_mesh::build_indexed_all;
use serde::{Deserialize, Serialize};

use crate::commands::paths::app_data_dir;

/// `texture hash -> model hashes`, split by how strongly the model uses it.
///
/// # Painted vs merely declared — the distinction that makes the 3D view honest
///
/// A model's `MTRL` table lists materials; a material lists textures. But a material only
/// *paints* something if some `PRMT` draw group binds it. Those are very different sets: of
/// 5,398 (model, texture) pairs measured against retail, only ~41% are bound by a draw group
/// in the model's own container. The rest are declared-but-unpainted, for real reasons:
///
/// * the geometry using them is the **wreck** variant (`al_veh_tank_m1a2` declares
///   `global_veh_tank_ruin_dm`),
/// * it belongs to a **separate sub-model** (that same tank declares
///   `ch_veh_tank_ztz98_tracks_dm` — the tracks are their own model),
/// * or the container is a **low-detail variant** that merges its groups
///   (`vz_hum_deathsquad_a` has one LOD tier and 4 draw groups, but 17 declared textures).
///
/// If we only had one list we'd have to choose between lying about coverage and lying about
/// what the 3D view can show. So we keep both: `painted` drives the viewer (a hit there is
/// *guaranteed* to highlight), `declared` keeps the honest superset, and the UI explains the
/// difference instead of shrugging with "not visible".
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageIndex {
    /// Identifies the WAD this was built from; a mismatch invalidates the cache.
    pub fingerprint: u32,
    /// Models with a draw group that actually binds this texture. Decimal-string keys
    /// because JSON has no integer keys.
    pub painted: HashMap<String, Vec<u32>>,
    /// Models whose MTRL table references it at all (a superset of `painted`).
    pub declared: HashMap<String, Vec<u32>>,
}

impl UsageIndex {
    /// Models that actually paint this texture on a surface — the ones the 3D view can show.
    pub fn painted_by(&self, texture: u32) -> &[u32] {
        self.painted
            .get(&texture.to_string())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Models that reference it in a material but never bind it to a visible group.
    pub fn declared_only_by(&self, texture: u32) -> Vec<u32> {
        let painted: HashSet<u32> = self.painted_by(texture).iter().copied().collect();
        self.declared
            .get(&texture.to_string())
            .map(|v| v.iter().copied().filter(|m| !painted.contains(m)).collect())
            .unwrap_or_default()
    }
}

/// A content key for the WAD's asset table.
///
/// Not the file's size or mtime: those can collide across builds and change without the
/// contents changing. The ASET table *is* what we index, so a checksum over it is the
/// honest key — and it costs nothing, since the table is already parsed.
fn aset_fingerprint(archive: &FfcsArchive) -> u32 {
    let mut buf = Vec::with_capacity(archive.aset.len() * 8);
    for e in &archive.aset {
        buf.extend_from_slice(&e.asset_hash.to_le_bytes());
        buf.extend_from_slice(&e.type_id.to_le_bytes());
    }
    crc32_mercs2(&buf)
}

fn cache_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("texture-usage.json"))
}

/// Build the index by scanning every model's MTRL slots.
fn build(f: &mut std::fs::File, archive: &FfcsArchive) -> UsageIndex {
    // Every distinct model hash, primary AND sub-entry (see module note).
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

    let mut painted: HashMap<String, Vec<u32>> = HashMap::new();
    let mut declared: HashMap<String, Vec<u32>> = HashMap::new();

    for m in models {
        let Ok(container) = extract_model(f, archive, m) else {
            continue;
        };

        // Declared: every texture any material mentions.
        let mut seen = HashSet::new();
        for mat in parse_mtrl(&container) {
            for tex in mat.textures {
                if tex != 0 && seen.insert(tex) {
                    declared.entry(tex.to_string()).or_default().push(m);
                }
            }
        }

        // Painted: every texture a draw group actually binds, across EVERY state/LOD tier —
        // a texture that only appears on a distant LOD or a damaged variant still counts as
        // painted, it just needs the right tier selected in the viewer.
        let mut bound = HashSet::new();
        if let Ok((_, _, groups, _)) = build_indexed_all(&container) {
            for d in groups {
                // EVERY slot of the group's material, not just diffuse/specular/normal — a
                // material binds up to 10 textures, and consulting only the first three
                // mis-reports more than half of them as "used by nothing".
                for tex in d.textures {
                    if tex != 0 && bound.insert(tex) {
                        painted.entry(tex.to_string()).or_default().push(m);
                    }
                }
            }
        }
    }

    UsageIndex {
        fingerprint: aset_fingerprint(archive),
        painted,
        declared,
    }
}

/// Load the cached index, or build and cache it.
///
/// The first call on a given install takes ~10s (release). Every call after is a file read.
pub fn load_or_build(wad: &Path) -> Result<UsageIndex, String> {
    let mut f = std::fs::File::open(wad).map_err(|e| format!("open vz.wad: {e}"))?;
    let size = f.metadata().map_err(|e| format!("stat: {e}"))?.len();
    let archive = load_ffcs_archive(&mut f, size).map_err(|e| format!("FFCS: {e}"))?;
    let want = aset_fingerprint(&archive);

    if let Ok(bytes) = std::fs::read(cache_path()?) {
        if let Ok(idx) = serde_json::from_slice::<UsageIndex>(&bytes) {
            if idx.fingerprint == want {
                return Ok(idx);
            }
            // Different WAD (DLC installed, patch applied) — rebuild rather than lie.
        }
    }

    let idx = build(&mut f, &archive);
    if let Ok(p) = cache_path() {
        if let Ok(json) = serde_json::to_vec(&idx) {
            let _ = std::fs::write(p, json); // a cache miss is not worth failing the request
        }
    }
    Ok(idx)
}

/// A model that samples a texture.
#[derive(Debug, Clone, Serialize)]
pub struct ModelRef {
    pub hash: u32,
    /// The model's name if we can recover it, else `None` (the WAD stores only hashes).
    pub name: Option<String>,
    /// What to pass back to `model_geometry` / `model_variants`: the name when we have one,
    /// otherwise `0x…`.
    ///
    /// Models are addressed **by hash** in the WAD, so an unnamed model is still perfectly
    /// loadable and renderable — only its label is missing. Handing the caller a ready-made
    /// reference keeps that fact from leaking into every consumer.
    pub reference: String,
}

/// Resolve model hashes to names using the bundled name table.
pub fn name_models(hashes: &[u32], names: &HashMap<u32, &str>) -> Vec<ModelRef> {
    let mut out: Vec<ModelRef> = hashes
        .iter()
        .map(|&h| {
            let name = names.get(&h).map(|s| s.to_string());
            ModelRef {
                hash: h,
                reference: name.clone().unwrap_or_else(|| format!("0x{h:08X}")),
                name,
            }
        })
        .collect();
    // Named first, then alphabetical — an unnamed hash is the least useful row.
    out.sort_by(|a, b| match (&a.name, &b.name) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.hash.cmp(&b.hash),
    });
    out
}

/// Build `hash -> name` for every name we know.
pub fn name_table(list: &str) -> HashMap<u32, &str> {
    let mut m = HashMap::new();
    for n in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
        m.insert(pandemic_hash_m2(n), n);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_models_sort_before_unnamed() {
        let names: HashMap<u32, &str> = [(1u32, "zebra"), (2, "alpha")].into_iter().collect();
        let out = name_models(&[3, 1, 2], &names);
        assert_eq!(out[0].name.as_deref(), Some("alpha"));
        assert_eq!(out[1].name.as_deref(), Some("zebra"));
        assert_eq!(out[2].name, None, "unnamed hashes go last");
    }

    #[test]
    fn painted_by_is_empty_for_an_unknown_texture() {
        let idx = UsageIndex::default();
        assert!(idx.painted_by(0xDEAD).is_empty());
        assert!(idx.declared_only_by(0xDEAD).is_empty());
    }

    /// `declared_only_by` must EXCLUDE models that actually paint the texture — otherwise the
    /// UI would list the same model under both "used by" and "referenced but not painted".
    #[test]
    fn declared_only_excludes_models_that_paint_it() {
        let mut idx = UsageIndex::default();
        idx.painted.insert("7".into(), vec![100]);
        idx.declared.insert("7".into(), vec![100, 200]);

        assert_eq!(idx.painted_by(7), &[100]);
        assert_eq!(idx.declared_only_by(7), vec![200]);
    }

    #[test]
    fn name_table_maps_names_to_their_hashes() {
        let t = name_table("pmc_hum_chris\nal_hum_boss\n");
        assert_eq!(t.get(&pandemic_hash_m2("pmc_hum_chris")).copied(), Some("pmc_hum_chris"));
    }
}
