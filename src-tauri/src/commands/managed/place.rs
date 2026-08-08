//! The one way modkit replaces a file on disk.
//!
//! There were four, and only one of them was correct. The others shared a shape:
//! move the existing file aside, then write the new bytes straight over the live
//! path. That has a failure mode worth spelling out, because it destroys the thing
//! the backup exists to protect.
//!
//! ```text
//!   run 1:  pmc_bb.dll -> pmc_bb.dll.bak        (good copy saved)
//!           write pmc_bb.dll                    (interrupted — truncated file)
//!   run 2:  pmc_bb.dll -> pmc_bb.dll.bak        (truncated file OVERWRITES the good copy)
//!           write pmc_bb.dll                    (interrupted again)
//!   now:    both the live file and the backup are garbage
//! ```
//!
//! [`place`] writes to a temporary sibling and renames it into position, so the
//! destination only ever holds a complete file, and the snapshot is taken from
//! bytes that were verified first. The `.bak` sibling is kept for familiarity —
//! people look for it — but it is no longer the only copy: every displaced file is
//! also snapshotted into the recoverable trash under a content-addressed name.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};


/// What [`place`] did, for the ledger and for the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Placed {
    pub abs_path: String,
    /// sha256 of the bytes that are on disk now, re-read after the rename rather
    /// than assumed from the buffer — this is what makes "modified by hand"
    /// detectable later.
    pub sha256: String,
    pub size: u64,
    /// Where the displaced file was snapshotted, if there was one.
    pub backup: Option<String>,
}

/// An extra check a caller can demand before bytes are allowed onto disk.
pub enum Verifier {
    /// The file must carry a valid Authenticode signature naming this
    /// organisation, e.g. `Microsoft Corporation`. Windows only; on other hosts
    /// nothing is verified because nothing there can run the binary either.
    Authenticode { organization: &'static str },
}

pub struct PlaceOpts {
    /// Expected sha256, lowercase hex. When the forge published a digest this is
    /// the only end-to-end integrity check a bare release binary gets.
    pub expect_sha256: Option<String>,
    /// Reject a suspiciously small file. A forge error page served with a 200 is
    /// a few hundred bytes; a DLL is not.
    pub min_size: Option<u64>,
    /// Mark executable on unix.
    pub executable: bool,
    /// Also leave the displaced file at `<name>.bak`, the convention users have
    /// learned to look for.
    pub keep_bak_sibling: bool,
    pub verify: Option<Verifier>,
    /// Where displaced files are banked. `None` means the app's recoverable trash.
    ///
    /// Injectable for the same reason [`crate::commands::deploy_wad`]'s
    /// `PlacementStore` is: the code that writes into somebody's game install is
    /// exactly the code that must not be untestable because it hardcoded a path to
    /// the real one.
    pub backup_dir: Option<PathBuf>,
}

impl Default for PlaceOpts {
    fn default() -> Self {
        Self {
            expect_sha256: None,
            min_size: None,
            executable: false,
            keep_bak_sibling: true,
            verify: None,
            backup_dir: None,
        }
    }
}

impl PlaceOpts {
    pub fn executable(mut self) -> Self {
        self.executable = true;
        self
    }

    pub fn expecting(mut self, sha256: Option<&str>) -> Self {
        self.expect_sha256 = sha256.map(|s| s.to_ascii_lowercase());
        self
    }

    pub fn at_least(mut self, bytes: u64) -> Self {
        self.min_size = Some(bytes);
        self
    }

    pub fn verified_by(mut self, verifier: Verifier) -> Self {
        self.verify = Some(verifier);
        self
    }

    pub fn backing_up_into(mut self, dir: &Path) -> Self {
        self.backup_dir = Some(dir.to_path_buf());
        self
    }

