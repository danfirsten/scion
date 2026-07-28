//! End-to-end tests for `sm merge`, the git merge driver.
//!
//! Everything here drives the **real binary** through `CARGO_BIN_EXE_sm`,
//! because the things that matter about a merge driver are things a library
//! test cannot observe: which file got written, whether it got written
//! atomically, what the process exit code was, and what happened to `%A` when
//! the driver gave up. SPEC.md §4.7 is blunt about the stakes — *"writing the
//! result to the wrong path silently destroys work"* — so the assertions here
//! are mostly about bytes on disk, not about merge quality. Merge quality is
//! `sm-merge`'s and `sm-emit`'s problem and they have their own suites.
//!
//! The organising idea: **every failure path is tested by checking that `%A`
//! survived.** For each way the driver can give up (unknown language, oversized
//! input, unparseable input, timeout, missing fallback command, unwritable
//! destination) there is a test that records `%A`'s bytes before the run and
//! asserts afterwards that they are either untouched or a valid line merge —
//! never anything else, and never truncated.

// The generator is `sm-merge`'s, and the driver-level property below is the same
// property stated one layer up. Including the module by path rather than copying
// it keeps one definition of what a plausible edit is; the alternative was a
// second, drifting generator in this crate.
#[path = "../../sm-merge/tests/mutate/mod.rs"]
mod mutate;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};

use mutate::{Mutation, MutationKind};

// ------------------------------------------------------------------ fixtures

const BASE: &str = "\
package app;

import java.util.List;

class Service {
  private int count = 0;

  void reset() {
    count = 0;
  }

  void log(int n) {
    print(n);
  }
}
";

/// Ours and theirs edit the same line differently: git conflicts, and so do we.
const OURS_CONFLICTING: &str = "\
package app;

import java.util.List;

class Service {
  private int count = 1;

  void reset() {
    count = 0;
  }

  void log(int n) {
    print(n);
  }
}
";
const THEIRS_CONFLICTING: &str = "\
package app;

import java.util.List;

class Service {
  private int count = 2;

  void reset() {
    count = 0;
  }

  void log(int n) {
    print(n);
  }
}
";

/// We move `reset` below `log`; they add a statement to `reset`'s body. Git
/// cannot merge this; the tree merge can.
const OURS_MOVED: &str = "\
package app;

import java.util.List;

class Service {
  private int count = 0;

  void log(int n) {
    print(n);
  }

  void reset() {
    count = 0;
  }
}
";
const THEIRS_EDITED_BODY: &str = "\
package app;

import java.util.List;

class Service {
  private int count = 0;

  void reset() {
    count = 0;
    notifyReset();
  }

  void log(int n) {
    print(n);
  }
}
";

/// Far-apart edits git merges on its own — the fast path's population.
const OURS_FAR: &str = "\
package app;

import java.util.List;

class Service {
  private int count = 9;

  void reset() {
    count = 0;
  }

  void log(int n) {
    print(n);
  }
}
";
const THEIRS_FAR: &str = "\
package app;

import java.util.List;

class Service {
  private int count = 0;

  void reset() {
    count = 0;
  }

  void log(int n) {
    print(n);
    flush();
  }
}
";

const UNPARSEABLE: &str = "\
package app;

