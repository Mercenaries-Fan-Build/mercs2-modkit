//! Where an installed entry came from — captured at ingest/install time, because it cannot
//! be reconstructed afterwards.
//!
//! Modkit composes its load order from six collections (WAD mods, ASI plugins, imported
//! prebuilt WADs, Shipments, the wardrobe, texture swaps). Until now every one of them threw
//! its catalogue identity away the moment it was installed: an `AsiMod` keeps only
//! `slugify("{repo_name}-{slug}")`, which is lossy and cannot be parsed back into the pair it
//! came from, and a WAD-kind install keeps nothing at all — not the repository, not the slug,
//! not the release tag. Re-association after the fact is done by matching `.asi` basenames,
//! which is a guess.
//!
//! So provenance is recorded once, at the only moment it is known.
//!
//! # Identity is source-scoped
//!
//! No slug is a usable identity on its own. A registry slug is the Shipment's `shipment.name`,
//! and every fork of a mod legitimately carries the same one; a catalog slug is unique only
//! *within its repository*. Both namespaces are composite, so an [`Origin::id`] is comparable
//! **only against another id with the same [`Origin::source`]**.
//!
//! [`Origin::version`] spans two namespaces the same way: for `catalog` it is a GitHub release
//! tag, for `registry`/`local` it is the qm manifest's `shipment.version`. `None` is a bucket
//! with a meaning (a mod being worked on and not yet released), not missing data.

use serde::{Deserialize, Serialize};

/// The kind of place an entry came from. Closed vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginSource {
    /// Installed through mercs.ink. Carries a full public id.
    Registry,
    /// Installed from a mod-source repository index (`CatalogMod`).
    Catalog,
    /// A hand-built qm source folder staged from disk. Neither registry nor catalog, and its
    /// folder name is user-chosen, so it has no id.
    Local,
    /// A local file the user picked (a prebuilt WAD, a loose `.asi`). User-named, so no id.
    Imported,
    /// Something modkit synthesizes rather than installs — the wardrobe and the texture
    /// queue. Fixed, enumerated ids.
    Modkit,
}

/// Provenance of one entry in the load order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub source: OriginSource,
    /// Source-scoped identity, or `None` when the entry has no public one.
    #[serde(default)]
    pub id: Option<String>,
    /// Release tag (`catalog`) or manifest version (`registry`/`local`); `None` is meaningful.
    #[serde(default)]
    pub version: Option<String>,
}

/// The wardrobe's fixed id — modkit synthesizes one outfit Shipment from every pick.
pub const MODKIT_WARDROBE_ID: &str = "modkit:wardrobe";
/// The texture queue's fixed id — swaps enter the WAD like any other asset claim.
pub const MODKIT_TEXTURES_ID: &str = "modkit:textures";

impl Origin {
    /// A mercs.ink install. The id keys on the **GitHub repo id, not the owner name**: owner
    /// names change on account rename and repo transfer, which would silently split one mod's
    /// history into two buckets; repo ids never do.
    pub fn registry(slug: &str, github_repo_id: u64, version: Option<String>) -> Self {
        Self {
            source: OriginSource::Registry,
            id: Some(format!("{slug}-{github_repo_id}")),
            version,
        }
    }

    /// A catalog install, addressed as `"repo-url#slug"` — modkit's existing composite ref
    /// (the same one `CatalogMod::incompatible` uses), not a new format.
    pub fn catalog(repository: &str, slug: &str, version: Option<String>) -> Self {
        Self {
            source: OriginSource::Catalog,
            id: Some(format!("{repository}#{slug}")),
            version,
        }
    }

    /// A hand-built source folder. Always `id: None` — guessing a repo from a folder name
    /// would assert a provenance it does not have.
    pub fn local(version: Option<String>) -> Self {
        Self { source: OriginSource::Local, id: None, version }
    }

    /// The `serde` default for an entry persisted before origins were captured: staged from
    /// disk, nothing known about it. Deliberately not a "reconstruct it later" placeholder —
    /// catalogue identity is unrecoverable once thrown away, which is the reason this module
    /// exists.
    pub fn local_unknown() -> Self {
        Self::local(None)
    }

    /// A file imported from disk. Always `id: None`, and no version to speak of.
    pub fn imported() -> Self {
        Self { source: OriginSource::Imported, id: None, version: None }
    }

    /// One of modkit's own synthesized contributors — pass [`MODKIT_WARDROBE_ID`] or
    /// [`MODKIT_TEXTURES_ID`].
    pub fn modkit(id: &'static str) -> Self {
        Self { source: OriginSource::Modkit, id: Some(id.to_string()), version: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry half is `{slug}-{repo_id}`, and a rename of the *owner* must not change it.
    #[test]
    fn registry_id_keys_on_the_repo_id() {
        let a = Origin::registry("vehicle-pack", 486_521_234, Some("2.1.0".into()));
        assert_eq!(a.id.as_deref(), Some("vehicle-pack-486521234"));
        assert_eq!(a.source, OriginSource::Registry);
    }

    /// The catalog half reuses the `"repo-url#slug"` ref modkit already addresses mods by.
    #[test]
    fn catalog_id_is_the_existing_composite_ref() {
        let c = Origin::catalog("https://github.com/elishacloud/dxwrapper", "dxwrapper", None);
        assert_eq!(
            c.id.as_deref(),
            Some("https://github.com/elishacloud/dxwrapper#dxwrapper")
        );
        // A missing version is a bucket, not an absence — it stays None rather than "".
        assert_eq!(c.version, None);
    }

    /// User-named things never get an id, however tempting the folder name looks.
    #[test]
    fn user_named_sources_have_no_id() {
        assert_eq!(Origin::local(Some("0.3.0".into())).id, None);
        assert_eq!(Origin::imported().id, None);
    }

    #[test]
    fn modkit_ids_are_the_fixed_pair() {
        assert_eq!(Origin::modkit(MODKIT_WARDROBE_ID).id.as_deref(), Some("modkit:wardrobe"));
        assert_eq!(Origin::modkit(MODKIT_TEXTURES_ID).id.as_deref(), Some("modkit:textures"));
    }

    /// The wire form is snake_case, and it is what gets persisted — a rename here silently
    /// re-buckets every stored entry.
    #[test]
    fn source_serializes_snake_case() {
        let json = serde_json::to_string(&Origin::imported()).unwrap();
        assert!(json.contains("\"source\":\"imported\""), "got {json}");
        let back: Origin = serde_json::from_str(&json).unwrap();
        assert_eq!(back.source, OriginSource::Imported);
    }
}
