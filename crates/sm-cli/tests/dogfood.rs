//! M4's exit criterion, as a test: *"installed as your own merge driver; you
//! have used it on a real repo and it has not lost data."*
//!
//! Everything here goes through **`git merge`**, not through `sm merge`. That
//! is the point. `cli_merge.rs` tests the driver's contract by calling it the
//! way git would; this file tests that git actually calls it that way — that
//! the `.gitattributes` rule matches, that the config line git parses produces
//! the argument list `sm merge` expects, that `%A` is where git looks for the
//! result afterwards, and that the exit code means to git what we think it
//! means.
//!
//! Every one of those is a place where a driver can be subtly wrong while all
//! its own tests pass.
//!
//! Three scenarios, chosen to cover the three outcomes that matter:
//!
//! | | what the branches do | git alone | with the driver |
//! |---|---|---|---|
//! | (a) | both add an import | conflict | **clean** |
//! | (b) | both rewrite the same line | conflict | conflict, with correct markers |
//! | (c) | one moves a method, the other edits its body | conflict | **clean, both changes applied** |
//!
//! (a) and (c) are the project's reason to exist. (b) is the guard rail: a tool
//! that resolves everything is not a merge driver, it is a random number
//! generator, and SPEC.md §0.4 is explicit that a conflict is always an
//! acceptable answer.
//!
//! # A note on git's version here
//!
//! This runs against whatever `git` is installed. On git older than 2.44 the
//! `%S` / `%X` / `%Y` placeholders are **not expanded** and the driver receives
//! the literal strings (docs/prior-art.md §2.9). The development machine runs
//! git 2.43, so the assertion in `a_real_conflict_keeps_correct_markers` that no
//! `%` placeholder reaches the file is a live test of that fallback rather than
//! a hypothetical one. On a newer git the same assertion passes for the other
//! reason — real labels — which is why it is written as "no placeholder
//! survives" rather than "the label is X".

use std::path::Path;
use std::process::{Command, Output};

// ------------------------------------------------------------------ harness

struct Repo {
    dir: tempfile::TempDir,
    log: std::cell::RefCell<Vec<String>>,
}

