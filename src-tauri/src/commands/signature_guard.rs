//! Verify a plugin's declared `signature_guard` against the actual bytes in the target exe.
//!
//! qm emits a `signature_guard` per hooked address on every `native_hook` plugin: the expected
//! prologue bytes at that VA in the exe the plugin was built against. If the exe on disk has
//! shifted (a patch level modkit does not know, a different variant, a foreign edit), the guard
//! flags it before the game runs. This is defence in depth: the plugin can (and should) verify
//! at runtime before patching, and M0199 checks the declaration against a known build at lint
//! time. This module is the third layer — modkit checks at install time so the user sees the
//! drift before launching.
//!
//! # Failure mode
//!
//! Warn, don't refuse. A mismatch produces a [`GuardWarning`] the install path attaches to
//! [`super::deploy_wad::PlacementOutcome::guard_warnings`], surfaced by the UI. The install
//! proceeds either way — a mismatch may still be safe (an offset that no callback ever exercises),
//! and the user's exe layout is not something modkit has a full inventory of.
//!
//! # Multi-exe check
//!
//! A game folder can hold several exes side by side — `Mercenaries2.exe`, `Mercenaries2.cracked.exe`
//! (setup's default cracked sibling), and `mercs2_nodrm_v*.exe` (SecuROM-unwrapped builds). A
//! plugin's guard may only be valid for one of them; a mismatch against another is expected. We
//! check every exe we find and emit one warning per (plugin, exe, address) mismatch so the user
//! can decide which one they intend to launch.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::load_plan::FileEntry;

/// One address whose bytes on disk did not match what the plugin's guard declared.
#[derive(Debug, Clone, Serialize)]
pub struct GuardWarning {
    /// The `file_name` of the plugin whose guard failed (e.g. `pmc_bb.asi`).
    pub plugin: String,
    /// Which exe was checked, as a file name (basename, not absolute path).
    pub exe: String,
    /// The VA the plugin declared it hooks, hex-formatted (`0x0085DF50`).
    pub address: String,
    /// Space-separated hex the plugin expected to find (`55 8B EC`).
    pub expected: String,
    /// What was actually at that VA. Empty when the address itself is unmapped in this exe.
    pub actual: String,
    /// Short human note: `size mismatch`, `unmapped in this exe`, or `bytes differ`.
    pub note: String,
}

/// Discover every game exe under `root`. Two families matter today: the retail `Mercenaries2*.exe`
/// tree (base + `.cracked` sibling) and the `mercs2_nodrm*.exe` tree (SecuROM-unwrapped builds).
/// Returns absolute paths, sorted by file name for stable warning order.
fn discover_exes(root: &Path) -> Vec<PathBuf> {
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or_default();
            (name.starts_with("mercenaries2") || name.starts_with("mercs2_nodrm"))
                && name.ends_with(".exe")
        })
        .collect();
    out.sort();
    out
}