class Service {
  void reset( {
}
";

// -------------------------------------------------------------- the harness

/// One driver invocation's worth of files, in a scratch directory.
///
/// `%A` is deliberately named the way git names it — no extension, nothing to
/// detect a language from — so that any test which accidentally passes because
/// the driver looked at `%A`'s name instead of `%P` fails here.
struct Case {
    dir: tempfile::TempDir,
    base: PathBuf,
    ours: PathBuf,
    theirs: PathBuf,
}

impl Case {
    fn new(base: &str, ours: &str, theirs: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Self {
            base: dir.path().join("BASE_1234"),
            ours: dir.path().join(".merge_file_aB3xQ7"),
            theirs: dir.path().join("THEIRS_5678"),
            dir,
        };
        std::fs::write(&paths.base, base).expect("write base");
        std::fs::write(&paths.ours, ours).expect("write ours");
        std::fs::write(&paths.theirs, theirs).expect("write theirs");
        paths
    }

    /// The bytes currently in `%A`.
    fn ours_bytes(&self) -> Vec<u8> {
        std::fs::read(&self.ours).expect("read %A")
    }

    fn ours_text(&self) -> String {
        String::from_utf8_lossy(&self.ours_bytes()).into_owned()
    }

    /// Run the driver with git's argument list and whatever extra flags.
    fn run(&self, pathname: &str, extra: &[&str]) -> Run {
        self.run_with_labels(pathname, &["7", "%S", "%X", "%Y"], extra)
    }

    /// `markers` is `[%L, %S, %X, %Y]`, so a test can simulate a git that does
    /// or does not expand them.
    fn run_with_labels(&self, pathname: &str, markers: &[&str; 4], extra: &[&str]) -> Run {
        let debug = self.dir.path().join("debug.json");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_sm"));
        cmd.arg("merge")
            .arg(&self.base)
            .arg(&self.ours)
            .arg(&self.theirs)
            .arg(markers[0])
            .arg(pathname)
            .arg(markers[1])
            .arg(markers[2])
            .arg(markers[3])
            .arg("--debug-json")
            .arg(&debug)
            .args(extra);
        let output = cmd.output().expect("running sm merge");
        let record = std::fs::read_to_string(&debug)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
        Run { output, record }
    }

    /// Everything in the scratch directory that is not one of the three inputs
    /// or the debug record. Must always be empty: a leftover temporary means
    /// the atomic write leaked.
    fn strays(&self) -> Vec<String> {
        let known = [
            "BASE_1234",
            ".merge_file_aB3xQ7",
            "THEIRS_5678",
            "debug.json",
        ];
        let mut names: Vec<String> = std::fs::read_dir(self.dir.path())
            .expect("read_dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .filter(|n| !known.contains(&n.as_str()))
            .collect();
        names.sort();
        names
    }
}

struct Run {
    output: Output,
    record: Option<serde_json::Value>,
}

impl Run {
    fn code(&self) -> i32 {
        self.output.status.code().expect("no signal")
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).into_owned()
    }

    fn record(&self) -> &serde_json::Value {
        self.record.as_ref().expect("a debug record was written")
    }

    fn path_taken(&self) -> String {
        self.record()["path_taken"]
            .as_str()
            .expect("path_taken")
            .to_owned()
    }

    fn fallback_reason(&self) -> Option<String> {
        self.record()["fallback_reason"]
            .as_str()
            .map(ToOwned::to_owned)
    }
}

fn has_markers(text: &str) -> bool {
    text.lines().any(|l| l.starts_with("<<<<<<<"))
}

fn java() -> &'static dyn sm_cst::Language {
    sm_cst::languages::detect(Path::new("x.java")).expect("java")
}

fn parses_cleanly(text: &str) -> bool {
    sm_cst::parse(text.as_bytes(), java()).is_ok_and(|t| !t.has_errors())
}

/// What `git merge-file` alone would have produced, for comparing a fallback
/// against.
fn line_merge(case: &Case) -> Option<(i32, Vec<u8>)> {
    let out = Command::new("git")
        .arg("merge-file")
        .arg("-p")
        .arg("--marker-size=7")
        .args(["-L", "ours", "-L", "base", "-L", "theirs"])
        .arg(&case.ours)
        .arg(&case.base)
        .arg(&case.theirs)
        .output()
        .ok()?;
    Some((out.status.code()?, out.stdout))
}

// ----------------------------------------------------- the destination, %A

#[test]
fn the_result_is_written_to_ours_and_nowhere_else() {
    let case = Case::new(BASE, OURS_MOVED, THEIRS_EDITED_BODY);
    let base_before = std::fs::read(&case.base).expect("read");
    let theirs_before = std::fs::read(&case.theirs).expect("read");

    let run = case.run("app/Service.java", &[]);
    assert_eq!(run.code(), 0, "stderr: {}", run.stderr());

    // The merge really happened: their new statement is in our arrangement.
    let merged = case.ours_text();
    assert!(merged.contains("notifyReset();"), "{merged}");
    let reset_at = merged.find("void reset()").expect("reset survives");
    let log_at = merged.find("void log(").expect("log survives");
    assert!(log_at < reset_at, "our move was not preserved:\n{merged}");

    assert_eq!(std::fs::read(&case.base).expect("read"), base_before);
    assert_eq!(std::fs::read(&case.theirs).expect("read"), theirs_before);
    assert_eq!(case.strays(), Vec::<String>::new());
}

#[test]
fn output_redirects_to_the_output_flag_and_leaves_ours_alone() {
    let case = Case::new(BASE, OURS_MOVED, THEIRS_EDITED_BODY);
    let before = case.ours_bytes();
    let elsewhere = case.dir.path().join("elsewhere.java");

    let run = case.run(
        "app/Service.java",
        &["--output", elsewhere.to_str().expect("utf-8")],
    );
    assert_eq!(run.code(), 0, "stderr: {}", run.stderr());
    assert_eq!(case.ours_bytes(), before, "%A must not be touched");
    assert!(
        std::fs::read_to_string(&elsewhere)
            .expect("read")
            .contains("notifyReset();")
    );
}

// -------------------------------------------------------------- exit codes

#[test]
fn a_clean_merge_exits_zero_and_leaves_no_markers() {
    let case = Case::new(BASE, OURS_MOVED, THEIRS_EDITED_BODY);
    let run = case.run("app/Service.java", &[]);
    assert_eq!(run.code(), 0, "stderr: {}", run.stderr());
    assert!(!has_markers(&case.ours_text()));
    assert!(parses_cleanly(&case.ours_text()), "{}", case.ours_text());
    assert_eq!(run.record()["conflicts"], 0);
}

#[test]
fn a_conflicting_merge_exits_one_and_writes_markers() {
    let case = Case::new(BASE, OURS_CONFLICTING, THEIRS_CONFLICTING);
    let run = case.run("app/Service.java", &[]);
    assert_eq!(run.code(), 1, "stderr: {}", run.stderr());
    let text = case.ours_text();
    assert!(has_markers(&text), "{text}");
    // Both sides' text is present, which is the point of a conflict.
    assert!(text.contains("count = 1"), "{text}");
    assert!(text.contains("count = 2"), "{text}");
    assert!(text.ends_with('\n'), "{text:?}");
    assert!(run.record()["conflicts"].as_u64().expect("number") >= 1);
}

// ------------------------------------------------------ language detection

#[test]
fn the_language_comes_from_the_pathname_not_from_the_temp_file_name() {
    // The same three files, run twice. `%A` has no useful name in either case;
    // only `%P` differs, and it is what decides whether the tree merge runs.
    let known = Case::new(BASE, OURS_MOVED, THEIRS_EDITED_BODY);
    let run = known.run("app/Service.java", &[]);
    assert_eq!(run.record()["language"], "java");
    assert_eq!(run.path_taken(), "semantic");
    assert_eq!(run.code(), 0);

    let unknown = Case::new(BASE, OURS_MOVED, THEIRS_EDITED_BODY);
    let run = unknown.run("app/Service.cobol", &[]);
    assert!(run.record()["language"].is_null());
    assert_eq!(run.fallback_reason().as_deref(), Some("unknown_language"));
    // Git could not merge this one, so the fallback conflicts — which is
    // exactly what would have happened with no driver installed.
    assert_eq!(run.code(), 1);
    assert!(has_markers(&unknown.ours_text()));
}

#[test]
fn a_missing_pathname_falls_back_rather_than_guessing() {
    let case = Case::new(BASE, OURS_MOVED, THEIRS_EDITED_BODY);
    // Git older than the `%P` placeholder, or a hand invocation.
    let run = case.run("%P", &[]);
    assert!(run.record()["path"].is_null());
    assert_eq!(run.fallback_reason().as_deref(), Some("unknown_language"));
}

// -------------------------------------------------------------- fast path

#[test]
fn a_clean_line_merge_takes_the_fast_path_and_never_builds_a_tree() {
    let case = Case::new(BASE, OURS_FAR, THEIRS_FAR);
    // Before the run, for the same reason as in `line_merge_only_…`.
    let (git_code, git_bytes) = line_merge(&case).expect("git merge-file");
    let run = case.run("app/Service.java", &[]);
    assert_eq!(run.code(), 0, "stderr: {}", run.stderr());
    assert_eq!(run.path_taken(), "fast");
    assert!(run.record()["semantic"].is_null(), "the tree merge ran");
    assert_eq!(run.record()["line_merge"]["conflict_hunks"], 0);
    assert_eq!(run.record()["line_merge"]["parsed_ok"], true);

    let text = case.ours_text();
    assert!(text.contains("count = 9"), "{text}");
    assert!(text.contains("flush();"), "{text}");

    // And the result is byte-identical to what git alone would have written,
    // which is the whole regression-safety claim of the fast path.
    assert_eq!(git_code, 0);
    assert_eq!(case.ours_bytes(), git_bytes);
}

#[test]
fn the_fast_path_can_be_switched_off() {
    let case = Case::new(BASE, OURS_FAR, THEIRS_FAR);
    let run = case.run("app/Service.java", &["--no-fast-path"]);
    assert_eq!(run.code(), 0, "stderr: {}", run.stderr());
    assert_eq!(run.path_taken(), "semantic");
    // Same answer by a different route.
    let text = case.ours_text();
    assert!(
        text.contains("count = 9") && text.contains("flush();"),
        "{text}"
    );
}

#[test]
fn line_merge_only_never_consults_the_tree() {
    let case = Case::new(BASE, OURS_MOVED, THEIRS_EDITED_BODY);
    // Taken *before* the run: the driver overwrites `%A`, so asking git
    // afterwards would be asking it to merge our own output.
    let (code, bytes) = line_merge(&case).expect("git merge-file");
    assert_eq!(code, 1, "git is supposed to conflict on this pair");

    let run = case.run("app/Service.java", &["--line-merge-only"]);
    assert_eq!(run.path_taken(), "fallback");
    assert_eq!(run.fallback_reason().as_deref(), Some("semantic_disabled"));
    assert!(run.record()["semantic"].is_null(), "the tree merge ran");
    assert_eq!(run.code(), 1);
    assert_eq!(case.ours_bytes(), bytes);
}

// --------------------------------------------------------- fallback ladder

/// Every fallback rung, checked the same way: the driver must end up writing
/// exactly what `git merge-file` would have written, and exit accordingly.
#[test]
fn every_fallback_rung_writes_the_line_merge_verbatim() {
    let rungs: &[(&str, &str, &[&str], &str)] = &[
        (
            "unknown language",
            "app/Service.cobol",
            &[],
            "unknown_language",
        ),
        (
            "over the size budget",
            "app/Service.java",
            &["--max-bytes", "1"],
            "too_large",
        ),
        (
            "timeout",
            "app/Service.java",
            &["--timeout-ms", "1"],
            "timeout",
        ),
        (
            "semantic disabled",
            "app/Service.java",
            &["--line-merge-only"],
            "semantic_disabled",
        ),
    ];
    for (name, pathname, extra, reason) in rungs {
        let case = Case::new(BASE, OURS_MOVED, THEIRS_EDITED_BODY);
        let expected = line_merge(&case).expect("git merge-file");
        let run = case.run(pathname, extra);
        assert_eq!(
            run.fallback_reason().as_deref(),
            Some(*reason),
            "{name}: stderr {}",
            run.stderr()
        );
        assert_eq!(run.path_taken(), "fallback", "{name}");
        assert_eq!(case.ours_bytes(), expected.1, "{name}: wrong bytes");
        assert_eq!(
            run.code(),
            i32::from(expected.0 != 0),
            "{name}: exit code does not follow the line merge"
        );
        assert_eq!(case.strays(), Vec::<String>::new(), "{name}");
    }
}

#[test]
fn an_unparseable_input_falls_back_and_says_which_one() {
    let case = Case::new(BASE, OURS_MOVED, UNPARSEABLE);
    let expected = line_merge(&case).expect("git merge-file");
    let run = case.run("app/Service.java", &[]);
    assert_eq!(run.fallback_reason().as_deref(), Some("parse_error"));
    assert_eq!(case.ours_bytes(), expected.1);
    assert_eq!(run.record()["inputs"]["theirs"]["parse_errors"], true);
    assert_eq!(run.record()["inputs"]["ours"]["parse_errors"], false);
    assert!(
        run.record()["warnings"]
            .as_array()
            .expect("array")
            .iter()
            .any(|w| w.as_str().is_some_and(|s| s.contains("theirs"))),
        "{:?}",
        run.record()["warnings"]
    );
}

/// A timeout of 0 means "no timeout", the way Mergiraf's does
/// (docs/prior-art.md §2.9).
#[test]
fn a_zero_timeout_disables_the_timeout() {
    let case = Case::new(BASE, OURS_MOVED, THEIRS_EDITED_BODY);
    let run = case.run("app/Service.java", &["--timeout-ms", "0"]);
    assert_eq!(run.code(), 0, "stderr: {}", run.stderr());
    assert_eq!(run.path_taken(), "semantic");
}

// ---------------------------------------------- the bottom of the ladder

/// When the fallback itself cannot run, `%A` must survive **byte-identical**
/// and the driver must exit ≥2.
///
/// This is the single most important test in the file. Git treats a non-zero
/// exit as "conflicts remain" and keeps whatever is in `%A`, so an untouched
/// `%A` degrades to exactly the state of not having installed the driver. A
/// truncated or half-written `%A` here is the data-loss scenario SPEC.md §4.7
/// exists to prevent.
#[test]
fn a_failed_fallback_leaves_ours_byte_identical_and_exits_two() {
    // Each of these forces the semantic path to be skipped, so the fallback is
    // the only possible answer — and it cannot start.
    //
    // Note what is *not* in this list: a broken `--fallback-cmd` on its own.
    // With the semantic path available the driver merges the file and exits 0,
    // never needing the fallback at all. That is the ladder working: a missing
    // `git` is only fatal when it is the last rung left.
    for extra in [
        vec![
            "--max-bytes",
            "1",
            "--fallback-cmd",
            "/nonexistent/git-binary",
        ],
        vec![
            "--timeout-ms",
            "1",
            "--fallback-cmd",
            "/nonexistent/git-binary",
        ],
        vec![
            "--line-merge-only",
            "--fallback-cmd",
            "/nonexistent/git-binary",
        ],
    ] {
        let case = Case::new(BASE, OURS_MOVED, THEIRS_EDITED_BODY);
        let before = case.ours_bytes();
        let run = case.run("app/Service.java", &extra);
        assert!(
            run.code() >= 2,
            "{extra:?}: exit was {}, stderr {}",
            run.code(),
            run.stderr()
        );
        assert_eq!(case.ours_bytes(), before, "{extra:?}: %A was modified");
        assert_eq!(case.strays(), Vec::<String>::new(), "{extra:?}");
        assert_eq!(run.path_taken(), "aborted", "{extra:?}");
        assert_eq!(
            run.fallback_reason().as_deref(),
            Some("line_merge_failed"),
            "{extra:?}"
        );
    }
}

#[test]
fn an_unreadable_input_exits_two_without_touching_ours() {
    let case = Case::new(BASE, OURS_MOVED, THEIRS_EDITED_BODY);
    let before = case.ours_bytes();
    std::fs::remove_file(&case.base).expect("remove base");
    let run = case.run("app/Service.java", &[]);
    assert!(run.code() >= 2, "exit {}", run.code());
    assert_eq!(case.ours_bytes(), before);
    assert_eq!(run.fallback_reason().as_deref(), Some("input_unreadable"));
    assert!(run.stderr().contains("BASE_1234"), "{}", run.stderr());
}

/// An unwritable destination exits ≥2 and leaves the original alone.
///
/// The destination is redirected into a directory that does not exist, rather
/// than `chmod`-ing the scratch directory read-only: `root` ignores directory
/// permissions, so the `chmod` version of this test passes vacuously in a
/// container and would have been worse than no test at all.
#[test]
fn an_unwritable_destination_exits_two_without_touching_the_inputs() {
    let case = Case::new(BASE, OURS_MOVED, THEIRS_EDITED_BODY);
    let before = case.ours_bytes();
    let nowhere = case.dir.path().join("no-such-directory").join("out.java");

    let run = case.run(
        "app/Service.java",
        &["--output", nowhere.to_str().expect("utf-8")],
    );
    assert!(
        run.code() >= 2,
        "exit {}, stderr {}",
        run.code(),
        run.stderr()
    );
    assert_eq!(case.ours_bytes(), before, "%A was damaged");
    assert_eq!(run.fallback_reason().as_deref(), Some("write_failed"));
    assert_eq!(run.path_taken(), "aborted");
    assert!(!nowhere.exists());
    assert_eq!(case.strays(), Vec::<String>::new());
}

// ---------------------------------------------------- markers and labels

#[test]
fn unexpanded_label_placeholders_never_reach_the_output() {
    // git < 2.44 passes the literal strings. Writing them into a user's file is
    // the bug docs/prior-art.md §2.9 warns about.
    let case = Case::new(BASE, OURS_CONFLICTING, THEIRS_CONFLICTING);
    let run = case.run_with_labels("app/Service.java", &["7", "%S", "%X", "%Y"], &[]);
    assert_eq!(run.code(), 1);
    let text = case.ours_text();
    assert!(!text.contains("%X"), "{text}");
    assert!(!text.contains("%Y"), "{text}");
    assert!(!text.contains("%S"), "{text}");
    assert!(text.contains("<<<<<<< ours"), "{text}");
    assert!(text.contains(">>>>>>> theirs"), "{text}");
    assert_eq!(run.record()["inputs"]["old_git_labels"], true);
}

#[test]
fn expanded_labels_are_used_verbatim() {
    let case = Case::new(BASE, OURS_CONFLICTING, THEIRS_CONFLICTING);
    let run = case.run_with_labels(
        "app/Service.java",
        &["7", "merged common ancestors", "HEAD", "feature/x"],
        &[],
    );
    assert_eq!(run.code(), 1);
    let text = case.ours_text();
    assert!(text.contains("<<<<<<< HEAD"), "{text}");
    assert!(text.contains(">>>>>>> feature/x"), "{text}");
    assert_eq!(run.record()["inputs"]["old_git_labels"], false);
}

#[test]
fn the_marker_size_git_passes_is_honoured() {
    let case = Case::new(BASE, OURS_CONFLICTING, THEIRS_CONFLICTING);
    let run = case.run_with_labels("app/Service.java", &["11", "%S", "%X", "%Y"], &[]);
    assert_eq!(run.code(), 1);
    let text = case.ours_text();
    assert!(text.contains("<<<<<<<<<<< ours"), "{text}");
    assert!(text.contains("==========="), "{text}");
    assert_eq!(run.record()["inputs"]["marker_size"], 11);
}

/// The same size reaches the *fallback*, so a fallback's markers cannot be a
/// different width from ours.
#[test]
fn the_marker_size_reaches_the_fallback_too() {
    let case = Case::new(BASE, OURS_CONFLICTING, THEIRS_CONFLICTING);
    let run = case.run_with_labels("app/Service.cobol", &["11", "%S", "%X", "%Y"], &[]);
    assert_eq!(run.fallback_reason().as_deref(), Some("unknown_language"));
    assert!(
        case.ours_text().contains("<<<<<<<<<<<"),
        "{}",
        case.ours_text()
    );
}

#[test]
fn diff3_style_includes_the_ancestor() {
    let case = Case::new(BASE, OURS_CONFLICTING, THEIRS_CONFLICTING);
    let run = case.run("app/Service.java", &["--diff3"]);
    assert_eq!(run.code(), 1);
    let text = case.ours_text();
    assert!(text.contains("||||||| base"), "{text}");
    assert!(text.contains("count = 0"), "the ancestor's line: {text}");
}

// ------------------------------------------------- the semantic check (M6)

/// A triple that trips `sm-bind`'s `BrokenReference`, **and reaches the
/// semantic path**.
///
/// The rename-plus-new-call shape on its own is not enough: git merges it
/// cleanly, so the driver's fast path ships git's bytes and never builds a tree
/// for the check to walk. That is the documented gap (see the driver's
/// `semantic` module), and testing through it rather than around it is the
/// point — so these revisions carry a *second*, unrelated change that git
/// genuinely cannot merge (we move `a` below `b`, they edit `a`'s body), which
/// is what sends the driver down the semantic path in the first place.
///
/// So: git conflicts, the tree merge is clean and applies all three edits, and
/// the merged file still calls a method that no longer exists.
const SEM_BASE: &str = "\
class Repo {
  void a() {
    x();
  }

