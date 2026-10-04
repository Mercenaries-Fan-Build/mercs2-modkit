//! Zip and `.tar.xz` handling for downloaded artifacts, with the guards a
//! downloaded archive needs.
//!
//! Four call sites unpacked untrusted archives straight through `ZipArchive::extract`
//! — mod releases from any GitHub or GitLab project, mercs.ink releases, the
//! Workshop data bundle, and the dxwrapper package. `extract` resolves entry names
//! relative to the destination, so an entry called `../../autoexec` is written
//! outside it. Nothing here trusts an archive to describe itself honestly.
//!
//! The tarball path exists for the macOS Wine build ([`super::super::managed::wine`]),
//! which ships as `.tar.xz` and carries what no zip modkit handles does: symlinks
//! (`libz.dylib -> libz.1.3.dylib`) and exec bits. A symlink is a second way out of
//! the destination, so its *target* is checked as strictly as an entry name, and
//! every entry kind other than file, directory and symlink is refused rather than
//! skipped.

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

/// Check that a symlink at archive path `entry` pointing at `target` stays inside
/// the destination.
///
/// The target is resolved lexically from the link's own directory: an absolute
/// target is refused outright, and a `..` that climbs above the destination root is
/// refused. Lexical is enough because every link this admits points inside the
/// tree, so no later entry can be written *through* a link to somewhere else.
fn check_link_target(entry: &str, target: &Path) -> Result<(), String> {
    let normalized = entry.replace('\\', "/");
    // Depth of the directory holding the link, below the destination root.
    let mut depth: i64 = Path::new(&normalized)
        .parent()
        .map(|p| {
            p.components()
                .filter(|c| matches!(c, Component::Normal(_)))
                .count() as i64
        })
        .unwrap_or(0);
    for component in target.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err(format!(
                        "Archive symlink '{entry}' -> '{}' points outside the destination — refusing to unpack it.",
                        target.display()
                    ));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "Archive symlink '{entry}' -> '{}' is an absolute link — refusing to unpack it.",
                    target.display()
                ))
            }
        }
    }
    Ok(())
}

/// Extract an in-memory `.tar.xz` into `dest`, keeping unix permissions and
/// symlinks.
///
/// Every entry is either a file, a directory or a symlink whose target stays in
/// `dest`; anything else (hard links, devices, FIFOs) fails the whole extraction,
/// naming the entry, because a partial unpack reported as an install is exactly
/// the silent failure this module exists to prevent.
pub fn extract_tar_xz(bytes: &[u8], dest: &Path) -> Result<(), String> {
    let decoder = xz2::read::XzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|e| format!("Bad tar.xz archive: {e}"))?;

    let mut count: usize = 0;
    let mut written: u64 = 0;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("Could not read a tar.xz entry: {e}"))?;
        count += 1;
        if count > MAX_ENTRIES {
            return Err(format!("Archive has more than {MAX_ENTRIES} entries."));
        }

        let name = entry
            .path()
            .map_err(|e| format!("Archive entry {count} has an unreadable path: {e}"))?
            .to_string_lossy()
            .into_owned();
        let out = safe_join(dest, &name)?;
        let kind = entry.header().entry_type();

        if kind.is_dir() {
            std::fs::create_dir_all(&out)
                .map_err(|e| format!("Could not create {}: {e}", out.display()))?;
            continue;
        }

        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
        }

        if kind.is_symlink() {
            let target = entry
                .link_name()
                .map_err(|e| format!("Archive symlink '{name}' has an unreadable target: {e}"))?
                .ok_or_else(|| format!("Archive symlink '{name}' has no target."))?
                .into_owned();
            check_link_target(&name, &target)?;
            make_symlink(&target, &out)?;
            continue;
        }

        if !kind.is_file() {
            return Err(format!(
                "Archive entry '{name}' is a {kind:?} entry, which modkit does not unpack — refusing the archive."
            ));
        }

        written += entry.size();
        if written > MAX_TOTAL_BYTES {
            return Err(format!(
                "Archive unpacks to more than {MAX_TOTAL_BYTES} bytes — refusing to continue."
            ));
        }
        let mut f = std::fs::File::create(&out)
            .map_err(|e| format!("Could not create {}: {e}", out.display()))?;
        std::io::copy(&mut entry, &mut f)
            .map_err(|e| format!("Could not write {}: {e}", out.display()))?;
        set_mode(&out, entry.header().mode().ok())?;
    }
    Ok(())
}

#[cfg(unix)]
fn make_symlink(target: &Path, link: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(target, link)
        .map_err(|e| format!("Could not create symlink {}: {e}", link.display()))
}

#[cfg(not(unix))]
fn make_symlink(target: &Path, link: &Path) -> Result<(), String> {
    Err(format!(
        "Archive symlink {} -> {} cannot be unpacked on this OS.",
        link.display(),
        target.display()
    ))
}