impl Repo {
    /// A repository with the driver installed and `*.java merge=semantic` set.
    fn new() -> Self {
        let repo = Self {
            dir: tempfile::tempdir().expect("tempdir"),
            log: std::cell::RefCell::new(Vec::new()),
        };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.email", "dogfood@example.invalid"]);
        repo.git(&["config", "user.name", "Dogfood"]);
        // No GPG, no hooks, no user config leaking in.
        repo.git(&["config", "commit.gpgsign", "false"]);

        let attrs = repo.path().join(".gitattributes");
        let out = repo.sm(&[
            "install-driver",
            "--local",
            "--langs",
            "java,ts,tsx",
            "--write-attributes",
            attrs.to_str().expect("utf-8"),
        ]);
        assert!(
            out.status.success(),
            "install-driver failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        repo.write(
            ".gitattributes",
            &std::fs::read_to_string(&attrs).expect("read"),
        );
        repo.git(&["add", "."]);
        repo.git(&["commit", "-qm", "attributes"]);
        repo
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write(&self, name: &str, text: &str) {
        let path = self.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, text).expect("write");
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.path().join(name)).expect("read")
    }

    fn sm(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sm"))
            .current_dir(self.path())
            .args(args)
            .output()
            .expect("running sm")
    }

    /// Run git, requiring success.
    fn git(&self, args: &[&str]) -> Output {
        let out = self.git_allow_failure(args);
        assert!(
            out.status.success(),
            "git {args:?} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    /// Run git, returning whatever happened. Used for `merge`, which is
    /// supposed to fail sometimes.
    fn git_allow_failure(&self, args: &[&str]) -> Output {
        let out = Command::new("git")
            .current_dir(self.path())
            .args(args)
            // `GIT_CONFIG_NOSYSTEM` and an empty HOME keep a developer's global
            // config — merge tools, diff drivers, `merge.conflictStyle` — from
            // changing what this test measures.
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", self.path())
            .env("XDG_CONFIG_HOME", self.path())
            .output()
            .expect("running git");
        self.log.borrow_mut().push(format!(
            "$ git {}\n{}{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
        out
    }

    fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-qm", message]);
    }

    /// Build `base` on `main`, then a branch per side, then merge `theirs` into
    /// `ours`. Returns the merge's exit status.
    fn three_way(&self, file: &str, base: &str, ours: &str, theirs: &str) -> Output {
        self.write(file, base);
        self.commit_all("base");
        self.git(&["checkout", "-q", "-b", "theirs"]);
        self.write(file, theirs);
        self.commit_all("theirs");
        self.git(&["checkout", "-q", "main"]);
        self.write(file, ours);
        self.commit_all("ours");
        self.git_allow_failure(&["merge", "--no-edit", "theirs"])
    }

    /// The recorded transcript, for a failure message.
    fn transcript(&self) -> String {
        self.log.borrow().join("\n")
    }
}

/// The same three revisions, merged in a repository with **no** driver, so
/// every claim of the form "git would have conflicted" is measured rather than
/// asserted from memory.
fn plain_git_merge(file: &str, base: &str, ours: &str, theirs: &str) -> bool {
    let dir = tempfile::tempdir().expect("tempdir");
    let run = |args: &[&str]| {
        Command::new("git")
            .current_dir(dir.path())
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", dir.path())
            .output()
            .expect("git")
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "t@example.invalid"]);
    run(&["config", "user.name", "T"]);
    let write = |text: &str| std::fs::write(dir.path().join(file), text).expect("write");
    write(base);
    run(&["add", "-A"]);
    run(&["commit", "-qm", "base"]);
    run(&["checkout", "-q", "-b", "theirs"]);
    write(theirs);
    run(&["add", "-A"]);
    run(&["commit", "-qm", "theirs"]);
    run(&["checkout", "-q", "main"]);
    write(ours);
    run(&["add", "-A"]);
    run(&["commit", "-qm", "ours"]);
    !run(&["merge", "--no-edit", "theirs"]).status.success()
}

fn has_markers(text: &str) -> bool {
    text.lines().any(|l| l.starts_with("<<<<<<<"))
}

fn parses_cleanly(text: &str) -> bool {
    let lang = sm_cst::languages::detect(Path::new("x.java")).expect("java");
    sm_cst::parse(text.as_bytes(), lang).is_ok_and(|t| !t.has_errors())
}

// ------------------------------------------------------------------ the cases

const IMPORTS_BASE: &str = "\
package app;

import java.util.List;

class Service {
  void run(List xs) {
    process(xs);
  }
}
";
const IMPORTS_OURS: &str = "\
package app;

import java.util.List;
import java.util.Map;

class Service {
  void run(List xs) {
    process(xs);
  }
}
";
const IMPORTS_THEIRS: &str = "\
package app;

import java.util.List;
import java.util.Set;

class Service {
  void run(List xs) {
    process(xs);
  }
}
";

/// (a) The import collision. Git conflicts on this every single time; it is the
/// example SPEC.md §1 opens with.
#[test]
fn an_import_collision_git_cannot_merge_merges_cleanly() {
    assert!(
        plain_git_merge("Service.java", IMPORTS_BASE, IMPORTS_OURS, IMPORTS_THEIRS),
        "the fixture is not a real git conflict; the test would prove nothing"
    );

    let repo = Repo::new();
    let out = repo.three_way("Service.java", IMPORTS_BASE, IMPORTS_OURS, IMPORTS_THEIRS);
    assert!(
        out.status.success(),
        "the merge should have succeeded:\n{}",
        repo.transcript()
    );

    let merged = repo.read("Service.java");
    assert!(!has_markers(&merged), "{merged}");
    assert!(
        merged.contains("import java.util.Map;"),
        "lost ours:\n{merged}"
    );
    assert!(
        merged.contains("import java.util.Set;"),
        "lost theirs:\n{merged}"
    );
    assert!(
        merged.contains("import java.util.List;"),
        "lost the ancestor:\n{merged}"
    );
    assert!(parses_cleanly(&merged), "{merged}");

    // git agrees the working tree is clean and the merge is recorded.
    assert!(
        String::from_utf8_lossy(&repo.git(&["status", "--porcelain"]).stdout)
            .trim()
            .is_empty(),
        "a clean merge left the tree dirty:\n{}",
        repo.transcript()
    );
    let parents =
        String::from_utf8_lossy(&repo.git(&["rev-list", "--parents", "-n1", "HEAD"]).stdout)
            .split_whitespace()
            .count();
    assert_eq!(parents, 3, "HEAD is not a merge commit");
}

const CONFLICT_BASE: &str = "\
class Config {
  int timeout = 30;

  void apply() {
    set(timeout);
  }
}
";
const CONFLICT_OURS: &str = "\
class Config {
  int timeout = 60;

  void apply() {
    set(timeout);
  }
}
";
const CONFLICT_THEIRS: &str = "\
class Config {
  int timeout = 90;

  void apply() {
    set(timeout);
  }
}
";