  void b() {
    y();
  }

  User getUser(String id) {
    return store.find(id);
  }

  User cached(String id) {
    return getUser(id);
  }
}
";
const SEM_OURS: &str = "\
class Repo {
  void b() {
    y();
  }

  void a() {
    x();
  }

  User fetchUser(String id) {
    return store.find(id);
  }

  User cached(String id) {
    return fetchUser(id);
  }
}
";
const SEM_THEIRS: &str = "\
class Repo {
  void a() {
    x2();
  }

  void b() {
    y();
  }

  User getUser(String id) {
    return store.find(id);
  }

  User cached(String id) {
    return getUser(id);
  }

  User first() {
    return getUser(\"1\");
  }
}
";

/// The merged text every mode below produces: clean, no markers, all three
/// edits applied, and `getUser("1")` still there — which is the bug.
fn assert_the_merge_itself_is_clean_and_complete(text: &str) {
    assert!(
        !text.contains("<<<<<<<"),
        "markers in a clean merge:\n{text}"
    );
    assert!(text.contains("x2();"), "lost their edit:\n{text}");
    assert!(text.contains("User fetchUser"), "lost our rename:\n{text}");
    assert!(
        text.contains("User first()"),
        "lost their new method:\n{text}"
    );
    assert!(
        text.contains("return getUser(\"1\");"),
        "the broken call should still be there — it is what the check found:\n{text}"
    );
}

