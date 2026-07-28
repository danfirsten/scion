//! Thin wrapper over the `git` command line.
//!
//! The miner shells out rather than linking a git library (SPEC §3 explicitly
//! allows this). Everything here is deliberately dumb: run a command in a
//! repository, hand back stdout as bytes, and never panic on a git that
//! returns non-zero — the caller decides whether that is fatal.
//!
//! Two properties matter for the corpus to be reproducible:
//!
//! * **No worktree is ever created.** All history walking and the merge replay
//!   itself run against a bare clone via `git merge-tree --write-tree`, which
//!   performs a real merge-ort merge in memory. This is what makes mining a
//!   repository with 40k merge commits practical.
//! * **Bytes in, bytes out.** Blob contents are `Vec<u8>` and paths are handled
//!   as `String` only because git's `-z` output gives us unquoted UTF-8 path
//!   bytes; non-UTF-8 paths are rejected rather than lossily decoded.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

/// Raw result of running a git subprocess.
pub struct GitOutput {
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl GitOutput {
    pub fn ok(&self) -> bool {
        self.status == Some(0)
    }

    /// stdout as UTF-8 with trailing newline removed.
    pub fn stdout_trimmed(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim_end().to_string()
    }
}

/// A bare git repository on disk.
#[derive(Clone, Debug)]
pub struct Repo {
    dir: PathBuf,
}

impl Repo {
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Run `git <args>` inside the repository, returning whatever it produced.
    pub fn run<I, S>(&self, args: I) -> Result<GitOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        run_git_in(Some(&self.dir), args)
    }

    /// Run `git <args>` and fail if git did not exit 0.
    pub fn run_ok<I, S>(&self, args: I) -> Result<GitOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<String> = args
            .into_iter()
            .map(|a| a.as_ref().to_string_lossy().into_owned())
            .collect();
        let out = self.run(&args)?;
        if !out.ok() {
            bail!(
                "git {} failed (status {:?}): {}",
                args.join(" "),
                out.status,
                out.stderr.trim()
            );
        }
        Ok(out)
    }

    /// `git rev-parse --verify <rev>`; `None` when the revision does not exist.
    pub fn rev_parse(&self, rev: &str) -> Result<Option<String>> {
        let out = self.run(["rev-parse", "--verify", "--quiet", rev])?;
        if !out.ok() {
            return Ok(None);
        }
        let s = out.stdout_trimmed();
        Ok(if s.is_empty() { None } else { Some(s) })
    }

    /// `git merge-base a b`; `None` when the two commits share no ancestor.
    pub fn merge_base(&self, a: &str, b: &str) -> Result<Option<String>> {
        let out = self.run(["merge-base", a, b])?;
        if !out.ok() {
            return Ok(None);
        }
        let s = out.stdout_trimmed();
        // Multiple merge bases (criss-cross history) are reported one per line
        // by `--all` only; plain merge-base already picks one deterministically.
        Ok(if s.is_empty() {
            None
        } else {
            s.lines().next().map(|l| l.trim().to_string())
        })
    }

    /// Two-parent merge commits, newest first.
    pub fn two_parent_merges(&self, limit: Option<usize>) -> Result<Vec<String>> {
        let out = self.run_ok([
            "log",
            "--merges",
            "--min-parents=2",
            "--max-parents=2",
            "--format=%H",
        ])?;
        let mut v: Vec<String> = out
            .stdout_trimmed()
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        if let Some(n) = limit
            && v.len() > n
        {
            v.truncate(n);
        }
        Ok(v)
    }

    /// The two parents of a merge commit.
    pub fn parents(&self, commit: &str) -> Result<Vec<String>> {
        let out = self.run_ok(["rev-list", "--parents", "-n", "1", commit])?;
        let line = out.stdout_trimmed();
        let mut it = line.split_whitespace();
        let _self = it.next();
        Ok(it.map(|s| s.to_string()).collect())
    }

    /// Entry for `path` under `tree_ish`, as `(mode, oid)`.
    ///
    /// `None` means the path does not exist there — which is the normal answer
    /// for add/add and delete/modify cases, not an error.
    pub fn tree_entry(&self, tree_ish: &str, path: &str) -> Result<Option<(String, String)>> {
        let out = self.run(["ls-tree", "-z", tree_ish, "--", path])?;
        if !out.ok() {
            return Ok(None);
        }
        // `<mode> SP <type> SP <oid> TAB <path> NUL`
        let record = out.stdout.split(|b| *b == 0).find(|r| !r.is_empty());
        let Some(record) = record else {
            return Ok(None);
        };
        let record = std::str::from_utf8(record).context("ls-tree output is not UTF-8")?;
        let (meta, _) = record
            .split_once('\t')
            .ok_or_else(|| anyhow!("ls-tree record has no TAB: {record:?}"))?;
        let fields: Vec<&str> = meta.split_whitespace().collect();
        if fields.len() < 3 {
            bail!("ls-tree record is malformed: {record:?}");
        }
        Ok(Some((fields[0].to_string(), fields[2].to_string())))
    }

    /// Size in bytes of a blob, without reading it.
    pub fn blob_size(&self, oid: &str) -> Result<Option<u64>> {
        let out = self.run(["cat-file", "-s", oid])?;
        if !out.ok() {
            return Ok(None);
        }
        Ok(out.stdout_trimmed().parse::<u64>().ok())
    }

    /// Contents of a blob.
    pub fn blob(&self, oid: &str) -> Result<Option<Vec<u8>>> {
        let out = self.run(["cat-file", "blob", oid])?;
        if !out.ok() {
            return Ok(None);
        }
        Ok(Some(out.stdout))
    }

    /// Paths changed between two commits, restricted to a pathspec.
    pub fn changed_paths(&self, from: &str, to: &str, pathspec: &str) -> Result<Vec<String>> {
        let out = self.run(["diff", "--name-only", "-z", from, to, "--", pathspec])?;
        if !out.ok() {
            return Ok(Vec::new());
        }
        let mut v = Vec::new();
        for field in out.stdout.split(|b| *b == 0) {
            if field.is_empty() {
                continue;
            }
            match std::str::from_utf8(field) {
                Ok(s) => v.push(s.to_string()),
                // A non-UTF-8 path is not something we can name in a case id.
                Err(_) => continue,
            }
        }
        Ok(v)
    }

    /// Replay the merge of `ours` and `theirs` in memory.
    pub fn merge_tree(&self, ours: &str, theirs: &str) -> Result<MergeTreeResult> {
        let out = self.run([
            "merge-tree",
            "--write-tree",
            "--name-only",
            "-z",
            ours,
            theirs,
        ])?;
        match out.status {
            Some(0) | Some(1) => {}
            // Anything else means git could not even start the merge; the man
            // page says the output is unspecified in that case.
            _ => {
                return Ok(MergeTreeResult::Unusable {
                    detail: format!("status {:?}: {}", out.status, out.stderr.trim()),
                });
            }
        }
        parse_merge_tree_z(&out.stdout)
    }
}

