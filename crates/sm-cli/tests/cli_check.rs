//! End-to-end tests for `sm check`, M6's semantic check as a standalone
//! command.
//!
//! The subcommand exists for two audiences and both are tested here: a human
//! looking at three files (the prose form, and the exit code that makes it
//! usable in a script) and M5's corpus scan (the `--json` form, whose shape is
//! an interface).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// One triple on disk.
struct Case {
    dir: tempfile::TempDir,
    base: PathBuf,
    ours: PathBuf,
    theirs: PathBuf,
}

impl Case {
    fn new(ext: &str, base: &str, ours: &str, theirs: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let case = Self {
            base: dir.path().join(format!("base.{ext}")),
            ours: dir.path().join(format!("ours.{ext}")),
            theirs: dir.path().join(format!("theirs.{ext}")),
            dir,
        };
        std::fs::write(&case.base, base).expect("write");
        std::fs::write(&case.ours, ours).expect("write");
        std::fs::write(&case.theirs, theirs).expect("write");
        case
    }

    fn run(&self, extra: &[&str]) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_sm"))
            .current_dir(self.dir.path())
            .arg("check")
            .arg(&self.base)
            .arg(&self.ours)
            .arg(&self.theirs)
            .args(extra)
            .output()
            .expect("running sm check");
        Run { out }
    }
}

struct Run {
    out: Output,
}

impl Run {
    fn code(&self) -> i32 {
        self.out.status.code().expect("no signal")
    }
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.out.stdout).into_owned()
    }
    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.out.stderr).into_owned()
    }
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.stdout()).expect("stdout is JSON")
    }
}

// ------------------------------------------------------------------ fixtures

/// SPEC.md §1's opening example: we rename, they add a call to the old name.
const BASE: &str = "\
class Repo {
  User getUser(String id) {
    return store.find(id);
  }

  User cached(String id) {
    return getUser(id);
  }
}
";
const OURS: &str = "\
class Repo {
  User fetchUser(String id) {
    return store.find(id);
  }

  User cached(String id) {
    return fetchUser(id);
  }
}
";
const THEIRS: &str = "\
class Repo {
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

// -------------------------------------------------------------------- tests

/// The demo: three files, one command, and the finding in prose.
#[test]
fn a_broken_reference_is_reported_and_exits_one() {
    let case = Case::new("java", BASE, OURS, THEIRS);
    let run = case.run(&[]);

    assert_eq!(run.code(), 1, "found something ⇒ exit 1");
    let out = run.stdout();
    assert!(out.contains("tree merge: clean"), "{out}");
    assert!(out.contains("semantic conflicts: 1"), "{out}");
    assert!(out.contains("[broken_reference] `getUser`"), "{out}");
    assert!(
        out.contains("fetchUser"),
        "the explanation names the rename:\n{out}"
    );
}

/// Nothing to report is exit 0 and says so, rather than printing nothing.
#[test]
fn a_merge_that_breaks_no_names_exits_zero() {
    let case = Case::new(
        "java",
        "class C {\n  int a = 1;\n  int b = 2;\n}\n",
        "class C {\n  int a = 11;\n  int b = 2;\n}\n",
        "class C {\n  int a = 1;\n  int b = 22;\n}\n",
    );
    let run = case.run(&[]);
    assert_eq!(run.code(), 0);
    assert!(
        run.stdout().contains("semantic conflicts: 0"),
        "{}",
        run.stdout()
    );
    assert!(
        run.stdout().contains("Nothing to report"),
        "silence would look like a bug:\n{}",
        run.stdout()
    );
}

/// `--exit-zero` keeps the report and drops the verdict, for a scan that wants
/// to collect findings without a non-zero exit stopping it.
#[test]
fn exit_zero_reports_without_failing() {
    let case = Case::new("java", BASE, OURS, THEIRS);
    let run = case.run(&["--exit-zero"]);
    assert_eq!(run.code(), 0);
    assert!(run.stdout().contains("semantic conflicts: 1"));
}

/// The JSON form is what M5's corpus scan reads, so its shape is asserted.
#[test]
fn the_json_form_has_the_documented_shape() {
    let case = Case::new("java", BASE, OURS, THEIRS);
    let run = case.run(&["--json"]);
    assert_eq!(run.code(), 1, "--json does not change the verdict");

    let doc = run.json();
    assert_eq!(doc["schema_version"], 1);
    assert!(doc["tool"].as_str().expect("tool").starts_with("sm-cli"));
    assert_eq!(doc["language"], "java");
    assert_eq!(doc["tree_merge_clean"], true);

    let conflicts = doc["conflicts"].as_array().expect("conflicts");
    assert_eq!(conflicts.len(), 1);
    let c = &conflicts[0];
    assert_eq!(c["kind"], "broken_reference");
    assert_eq!(c["name"], "getUser");
    assert_eq!(c["reference"]["side"], "theirs");
    assert!(c["reference"]["range"]["start"].is_u64());
    assert_eq!(c["origin_declaration"]["kind"], "method");
    assert_eq!(c["origin_declaration"]["scope_kind"], "class_declaration");
    assert!(c["merged_declaration"].is_null());
    assert!(
        c["explanation"]
            .as_str()
            .expect("explanation")
            .contains("fetchUser")
    );

    for key in [
        "references",
        "resolved_in_origin",
        "unresolved_in_origin",
        "origin_not_found",
        "conflict_regions",
        "conflicts",
    ] {
        assert!(doc["stats"][key].is_u64(), "stats.{key} missing");
    }
}