/// **`report` is the default**: findings on stderr, exit code untouched.
#[test]
fn the_semantic_check_reports_by_default() {
    let case = Case::new(SEM_BASE, SEM_OURS, SEM_THEIRS);
    let run = case.run("app/Repo.java", &[]);

    assert_eq!(run.code(), 0, "report mode must not change the exit code");
    assert_eq!(run.path_taken(), "semantic");
    assert_the_merge_itself_is_clean_and_complete(&case.ours_text());

    let err = run.stderr();
    assert!(
        err.contains("semantic-merge: warning:"),
        "no warning prefix:\n{err}"
    );
    assert!(err.contains("getUser"), "the name is not named:\n{err}");
    assert!(
        err.contains("1 semantic conflict (broken_reference)"),
        "no count line:\n{err}"
    );
    assert!(
        err.contains("the merge itself was clean"),
        "report mode should say the merge was fine:\n{err}"
    );

    let check = &run.record()["semantic_check"];
    assert_eq!(check["mode"], "report");
    assert_eq!(check["conflicts"], 1);
    assert_eq!(check["kinds"][0], "broken_reference");
}

/// `off` does not run it at all, and says so by leaving the record's
/// `semantic_check` null rather than reporting zero findings.
#[test]
fn the_semantic_check_can_be_switched_off() {
    let case = Case::new(SEM_BASE, SEM_OURS, SEM_THEIRS);
    let run = case.run("app/Repo.java", &["--semantic=off"]);

    assert_eq!(run.code(), 0);
    assert_the_merge_itself_is_clean_and_complete(&case.ours_text());
    assert!(
        !run.stderr().contains("semantic-merge:"),
        "off must be silent:\n{}",
        run.stderr()
    );
    assert!(run.record()["semantic_check"].is_null());
    assert!(run.record()["timings_ms"]["semantic_check"].is_null());
}

