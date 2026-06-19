//! Asset type detection: peek a UCFX block's type hash, else fall back to extension.

use std::path::Path;

use mercs2_formats::sges::decompress_sges;
use mercs2_formats::types::type_name_from_hash;
use mercs2_formats::ucfx::parse_block_entry_table;
use serde::Serialize;

/// Detect an asset's type from a file on disk.
///
/// Strategy:
/// 1. If the file is an SGES-compressed UCFX block, decompress it, read the
///    block entry table, and map the first entry's `type_hash` to a name.
/// 2. Otherwise fall back to the file extension.
///
/// Returns `"unknown"` when neither path yields a confident answer.
pub fn detect_type_for(abs: &Path) -> String {
    if let Ok(data) = std::fs::read(abs) {
        if let Some(t) = detect_from_block_bytes(&data) {
            return t;
        }
    }
    detect_from_extension(abs)
}

fn detect_from_block_bytes(data: &[u8]) -> Option<String> {
    let decompressed = decompress_sges(data).ok()?;
    let (_count, entries) = parse_block_entry_table(&decompressed);
    let first = entries.first()?;
    let name = type_name_from_hash(first.type_hash);
    if name == "unknown" {
        None
    } else {
        Some(name.to_string())
    }
}

fn detect_from_extension(abs: &Path) -> String {
    let ext = abs
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some("lua") => "script",
        Some("dds") | Some("png") | Some("tga") => "texture",
        Some("mesh") | Some("model") => "model",
        Some("anim") => "animation",
        Some("wav") | Some("xma") | Some("snd") => "sound",
        Some("strdb") | Some("stringdb") => "stringdb",
        _ => "unknown",
    }
    .to_string()
}

#[derive(Serialize)]
pub struct DetectedTypeInfo {
    pub detected_type: String,
}

/// Frontend command: detect the type of a single asset file by path.
#[tauri::command]
pub fn detect_asset_type(path: String) -> DetectedTypeInfo {
    DetectedTypeInfo {
        detected_type: detect_type_for(Path::new(&path)),
    }
}
