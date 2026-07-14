//! Claim groups — the unit of conflict resolution.
//!
//! # Why not resolve per asset hash?
//!
//! The obvious design is "each asset hash is won by the last mod that claims it." It is
//! wrong, and the way it is wrong is silent.
//!
//! A real mod is not one asset. A vehicle reskin is a model **plus** its six textures
//! **plus** a script that makes the garage spawn it — see `docs/asset_injection_playbook.md`
//! §5.1: *"a patch WAD's BLOCK SET is the contract, not its block count."* Resolve those
//! hash-by-hash and two overlapping vehicle reskins produce a **chimera**: mod A's model
//! wearing mod B's textures. The WAD validates. The game loads. It just looks wrong, and
//! no error was ever raised.
//!
//! So the unit of resolution is the [`ClaimGroup`]: everything one recipe op emits, won or
//! lost **together**. That single change also makes the UI speakable — "Cool Skins'
//! *Abrams Reskin* (7 assets) overrides Desert Pack's *Desert Abrams* (7 assets)" is a
//! sentence a user can act on; a list of bare hashes is not.
//!
//! # Load order is LAST-wins
//!
//! The engine mounts `vz.wad` then `vz-patch.wad` and resolves an asset by taking the
//! **last** match (`docs/comprehensive_engine_understanding.md` §3.2). Every mod manager
//! users have ever touched (MO2, Vortex) is also last-wins. So: later in the list beats
//! earlier. Do not describe this as "priority" in the UI — say *"later mods override
//! earlier ones."*

use serde::{Deserialize, Serialize};

/// One indivisible set of asset claims emitted by a single recipe op.
#[derive(Debug, Clone)]
pub struct ClaimGroup {
    /// Mod that owns this group.
    pub mod_id: String,
    /// Mod's display name (for conflict messages).
    pub mod_name: String,
    /// Human label for the op itself, e.g. `"Abrams Reskin"`.
    pub label: String,
    /// When true (the default), this group is all-or-nothing: if anything it claims is
    /// overridden, the whole group loses. Set false only for a deliberate
    /// layer-on-top tweak that expects to override part of a larger mod.
    pub atomic: bool,
    /// The blocks this op contributes to the WAD. The claimed asset hashes are derived
    /// from these blocks' primary ASET rows, so the two can never drift apart.
    pub blocks: Vec<mercs2_formats::patch_wad::PatchBlock>,
}

impl ClaimGroup {
    /// The asset hashes this group claims: the **primary** ASET rows of its blocks.
    ///
    /// Sub-entry rows (low 16 bits != 0xFFFF) are not claims — they are internal
    /// references within a block and legitimately repeat across blocks.
    pub fn claimed_hashes(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .blocks
            .iter()
            .flat_map(|b| &b.aset_entries)
            .filter(|e| e.u32_2 & 0xFFFF == 0xFFFF)
            .map(|e| e.asset_hash)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// What happened to one group during resolution — surfaced to the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum GroupOutcome {
    /// Every hash it claims made it into the WAD.
    Applied {
        mod_id: String,
        label: String,
        asset_count: usize,
    },
    /// Fully overridden by a later mod. Clean, expected, not an error.
    Overridden {
        mod_id: String,
        label: String,
        asset_count: usize,
        /// The later mod that won.
        overridden_by_mod: String,
        overridden_by_label: String,
    },
    /// A non-atomic group layered on top: some of its hashes were taken, the rest applied.
    PartiallyApplied {
        mod_id: String,
        label: String,
        applied: usize,
        overridden: usize,
    },
}

/// Resolution failed — the user must change the load order or remove a mod.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimConflict {
    pub mod_id: String,
    pub label: String,
    pub other_mod_id: String,
    pub other_label: String,
    /// Hashes both groups claim.
    pub shared: Vec<u32>,
    /// Hashes only the losing group claims — the ones that would be silently dropped.
    pub only_mine: Vec<u32>,
    pub message: String,
}

/// The outcome of resolving a whole load order.
#[derive(Debug, Default)]
pub struct Resolution {
    /// Blocks to assemble, in load order.
    pub blocks: Vec<mercs2_formats::patch_wad::PatchBlock>,
    /// Per-group report, for the UI.
    pub outcomes: Vec<GroupOutcome>,
    /// Unresolvable partial overlaps. Non-empty => refuse to build.
    pub conflicts: Vec<ClaimConflict>,
}

