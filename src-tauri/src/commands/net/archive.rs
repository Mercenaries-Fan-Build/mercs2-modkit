//! Zip handling for downloaded artifacts, with the guards a downloaded zip needs.
//!
//! Four call sites unpacked untrusted archives straight through `ZipArchive::extract`
//! — mod releases from any GitHub or GitLab project, mercs.ink releases, the
//! Workshop data bundle, and the dxwrapper package. `extract` resolves entry names
//! relative to the destination, so an entry called `../../autoexec` is written
//! outside it. Nothing here trusts an archive to describe itself honestly.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// Ceiling on total uncompressed output. The Workshop data bundle is the largest
/// legitimate archive at tens of MB; past this is a zip bomb or a mistake.
pub const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Ceiling on entry count, which a bomb can exhaust without exceeding the byte cap.
pub const MAX_ENTRIES: usize = 50_000;

/// Resolve an archive entry name to a path under `dest`, or reject it.
///
/// Rejects absolute paths, drive-qualified paths, and any `..` component. Note the
/// check is on the *entry name as written*, not on the resolved path: comparing a
/// canonicalized result against `dest` fails open when the destination does not
/// exist yet, and follows symlinks the archive itself may have just created.
fn safe_join(dest: &Path, name: &str) -> Result<PathBuf, String> {
    let normalized = name.replace('\\', "/");
    let rel = Path::new(&normalized);

    let mut out = dest.to_path_buf();
    for component in rel.components() {
        match component {
            Component::Normal(part) => out.push(part),
            // `./` is harmless noise some writers emit.
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!(
                    "Archive entry '{name}' escapes the destination directory with '..' — refusing to unpack it."
                ))
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "Archive entry '{name}' is an absolute path — refusing to unpack it."
                ))
            }
        }
    }
    Ok(out)
}

/// Extract `zip` into `dest`, creating directories as needed.
pub fn extract_into<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    dest: &Path,
) -> Result<(), String> {
    if zip.len() > MAX_ENTRIES {
        return Err(format!(
            "Archive has {} entries, past the {MAX_ENTRIES} limit.",
            zip.len()
        ));
    }

    let mut written: u64 = 0;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("Could not read archive entry {i}: {e}"))?;

        // `enclosed_name` is the zip crate's own traversal check; `safe_join` is
        // ours. Keeping both means an entry has to satisfy two independent
        // implementations, and the error names the offending entry either way.
        let name = entry.name().to_string();
        if entry.enclosed_name().is_none() {
            return Err(format!(
                "Archive entry '{name}' is not safely contained — refusing to unpack it."
            ));
        }
        let out = safe_join(dest, &name)?;

        if entry.is_dir() {
            std::fs::create_dir_all(&out)
                .map_err(|e| format!("Could not create {}: {e}", out.display()))?;
            continue;
        }

        written += entry.size();
        if written > MAX_TOTAL_BYTES {
            return Err(format!(
                "Archive unpacks to more than {MAX_TOTAL_BYTES} bytes — refusing to continue."
            ));
        }

        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
        }
        let mut f = std::fs::File::create(&out)
            .map_err(|e| format!("Could not create {}: {e}", out.display()))?;
        std::io::copy(&mut entry, &mut f)
            .map_err(|e| format!("Could not write {}: {e}", out.display()))?;
    }
    Ok(())
}

/// Extract an on-disk archive into `dest`.
pub fn extract_zip(archive: &Path, dest: &Path) -> Result<(), String> {
    let f = std::fs::File::open(archive)
        .map_err(|e| format!("Could not open {}: {e}", archive.display()))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("Bad zip archive: {e}"))?;
    extract_into(&mut z, dest)
}

/// Extract an in-memory archive into `dest`.
pub fn extract_bytes(bytes: Vec<u8>, dest: &Path) -> Result<(), String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("Bad zip archive: {e}"))?;
    extract_into(&mut z, dest)
}

/// Read one entry whose full archive path ends with `suffix`, case-insensitively.
///
/// Matched on the whole path, not the file name, so a caller can target
/// `Stub/d3d9.dll` specifically rather than whichever `d3d9.dll` comes first.
pub fn read_entry<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    suffix: &str,
) -> Option<Vec<u8>> {
    let want = suffix.to_ascii_lowercase().replace('\\', "/");
    // Find the name first (immutable borrow), then read it (mutable borrow).
    let name = (0..zip.len()).find_map(|i| {
        let f = zip.by_index(i).ok()?;
        let n = f.name().replace('\\', "/").to_ascii_lowercase();
        n.ends_with(&want).then(|| f.name().to_string())
    })?;
    let mut f = zip.by_name(&name).ok()?;
    let mut buf = Vec::with_capacity(f.size() as usize);
    f.read_to_end(&mut buf).ok()?;
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            for (name, body) in entries {
                w.start_file(*name, SimpleFileOptions::default()).unwrap();
                w.write_all(body).unwrap();
            }
            w.finish().unwrap();
        }
        buf
    }

    #[test]
    fn an_ordinary_archive_unpacks() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = zip_with(&[("a.txt", b"one"), ("sub/b.txt", b"two")]);
        extract_bytes(bytes, dir.path()).expect("unpacks");
        assert_eq!(std::fs::read(dir.path().join("a.txt")).unwrap(), b"one");
        assert_eq!(std::fs::read(dir.path().join("sub/b.txt")).unwrap(), b"two");
    }

    /// The guard this module exists for: an entry that climbs out of the
    /// destination must be refused, and nothing must be written outside it.
    #[test]
    fn a_traversal_entry_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("stage");
        std::fs::create_dir_all(&dest).unwrap();

        let bytes = zip_with(&[("../escaped.txt", b"pwned")]);
        let err = extract_bytes(bytes, &dest).unwrap_err();
        assert!(err.contains("escaped.txt"), "{err}");
        assert!(
            !dir.path().join("escaped.txt").exists(),
            "the entry was written outside the destination"
        );
    }

    #[test]
    fn a_deeply_nested_traversal_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = zip_with(&[("ok/../../escaped.txt", b"pwned")]);
        assert!(extract_bytes(bytes, dir.path()).is_err());
        assert!(!dir.path().parent().unwrap().join("escaped.txt").exists());
    }

    #[test]
    fn safe_join_rejects_absolute_and_parent_paths() {
        let dest = Path::new("/tmp/dest");
        assert!(safe_join(dest, "../x").is_err());
        assert!(safe_join(dest, "a/../../x").is_err());
        assert!(safe_join(dest, "/etc/passwd").is_err());
        assert_eq!(safe_join(dest, "./a/b").unwrap(), dest.join("a/b"));
        // Backslashes are separators in zips written on Windows, not name characters.
        assert_eq!(safe_join(dest, "a\\b").unwrap(), dest.join("a").join("b"));
        assert!(safe_join(dest, "a\\..\\..\\x").is_err());
    }

    #[test]
    fn an_entry_is_read_by_full_path_suffix() {
        let bytes = zip_with(&[("Stub/d3d9.dll", b"stub"), ("d3d9.dll", b"root")]);
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(read_entry(&mut z, "stub/d3d9.dll").unwrap(), b"stub");
        assert_eq!(read_entry(&mut z, "nope.dll"), None);
    }
}
