//! Offline manifest generator (maintainer tool).
//!
//! Builds the full verify manifest — shared files, per-WAD block checksums, and
//! the multi-build exe catalog — from a reference `storage/` folder, using the
//! same core as the in-app `generate_manifest` command.
//!
//!   cargo run --release --example gen_manifest -- [storage_dir] [out.json]
//!
//! Defaults (run from `src-tauri/`): storage = `../storage`,
//! out = `../manifests/mercs2.manifest.json`. `storage_dir` must contain the
//! clean `Mercenaries 2 World in Flames/` tree plus the loose `*.exe` variants.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use mercs2_modkit_lib::commands::verify::{build_manifest, ExeSpec};

/// Exe byte size → (version, variant); mirrors `classify()` in game.rs.
fn classify(size: u64) -> (&'static str, &'static str) {
    match size {
        16_846_848 => ("v1.0", "unsigned"),
        17_122_568 => ("v1.0", "ea-signed"),
        53_944_080 => ("v1.1", "patched"),
        53_482_288 => ("v1.1", "cracked"),
        _ => ("unknown", "unknown"),
    }
}

/// Per-build metadata keyed by storage filename: the catalogue id, a distinguishing
/// label, the sidecar DLL the exe imports (must be present to start), and a caveat.
///
/// The **id** is the part that must not drift. It is the stable key every per-build
/// record is filed under, so a regenerated manifest has to reproduce the same id for
/// the same build — which is why it lives in this table beside the label rather than
/// being derived from the filename, the size, or the (version, variant) pair. All
/// three of those change; the id may not. Ids come from `verify::EXE_IDS`.
struct ExeMeta {
    id: Option<String>,
    label: Option<String>,
    requires: Option<String>,
    note: Option<String>,
    /// Overrides `classify(size)` for a build whose size misidentifies it.
    ///
    /// `classify` maps a byte size to a `(version, variant)` pair, which works only while
    /// each build has its own size. It does not hold: the DRM-free v1.1 build is a rebuild
    /// of the *patched* exe that lands at exactly the *cracked* size, so `classify` calls it
    /// `cracked` with confidence. The catalogue is hash-keyed precisely so size does not have
    /// to be trusted; this is where that stops being an argument and starts being a value.
    classified: Option<(&'static str, &'static str)>,
}

fn meta_for(name: &str) -> ExeMeta {
    // The DRM-free build is the only entry needing a `classified` override today. Adding one
    // is the correct response to a size collision — never renaming the id to match `classify`.
    let classified = match name {
        "Mercenaries2(v1.1 DRM-free).exe" => Some(("v1.1", "patched")),
        _ => None,
    };
    let (id, label, requires, note): (&str, &str, Option<&str>, Option<&str>) = match name {
        "Mercenaries2 (v1.0 unsigned).exe" => (
            "v10-unsigned",
            "retail unsigned",
            None,
            Some("Stock SecuROM exe — not de-DRM'd; crack it in Setup before modding."),
        ),
        "Mercenaries2 (v1.0 signed).exe" => (
            "v10-ea-signed",
            "retail EA-signed",
            None,
            Some("Stock SecuROM exe — not de-DRM'd; crack it in Setup before modding."),
        ),
        "Mercenaries2(v1.1).exe" => (
            "v11-patched-securom",
            "v1.1 patched (uncracked)",
            None,
            Some("Stock patched exe — still SecuROM; crack it in Setup before modding."),
        ),
        "Mercenaries2 (v1.1 cracked).exe" => (
            "v11-cracked-pmcbb",
            "pmc_bb crack (modkit)",
            Some("pmc_bb.dll"),
            Some("Loads ASI mods via the pmc_bb.dll loader — modkit's supported crack."),
        ),
        "Mercenaries2(v1.1 cruise.dll).exe" => (
            "v11-cracked-cruise",
            "cruise.dll crack (archive.org)",
            Some("cruise.dll"),
            Some("SecuROM bypass only — does NOT load ASI mods; use modkit's pmc_bb crack for modding."),
        ),
        "Mercenaries2(v1.1 DRM-free).exe" => (
            "v11-patched-drmfree",
            "v1.1 patched, DRM removed",
            None,
            // Deliberately silent on ASI capability. Whether plugins load is decided by a
            // separate loader — dxwrapper's, pmc_bb's, or another proxy DLL — and by that
            // loader's own config, none of which is a property of this executable.
            Some("DRM-free rebuild of the v1.1 patched exe; imports no sidecar DLL."),
        ),
        // An exe this table doesn't know gets no id. It is not the catalogue's build, and
        // inventing a key for it would put an unrecognised binary in a real build's bucket.
        _ => ("", "", None, None),
    };
    ExeMeta {
        id: (!id.is_empty()).then(|| id.to_string()),
        label: (!label.is_empty()).then(|| label.to_string()),
        requires: requires.map(str::to_string),
        note: note.map(str::to_string),
        classified,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let storage = PathBuf::from(args.next().unwrap_or_else(|| "../storage".into()));
    let out = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "../manifests/mercs2.manifest.json".into()),
    );

    let tree = storage.join("Mercenaries 2 World in Flames");
    if !tree.is_dir() {
        return Err(format!("reference tree not found: {}", tree.display()).into());
    }

    // Catalog every loose exe variant in storage/.
    let mut specs = Vec::new();
    for entry in std::fs::read_dir(&storage)? {
        let path = entry?.path();
        let is_exe = path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("exe"));
        if is_exe != Some(true) {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        let size = std::fs::metadata(&path)?.len();
        let meta = meta_for(&name);
        let (version, variant) = meta.classified.unwrap_or_else(|| classify(size));
        specs.push(ExeSpec {
            path,
            id: meta.id,
            version: version.into(),
            variant: variant.into(),
            label: meta.label,
            requires: meta.requires,
            note: meta.note,
        });
    }
    specs.sort_by(|a, b| {
        (&a.version, &a.variant, &a.label).cmp(&(&b.version, &b.variant, &b.label))
    });

    eprintln!("hashing {} ({} exe variants)…", tree.display(), specs.len());
    let last_decile = AtomicUsize::new(usize::MAX);
    let progress = |done: usize, total: usize| {
        let decile = (done * 10).checked_div(total).unwrap_or(10);
        if last_decile.swap(decile, Ordering::Relaxed) != decile {
            eprintln!("  {}%  ({done}/{total})", decile * 10);
        }
    };

    let (manifest, total_bytes) = build_manifest(&tree, &specs, &progress)?;

    // Safety: a truncated/damaged reference WAD hashes every block to empty.
    // Refuse to write rather than ship a bogus baseline (we got bitten by a
    // 2.4 GB vz.wad truncated to its 2 MB header).
    for (key, w) in &manifest.wads {
        if mercs2_modkit_lib::commands::verify::wad_looks_truncated(w) {
            return Err(format!(
                "{key}: every block hashed to empty — the reference WAD looks \
                 truncated/damaged ({} bytes). Restore a clean copy and re-run.",
                manifest.files.get(key).map(|e| e.size).unwrap_or(0)
            )
            .into());
        }
    }

    let block_count: usize = manifest.wads.values().map(|w| w.blocks.len()).sum();
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out, serde_json::to_string_pretty(&manifest)?)?;

    println!(
        "wrote {} — {} files, {} WAD blocks ({} WADs), {} exes, {:.1} GB hashed",
        out.display(),
        manifest.files.len(),
        block_count,
        manifest.wads.len(),
        manifest.exes.len(),
        total_bytes as f64 / 1e9,
    );
    Ok(())
}