    /// Skip the `<name>.bak` sibling. For destinations where nothing is ever
    /// displaced (a fresh version directory) or where a second copy beside the
    /// original would invite launching the wrong one.
    pub fn keeping_no_bak(mut self) -> Self {
        self.keep_bak_sibling = false;
        self
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    loadprobe::sha256::sha256_hex(bytes)
}

pub fn sha256_of_file(path: &Path) -> Result<String, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    Ok(sha256_hex(&bytes))
}

/// Move `src` into `into` (default: the recoverable trash), content-addressed.
///
/// See [`super::trash::bank`] — the digest keying is what keeps the *original*
/// recoverable after the tenth reinstall instead of burying it under nine
/// identical intermediates.
pub fn snapshot(src: &Path, into: Option<&Path>) -> Result<PathBuf, String> {
    super::trash::bank(src, into)
}

/// Write `bytes` to `dest`, atomically, snapshotting whatever was there.
///
/// Order matters and is the whole design: verify the incoming bytes, stage them,
/// snapshot the outgoing file, then swap. Nothing touches the destination until
/// there is a complete, checked replacement ready to take its place.
pub fn place(dest: &Path, bytes: &[u8], opts: PlaceOpts) -> Result<Placed, String> {
    let label = dest
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("the file")
        .to_string();

    if let Some(min) = opts.min_size {
        if (bytes.len() as u64) < min {
            return Err(format!(
                "Refusing to install {label}: got {} bytes, expected at least {min}. \
                 That is the size of an error page, not an artifact.",
                bytes.len()
            ));
        }
    }

    let digest = sha256_hex(bytes);
    if let Some(want) = &opts.expect_sha256 {
        if &digest != want {
            return Err(format!(
                "Refusing to install {label}: its sha256 is {digest}, but the release \
                 publishes {want}. The download was corrupted or the artifact was changed."
            ));
        }
    }

    let dir = dest
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", dest.display()))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;

    // Staged in the destination's own directory so the rename is same-volume, and
    // therefore atomic. A temp dir elsewhere would silently degrade to copy.
    let staged = dir.join(format!(".{label}.part"));
    let _ = std::fs::remove_file(&staged);
    std::fs::write(&staged, bytes)
        .map_err(|e| format!("Could not stage {label}: {e}"))?;

    let result = (|| -> Result<Placed, String> {
        #[cfg(unix)]
        if opts.executable {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&staged)
                .map_err(|e| format!("Could not stat the staged {label}: {e}"))?
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&staged, perms)
                .map_err(|e| format!("Could not mark {label} executable: {e}"))?;
        }

        if let Some(v) = &opts.verify {
            verify(&staged, v)?;
        }

        // Only now is the destination touched.
        let backup = if dest.exists() {
            let snap = snapshot(dest, opts.backup_dir.as_deref())?;
            if opts.keep_bak_sibling {
                // Best-effort: the recoverable copy is already banked, so failing
                // to also leave a sibling is not worth aborting an install over.
                let _ = std::fs::copy(&snap, sibling_bak(dest));
            }
            Some(snap.to_string_lossy().to_string())
        } else {
            None
        };

        std::fs::rename(&staged, dest)
            .map_err(|e| format!("Could not install {label}: {e}"))?;

        // Re-read rather than trusting the buffer: this is the value the ledger
        // stores, and "what is actually on disk" is the question it must answer.
        let on_disk = sha256_of_file(dest)?;
        if on_disk != digest {
            return Err(format!(
                "Installed {label} does not match what was written (expected {digest}, \
                 found {on_disk}) — something else is writing to this folder."
            ));
        }

        Ok(Placed {
            abs_path: dest.to_string_lossy().to_string(),
            sha256: on_disk,
            size: bytes.len() as u64,
            backup,
        })
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    result
}

/// `foo.dll` -> `foo.dll.bak`.
///
/// Appended, not substituted. `Path::with_extension("dll.bak")` — which two of the
/// four call sites used — replaces the existing extension, so it happens to be
/// right for `foo.dll` and wrong for anything with a different one.
fn sibling_bak(dest: &Path) -> PathBuf {
    let mut name = dest
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".bak");
    dest.with_file_name(name)
}

