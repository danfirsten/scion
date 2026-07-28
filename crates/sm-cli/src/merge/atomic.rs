//! Replacing `%A` without ever leaving it half-written.
//!
//! SPEC.md §4.7: *"writing the result to the wrong path silently destroys
//! work."* `%A` is not a scratch file — during a real `git merge` it is a
//! temporary the merge machinery hands us, but during `git checkout --merge`,
//! `git stash apply` and a rerere replay it can be **the user's working-tree
//! file**, and its previous contents are the only copy of "ours" that exists
//! outside the object database. So the write has exactly one acceptable
//! failure mode: the file is either the old bytes or the new bytes, never a
//! prefix of the new ones and never missing.
//!
//! The recipe is the standard one and each step earns its place:
//!
//! 1. Create a uniquely named temporary **in the same directory** as the
//!    target, with `O_EXCL` so two concurrent drivers cannot collide. Same
//!    directory because [`std::fs::rename`] is only atomic within a filesystem,
//!    and `/tmp` routinely is not the same filesystem as a checkout.
//! 2. Write, then [`std::fs::File::sync_all`]. Without the fsync a crash
//!    between the rename and the writeback can leave the renamed file's *data*
//!    unwritten — the classic "zero-length file after a power cut".
//! 3. Copy the target's permission bits onto the temporary, so replacing an
//!    executable script does not clear its `+x` bit.
//! 4. `rename` over the target. On every platform we support this is atomic
//!    with respect to other processes reading the path.
//! 5. Best-effort fsync of the *directory*, which is what makes the rename
//!    itself durable. Best effort because it fails on some filesystems and a
//!    failure there is not worth losing a merge over.
//!
//! If any step before the rename fails, the temporary is deleted by
//! [`TempGuard`]'s `Drop` and the target has not been touched at all. That is
//! the property `crates/sm-cli/tests/cli_merge.rs` asserts by byte-comparing
//! `%A` before and after every failing run.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// Deletes its path unless [`TempGuard::keep`] is called.
struct TempGuard(Option<PathBuf>);

impl TempGuard {
    fn keep(mut self) -> PathBuf {
        self.0.take().expect("guard is armed exactly once")
    }
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

/// A process-local counter so two writes in one run cannot pick the same name.
static SEQ: AtomicU32 = AtomicU32::new(0);

/// Replace `target`'s contents with `bytes`, atomically.
///
/// On `Err` the target is guaranteed to be exactly as it was.
pub fn replace(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };

    let (mut file, tmp_path) = create_temp(dir)?;
    // Armed for the whole function: every `?` below unwinds through its `Drop`,
    // which is what guarantees a failed write leaves no debris and no partially
    // written target.
    let guard = TempGuard(Some(tmp_path));
    let tmp = guard.0.as_deref().expect("just armed");

    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);

    if let Ok(meta) = fs::metadata(target) {
        // Failing to carry the mode over is not worth aborting a merge for: the
        // contents are what matter and the rename still produces a correct
        // file. It only ever fails on filesystems without permission bits.
        let _ = fs::set_permissions(tmp, meta.permissions());
    }

    fs::rename(tmp, target)?;
    let _ = guard.keep(); // renamed away; there is nothing left to delete

    // Durability of the rename itself. Deliberately ignored on failure.
    let _ = File::open(dir).and_then(|d| d.sync_all());
    Ok(())
}

/// Create a fresh `O_EXCL` temporary next to the target.
fn create_temp(dir: &Path) -> io::Result<(File, PathBuf)> {
    let pid = std::process::id();
    for _ in 0..64 {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let path = dir.join(format!(".sm-merge.{pid}.{seq}.{nanos}.tmp"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create a temporary file next to the merge target",
    ))
}

#[cfg(test)]
mod tests {
    use super::replace;

    #[test]
    fn replacing_leaves_exactly_the_new_bytes() {
        let dir = tempdir();
        let target = dir.join("f.java");
        std::fs::write(&target, b"old\n").expect("seed");
        replace(&target, b"new\n").expect("replace");
        assert_eq!(std::fs::read(&target).expect("read"), b"new\n");
        assert_eq!(leftovers(&dir), Vec::<String>::new());
    }

    #[test]
    fn replacing_a_file_that_does_not_exist_creates_it() {
        let dir = tempdir();
        let target = dir.join("f.java");
        replace(&target, b"new\n").expect("replace");
        assert_eq!(std::fs::read(&target).expect("read"), b"new\n");
    }

    #[test]
    fn a_failure_to_create_the_temporary_leaves_the_target_alone() {
        let dir = tempdir();
        let target = dir.join("missing-dir").join("f.java");
        assert!(replace(&target, b"new\n").is_err());
        assert!(!target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_permission_bits_survive() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempdir();
        let target = dir.join("f.sh");
        std::fs::write(&target, b"old\n").expect("seed");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        replace(&target, b"new\n").expect("replace");
        let mode = std::fs::metadata(&target)
            .expect("stat")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755, "mode was {mode:o}");
    }

    #[test]
    fn no_temporary_survives_a_hundred_writes() {
        let dir = tempdir();
        let target = dir.join("f.java");
        for i in 0..100 {
            replace(&target, format!("{i}\n").as_bytes()).expect("replace");
        }
        assert_eq!(leftovers(&dir), Vec::<String>::new());
    }

    /// Names of everything in `dir` that is not the merge target.
    fn leftovers(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("read_dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".sm-merge"))
            .collect();
        names.sort();
        names
    }

    /// A scratch directory that outlives the test — these are tiny and the OS
    /// reclaims `TMPDIR`; pulling in a temp-dir crate for the *unit* tests
    /// would put it in the binary's dependency graph.
    fn tempdir() -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "sm-atomic-{}-{}",
            std::process::id(),
            super::SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&base).expect("mkdir");
        base
    }
}