/// (b) A genuine disagreement. It must stay a conflict, the markers must be
/// well formed, and git must know the merge failed.
#[test]
fn a_real_conflict_keeps_correct_markers() {
    let repo = Repo::new();
    let out = repo.three_way("Config.java", CONFLICT_BASE, CONFLICT_OURS, CONFLICT_THEIRS);
    assert!(
        !out.status.success(),
        "a real disagreement must not be auto-resolved:\n{}",
        repo.transcript()
    );

    let merged = repo.read("Config.java");
    assert!(has_markers(&merged), "{merged}");
    let opens = merged.lines().filter(|l| l.starts_with("<<<<<<<")).count();
    let seps = merged.lines().filter(|l| *l == "=======").count();
    let closes = merged.lines().filter(|l| l.starts_with(">>>>>>>")).count();
    assert_eq!((opens, seps, closes), (1, 1, 1), "{merged}");
    assert!(merged.contains("timeout = 60"), "lost our side:\n{merged}");
    assert!(
        merged.contains("timeout = 90"),
        "lost their side:\n{merged}"
    );
    // The untouched method is still there and was not mangled by the markers.
    assert!(merged.contains("void apply()"), "{merged}");
    assert!(merged.ends_with('\n'), "{merged:?}");

    // No unexpanded placeholder reached the file. On git < 2.44 this is the
    // old-git label fallback doing its job; on 2.44+ it is trivially true.
    for placeholder in ["%X", "%Y", "%S", "%L", "%P", "%A", "%O", "%B"] {
        assert!(
            !merged.contains(placeholder),
            "{placeholder} leaked into the merged file:\n{merged}"
        );
    }

    // git recorded the conflict properly: the path is unmerged and the merge is
    // still in progress.
    let status = String::from_utf8_lossy(&repo.git(&["status", "--porcelain"]).stdout).into_owned();
    assert!(status.contains("UU Config.java"), "status was {status:?}");
    assert!(repo.path().join(".git/MERGE_HEAD").exists());
}

const MOVE_BASE: &str = "\
package app;

class Repo {
  void save(Item item) {
    store.put(item);
  }

  void load(String id) {
    store.get(id);
  }

  void flush() {
    store.sync();
  }
}
";
/// We reorder: `save` moves to the bottom.
const MOVE_OURS: &str = "\
package app;

class Repo {
  void load(String id) {
    store.get(id);
  }

  void flush() {
    store.sync();
  }

  void save(Item item) {
    store.put(item);
  }
}
";
/// They edit `save`'s body where it used to be.
const MOVE_THEIRS: &str = "\
package app;

class Repo {
  void save(Item item) {
    validate(item);
    store.put(item);
  }

  void load(String id) {
    store.get(id);
  }

  void flush() {
    store.sync();
  }
}
";

/// (c) The headline case: a moved declaration edited on the other branch. Line
/// merge cannot express the answer; the tree merge applies both changes.
#[test]
fn a_moved_and_edited_method_merges_with_both_changes_applied() {
    assert!(
        plain_git_merge("Repo.java", MOVE_BASE, MOVE_OURS, MOVE_THEIRS),
        "the fixture is not a real git conflict; the test would prove nothing"
    );

    let repo = Repo::new();
    let out = repo.three_way("Repo.java", MOVE_BASE, MOVE_OURS, MOVE_THEIRS);
    assert!(
        out.status.success(),
        "the merge should have succeeded:\n{}",
        repo.transcript()
    );

    let merged = repo.read("Repo.java");
    assert!(!has_markers(&merged), "{merged}");
    assert!(parses_cleanly(&merged), "{merged}");

    // Their edit applied…
    assert!(
        merged.contains("validate(item);"),
        "lost their edit:\n{merged}"
    );
    // …to the method in the position we moved it to.
    let save_at = merged.find("void save(").expect("save survives");
    let load_at = merged.find("void load(").expect("load survives");
    let flush_at = merged.find("void flush(").expect("flush survives");
    assert!(
        load_at < flush_at && flush_at < save_at,
        "our reordering was lost:\n{merged}"
    );
    // Nothing was duplicated: exactly one of each method.
    for needle in [
        "void save(",
        "void load(",
        "void flush(",
        "store.put(item);",
    ] {
        assert_eq!(
            merged.matches(needle).count(),
            1,
            "{needle} appears twice:\n{merged}"
        );
    }
}

