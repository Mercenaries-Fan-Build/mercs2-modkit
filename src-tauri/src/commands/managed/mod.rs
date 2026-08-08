//! Artifacts modkit installs, and the record of what it installed.
//!
//! [`super::net`] gets bytes; this decides *which* bytes, puts them on disk without
//! ever leaving a destination in a half-written state, and writes down what landed
//! there so a later run can answer "what is installed, is it current, and is it
//! still the file we wrote?"
//!
//! Those three questions had three different answers before. The Workshop toolset
//! kept a backend sidecar and answered all three; `pmc_bb`, `dxwrapper` and
//! `apply_crack` kept a version string in browser `localStorage` and could answer
//! none of them reliably; and no download in the app verified an integrity digest
//! except the one Microsoft-signed installer.

use std::path::PathBuf;

pub mod ledger;
pub mod place;
pub mod pmc_bb;

pub use ledger::{Component, InstalledFile, Ledger};
pub use place::{place, snapshot, PlaceOpts, Placed, Verifier};

/// `<app-data>/managed`, where the install record lives.
pub fn managed_dir() -> Result<PathBuf, String> {
    let dir = super::paths::app_data_dir()?.join("managed");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Could not create the managed-components dir: {e}"))?;
    Ok(dir)
}

/// Whether `latest` is a newer release than `installed`.
///
/// One implementation, in Rust, for every managed artifact. There were two: the
/// toolset compared tags with `!=`, and the frontend used a semver comparison — so
/// a re-tagged or rolled-back release meant the Workshop Tools page and the
/// component chips disagreed about whether an update existed.
///
/// Leading `v` is optional and dotted numeric components compare numerically, so
/// `v0.10.0` is correctly newer than `v0.9.3` (string ordering says otherwise).
/// Anything non-numeric falls back to "different means newer", which is the
/// toolset's old behaviour and the safe direction: offering an update that turns
/// out to be a sidegrade is recoverable, silently pinning someone to an old build
/// is what this whole change exists to stop.
pub fn is_newer(installed: &str, latest: &str) -> bool {
    let (installed, latest) = (installed.trim(), latest.trim());
    if installed.is_empty() || latest.is_empty() || installed == latest {
        return false;
    }

    let parts = |s: &str| -> Option<Vec<u64>> {
        s.trim_start_matches(['v', 'V'])
            .split('.')
            .map(|p| p.parse::<u64>().ok())
            .collect()
    };

    match (parts(installed), parts(latest)) {
        (Some(a), Some(b)) => {
            let len = a.len().max(b.len());
            for i in 0..len {
                let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
                if x != y {
                    return y > x;
                }
            }
            false
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn numeric_components_compare_numerically() {
        assert!(is_newer("v0.9.3", "v0.10.0"), "string ordering gets this backwards");
        assert!(!is_newer("v0.10.0", "v0.9.3"));
        assert!(is_newer("1.2.3", "1.2.4"));
        assert!(!is_newer("1.2.4", "1.2.3"));
    }

    #[test]
    fn the_v_prefix_is_optional_on_either_side() {
        assert!(is_newer("0.9.0", "v0.10.0"));
        assert!(is_newer("v0.9.0", "0.10.0"));
        assert!(!is_newer("v1.0.0", "1.0.0"));
    }

    #[test]
    fn missing_components_read_as_zero() {
        assert!(is_newer("v1.2", "v1.2.1"));
        assert!(!is_newer("v1.2.0", "v1.2"));
    }

    #[test]
    fn identical_and_empty_are_never_newer() {
        assert!(!is_newer("v1.0.0", "v1.0.0"));
        assert!(!is_newer("", "v1.0.0"), "nothing installed is not an update");
        assert!(!is_newer("v1.0.0", ""));
    }

    /// Non-numeric tags cannot be ordered, so a difference is treated as an
    /// update — the safe direction.
    #[test]
    fn unparseable_tags_fall_back_to_difference() {
        assert!(is_newer("nightly-a", "nightly-b"));
        assert!(!is_newer("nightly-a", "nightly-a"));
        assert!(is_newer("v1.0.0", "v1.0.0-rc1"));
    }
}