/// Parsed `git merge-tree --write-tree --name-only -z` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeTreeResult {
    /// The merge completed; `conflicts` is empty for a clean merge.
    Merged {
        tree: String,
        conflicts: Vec<String>,
        messages: Vec<ConflictMessage>,
    },
    /// git could not produce a merge, or its output could not be parsed.
    Unusable { detail: String },
}

/// One entry of merge-tree's informational-message section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictMessage {
    pub paths: Vec<String>,
    pub kind: String,
    pub message: String,
}

/// Parse the `-z` form documented in git-merge-tree(1).
///
/// ```text
/// <tree-oid> NUL
/// ( <path> NUL )*          -- conflicted file info (--name-only)
/// NUL                      -- section terminator, absent on a clean merge
/// ( <n> NUL (<path> NUL){n} <kind> NUL <message> NUL )*
/// ```
pub fn parse_merge_tree_z(stdout: &[u8]) -> Result<MergeTreeResult> {
    let mut fields = stdout.split(|b| *b == 0);
    let Some(tree) = fields.next() else {
        return Ok(MergeTreeResult::Unusable {
            detail: "empty merge-tree output".into(),
        });
    };
    let tree = String::from_utf8_lossy(tree).trim().to_string();
    if tree.len() != 40 || !tree.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(MergeTreeResult::Unusable {
            detail: format!("merge-tree did not start with a tree oid: {tree:?}"),
        });
    }

    let mut conflicts = Vec::new();
    let mut saw_separator = false;
    let mut rest: Vec<&[u8]> = Vec::new();
    for field in fields.by_ref() {
        if field.is_empty() {
            // Either the section separator or the trailing empty field that
            // follows the last NUL of a clean merge.
            if saw_separator {
                continue;
            }
            saw_separator = true;
            continue;
        }
        if saw_separator {
            rest.push(field);
        } else {
            match std::str::from_utf8(field) {
                Ok(s) => conflicts.push(s.to_string()),
                Err(_) => {
                    return Ok(MergeTreeResult::Unusable {
                        detail: "conflicted path is not UTF-8".into(),
                    });
                }
            }
        }
    }

    // Informational messages are best-effort: if the shape is not what we
    // expect we keep what we parsed rather than discarding a usable merge.
    let mut messages = Vec::new();
    let mut i = 0usize;
    while i < rest.len() {
        let Ok(count) = String::from_utf8_lossy(rest[i]).trim().parse::<usize>() else {
            break;
        };
        // Need paths at i+1..=i+count, the kind at i+1+count and the message
        // at i+2+count.
        if i + count + 2 >= rest.len() {
            break;
        }
        let mut paths = Vec::with_capacity(count);
        for k in 0..count {
            paths.push(String::from_utf8_lossy(rest[i + 1 + k]).into_owned());
        }
        let kind = String::from_utf8_lossy(rest[i + 1 + count]).into_owned();
        let message = String::from_utf8_lossy(rest[i + 2 + count])
            .trim_end()
            .to_string();
        messages.push(ConflictMessage {
            paths,
            kind,
            message,
        });
        i += count + 3;
    }

    Ok(MergeTreeResult::Merged {
        tree,
        conflicts,
        messages,
    })
}

