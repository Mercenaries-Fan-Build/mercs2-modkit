//! Which pmc_bb build belongs on this install, and why.
//!
//! # The DLL is six DLLs
//!
//! `pmc-blackbox` publishes one subset of three independent features per
//! asset. There is **no** `pmc_bb.dll` asset and no required install name — the
//! `.def` carries no `LIBRARY` line, so `ld` stamps each output's own filename into
//! its export directory and every build self-describes.
//!
//! ```text
//!   crack   SecuROM v7 event spoof
//!   asi     the ASI loader, four search paths
//!   log     console, pmc_blackbox.log, pmc_log, crash handler, Lua hooks
//!
//!   pmc_bb_fully_loaded.dll   crack asi log
//!   pmc_bb_crack_only.dll     crack
//!   pmc_bb_crack_asi.dll      crack asi
//!   pmc_bb_crack_log.dll      crack     log
//!   pmc_bb_asi_log.dll              asi log
//!   pmc_bb_log_only.dll                 log
//! ```
//!
//! asi-only is deliberately unpublished: other loaders already do that job, and a
//! build offering nothing else has no reason to be chosen over them.
//!
//! # Selection is by feature, never by filename
//!
//! modkit used to ask for two literal names, `pmc_bb.dll` and `pmc_bb_log.dll`.
//! Neither exists any more. The repair that looks obvious is worse than the break:
//! `pmc_bb_log.dll` reads as if it became `pmc_bb_log_only.dll`, but the old build
//! was *asi + log* and `log_only` has the loader compiled **out** — so a
//! name-for-name swap installs a loader that loads nothing, reports success, and
//! fails silently at the only moment that matters. [`resolve`] states the features
//! it needs and lets [`Variant::for_features`] name the asset.
//!
//! # Ownership is decided here
//!
//! pmc-blackbox used to scan for competing ASI loaders and stand down. It stopped:
//! the canonical proxy-DLL name set it carried as literal strings is the signature
//! of a hijack toolkit, and Defender began flagging the build. That commit hands
//! the job over explicitly — modkit owns the install directory and can settle it at
//! install time, where a wrong answer is a different variant rather than a guess
//! from inside the process. A build with the loader compiled out has no second
//! loader to coordinate with, because the code is absent rather than idle.

use serde::{Deserialize, Serialize};

/// GitHub repo publishing the variants. Note the name has no `mercs2-` prefix,
/// unlike its siblings and unlike the usual local checkout directory.
pub const REPO: &str = "Mercenaries-Fan-Build/pmc-blackbox";

/// What modkit installs every variant as.
///
/// The artifact requires no particular name, but modkit's two loading routes both
/// do: `apply_crack` rewrites the exe's import table to name `pmc_bb.dll`, and the
/// dxwrapper config side-loads `LoadCustomDllPath = pmc_bb.dll`. So the install
/// name is modkit's decision — which is exactly why the ledger records which asset
/// is underneath it.
pub const INSTALL_NAME: &str = "pmc_bb.dll";

/// Last-resort floor for a build, used only when the release declares no size.
///
/// Deliberately far below the smallest published variant. The first value here
/// was 16 KB, picked without looking: `pmc_bb_crack_only.dll` is 13,838 bytes and
/// `pmc_bb_crack_asi.dll` is 15,886, so that floor rejected two of the six real
/// builds as "the size of an error page". A guess about how big an artifact ought
/// to be is worth less than the exact size the release publishes — see
/// [`super::place::PlaceOpts::expect_size`], which is what actually guards this
/// now. Any change here should be checked against the release page, not reasoned
/// about.
pub const MIN_SIZE: u64 = 4 * 1024;

/// Size of the smallest build the release publishes (`pmc_bb_crack_only.dll`,
/// v0.6.0). The floor has to clear it, because the floor applies to whichever
/// variant an *override* selects, not only to the one auto-resolution picks.
const SMALLEST_PUBLISHED_BUILD: u64 = 13_838;

// Fails the build rather than a test: raising MIN_SIZE past a real artifact is a
// mistake that only shows up when somebody picks that variant by hand, which is
// exactly the path least likely to be exercised before release.
const _: () = assert!(
    MIN_SIZE < SMALLEST_PUBLISHED_BUILD,
    "MIN_SIZE rejects the smallest published pmc_bb build — check the release \
     page before raising it",
);

/// The three independent features a build may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Features {
    /// SecuROM v7 event spoof.
    pub crack: bool,
    /// The ASI loader.
    pub asi: bool,
    /// Log stack, crash handler, Lua hooks.
    pub log: bool,
}