/// **`conflict` exits 1 and writes the clean merge anyway.**
///
/// This is the UX decision the `semantic` module argues: there is no textual
/// disagreement to bracket, so there are no markers; git sees a non-zero exit
/// and leaves the path unmerged; the user reads stderr, opens a perfectly
/// well-formed file, and decides. `dogfood.rs` checks what git actually shows
/// them.
#[test]
fn the_semantic_check_can_make_the_merge_a_conflict() {
    let case = Case::new(SEM_BASE, SEM_OURS, SEM_THEIRS);
    let run = case.run("app/Repo.java", &["--semantic=conflict"]);

    assert_eq!(run.code(), 1, "conflict mode must exit 1");
    let text = case.ours_text();
    assert_the_merge_itself_is_clean_and_complete(&text);

    let err = run.stderr();
    assert!(err.contains("getUser"), "{err}");
    assert!(
        err.contains("has no conflict markers"),
        "the user has to be told why a clean-looking file is conflicted:\n{err}"
    );

    // The record distinguishes the two reasons for exit 1: no *textual*
    // conflict regions, one semantic one.
    assert_eq!(run.record()["exit_code"], 1);
    assert_eq!(run.record()["conflicts"], 0);
    assert_eq!(run.record()["semantic"]["clean"], true);
    assert_eq!(run.record()["semantic_check"]["conflicts"], 1);
}