/// Run git in `dir` (or the current directory when `None`).
pub fn run_git_in<I, S>(dir: Option<&Path>, args: I) -> Result<GitOutput>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new("git");
    if let Some(d) = dir {
        cmd.arg("-C").arg(d);
    }
    cmd.args(args);
    // Keep mining reproducible and non-interactive regardless of the ambient
    // user configuration.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    cmd.env("GIT_ASKPASS", "true");
    cmd.env("LC_ALL", "C");
    cmd.stdin(Stdio::null());
    let out = cmd.output().context("failed to spawn git")?;
    Ok(GitOutput {
        status: out.status.code(),
        stdout: out.stdout,
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// Bare-clone `url` into `dest`, giving up after `timeout`.
///
/// `--single-branch` keeps us on the default branch (SPEC §6.1 walks one
/// history, and release/maintenance branches would otherwise multiply the merge
/// count without adding independent cases).
pub fn bare_clone(url: &str, dest: &Path, timeout: Duration) -> Result<()> {
    let mut child = Command::new("git")
        .arg("clone")
        .arg("--bare")
        .arg("--single-branch")
        .arg("--quiet")
        .arg(url)
        .arg(dest)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_ASKPASS", "true")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn git clone")?;

    let started = Instant::now();
    loop {
        match child.try_wait().context("waiting on git clone")? {
            Some(status) => {
                if status.success() {
                    return Ok(());
                }
                let mut stderr = String::new();
                if let Some(mut e) = child.stderr.take() {
                    use std::io::Read;
                    let _ = e.read_to_string(&mut stderr);
                }
                bail!("git clone exited {:?}: {}", status.code(), stderr.trim());
            }
            None => {
                if started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    bail!(
                        "git clone exceeded the {}s budget",
                        timeout.as_secs().max(1)
                    );
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_clean_merge() {
        let out = b"f41974d7767c34d198a40b1c22dfcab313cae51b\0";
        let r = parse_merge_tree_z(out).unwrap();
        assert_eq!(
            r,
            MergeTreeResult::Merged {
                tree: "f41974d7767c34d198a40b1c22dfcab313cae51b".into(),
                conflicts: vec![],
                messages: vec![],
            }
        );
    }

    #[test]
    fn parses_a_conflicted_merge() {
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"28b806852f916a8b4d1491c583ea1b54dfaac8b1\0");
        out.extend_from_slice(b"a/A.java\0");
        out.extend_from_slice(b"b/B.java\0");
        out.extend_from_slice(b"\0");
        out.extend_from_slice(b"1\0a/A.java\0Auto-merging\0Auto-merging a/A.java\n\0");
        out.extend_from_slice(
            b"1\0a/A.java\0CONFLICT (contents)\0CONFLICT (content): Merge conflict in a/A.java\n\0",
        );
        let MergeTreeResult::Merged {
            tree,
            conflicts,
            messages,
        } = parse_merge_tree_z(&out).unwrap()
        else {
            panic!("expected a merged result");
        };
        assert_eq!(tree, "28b806852f916a8b4d1491c583ea1b54dfaac8b1");
        assert_eq!(conflicts, vec!["a/A.java", "b/B.java"]);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].kind, "CONFLICT (contents)");
        assert_eq!(messages[1].paths, vec!["a/A.java"]);
    }

    #[test]
    fn rejects_garbage() {
        let r = parse_merge_tree_z(b"not-a-tree\0").unwrap();
        assert!(matches!(r, MergeTreeResult::Unusable { .. }));
        let r = parse_merge_tree_z(b"").unwrap();
        assert!(matches!(r, MergeTreeResult::Unusable { .. }));
    }

    #[test]
    fn tolerates_a_truncated_message_section() {
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"28b806852f916a8b4d1491c583ea1b54dfaac8b1\0");
        out.extend_from_slice(b"a/A.java\0");
        out.extend_from_slice(b"\0");
        out.extend_from_slice(b"2\0a/A.java\0");
        let MergeTreeResult::Merged {
            conflicts,
            messages,
            ..
        } = parse_merge_tree_z(&out).unwrap()
        else {
            panic!("expected a merged result");
        };
        assert_eq!(conflicts, vec!["a/A.java"]);
        assert!(messages.is_empty());
    }
}