/// Verify every plugin's guards against every exe found under `game_root`. See the module doc for
/// the failure mode.
pub fn verify_plugin_guards(game_root: &Path, plugins: &[FileEntry]) -> Vec<GuardWarning> {
    let guarded: Vec<&FileEntry> = plugins.iter().filter(|p| !p.signature_guard.is_empty()).collect();
    if guarded.is_empty() {
        return Vec::new();
    }
    let exes = discover_exes(game_root);
    if exes.is_empty() {
        return Vec::new();
    }

    let mut warnings = Vec::new();
    for exe_path in &exes {
        let bytes = match std::fs::read(exe_path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let exe_name = exe_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("(exe)")
            .to_string();
        for plugin in &guarded {
            for (va_hex, expected_hex) in &plugin.signature_guard {
                if let Some(w) = check_one(&plugin.file_name, &exe_name, &bytes, va_hex, expected_hex)
                {
                    warnings.push(w);
                }
            }
        }
    }
    warnings
}

/// Check one `(va, expected bytes)` pair against one exe. Returns `Some(warning)` on any
/// mismatch — bad hex, unmapped VA, wrong bytes — and `None` when the bytes match.
fn check_one(
    plugin: &str,
    exe_name: &str,
    exe_bytes: &[u8],
    va_hex: &str,
    expected_hex: &str,
) -> Option<GuardWarning> {
    let address = va_hex.to_string();
    let expected = expected_hex.to_string();
    let mismatch = |actual: String, note: &str| -> Option<GuardWarning> {
        Some(GuardWarning {
            plugin: plugin.to_string(),
            exe: exe_name.to_string(),
            address: address.clone(),
            expected: expected.clone(),
            actual,
            note: note.to_string(),
        })
    };

    let va = match parse_hex_u32(va_hex) {
        Some(v) => v,
        None => return mismatch(String::new(), "bad address in signature_guard"),
    };
    let expected_bytes = match parse_hex_bytes(expected_hex) {
        Some(b) if !b.is_empty() => b,
        _ => return mismatch(String::new(), "bad expected bytes in signature_guard"),
    };
    let offset = match va_to_file_offset(exe_bytes, va) {
        Some(o) => o,
        None => return mismatch(String::new(), "unmapped in this exe"),
    };
    if offset + expected_bytes.len() > exe_bytes.len() {
        return mismatch(String::new(), "reaches past end of this exe");
    }
    let actual_bytes = &exe_bytes[offset..offset + expected_bytes.len()];
    if actual_bytes == expected_bytes.as_slice() {
        return None;
    }
    Some(GuardWarning {
        plugin: plugin.to_string(),
        exe: exe_name.to_string(),
        address,
        expected,
        actual: format_hex_bytes(actual_bytes),
        note: "bytes differ".to_string(),
    })
}

// -- Tiny PE reader ---------------------------------------------------------------------------
//
// Just enough of the 32-bit PE format to translate a runtime VA to a file offset. Hand-rolled
// to avoid pulling in a full PE crate for this one job. Rejects anything that does not look like
// a well-formed 32-bit PE, returning None from every helper.

fn read_u16_le(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}
fn read_u32_le(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// The image base of the module (`OptionalHeader.ImageBase`), and the section table's byte range.
struct PeLayout {
    image_base: u32,
    sections_at: usize,
    section_count: usize,
}

fn read_pe_layout(bytes: &[u8]) -> Option<PeLayout> {
    // DOS header: `MZ` + e_lfanew@0x3C.
    if bytes.get(0..2)? != b"MZ" {
        return None;
    }
    let pe_at = read_u32_le(bytes, 0x3C)? as usize;
    if bytes.get(pe_at..pe_at + 4)? != b"PE\0\0" {
        return None;
    }
    // COFF file header at pe_at+4 (20 bytes), then optional header. `Machine` at pe_at+4.
    let machine = read_u16_le(bytes, pe_at + 4)?;
    if machine != 0x014C {
        // We only translate for i386 (0x014C). x64 (0x8664) uses the 64-bit optional header.
        return None;
    }
    let opt_size = read_u16_le(bytes, pe_at + 4 + 16)? as usize;
    let opt_at = pe_at + 4 + 20;
    let magic = read_u16_le(bytes, opt_at)?;
    if magic != 0x010B {
        // PE32 optional header only; PE32+ (0x020B) is 64-bit.
        return None;
    }
    // ImageBase in PE32 optional header lives at opt_at + 28.
    let image_base = read_u32_le(bytes, opt_at + 28)?;
    let sections_at = opt_at + opt_size;
    let section_count = read_u16_le(bytes, pe_at + 4 + 2)? as usize;
    Some(PeLayout {
        image_base,
        sections_at,
        section_count,
    })
}

/// Translate a runtime VA to a file offset by walking the section table. Returns `None` when the
/// bytes are not a 32-bit PE, when the VA does not fall in any section, or when the resulting
/// offset would be outside the file.
pub fn va_to_file_offset(bytes: &[u8], va: u32) -> Option<usize> {
    let layout = read_pe_layout(bytes)?;
    let rva = va.checked_sub(layout.image_base)?;
    // Section header: 40 bytes each. VirtualSize@8, VirtualAddress@12, SizeOfRawData@16, PointerToRawData@20.
    for i in 0..layout.section_count {
        let at = layout.sections_at + i * 40;
        let vsize = read_u32_le(bytes, at + 8)?;
        let vaddr = read_u32_le(bytes, at + 12)?;
        let rsize = read_u32_le(bytes, at + 16)?;
        let raw = read_u32_le(bytes, at + 20)?;
        if rva >= vaddr && rva < vaddr.checked_add(vsize.max(rsize))? {
            let off_in_section = rva - vaddr;
            if off_in_section >= rsize {
                // In a section whose virtual size is larger than its raw size (uninitialised
                // trailing bytes). There is no file-backed byte for this VA.
                return None;
            }
            let offset = raw.checked_add(off_in_section)? as usize;
            if offset > bytes.len() {
                return None;
            }
            return Some(offset);
        }
    }
    None
}

fn parse_hex_u32(s: &str) -> Option<u32> {
    let s = s.trim();
    let stripped = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u32::from_str_radix(stripped, 16).ok()
}

fn parse_hex_bytes(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for tok in s.split_ascii_whitespace() {
        let t = tok.strip_prefix("0x").or_else(|| tok.strip_prefix("0X")).unwrap_or(tok);
        if t.len() % 2 != 0 {
            return None;
        }
        for pair in t.as_bytes().chunks(2) {
            let hi = (pair[0] as char).to_digit(16)? as u8;
            let lo = (pair[1] as char).to_digit(16)? as u8;
            out.push((hi << 4) | lo);
        }
    }
    Some(out)
}

fn format_hex_bytes(b: &[u8]) -> String {
    b.iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Convenience: read the plugins list out of a build output directory's `load-plan.json`.
///
/// Returns `Ok(None)` when the file is missing — a plan predates the fields, or the build did
/// not go through `qm link`. Deserialisation failure IS surfaced, because a plan the install
/// path could not read is exactly what got the whole file into this repo.
pub fn read_plugins_from_staging(dir: &Path) -> Result<Option<Vec<FileEntry>>, String> {
    let path = dir.join(super::load_plan::PLAN_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("reading {}: {e}", path.display())),
    };
    // Parse just enough to get `plugins`. We do NOT run LoadPlan::parse here, which requires a
    // matching request; the install path may not have one, but the plugins list is well-formed
    // even without the surrounding load-order validation.
    #[derive(serde::Deserialize)]
    struct JustPlugins {
        #[serde(default)]
        plugins: Vec<FileEntry>,
    }
    let parsed: JustPlugins = serde_json::from_str(&text)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(parsed.plugins))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // A synthetic minimal PE32: DOS stub → NT headers → one .text section at RVA 0x1000 whose
    // raw bytes are `EB FE` (the classic infinite loop). ImageBase 0x00400000, so VA 0x00401000
    // maps to file offset (the section's raw pointer).
    fn tiny_pe() -> Vec<u8> {
        let mut b = vec![0u8; 0x400];
        // DOS: "MZ" + e_lfanew=0x80
        b[0..2].copy_from_slice(b"MZ");
        b[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        // PE signature
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        // COFF: Machine=0x014C (i386), NumberOfSections=1, SizeOfOptionalHeader=0xE0
        b[0x84..0x86].copy_from_slice(&0x014Cu16.to_le_bytes());
        b[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
        b[0x84 + 16..0x84 + 18].copy_from_slice(&0xE0u16.to_le_bytes());
        // Optional header: Magic=0x010B, ImageBase=0x00400000 at opt+28
        let opt = 0x84 + 20;
        b[opt..opt + 2].copy_from_slice(&0x010Bu16.to_le_bytes());
        b[opt + 28..opt + 32].copy_from_slice(&0x00400000u32.to_le_bytes());
        // Section header follows optional header. .text: VA=0x1000, VSize=0x2, RawSize=0x200, Raw=0x200
        let sec = opt + 0xE0;
        b[sec..sec + 5].copy_from_slice(b".text");
        b[sec + 8..sec + 12].copy_from_slice(&0x2u32.to_le_bytes());
        b[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        b[sec + 16..sec + 20].copy_from_slice(&0x200u32.to_le_bytes());
        b[sec + 20..sec + 24].copy_from_slice(&0x200u32.to_le_bytes());
        // Raw section bytes at file offset 0x200
        b[0x200] = 0xEB;
        b[0x201] = 0xFE;
        b
    }

    #[test]
    fn va_to_offset_translates_a_known_section_va() {
        let pe = tiny_pe();
        assert_eq!(va_to_file_offset(&pe, 0x00401000), Some(0x200));
        assert_eq!(va_to_file_offset(&pe, 0x00401001), Some(0x201));
    }

    #[test]
    fn va_below_image_base_is_unmapped() {
        let pe = tiny_pe();
        assert_eq!(va_to_file_offset(&pe, 0x00300000), None);
    }

    #[test]
    fn va_in_a_gap_between_sections_is_unmapped() {
        let pe = tiny_pe();
        // 0x00402000 is past the .text section (VA 0x1000, VSize 0x2, so ends at RVA 0x1002).
        assert_eq!(va_to_file_offset(&pe, 0x00402000), None);
    }

    #[test]
    fn not_a_pe_returns_none() {
        assert_eq!(va_to_file_offset(b"hello world", 0x00401000), None);
    }

    #[test]
    fn parse_hex_bytes_reads_space_separated_pairs() {
        assert_eq!(parse_hex_bytes("EB FE"), Some(vec![0xEB, 0xFE]));
        assert_eq!(parse_hex_bytes("eb fe"), Some(vec![0xEB, 0xFE]));
        assert_eq!(parse_hex_bytes("0xEB 0xFE"), Some(vec![0xEB, 0xFE]));
        assert_eq!(parse_hex_bytes(""), Some(Vec::new()));
    }

    #[test]
    fn parse_hex_bytes_rejects_odd_length() {
        assert_eq!(parse_hex_bytes("EBF"), None);
    }

    #[test]
    fn check_one_matches_on_correct_bytes() {
        let pe = tiny_pe();
        assert!(check_one("p.asi", "test.exe", &pe, "0x00401000", "EB FE").is_none());
    }

    #[test]
    fn check_one_reports_a_byte_mismatch_with_actual_bytes() {
        let pe = tiny_pe();
        let w = check_one("p.asi", "test.exe", &pe, "0x00401000", "90 90").expect("mismatch");
        assert_eq!(w.plugin, "p.asi");
        assert_eq!(w.exe, "test.exe");
        assert_eq!(w.address, "0x00401000");
        assert_eq!(w.expected, "90 90");
        assert_eq!(w.actual, "EB FE");
        assert_eq!(w.note, "bytes differ");
    }

    #[test]
    fn check_one_reports_an_unmapped_va_without_actual_bytes() {
        let pe = tiny_pe();
        let w = check_one("p.asi", "test.exe", &pe, "0x00300000", "EB FE").expect("unmapped");
        assert!(w.actual.is_empty());
        assert_eq!(w.note, "unmapped in this exe");
    }

    #[test]
    fn check_one_rejects_a_malformed_address() {
        let pe = tiny_pe();
        let w = check_one("p.asi", "test.exe", &pe, "not-hex", "EB FE").expect("bad addr");
        assert_eq!(w.note, "bad address in signature_guard");
    }

    #[test]
    fn verify_plugin_guards_returns_empty_when_nothing_guarded() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins = vec![FileEntry {
            contribution: 0,
            file_name: "p.asi".into(),
            source: "p.asi".into(),
            relative: "scripts/p.asi".into(),
            sha256: String::new(),
            touches: Vec::new(),
            signature_guard: BTreeMap::new(),
        }];
        assert!(verify_plugin_guards(tmp.path(), &plugins).is_empty());
    }

    #[test]
    fn verify_plugin_guards_emits_per_exe_per_address_warnings() {
        let tmp = tempfile::tempdir().unwrap();
        // Two exes side by side, both a copy of tiny_pe().
        std::fs::write(tmp.path().join("Mercenaries2.exe"), tiny_pe()).unwrap();
        std::fs::write(tmp.path().join("mercs2_nodrm_v3.exe"), tiny_pe()).unwrap();
        let mut guards = BTreeMap::new();
        // One matching guard, one mismatching guard.
        guards.insert("0x00401000".into(), "EB FE".into());
        guards.insert("0x00401001".into(), "AA BB".into());
        let plugins = vec![FileEntry {
            contribution: 0,
            file_name: "p.asi".into(),
            source: "p.asi".into(),
            relative: "scripts/p.asi".into(),
            sha256: String::new(),
            touches: Vec::new(),
            signature_guard: guards,
        }];
        let ws = verify_plugin_guards(tmp.path(), &plugins);
        // Two exes × one mismatching address = 2 warnings; the matching one emits nothing.
        assert_eq!(ws.len(), 2, "got {ws:?}");
        assert!(ws.iter().all(|w| w.address == "0x00401001"));
        assert!(ws.iter().any(|w| w.exe == "Mercenaries2.exe"));
        assert!(ws.iter().any(|w| w.exe == "mercs2_nodrm_v3.exe"));
    }
}