/// Apply the archive's permission bits, so `wine64` stays executable.
#[cfg(unix)]
fn set_mode(path: &Path, mode: Option<u32>) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let Some(mode) = mode else { return Ok(()) };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o777))
        .map_err(|e| format!("Could not set permissions on {}: {e}", path.display()))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: Option<u32>) -> Result<(), String> {
    Ok(())
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

    /// One tar entry for [`tar_xz_with`]: a file with a mode, a directory, or a
    /// symlink with a target.
    enum TarEntry<'a> {
        File(&'a str, &'a [u8], u32),
        Dir(&'a str),
        Link(&'a str, &'a str),
        HardLink(&'a str, &'a str),
    }

    fn tar_xz_with(entries: &[TarEntry]) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut b = tar::Builder::new(&mut tar_bytes);
            for e in entries {
                let mut h = tar::Header::new_gnu();
                match e {
                    TarEntry::File(name, body, mode) => {
                        h.set_entry_type(tar::EntryType::Regular);
                        h.set_size(body.len() as u64);
                        h.set_mode(*mode);
                        b.append_data(&mut h, name, *body).unwrap();
                    }
                    TarEntry::Dir(name) => {
                        h.set_entry_type(tar::EntryType::Directory);
                        h.set_size(0);
                        h.set_mode(0o755);
                        b.append_data(&mut h, name, std::io::empty()).unwrap();
                    }
                    TarEntry::Link(name, target) | TarEntry::HardLink(name, target) => {
                        h.set_entry_type(if matches!(e, TarEntry::Link(..)) {
                            tar::EntryType::Symlink
                        } else {
                            tar::EntryType::Link
                        });
                        h.set_size(0);
                        h.set_mode(0o777);
                        b.append_link(&mut h, name, target).unwrap();
                    }
                }
            }
            b.finish().unwrap();
        }
        let mut enc = xz2::write::XzEncoder::new(Vec::new(), 1);
        enc.write_all(&tar_bytes).unwrap();
        enc.finish().unwrap()
    }

    /// The shape of the real Wine build: nested dirs, an executable, and a
    /// sibling dylib symlink.
    #[cfg(unix)]
    #[test]
    fn a_tar_xz_unpacks_files_modes_and_sibling_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bytes = tar_xz_with(&[
            TarEntry::Dir("Wine/bin/"),
            TarEntry::File("Wine/bin/wine64", b"#!exe", 0o755),
            TarEntry::File("Wine/lib/libz.1.3.dylib", b"lib", 0o644),
            TarEntry::Link("Wine/lib/libz.dylib", "libz.1.3.dylib"),
        ]);
        extract_tar_xz(&bytes, dir.path()).expect("unpacks");

        let exe = dir.path().join("Wine/bin/wine64");
        assert_eq!(std::fs::read(&exe).unwrap(), b"#!exe");
        assert_eq!(
            std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777,
            0o755,
            "wine64 must stay executable"
        );
        let link = dir.path().join("Wine/lib/libz.dylib");
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("libz.1.3.dylib"));
        assert_eq!(std::fs::read(&link).unwrap(), b"lib");
    }

    #[test]
    fn a_tar_traversal_entry_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("stage");
        // tar::Builder refuses to write `..` itself, so write the name raw.
        let mut tar_bytes = Vec::new();
        {
            let mut b = tar::Builder::new(&mut tar_bytes);
            let mut h = tar::Header::new_old();
            h.as_old_mut().name[..14].copy_from_slice(b"../escaped.txt");
            h.set_entry_type(tar::EntryType::Regular);
            h.set_size(5);
            h.set_mode(0o644);
            h.set_cksum();
            b.append(&h, &b"pwned"[..]).unwrap();
            b.finish().unwrap();
        }
        let mut enc = xz2::write::XzEncoder::new(Vec::new(), 1);
        enc.write_all(&tar_bytes).unwrap();
        let bytes = enc.finish().unwrap();

        let err = extract_tar_xz(&bytes, &dest).unwrap_err();
        assert!(err.contains("escaped.txt"), "{err}");
        assert!(!dir.path().join("escaped.txt").exists());
    }

    #[test]
    fn a_symlink_out_of_the_destination_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        for target in ["../../../outside", "/etc/passwd"] {
            let bytes = tar_xz_with(&[TarEntry::Link("Wine/lib/evil", target)]);
            let err = extract_tar_xz(&bytes, dir.path()).unwrap_err();
            assert!(err.contains("Wine/lib/evil"), "{target}: {err}");
        }
    }

    #[test]
    fn a_symlink_that_climbs_but_stays_inside_is_allowed_by_the_check() {
        assert!(check_link_target("Wine/lib/x", Path::new("../bin/wine64")).is_ok());
        assert!(check_link_target("Wine/lib/x", Path::new("../../top")).is_ok());
        assert!(check_link_target("Wine/lib/x", Path::new("../../../out")).is_err());
        assert!(check_link_target("x", Path::new("../out")).is_err());
    }

    /// Anything but file / dir / symlink fails the archive, never silently skips.
    #[test]
    fn a_hard_link_entry_fails_the_extraction() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = tar_xz_with(&[
            TarEntry::File("a", b"one", 0o644),
            TarEntry::HardLink("b", "a"),
        ]);
        let err = extract_tar_xz(&bytes, dir.path()).unwrap_err();
        assert!(err.contains("'b'"), "{err}");
    }

    #[test]
    fn an_entry_is_read_by_full_path_suffix() {
        let bytes = zip_with(&[("Stub/d3d9.dll", b"stub"), ("d3d9.dll", b"root")]);
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(read_entry(&mut z, "stub/d3d9.dll").unwrap(), b"stub");
        assert_eq!(read_entry(&mut z, "nope.dll"), None);
    }
}