impl Features {
    pub fn names(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.crack {
            out.push("crack".to_string());
        }
        if self.asi {
            out.push("asi".to_string());
        }
        if self.log {
            out.push("log".to_string());
        }
        out
    }
}

/// One published build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Variant {
    /// Release asset name, which is also the name stamped in its export directory.
    pub asset: &'static str,
    pub features: Features,
    /// One line for the advanced picker.
    pub blurb: &'static str,
}

const fn f(crack: bool, asi: bool, log: bool) -> Features {
    Features { crack, asi, log }
}

/// Every variant the release publishes, in the order the Makefile lists them.
pub const VARIANTS: &[Variant] = &[
    Variant {
        asset: "pmc_bb_fully_loaded.dll",
        features: f(true, true, true),
        blurb: "Everything: SecuROM spoof, ASI loader, and the full log stack.",
    },
    Variant {
        asset: "pmc_bb_crack_only.dll",
        features: f(true, false, false),
        blurb: "SecuROM spoof alone — no loader, no logging.",
    },
    Variant {
        asset: "pmc_bb_crack_asi.dll",
        features: f(true, true, false),
        blurb: "SecuROM spoof and the ASI loader, without the log stack.",
    },
    Variant {
        asset: "pmc_bb_crack_log.dll",
        features: f(true, false, true),
        blurb: "SecuROM spoof and logging, with the loader left to something else.",
    },
    Variant {
        asset: "pmc_bb_asi_log.dll",
        features: f(false, true, true),
        blurb: "ASI loader and logging, with no SecuROM spoof.",
    },
    Variant {
        asset: "pmc_bb_log_only.dll",
        features: f(false, false, true),
        blurb: "Logging and crash reports only — another loader owns the plugins.",
    },
];

impl Variant {
    /// The published build carrying exactly `features`.
    ///
    /// Exact, not nearest. A superset would hand back a build with the SecuROM
    /// spoof to an install that must not fire it, or a second ASI loader to an
    /// install that already has one; a subset would drop the loader. There is no
    /// safe direction to round in, so an unpublished combination is `None` and the
    /// caller has to say so.
    pub fn for_features(features: Features) -> Option<&'static Variant> {
        VARIANTS.iter().find(|v| v.features == features)
    }

    pub fn by_asset(asset: &str) -> Option<&'static Variant> {
        VARIANTS
            .iter()
            .find(|v| v.asset.eq_ignore_ascii_case(asset))
    }
}

/// What modkit knows about the exe it would launch, as far as this choice cares.
///
/// Deliberately not the UI's three-way setup path. That is a description of which
/// screen the user is on; this is a fact about the binary, and the two disagree —
/// `classify(size)` calls the DRM-free rebuild "cracked" because it lands at
/// exactly the cracked size, which is the misidentification that made modkit offer
/// a spoof build to an exe that imports no sidecar at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExeKind {
    /// Cracked, with `pmc_bb.dll` in its import table — the `apply_crack` output.
    /// The crack is split across the exe patch and the DLL, so this build is the
    /// one that has to answer the SecuROM event.
    CrackedImportingPmcBb,
    /// Cracked by something else, importing that tool's own sidecar (`cruise.dll`).
    /// It brings its own SecuROM handling and its own loader; pmc_bb must not fire
    /// a second event or run a second loader.
    CrackedImportingOther,
    /// Not cracked: stock SecuROM satisfied by a real activation, or a DRM-free
    /// build that imports no sidecar. Either way nothing here wants a spoof.
    NotCracked,
    /// No catalogue match. Treated as [`Self::NotCracked`] for feature purposes —
    /// the conservative direction, since installing a spoof where none is wanted
    /// is the harmful mistake.
    Unknown,
}

impl ExeKind {
    /// Read the exe kind from a catalogue id assigned by
    /// [`crate::commands::verify`], the hash-based identification.
    pub fn from_exe_id(id: Option<&str>) -> Self {
        match id {
            Some("v11-cracked-pmcbb") => Self::CrackedImportingPmcBb,
            Some("v11-cracked-cruise") => Self::CrackedImportingOther,
            Some("v10-ea-signed" | "v10-unsigned" | "v11-patched-securom" | "v11-patched-drmfree") => {
                Self::NotCracked
            }
            _ => Self::Unknown,
        }
    }
}

/// The chosen build, and the reasoning, so the UI never has to restate it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    pub asset: String,
    pub features: Features,
    /// Why this build, in the user's terms.
    pub reason: String,
    /// True when the user forced it rather than modkit choosing.
    pub overridden: bool,
}