/// TypeScript goes through the same command; the point of the `Language` trait
/// is that nothing above it is language-specific.
#[test]
fn typescript_works_the_same_way() {
    let case = Case::new(
        "ts",
        "function getUser(id: string) { return find(id); }\n\nexport const a = getUser(\"1\");\n",
        "function fetchUser(id: string) { return find(id); }\n\nexport const a = fetchUser(\"1\");\n",
        "function getUser(id: string) { return find(id); }\n\nexport const a = getUser(\"1\");\n\nexport const b = getUser(\"2\");\n",
    );
    let run = case.run(&["--json"]);
    assert_eq!(run.code(), 1, "{}", run.stdout());
    assert_eq!(run.json()["language"], "typescript");
    assert_eq!(run.json()["conflicts"][0]["kind"], "broken_reference");
}

/// A conflicted tree merge is still checked — `sm-bind` walks the plan and
/// skips conflict regions — and the report says how much it skipped, so the
/// number can be read honestly.
#[test]
fn a_conflicted_merge_is_still_checked_and_says_what_it_skipped() {
    let case = Case::new(
        "java",
        "class C {\n  int x = 1;\n}\n",
        "class C {\n  int x = 2;\n}\n",
        "class C {\n  int x = 3;\n}\n",
    );
    let run = case.run(&[]);
    assert_eq!(run.code(), 0, "no names were broken");
    assert!(
        run.stdout().contains("tree merge: 1 conflict region(s)"),
        "{}",
        run.stdout()
    );
    assert!(
        run.stdout().contains("skipped conflict regions:"),
        "{}",
        run.stdout()
    );
}

/// An unreadable file or an unknown extension is exit 2, the same usage code
/// every other inspection subcommand uses.
#[test]
fn an_unusable_input_is_a_usage_error() {
    let case = Case::new("cobol", BASE, OURS, THEIRS);
    let run = case.run(&[]);
    assert_eq!(run.code(), 2);
    assert!(
        run.stderr().contains("unsupported file type"),
        "{}",
        run.stderr()
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("nope.java");
    let out = Command::new(env!("CARGO_BIN_EXE_sm"))
        .arg("check")
        .arg(&missing)
        .arg(&missing)
        .arg(&missing)
        .output()
        .expect("running sm check");
    assert_eq!(out.status.code(), Some(2));
}

/// Three files of different languages is a usage error, not a guess.
#[test]
fn mixing_languages_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let java: PathBuf = dir.path().join("a.java");
    let ts: PathBuf = dir.path().join("a.ts");
    std::fs::write(&java, "class C {}\n").expect("write");
    std::fs::write(&ts, "const x = 1;\n").expect("write");
    let out = Command::new(env!("CARGO_BIN_EXE_sm"))
        .arg("check")
        .arg(&java)
        .arg(&java)
        .arg(&ts)
        .output()
        .expect("running sm check");
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("different languages"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A file with syntax errors is checked anyway, with a warning — the same
/// policy `sm match` and `sm diff` follow, and the reason is in `sm-bind`'s
/// crate docs: both sides see the same recovered tree, so the weakness cancels.
#[test]
fn a_broken_input_warns_but_still_reports() {
    let case = Case::new(
        "java",
        "class C {\n  void f() { }\n}\n",
        "class C {\n  void f() { if ( }\n}\n",
        "class C {\n  void f() { g(); }\n}\n",
    );
    let run = case.run(&[]);
    assert!(
        run.stderr().contains("ERROR/MISSING nodes"),
        "{}",
        run.stderr()
    );
    assert!(
        run.stdout().contains("semantic conflicts:"),
        "{}",
        run.stdout()
    );
}

/// The command is discoverable: it is in `--help` and it has its own help.
#[test]
fn the_subcommand_is_documented_in_help() {
    let out = Command::new(env!("CARGO_BIN_EXE_sm"))
        .arg("--help")
        .output()
        .expect("running sm");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("check"), "{text}");

    let out = Command::new(env!("CARGO_BIN_EXE_sm"))
        .args(["check", "--help"])
        .output()
        .expect("running sm");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("--json"), "{text}");
    assert!(text.contains("BASE"), "{text}");
}

/// Paths in the output are the ones the caller passed, so a scan can correlate.
#[test]
fn nothing_is_written_anywhere() {
    let case = Case::new("java", BASE, OURS, THEIRS);
    let before: Vec<String> = entries(case.dir.path());
    case.run(&[]);
    assert_eq!(before, entries(case.dir.path()), "sm check wrote something");
    assert_eq!(
        std::fs::read_to_string(&case.ours).expect("read"),
        OURS,
        "sm check modified an input"
    );
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read_dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}