/// `--quiet` silences the findings but not the exit code, so a scripted caller
/// can have the gate without the prose.
#[test]
fn quiet_suppresses_the_findings_but_not_the_verdict() {
    let case = Case::new(SEM_BASE, SEM_OURS, SEM_THEIRS);
    let run = case.run("app/Repo.java", &["--semantic=conflict", "--quiet"]);
    assert_eq!(run.code(), 1);
    assert!(
        !run.stderr().contains("semantic-merge:"),
        "{}",
        run.stderr()
    );
    assert_eq!(run.record()["semantic_check"]["conflicts"], 1);
}

/// **The fast path skips the check**, which is the gap the `semantic` module
/// documents. Pinned so that closing it later is a deliberate change to this
/// test rather than a surprise.
#[test]
fn the_fast_path_does_not_run_the_semantic_check() {
    // Rename on one side, a new call on the other, and nothing else: git merges
    // this cleanly, so the driver never builds a tree.
    let base = "\
class R {
  User getUser(String id) {
    return store.find(id);
  }

  User cached(String id) {
    return getUser(id);
  }
}
";
    let ours = "\
class R {
  User fetchUser(String id) {
    return store.find(id);
  }

  User cached(String id) {
    return fetchUser(id);
  }
}
";
    let theirs = "\
class R {
  User getUser(String id) {
    return store.find(id);
  }

  User cached(String id) {
    return getUser(id);
  }

  User first() {
    return getUser(\"1\");
  }
}
";

    let case = Case::new(base, ours, theirs);
    let run = case.run("app/R.java", &["--semantic=conflict"]);
    assert_eq!(run.path_taken(), "fast");
    assert_eq!(
        run.code(),
        0,
        "the fast path cannot report what it never ran"
    );
    assert!(run.record()["semantic_check"].is_null());

    // And with the fast path off, the very same triple is caught. This is the
    // measurement M5's corpus scan will make offline.
    let case = Case::new(base, ours, theirs);
    let run = case.run("app/R.java", &["--semantic=conflict", "--no-fast-path"]);
    assert_eq!(run.path_taken(), "semantic");
    assert_eq!(run.code(), 1);
    assert_eq!(run.record()["semantic_check"]["conflicts"], 1);
}

/// A merge that is *not* clean does not get checked: the file already has
/// markers in it and a human is about to read every line of it.
#[test]
fn a_conflicted_merge_does_not_get_a_semantic_check() {
    let case = Case::new(BASE, OURS_CONFLICTING, THEIRS_CONFLICTING);
    let run = case.run("app/Service.java", &["--semantic=conflict"]);
    assert_eq!(run.code(), 1);
    assert!(case.ours_text().contains("<<<<<<<"));
    assert!(run.record()["semantic_check"].is_null());
}

/// A fallback never gets one either — there is no merge plan to walk.
#[test]
fn a_fallback_does_not_get_a_semantic_check() {
    let case = Case::new(SEM_BASE, SEM_OURS, SEM_THEIRS);
    let run = case.run("app/Repo.cobol", &["--semantic=conflict"]);
    assert_eq!(run.path_taken(), "fallback");
    assert!(run.record()["semantic_check"].is_null());
}

// ------------------------------------------------------------- debug json

/// The `--debug-json` record is M5's input, so its shape is asserted rather
/// than assumed.
#[test]
fn the_debug_record_has_the_documented_shape() {
    let case = Case::new(BASE, OURS_MOVED, THEIRS_EDITED_BODY);
    let run = case.run("app/Service.java", &[]);
    let rec = run.record();

    assert_eq!(rec["schema_version"], 1);
    assert!(rec["tool"].as_str().expect("tool").starts_with("sm-cli"));
    assert_eq!(rec["path"], "app/Service.java");
    assert_eq!(rec["language"], "java");
    assert_eq!(rec["path_taken"], "semantic");
    assert!(rec["fallback_reason"].is_null());
    assert_eq!(rec["exit_code"], 0);
    assert_eq!(rec["conflicts"], 0);

    for side in ["base", "ours", "theirs"] {
        assert!(rec["inputs"][side]["bytes"].as_u64().expect("bytes") > 0);
        assert_eq!(rec["inputs"][side]["parse_errors"], false);
    }
    assert_eq!(rec["inputs"]["marker_size"], 7);
    assert_eq!(rec["inputs"]["labels"]["ours"], "ours");

    assert!(rec["line_merge"]["conflict_hunks"].as_u64().expect("hunks") > 0);

    let sem = &rec["semantic"];
    assert_eq!(sem["clean"], true);
    assert_eq!(sem["conflicts"], 0);
    assert_eq!(sem["synthesized_bytes"], 0);
    assert_eq!(sem["synthesized_separators"], 0);
    assert!(sem["output_bytes"].as_u64().expect("bytes") > 0);
    for key in [
        "base_nodes",
        "ours_nodes",
        "theirs_nodes",
        "merged_nodes",
        "splices",
        "rebuilds",
        "reparented",
        "set_merged_regions",
        "deduplicated_insertions",
        "covered_deletions",
    ] {
        assert!(sem["stats"][key].is_u64(), "stats.{key} missing");
    }

    let t = &rec["timings_ms"];
    for key in [
        "read",
        "line_merge",
        "parse",
        "merge",
        "emit",
        "verify",
        "write",
    ] {
        assert!(t[key].is_f64(), "timings_ms.{key} missing or not a number");
    }
    assert!(t["total"].as_f64().expect("total") > 0.0);
    // A stage that did not run reports null, never a misleading zero.
    assert!(t["fast_path_verify"].is_null());

    // The semantic check ran (clean semantic merge, default `report`) and found
    // nothing, which is a different thing from not having run — see the
    // `semantic_check` object's docs in `merge::record`.
    let check = &rec["semantic_check"];
    assert_eq!(check["mode"], "report");
    assert_eq!(check["conflicts"], 0);
    assert!(check["kinds"].as_array().expect("kinds").is_empty());
    assert!(check["findings"].as_array().expect("findings").is_empty());
    for key in [
        "references",
        "resolved_in_origin",
        "unresolved_in_origin",
        "origin_not_found",
        "conflict_regions",
        "conflicts",
    ] {
        assert!(
            check["stats"][key].is_u64(),
            "semantic_check.stats.{key} missing"
        );
    }
    assert!(
        t["semantic_check"].is_f64(),
        "the check's timing is missing"
    );
}