/// Pick the build for `kind`, or honour an explicit `override_asset`.
///
/// | exe | crack | asi | log | build |
/// |---|---|---|---|---|
/// | cracked, imports `pmc_bb.dll` | Y | Y | Y | `pmc_bb_fully_loaded.dll` |
/// | cracked, imports another sidecar | – | – | Y | `pmc_bb_log_only.dll` |
/// | not cracked | – | – | Y | `pmc_bb_log_only.dll` |
///
/// `log` is always on: `pmc_blackbox.log` is what the log analyzer, the debug
/// bundle and every crash report modkit collects are built on. A build without it
/// leaves modkit unable to answer any question a user brings.
pub fn resolve(kind: ExeKind, override_asset: Option<&str>) -> Result<Choice, String> {
    if let Some(asset) = override_asset.filter(|a| !a.trim().is_empty()) {
        let v = Variant::by_asset(asset).ok_or_else(|| {
            format!(
                "'{asset}' is not a build this release publishes. Known builds: {}",
                VARIANTS
                    .iter()
                    .map(|v| v.asset)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        return Ok(Choice {
            asset: v.asset.to_string(),
            features: v.features,
            reason: format!("Chosen manually: {}", v.blurb),
            overridden: true,
        });
    }

    let (features, reason) = match kind {
        ExeKind::CrackedImportingPmcBb => (
            f(true, true, true),
            "Your exe is cracked and imports pmc_bb.dll, so this build supplies the \
             SecuROM response the crack relies on, loads your plugins, and logs."
                .to_string(),
        ),
        ExeKind::CrackedImportingOther => (
            f(false, false, true),
            "Your exe is cracked by another tool and imports its sidecar, which already \
             answers SecuROM and loads plugins. This build only logs, so nothing is done twice."
                .to_string(),
        ),
        ExeKind::NotCracked => (
            f(false, false, true),
            "Your exe is not cracked, so no SecuROM spoof is installed. Plugin loading is \
             left to dxwrapper, and this build handles logging and crash reports."
                .to_string(),
        ),
        ExeKind::Unknown => (
            f(false, false, true),
            "modkit does not recognise this exe build, so it installs the build that changes \
             nothing about how the game starts — logging only."
                .to_string(),
        ),
    };

    let v = Variant::for_features(features).ok_or_else(|| {
        format!(
            "No published pmc_bb build carries exactly {:?} — the release matrix has changed.",
            features.names()
        )
    })?;

    Ok(Choice {
        asset: v.asset.to_string(),
        features: v.features,
        reason,
        overridden: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::verify::EXE_IDS;

    #[test]
    fn every_published_variant_is_uniquely_addressable() {
        for v in VARIANTS {
            assert_eq!(
                Variant::for_features(v.features).map(|x| x.asset),
                Some(v.asset),
                "{} is not reachable by its own feature set",
                v.asset
            );
        }
        let mut assets: Vec<_> = VARIANTS.iter().map(|v| v.asset).collect();
        assets.sort_unstable();
        let before = assets.len();
        assets.dedup();
        assert_eq!(before, assets.len(), "two variants share an asset name");
    }

    /// asi-only and the empty set are deliberately unpublished; asking for either
    /// must fail rather than round to a neighbour.
    #[test]
    fn unpublished_combinations_have_no_variant() {
        assert!(Variant::for_features(f(false, true, false)).is_none(), "asi-only");
        assert!(Variant::for_features(f(false, false, false)).is_none(), "nothing");
    }

    #[test]
    fn a_cracked_pmcbb_exe_gets_the_full_kit() {
        let c = resolve(ExeKind::CrackedImportingPmcBb, None).unwrap();
        assert_eq!(c.asset, "pmc_bb_fully_loaded.dll");
        assert_eq!(c.features, f(true, true, true));
        assert!(!c.overridden);
    }

    #[test]
    fn a_cruise_cracked_exe_gets_neither_the_crack_nor_the_loader() {
        let c = resolve(ExeKind::CrackedImportingOther, None).unwrap();
        assert_eq!(c.asset, "pmc_bb_log_only.dll");
        assert!(!c.features.crack, "cruise already answers SecuROM");
        assert!(!c.features.asi, "cruise already loads plugins");
        assert!(c.features.log);
    }

    /// The regression that motivated all of this: an exe with no SecuROM to answer
    /// must never be handed a build that fires the event.
    #[test]
    fn no_uncracked_exe_ever_receives_the_crack() {
        for kind in [ExeKind::NotCracked, ExeKind::Unknown] {
            let c = resolve(kind, None).unwrap();
            assert!(
                !c.features.crack,
                "{kind:?} resolved to {} which carries the SecuROM spoof",
                c.asset
            );
        }
    }

    /// `v11-patched-drmfree` is the build `classify(size)` calls "cracked" because
    /// it lands at exactly the cracked size. Routed through the hash-assigned id it
    /// is correctly not cracked, and imports no sidecar — so a spoof build would
    /// never even be loaded.
    #[test]
    fn the_drm_free_rebuild_is_not_treated_as_cracked() {
        let kind = ExeKind::from_exe_id(Some("v11-patched-drmfree"));
        assert_eq!(kind, ExeKind::NotCracked);
        assert!(!resolve(kind, None).unwrap().features.crack);
    }

    /// Every id the catalogue can assign must map to a deliberate kind. A new
    /// catalogue entry that nobody classified would silently fall into `Unknown`.
    #[test]
    fn every_catalogue_id_is_classified() {
        for id in EXE_IDS {
            assert_ne!(
                ExeKind::from_exe_id(Some(id)),
                ExeKind::Unknown,
                "{id} is in EXE_IDS but no pmc_bb variant policy covers it — add it to \
                 ExeKind::from_exe_id rather than letting it default"
            );
        }
        assert_eq!(ExeKind::from_exe_id(None), ExeKind::Unknown);
        assert_eq!(ExeKind::from_exe_id(Some("not-a-real-id")), ExeKind::Unknown);
    }

    #[test]
    fn every_resolution_explains_itself() {
        for kind in [
            ExeKind::CrackedImportingPmcBb,
            ExeKind::CrackedImportingOther,
            ExeKind::NotCracked,
            ExeKind::Unknown,
        ] {
            let c = resolve(kind, None).unwrap();
            assert!(!c.reason.is_empty(), "{kind:?} chose {} with no reason", c.asset);
        }
    }

    /// Logging is what every diagnostic modkit offers is built on.
    #[test]
    fn every_automatic_choice_keeps_logging() {
        for kind in [
            ExeKind::CrackedImportingPmcBb,
            ExeKind::CrackedImportingOther,
            ExeKind::NotCracked,
            ExeKind::Unknown,
        ] {
            assert!(resolve(kind, None).unwrap().features.log, "{kind:?}");
        }
    }

    #[test]
    fn an_override_is_honoured_and_flagged() {
        let c = resolve(ExeKind::NotCracked, Some("pmc_bb_asi_log.dll")).unwrap();
        assert_eq!(c.asset, "pmc_bb_asi_log.dll");
        assert!(c.features.asi);
        assert!(c.overridden);
    }

    #[test]
    fn an_empty_override_falls_back_to_the_automatic_choice() {
        let c = resolve(ExeKind::NotCracked, Some("   ")).unwrap();
        assert_eq!(c.asset, "pmc_bb_log_only.dll");
        assert!(!c.overridden);
    }

    #[test]
    fn an_unknown_override_is_refused_and_lists_the_real_builds() {
        let err = resolve(ExeKind::NotCracked, Some("pmc_bb.dll")).unwrap_err();
        assert!(err.contains("pmc_bb_fully_loaded.dll"), "{err}");
        assert!(err.contains("pmc_bb_log_only.dll"), "{err}");
    }

    /// The trap: `pmc_bb_log.dll` was *asi + log*, and the similarly-named
    /// `pmc_bb_log_only.dll` has the loader compiled out. Anything that reads the
    /// old name as the new one silently stops loading mods.
    #[test]
    fn log_only_is_not_the_successor_to_the_old_log_build() {
        let log_only = Variant::by_asset("pmc_bb_log_only.dll").unwrap();
        assert!(
            !log_only.features.asi,
            "log_only must not carry the loader — if it ever does, the warning this \
             test encodes is obsolete"
        );
        let successor = Variant::by_asset("pmc_bb_asi_log.dll").unwrap();
        assert!(successor.features.asi && successor.features.log && !successor.features.crack);
    }

    #[test]
    fn the_dead_asset_names_are_not_variants() {
        assert!(Variant::by_asset("pmc_bb.dll").is_none());
        assert!(Variant::by_asset("pmc_bb_log.dll").is_none());
    }

    #[test]
    fn feature_names_are_stable_and_ordered() {
        assert_eq!(f(true, true, true).names(), ["crack", "asi", "log"]);
        assert_eq!(f(false, false, true).names(), ["log"]);
        assert!(f(false, false, false).names().is_empty());
    }
}