#[cfg(not(target_os = "windows"))]
fn verify(_path: &Path, _v: &Verifier) -> Result<(), String> {
    // Authenticode is a Windows concept and the binaries it guards only run there.
    Ok(())
}

/// Confirm `path` carries a valid Authenticode signature issued to `organization`,
/// via PowerShell's `Get-AuthenticodeSignature`. The path is passed through an env
/// var to sidestep all command-line quoting concerns.
#[cfg(target_os = "windows")]
fn verify(path: &Path, v: &Verifier) -> Result<(), String> {
    use crate::commands::proc::NoWindow;

    let Verifier::Authenticode { organization } = v;
    let script = format!(
        "$ErrorActionPreference='Stop'; \
         $s = Get-AuthenticodeSignature -FilePath $env:MODKIT_VERIFY_PATH; \
         if ($s.Status -ne 'Valid') {{ Write-Error \"signature status is $($s.Status)\"; exit 2 }}; \
         if ($s.SignerCertificate.Subject -notmatch 'O={organization}') {{ \
            Write-Error \"unexpected signer $($s.SignerCertificate.Subject)\"; exit 3 }}"
    );

    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script.as_str(),
        ])
        .env("MODKIT_VERIFY_PATH", path)
        .no_window()
        .output()
        .map_err(|e| format!("Failed to run PowerShell: {e}"))?;

    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Refusing to install the download — could not confirm it is signed by \
             {organization}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bak_suffix_is_appended_not_substituted() {
        assert_eq!(sibling_bak(Path::new("/g/pmc_bb.dll")), Path::new("/g/pmc_bb.dll.bak"));
        // with_extension("dll.bak") would have produced `dxwrapper.dll.bak` here.
        assert_eq!(
            sibling_bak(Path::new("/g/dxwrapper.ini")),
            Path::new("/g/dxwrapper.ini.bak")
        );
        assert_eq!(sibling_bak(Path::new("/g/noext")), Path::new("/g/noext.bak"));
    }

    /// A game folder and a bank, both hermetic — nothing here touches the real
    /// app-data trash.
    struct Fixture {
        _tmp: tempfile::TempDir,
        game: PathBuf,
        bank: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let game = tmp.path().join("game");
            let bank = tmp.path().join("bank");
            std::fs::create_dir_all(&game).unwrap();
            Self { _tmp: tmp, game, bank }
        }

        fn opts(&self) -> PlaceOpts {
            PlaceOpts::default().backing_up_into(&self.bank)
        }

        fn banked(&self) -> Vec<Vec<u8>> {
            std::fs::read_dir(&self.bank)
                .map(|entries| {
                    entries
                        .flatten()
                        .filter_map(|e| std::fs::read(e.path()).ok())
                        .collect()
                })
                .unwrap_or_default()
        }
    }

    #[test]
    fn placing_writes_the_bytes_and_reports_their_digest() {
        let f = Fixture::new();
        let dest = f.game.join("pmc_bb.dll");

        let placed = place(&dest, b"new bytes", f.opts()).expect("places");
        assert_eq!(std::fs::read(&dest).unwrap(), b"new bytes");
        assert_eq!(placed.sha256, sha256_hex(b"new bytes"));
        assert_eq!(placed.size, 9);
        assert!(placed.backup.is_none(), "nothing was displaced");
    }

    #[test]
    fn a_displaced_file_is_snapshotted_and_recoverable() {
        let f = Fixture::new();
        let dest = f.game.join("pmc_bb.dll");
        std::fs::write(&dest, b"the good original").unwrap();

        let placed = place(&dest, b"replacement", f.opts()).expect("places");
        let snap = placed.backup.expect("a snapshot was taken");
        assert_eq!(std::fs::read(&snap).unwrap(), b"the good original");
        assert_eq!(
            std::fs::read(f.game.join("pmc_bb.dll.bak")).unwrap(),
            b"the good original",
            "the familiar sibling is kept too"
        );
    }

    /// The bug this module exists to kill. Under the old write-over-the-live-path
    /// shape, a second install moved the *previous run's output* into the backup
    /// slot; if that output was a partial write, the only good copy was gone.
    /// Content-addressed snapshots mean the original's bytes stay banked under
    /// their own digest no matter how many times this runs.
    #[test]
    fn repeated_installs_never_destroy_the_original_snapshot() {
        let f = Fixture::new();
        let dest = f.game.join("pmc_bb.dll");
        std::fs::write(&dest, b"the good original").unwrap();

        place(&dest, b"v2", f.opts()).unwrap();
        place(&dest, b"v3", f.opts()).unwrap();
        place(&dest, b"v4", f.opts()).unwrap();

        let banked = f.banked();
        assert!(
            banked.iter().any(|b| b == b"the good original"),
            "the pre-modkit original is gone from the bank after three installs"
        );
        assert_eq!(
            banked.iter().filter(|b| *b == b"the good original").count(),
            1,
            "banked content-addressed, so identical bytes are stored once"
        );
    }

    #[test]
    fn a_digest_mismatch_refuses_before_touching_the_destination() {
        let f = Fixture::new();
        let dest = f.game.join("tool");
        std::fs::write(&dest, b"incumbent").unwrap();

        let err = place(
            &dest,
            b"impostor",
            f.opts().expecting(Some(&sha256_hex(b"expected"))),
        )
        .unwrap_err();

        assert!(err.contains("sha256"), "{err}");
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            b"incumbent",
            "the existing file must be untouched when the incoming bytes are rejected"
        );
        assert!(f.banked().is_empty(), "nothing should have been displaced");
    }

    #[test]
    fn a_matching_digest_is_accepted() {
        let f = Fixture::new();
        let dest = f.game.join("tool");
        let opts = f.opts().expecting(Some(&sha256_hex(b"genuine")));
        place(&dest, b"genuine", opts).expect("the digest matches");
        assert_eq!(std::fs::read(&dest).unwrap(), b"genuine");
    }

    #[test]
    fn an_undersized_download_is_refused() {
        let f = Fixture::new();
        let dest = f.game.join("pmc_bb.dll");
        std::fs::write(&dest, b"incumbent").unwrap();

        let err = place(&dest, b"404 not found", f.opts().at_least(4096)).unwrap_err();
        assert!(err.contains("error page"), "{err}");
        assert_eq!(std::fs::read(&dest).unwrap(), b"incumbent");
    }

    #[test]
    fn no_part_file_survives_a_rejection() {
        let f = Fixture::new();
        let dest = f.game.join("tool");
        let _ = place(
            &dest,
            b"nope",
            f.opts().expecting(Some(&sha256_hex(b"other"))),
        );
        let leftovers: Vec<_> = std::fs::read_dir(&f.game)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".part"))
            .collect();
        assert!(leftovers.is_empty(), "left behind {leftovers:?}");
    }

    #[test]
    fn the_bak_sibling_can_be_declined() {
        let f = Fixture::new();
        let dest = f.game.join("vz-patch.wad");
        std::fs::write(&dest, b"old").unwrap();

        let opts = PlaceOpts {
            keep_bak_sibling: false,
            ..f.opts()
        };
        let placed = place(&dest, b"new", opts).expect("places");
        assert!(placed.backup.is_some(), "still snapshotted");
        assert!(
            !f.game.join("vz-patch.wad.bak").exists(),
            "no sibling was asked for"
        );
    }

    #[test]
    fn a_missing_destination_directory_is_created() {
        let f = Fixture::new();
        let dest = f.game.join("scripts").join("nested").join("mod.asi");
        place(&dest, b"plugin", f.opts()).expect("places");
        assert_eq!(std::fs::read(&dest).unwrap(), b"plugin");
    }
}