/// The `semantic_check` object in full, on a record that actually has findings.
///
/// It is an **additive** schema change: `schema_version` stays 1, a consumer
/// that does not know the field ignores it, and one that does gets the same
/// `SemanticConflict` shape `sm-bind` serializes everywhere else.
#[test]
fn the_debug_record_carries_the_semantic_check_findings() {
    let case = Case::new(SEM_BASE, SEM_OURS, SEM_THEIRS);
    let run = case.run("app/Repo.java", &[]);
    let rec = run.record();

    assert_eq!(rec["schema_version"], 1, "this is an additive change");
    let check = &rec["semantic_check"];
    assert_eq!(check["mode"], "report");
    assert_eq!(check["conflicts"], 1);
    assert_eq!(check["kinds"], serde_json::json!(["broken_reference"]));

    let finding = &check["findings"][0];
    assert_eq!(finding["kind"], "broken_reference");
    assert_eq!(finding["name"], "getUser");
    assert_eq!(finding["reference"]["side"], "theirs");
    assert_eq!(finding["origin_declaration"]["kind"], "method");
    assert!(finding["merged_declaration"].is_null());
    assert!(
        finding["explanation"]
            .as_str()
            .expect("explanation")
            .contains("fetchUser"),
        "{finding}"
    );

    assert_eq!(check["stats"]["conflicts"], 1);
    assert!(check["stats"]["references"].as_u64().expect("references") > 0);
}

/// The fast path's parse gate actually runs, and its answer is recorded.
///
/// # Why there is no test of the gate *rejecting*
///
/// A fixture where git's line merge is clean and its output does not parse
/// turned out to be very hard to construct for Java, and the reason is worth
/// writing down because it bounds how often the gate can fire at all:
///
/// - Git only merges cleanly when the two sides' hunks are **disjoint**, so the
///   output is `base` with two independent regions replaced.
/// - Brace balance is additive over disjoint regions. If `ours` parses then its
///   edit is brace-neutral, and likewise `theirs`, so the merged file is
///   brace-balanced too. **The entire "unbalanced braces" family is impossible**
///   whenever both inputs parse.
/// - Everything left is a non-brace syntax error, and `tree-sitter-java` is
///   deliberately permissive about those: `class C {}` followed by an `import`,
///   or a bare statement at file scope, both parse without an `ERROR` node
///   (checked directly).
///
/// A brute-force search over ~1400 pairs of single-line edits to a small class
/// found no example. So the gate is cheap insurance against a rare event rather
/// than a common branch — which is a good property for a fast path to have, and
/// the reason the driver records every rejection in `--debug-json` instead of
/// relying on a unit test to characterise it. `sm merge`'s module docs, §3,
/// carry the same note.
#[test]
fn the_fast_path_verifies_its_answer_by_parsing_it() {
    let case = Case::new(BASE, OURS_FAR, THEIRS_FAR);
    let run = case.run("app/Service.java", &[]);
    assert_eq!(run.path_taken(), "fast");
    assert_eq!(run.record()["line_merge"]["parsed_ok"], true);
    assert!(
        run.record()["timings_ms"]["fast_path_verify"]
            .as_f64()
            .is_some(),
        "the verification step did not run"
    );

    // And it is skipped, not silently assumed, when there is no grammar: the
    // record shows the check was never made.
    let case = Case::new(BASE, OURS_FAR, THEIRS_FAR);
    let run = case.run("app/Service.cobol", &[]);
    assert_eq!(run.path_taken(), "fallback");
    assert!(run.record()["line_merge"]["parsed_ok"].is_null());
    assert!(run.record()["timings_ms"]["fast_path_verify"].is_null());
    assert_eq!(run.code(), 0, "git merged this one cleanly");
}