// ------------------------------------------------- the data-safety guarantee

/// **No file is ever truncated or emptied, on any of the three scenarios.**
///
/// The single claim M4's exit criterion is about. Checked for every case at
/// once, and separately from the merge-quality assertions above so that a
/// failure here is unmistakable.
#[test]
fn no_scenario_ever_truncates_or_empties_the_file() {
    let cases: &[(&str, &str, &str, &str)] = &[
        ("Service.java", IMPORTS_BASE, IMPORTS_OURS, IMPORTS_THEIRS),
        ("Config.java", CONFLICT_BASE, CONFLICT_OURS, CONFLICT_THEIRS),
        ("Repo.java", MOVE_BASE, MOVE_OURS, MOVE_THEIRS),
    ];
    for (file, base, ours, theirs) in cases {
        let repo = Repo::new();
        repo.three_way(file, base, ours, theirs);
        let merged = repo.read(file);

        assert!(!merged.is_empty(), "{file}: the merge emptied the file");
        // A merge result is never dramatically smaller than the smaller input:
        // a truncation bug shows up here long before anyone notices the missing
        // method.
        let floor = ours.len().min(theirs.len()) / 2;
        assert!(
            merged.len() >= floor,
            "{file}: {} bytes out of inputs of {} and {} — that is a truncation",
            merged.len(),
            ours.len(),
            theirs.len()
        );
        // Every declaration present in *both* inputs survives.
        for line in ours
            .lines()
            .filter(|l| l.contains("class ") || l.contains("void "))
        {
            if theirs.contains(line) {
                assert!(
                    merged.contains(line.trim()),
                    "{file}: lost {:?}, which both sides kept:\n{merged}",
                    line.trim()
                );
            }
        }
        // And no leftover temporary from the atomic write.
        let strays: Vec<String> = std::fs::read_dir(repo.path())
            .expect("read_dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".sm-merge"))
            .collect();
        assert_eq!(strays, Vec::<String>::new(), "{file}: {strays:?}");
    }
}

/// `git diff` after a clean merge shows the other branch's changes and nothing
/// else — in particular, no reformatting of untouched code (SPEC.md §0.5).
#[test]
fn a_clean_merge_diffs_only_what_the_other_branch_changed() {
    let repo = Repo::new();
    let out = repo.three_way("Repo.java", MOVE_BASE, MOVE_OURS, MOVE_THEIRS);
    assert!(out.status.success(), "{}", repo.transcript());

    // Against our own pre-merge commit: the only added line should be theirs.
    let diff = String::from_utf8_lossy(
        &repo
            .git(&["diff", "HEAD^1", "HEAD", "--", "Repo.java"])
            .stdout,
    )
    .into_owned();
    let added: Vec<&str> = diff
        .lines()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .collect();
    let removed: Vec<&str> = diff
        .lines()
        .filter(|l| l.starts_with('-') && !l.starts_with("---"))
        .collect();
    assert_eq!(
        added,
        vec!["+    validate(item);"],
        "the merge changed more than their edit:\n{diff}"
    );
    assert!(
        removed.is_empty(),
        "the merge removed lines from our side:\n{diff}"
    );
}

/// The driver is only consulted for paths `.gitattributes` matches.
///
/// A driver that quietly applied to everything would be a much bigger blast
/// radius than the one advertised.
#[test]
fn files_outside_gitattributes_still_use_gits_own_merge() {
    let repo = Repo::new();
    let out = repo.three_way(
        "notes.txt",
        "alpha\nbeta\ngamma\n",
        "alpha\nBETA\ngamma\n",
        "alpha\nbeta2\ngamma\n",
    );
    assert!(!out.status.success(), "a text conflict must remain one");
    let merged = repo.read("notes.txt");
    assert!(has_markers(&merged), "{merged}");
    // git's own labels, which our driver never sees.
    assert!(merged.contains("<<<<<<< HEAD"), "{merged}");
}