/// Resolve a load order into a coherent block list.
///
/// `groups` is in **load order** — index 0 loads first, the last index loads last and
/// therefore **wins** ties.
///
/// The rules, applied by walking from the highest-priority group down:
///
/// | Overlap with an already-won group | Result |
/// |---|---|
/// | none | applied |
/// | this group is *entirely* overridden | dropped, reported as `Overridden` (clean) |
/// | partial, and every taker is non-atomic | `PartiallyApplied` (deliberate layering) |
/// | partial, and some taker is atomic | **`ClaimConflict`** — refuse to build |
///
/// That last row is the important one. A proper partial overlap has no correct automatic
/// answer: last-wins-per-hash ships an incoherent WAD, and all-or-nothing silently drops
/// assets the user asked for. Both are wrong, so we make the user choose.
pub fn resolve(groups: &[ClaimGroup]) -> Resolution {
    let mut out = Resolution::default();

    // hash -> index of the group that won it (highest priority seen so far).
    let mut winner: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    // Groups we keep, highest-priority first; reversed back into load order at the end.
    let mut kept: Vec<(usize, Vec<u32>)> = Vec::new();

    for (gi, g) in groups.iter().enumerate().rev() {
        let mine = g.claimed_hashes();
        if mine.is_empty() {
            // An op that emits no by-hash claims (e.g. a pure Lua edit) always applies.
            out.outcomes.push(GroupOutcome::Applied {
                mod_id: g.mod_id.clone(),
                label: g.label.clone(),
                asset_count: 0,
            });
            kept.push((gi, mine));
            continue;
        }

        let taken: Vec<u32> = mine.iter().copied().filter(|h| winner.contains_key(h)).collect();

        if taken.is_empty() {
            for &h in &mine {
                winner.insert(h, gi);
            }
            out.outcomes.push(GroupOutcome::Applied {
                mod_id: g.mod_id.clone(),
                label: g.label.clone(),
                asset_count: mine.len(),
            });
            kept.push((gi, mine));
            continue;
        }

        // Fully shadowed by later mods: a clean override. Drop it, say so, move on.
        if taken.len() == mine.len() {
            let by = winner[&taken[0]];
            out.outcomes.push(GroupOutcome::Overridden {
                mod_id: g.mod_id.clone(),
                label: g.label.clone(),
                asset_count: mine.len(),
                overridden_by_mod: groups[by].mod_id.clone(),
                overridden_by_label: groups[by].label.clone(),
            });
            continue;
        }

        // Partial overlap. Legal only if every group that took a hash from us opted into
        // layering (atomic = false). Otherwise there is no right answer — surface it.
        let atomic_taker = taken
            .iter()
            .map(|h| winner[h])
            .find(|&wi| groups[wi].atomic);

        if let Some(wi) = atomic_taker {
            let only_mine: Vec<u32> =
                mine.iter().copied().filter(|h| !winner.contains_key(h)).collect();
            out.conflicts.push(ClaimConflict {
                mod_id: g.mod_id.clone(),
                label: g.label.clone(),
                other_mod_id: groups[wi].mod_id.clone(),
                other_label: groups[wi].label.clone(),
                shared: taken.clone(),
                only_mine: only_mine.clone(),
                message: format!(
                    "\"{}\" ({}) and \"{}\" ({}) overlap on {} asset(s) but neither contains the \
                     other — {} would keep {} of its own asset(s) while losing {} to the later mod, \
                     which produces a half-applied mod. Remove one of them, or reorder so that one \
                     fully replaces the other.",
                    g.label,
                    g.mod_name,
                    groups[wi].label,
                    groups[wi].mod_name,
                    taken.len(),
                    g.label,
                    only_mine.len(),
                    taken.len(),
                ),
            });
            continue;
        }

        // Every taker was non-atomic: keep the parts nobody else claimed.
        let survives: Vec<u32> =
            mine.iter().copied().filter(|h| !winner.contains_key(h)).collect();
        for &h in &survives {
            winner.insert(h, gi);
        }
        out.outcomes.push(GroupOutcome::PartiallyApplied {
            mod_id: g.mod_id.clone(),
            label: g.label.clone(),
            applied: survives.len(),
            overridden: taken.len(),
        });
        kept.push((gi, survives));
    }

    if !out.conflicts.is_empty() {
        return out; // caller refuses to build
    }

    // Emit blocks in load order. Within a kept group, drop any block whose primary claims
    // were all taken by a later mod (only reachable via non-atomic layering).
    kept.reverse();
    for (gi, survives) in kept {
        for blk in &groups[gi].blocks {
            let primaries: Vec<u32> = blk
                .aset_entries
                .iter()
                .filter(|e| e.u32_2 & 0xFFFF == 0xFFFF)
                .map(|e| e.asset_hash)
                .collect();
            // A block with no primary rows (or one still claiming a surviving hash) ships.
            if primaries.is_empty() || primaries.iter().any(|h| survives.contains(h)) {
                out.blocks.push(blk.clone());
            }
        }
    }

    out.outcomes.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mercs2_formats::patch_wad::{AsetEntry, PatchBlock};

    fn group(mod_id: &str, label: &str, atomic: bool, hashes: &[u32]) -> ClaimGroup {
        let blocks = hashes
            .iter()
            .map(|&h| {
                PatchBlock::from_decompressed(
                    format!("payload for {h:08X}").as_bytes(),
                    format!("blocks\\{mod_id}\\{h:08x}.block"),
                    vec![AsetEntry::new(h, 0xFFFF_FFFF, 0x0000_FFFF, 19)],
                    None,
                )
                .unwrap()
            })
            .collect();
        ClaimGroup {
            mod_id: mod_id.into(),
            mod_name: mod_id.into(),
            label: label.into(),
            atomic,
            blocks,
        }
    }

    /// The headline semantic: the mod loaded LAST wins.
    #[test]
    fn identical_claims_last_mod_in_load_order_wins() {
        let a = group("early", "Abrams Reskin", true, &[1, 2, 3]);
        let b = group("late", "Desert Abrams", true, &[1, 2, 3]);
        let r = resolve(&[a, b]);

        assert!(r.conflicts.is_empty());
        assert_eq!(r.blocks.len(), 3, "only the winner's blocks ship");
        for blk in &r.blocks {
            assert!(blk.path_string.contains("late"), "the LAST mod won");
        }
        assert!(matches!(
            r.outcomes[0],
            GroupOutcome::Overridden { .. },
            ),
            "the earlier mod is reported as cleanly overridden"
        );
    }

    /// Disjoint mods all apply — no conflict, no drops.
    #[test]
    fn disjoint_groups_all_apply() {
        let r = resolve(&[
            group("a", "Skins", true, &[1, 2]),
            group("b", "Sounds", true, &[3, 4]),
        ]);
        assert!(r.conflicts.is_empty());
        assert_eq!(r.blocks.len(), 4);
    }

    /// The chimera case. A ships {model, tex}, B ships {tex, script}. Per-hash last-wins
    /// would give A's model + B's tex — a mod nobody authored. We refuse instead.
    #[test]
    fn proper_partial_overlap_is_a_hard_conflict_not_a_chimera() {
        let r = resolve(&[
            group("a", "Tank Reskin", true, &[10, 20]),
            group("b", "Tank Retexture", true, &[20, 30]),
        ]);
        assert_eq!(r.conflicts.len(), 1, "must refuse, not silently blend");
        let c = &r.conflicts[0];
        assert_eq!(c.shared, vec![20]);
        assert_eq!(c.only_mine, vec![10], "the asset that would be silently dropped");
        assert!(c.message.contains("half-applied"));
    }

    /// A deliberate tweak-on-top (atomic = false) may layer over a bigger mod.
    #[test]
    fn non_atomic_group_may_layer_over_a_larger_mod() {
        let base = group("base", "Vehicle Pack", true, &[1, 2, 3]);
        let mut tweak = group("tweak", "Just The Decal", true, &[2]);
        tweak.atomic = false;

        let r = resolve(&[base, tweak]);
        assert!(r.conflicts.is_empty(), "layering is allowed: {:?}", r.conflicts);
        assert_eq!(r.blocks.len(), 3, "1+2 from base minus the one the tweak took, plus the tweak");

        // Hash 2 must come from the tweak; 1 and 3 from the base.
        let from_tweak = r.blocks.iter().filter(|b| b.path_string.contains("tweak")).count();
        let from_base = r.blocks.iter().filter(|b| b.path_string.contains("base")).count();
        assert_eq!((from_base, from_tweak), (2, 1));
    }

    /// Whatever we emit must satisfy the WAD writer's own invariant: one primary row per
    /// hash. This is the property that guarantees the engine's winner is never undefined.
    #[test]
    fn resolved_blocks_always_satisfy_the_one_primary_row_per_hash_invariant() {
        let r = resolve(&[
            group("a", "A", true, &[1, 2, 3]),
            group("b", "B", true, &[3, 4]), // overlaps on 3
        ]);
        // {1,2,3} vs {3,4} is a proper partial overlap => conflict, nothing built.
        assert_eq!(r.conflicts.len(), 1);

        // The clean case does build, and validates.
        let r = resolve(&[
            group("a", "A", true, &[1, 2]),
            group("b", "B", true, &[1, 2]),
        ]);
        assert!(r.conflicts.is_empty());
        mercs2_formats::patch_wad::validate_blocks(&r.blocks)
            .expect("resolved output must never contain two primary rows for one hash");
    }
}
