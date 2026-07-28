//! The bottom of the fallback ladder: git's own line merge.
//!
//! # Why a subprocess and not a diff3 crate
//!
//! The alternative was to implement diff3 in-process over `imara-diff` or
//! `diffy`. We shell out to `git merge-file` instead, deliberately:
//!
//! - **The fallback's job is to be indistinguishable from not installing us.**
//!   SPEC.md §0.4 makes the line merge the answer whenever we are unsure, and
//!   the only way to guarantee that answer is byte-for-byte what the user would
//!   have got is to ask the same program. A reimplementation would differ on
//!   conflict-hunk boundaries, on the `--diff-algorithm` git is configured with,
//!   and on `merge.conflictStyle` — three ways to be subtly worse in exactly the
//!   situation where we have already admitted we do not understand the file.
//! - **It is a dependency we already have.** The driver is invoked *by* git.
//!   `git merge-file` cannot be missing in any environment where we run.
//! - **The cost is one `fork`/`exec` (~2–4 ms).** Which is real, and is why the
//!   fast path (§ the `merge` module docs) is worth having: that same
//!   subprocess result is what the fast path returns when it is clean, so the
//!   common case pays for the process once and never parses anything.
//!
//! # `-p`, and why `%A` is safe
//!
//! `git merge-file` without `-p` writes the result **into its first argument**,
//! which for us would be `%A` — a non-atomic in-place overwrite, precisely the
//! thing this driver must never do. `-p` sends the result to standard output
//! instead and leaves all three files untouched, so the bytes reach `%A` only
//! through [`super::atomic::replace`]. That is the whole reason for the flag.
//!
//! # Exit codes
//!
//! `git merge-file` exits with *the number of conflict hunks*, truncated at
//! 127, and with a value ≥ 128 (or a signal) on a real error. So `0` means
//! clean, `1..=127` means conflicts, anything else means the fallback itself
//! failed and the caller must leave `%A` alone.

use std::path::Path;
use std::process::Command;

/// A successful line merge.
pub struct LineMerge {
    /// The merged file.
    pub bytes: Vec<u8>,
    /// Number of conflict hunks git reported (0 means clean).
    pub conflict_hunks: u32,
}

impl LineMerge {
    pub const fn is_clean(&self) -> bool {
        self.conflict_hunks == 0
    }
}

/// Why the line merge could not be trusted.
#[derive(Debug)]
pub enum LineMergeError {
    /// The fallback command could not be started at all.
    Spawn(std::io::Error),
    /// It ran and reported failure.
    Failed { status: String, stderr: String },
}

impl std::fmt::Display for LineMergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(err) => write!(f, "could not run the fallback merge command: {err}"),
            Self::Failed { status, stderr } => {
                write!(f, "the fallback merge command failed ({status})")?;
                let trimmed = stderr.trim();
                if !trimmed.is_empty() {
                    write!(f, ": {trimmed}")?;
                }
                Ok(())
            }
        }
    }
}

/// The three conflict-marker labels, in git's `%X` / `%S` / `%Y` order.
pub struct Labels<'a> {
    pub ours: &'a str,
    pub base: &'a str,
    pub theirs: &'a str,
}

/// Run `<cmd> merge-file -p` over the three revisions.
///
/// `ours`/`base`/`theirs` are read and never written.
pub fn run(
    cmd: &str,
    ours: &Path,
    base: &Path,
    theirs: &Path,
    marker_size: usize,
    labels: &Labels<'_>,
    diff3: bool,
) -> Result<LineMerge, LineMergeError> {
    let mut command = Command::new(cmd);
    command
        .arg("merge-file")
        .arg("-p")
        .arg(format!("--marker-size={marker_size}"));
    if diff3 {
        command.arg("--diff3");
    }
    // `-L` applies to the three files in the order they are given.
    command
        .arg("-L")
        .arg(labels.ours)
        .arg("-L")
        .arg(labels.base)
        .arg("-L")
        .arg(labels.theirs)
        .arg(ours)
        .arg(base)
        .arg(theirs);

    let output = command.output().map_err(LineMergeError::Spawn)?;
    match output.status.code() {
        Some(code @ 0..=127) => Ok(LineMerge {
            bytes: output.stdout,
            conflict_hunks: u32::try_from(code).expect("0..=127 fits"),
        }),
        _ => Err(LineMergeError::Failed {
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }),
    }
}
