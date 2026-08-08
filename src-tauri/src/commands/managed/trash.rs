//! Taking a file out of the way without destroying it.
//!
//! Nothing modkit removes is hard-deleted by default. That policy was implemented
//! three times — in [`crate::commands::deploy`], [`crate::commands::deploy_wad`]
//! and [`crate::commands::language`] — each with its own naming scheme and its own
//! copy of the one part that is easy to get wrong: `rename` fails across volumes,
//! so it has to fall back to copy-then-remove, and a caller that forgets leaves
//! the file in place while reporting it removed.
//!
//! Two naming schemes survive, because they answer different questions:
//!
//! * [`bank`] is content-addressed. For a file modkit replaces over and over — a
//!   loader DLL, a patch WAD — where the thing that matters is that the *original*
//!   is still recoverable after the tenth reinstall, and ten identical copies of
//!   an intermediate are noise.
//! * [`discard`] is timestamped. For a file removed once, by the user's choice,
//!   whose bytes modkit has no reason to read — a 500 MB language WAD should not
//!   be hashed on its way to the trash just to name it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::commands::paths::trash_dir;

/// Disambiguates two removals of the same name inside one millisecond. A
/// timestamp alone is not unique, and the collision silently loses a file.
static SEQ: AtomicU64 = AtomicU64::new(0);

fn resolve(into: Option<&Path>) -> Result<PathBuf, String> {
    let dir = match into {
        Some(d) => d.to_path_buf(),
        None => trash_dir()?,
    };
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    Ok(dir)
}

fn file_name(src: &Path) -> &str {
    src.file_name().and_then(|n| n.to_str()).unwrap_or("file")
}

/// Move `src` to `dest`, falling back to copy-then-remove across volumes.
///
/// The fallback is the whole reason this is one function: `rename` between the
/// game folder and the app-data trash crosses a filesystem boundary on any setup
/// where the game lives on a second drive, which is most of them.
pub fn move_into(src: &Path, dest: &Path) -> Result<(), String> {
    if std::fs::rename(src, dest).is_ok() {
        return Ok(());
    }
    let name = file_name(src);
    std::fs::copy(src, dest).map_err(|e| format!("Could not move {name} out of the way: {e}"))?;
    std::fs::remove_file(src).map_err(|e| format!("Could not remove {name}: {e}"))
}

/// Bank `src` under its content digest, so identical bytes are stored once.
///
/// Returns where it landed. If those bytes are already banked the file is simply
/// removed and the existing copy's path returned — re-installing the same
/// artifact twice must not pile up duplicates, and must not lose the original it
/// displaced the first time.
pub fn bank(src: &Path, into: Option<&Path>) -> Result<PathBuf, String> {
    let dir = resolve(into)?;
    let hash = super::place::sha256_of_file(src)?;
    let dest = dir.join(format!("{}-{}", &hash[..16.min(hash.len())], file_name(src)));

    if dest.exists() {
        std::fs::remove_file(src)
            .map_err(|e| format!("Could not remove {}: {e}", file_name(src)))?;
        return Ok(dest);
    }
    move_into(src, &dest)?;
    Ok(dest)
}

/// Move `src` to the trash under a timestamped name, without reading its bytes.
pub fn discard(src: &Path, into: Option<&Path>) -> Result<PathBuf, String> {
    let dir = resolve(into)?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dest = dir.join(format!("{millis}-{seq}-{}", file_name(src)));
    move_into(src, &dest)?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn banking_moves_the_file_and_leaves_the_source_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let bank_dir = tmp.path().join("bank");
        let src = write(tmp.path(), "pmc_bb.dll", b"contents");

        let dest = bank(&src, Some(&bank_dir)).unwrap();
        assert!(!src.exists(), "the source must not be left behind");
        assert_eq!(std::fs::read(&dest).unwrap(), b"contents");
    }

    #[test]
    fn banking_identical_bytes_twice_stores_them_once() {
        let tmp = tempfile::tempdir().unwrap();
        let bank_dir = tmp.path().join("bank");

        let a = write(tmp.path(), "a.dll", b"same");
        let first = bank(&a, Some(&bank_dir)).unwrap();
        let b = write(tmp.path(), "a.dll", b"same");
        let second = bank(&b, Some(&bank_dir)).unwrap();

        assert_eq!(first, second);
        assert!(!b.exists(), "the duplicate source is still removed");
        assert_eq!(std::fs::read_dir(&bank_dir).unwrap().count(), 1);
    }

    #[test]
    fn banking_different_bytes_keeps_both() {
        let tmp = tempfile::tempdir().unwrap();
        let bank_dir = tmp.path().join("bank");

        bank(&write(tmp.path(), "a.dll", b"one"), Some(&bank_dir)).unwrap();
        bank(&write(tmp.path(), "a.dll", b"two"), Some(&bank_dir)).unwrap();

        assert_eq!(std::fs::read_dir(&bank_dir).unwrap().count(), 2);
    }

    /// Two removals of the same name in the same millisecond must not collide —
    /// a timestamp alone silently loses one of them.
    #[test]
    fn discarding_the_same_name_repeatedly_never_clobbers() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");

        for body in [b"first".as_slice(), b"second", b"third"] {
            let f = write(tmp.path(), "English.wad", body);
            discard(&f, Some(&bin)).unwrap();
        }

        let kept: Vec<_> = std::fs::read_dir(&bin)
            .unwrap()
            .flatten()
            .map(|e| std::fs::read(e.path()).unwrap())
            .collect();
        assert_eq!(kept.len(), 3, "one of the three was overwritten");
        for body in [b"first".as_slice(), b"second", b"third"] {
            assert!(kept.iter().any(|k| k == body), "{body:?} was lost");
        }
    }

    #[test]
    fn discarding_does_not_need_to_read_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        let f = write(tmp.path(), "vo_stream.german.pws", b"audio");

        let dest = discard(&f, Some(&bin)).unwrap();
        assert!(!f.exists());
        assert_eq!(std::fs::read(&dest).unwrap(), b"audio");
    }

    #[test]
    fn moving_reports_a_missing_source_rather_than_claiming_success() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("not-there");
        assert!(discard(&missing, Some(&tmp.path().join("bin"))).is_err());
    }
}