/// **The token-fusion case now merges, and correctly.**
///
/// These three revisions are the reduction of a real case from the mined corpus
/// (`oracle/graal`). The tree merge always resolved them correctly *as a
/// decision* — our value change and their `static` both apply — but the emitter
/// used to write `staticint a = 2;`, with the two tokens fused, and **that
/// output parses**: `tree-sitter-java` reads `staticint` as a type name, so the
/// reparse gate passed it. Only the token check below it refused the file, at
/// the cost of the resolve.
///
/// The bug is fixed in the libraries (`crates/sm-emit/tests/token_fusion.rs` is
/// the regression suite), so what this asserts now is the whole chain
/// end-to-end: the driver takes the semantic path, writes a correct
/// `static int a = 2;`, exits 0, and reports a pure splice with no separator
/// synthesized on the way.
#[test]
fn the_corpus_token_fusion_case_merges_cleanly_and_correctly() {
    let case = Case::new(
        "class C {\n    int a = 1;\n}\n",
        "class C {\n    int a = 2;\n}\n",
        "class C {\n    static int a = 1;\n}\n",
    );
    let run = case.run("app/C.java", &["--no-fast-path"]);

    assert_eq!(run.code(), 0);
    assert_eq!(run.path_taken(), "semantic");
    assert!(
        run.fallback_reason().is_none(),
        "{:?}",
        run.fallback_reason()
    );
    assert_eq!(case.ours_text(), "class C {\n    static int a = 2;\n}\n");
    assert_eq!(run.record()["semantic"]["synthesized_bytes"], 0);
    assert_eq!(run.record()["semantic"]["synthesized_separators"], 0);
}

// -------------------------------------------- the driver-level property

/// **On generated three-way merges, the driver never damages `%A`.**
///
/// The library-level properties live in `sm-merge/tests/proptest_merge.rs`.
/// This is the same idea one layer up, and it is checking three different
/// things at once:
///
/// 1. **Exit codes follow the output.** Exit 0 ⇒ no markers and the file
///    parses; exit 1 ⇒ markers present. A driver that exits 0 with markers in
///    the file makes git believe a conflicted merge succeeded.
/// 2. **Injected failures degrade, they do not damage.** `--max-bytes 1` and an
///    unparseable input each force the fallback; the bytes written must be
///    exactly `git merge-file`'s.
/// 3. **A failure with no fallback leaves `%A` byte-identical.** The data-loss
///    case.
///
/// Case count is low on purpose — each case is several process spawns, so this
/// trades breadth for the thing only an end-to-end test can see.
#[test]
fn the_driver_never_damages_ours_on_any_path() {
    let mut runner = TestRunner::new_with_rng(
        Config {
            cases: 24,
            failure_persistence: None,
            ..Config::default()
        },
        TestRng::deterministic_rng(RngAlgorithm::ChaCha),
    );

    let strategy = {
        let mutation = (0..MutationKind::ALL.len(), any::<u16>(), any::<u16>()).prop_map(
            |(k, site, variant)| Mutation {
                kind: MutationKind::ALL[k],
                site,
                variant,
            },
        );
        (
            0..mutate::bases().len(),
            prop::collection::vec(mutation.clone(), 1..=4),
            prop::collection::vec(mutation, 1..=4),
        )
    };

    runner
        .run(&strategy, |(i, ours_ms, theirs_ms)| {
            let base = &mutate::bases()[i];
            // Java only: the fixture filename has to match, and a mixed-language
            // property would only be testing `languages::detect`.
            if base.lang != "java" {
                return Ok(());
            }
            let lang = base.language();
            let (ours, _) = mutate::apply_all(base.source, lang, &ours_ms);
            let (theirs, _) = mutate::apply_all(base.source, lang, &theirs_ms);
            let describe = format!(
                "--- BASE ---\n{}--- OURS ---\n{ours}--- THEIRS ---\n{theirs}",
                base.source
            );

            // 1. the normal path
            let case = Case::new(base.source, &ours, &theirs);
            let run = case.run("app/Service.java", &[]);
            let text = case.ours_text();
            prop_assert!(
                run.code() == 0 || run.code() == 1,
                "exit {} on a well-formed input\nstderr: {}\n{describe}",
                run.code(),
                run.stderr()
            );
            if run.code() == 0 {
                prop_assert!(
                    !has_markers(&text),
                    "exit 0 with markers:\n{text}\n{describe}"
                );
                prop_assert!(
                    parses_cleanly(&text),
                    "exit 0 with output that does not parse:\n{text}\n{describe}"
                );
            } else {
                prop_assert!(
                    has_markers(&text),
                    "exit 1 with no markers:\n{text}\n{describe}"
                );
            }
            prop_assert_eq!(case.strays(), Vec::<String>::new(), "{}", describe);

            // 2. injected failures still write a valid line merge
            for extra in [
                vec!["--max-bytes", "1"],
                vec!["--timeout-ms", "1"],
                vec!["--line-merge-only"],
            ] {
                let case = Case::new(base.source, &ours, &theirs);
                let expected = line_merge(&case).expect("git merge-file");
                let run = case.run("app/Service.java", &extra);
                prop_assert_eq!(
                    case.ours_bytes(),
                    expected.1,
                    "{:?} did not write the line merge\n{}",
                    extra,
                    describe
                );
                prop_assert_eq!(run.code(), i32::from(expected.0 != 0), "{:?}", extra);
            }

            // 3. an unparseable side: the fallback, never the tree merge
            {
                let case = Case::new(base.source, &ours, UNPARSEABLE);
                let expected = line_merge(&case).expect("git merge-file");
                let run = case.run("app/Service.java", &[]);
                let reason = run.fallback_reason();
                prop_assert_eq!(reason.as_deref(), Some("parse_error"));
                prop_assert_eq!(case.ours_bytes(), expected.1, "{}", describe);
            }

            // 4. no fallback available: %A must be exactly as it was
            {
                let case = Case::new(base.source, &ours, &theirs);
                let before = case.ours_bytes();
                let run = case.run(
                    "app/Service.java",
                    &[
                        "--max-bytes",
                        "1",
                        "--fallback-cmd",
                        "/nonexistent/git-binary",
                    ],
                );
                prop_assert!(run.code() >= 2, "exit {}\n{}", run.code(), describe);
                prop_assert_eq!(case.ours_bytes(), before, "%A damaged\n{}", describe);
                prop_assert_eq!(case.strays(), Vec::<String>::new(), "{}", describe);
            }

            Ok(())
        })
        .expect("driver property");
}
