//! End-to-end tests for `sm diff`.
//!
//! These drive the real binary through `CARGO_BIN_EXE_sm`, because the things
//! worth testing here are the things the library cannot get wrong on its own:
//! argument parsing, exit codes, colour policy, and which stream each kind of
//! output lands on. The *content* of the rendering is `sm-diff`'s own snapshot
//! suite's problem.

use std::path::PathBuf;
use std::process::{Command, Output};

fn sm() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sm"))
}

/// A fixture pair from `sm-match`'s corpus, as `cli_match.rs` already does.
/// Reusing them keeps the CLI tests from inventing a third set of Java
/// snippets.
fn fixture(case: &str, side: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../sm-match/tests/fixtures")
        .join(case)
        .join(format!("{side}.java"))
}

fn run(args: &[&str]) -> Output {
    sm().args(args).output().expect("running sm")
}

fn diff(case: &str, extra: &[&str]) -> Output {
    let a = fixture(case, "a");
    let b = fixture(case, "b");
    let mut args = vec!["diff", a.to_str().unwrap(), b.to_str().unwrap()];
    args.extend_from_slice(extra);
    run(&args)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn diff_reports_a_wrapped_block_as_an_insert_and_a_move() {
    let out = diff("wrapped_in_if", &["--color", "never"]);
    assert!(out.status.success(), "exit: {:?}", out.status);
    let text = stdout(&out);
    assert!(text.contains("wrapped_in_if"), "{text}");
    assert!(text.contains("+ insert"), "no insert hunk in:\n{text}");
    assert!(text.contains("~ reparent"), "no move hunk in:\n{text}");
    // The killer claim: the body did not change, it only moved and reindented.
    assert!(text.contains("reindented"), "no reindent tag in:\n{text}");
    // And the statements themselves are never printed as changed lines.
    assert!(!text.contains("job.prepare();"), "{text}");
}

#[test]
fn identical_files_produce_no_hunks() {
    let a = fixture("identical", "a");
    let out = run(&[
        "diff",
        a.to_str().unwrap(),
        a.to_str().unwrap(),
        "--color",
        "never",
    ]);
    assert!(out.status.success());
    assert!(
        stdout(&out).contains("no structural changes"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn stat_prints_one_summary_line() {
    let out = diff("wrapped_in_if", &["--stat"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(text.contains("insert"), "{text}");
    assert!(text.contains("move"), "{text}");
}

#[test]
fn json_is_a_versioned_object_with_one_entry_per_operation() {
    let out = diff("wrapped_in_if", &["--json"]);
    assert!(out.status.success());
    let value: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("stdout should be JSON");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["src"]["language"], "java");
    let ops = value["ops"].as_array().expect("ops array");
    assert_eq!(ops.len(), 2, "{value:#}");
    let summary = &value["summary"];
    assert_eq!(
        summary["inserts"].as_u64().unwrap() + summary["moves"].as_u64().unwrap(),
        ops.len() as u64
    );
    assert!(ops.iter().any(|op| op["op"] == "insert"), "{value:#}");
    assert!(
        ops.iter()
            .any(|op| op["op"] == "move" && op["move_kind"] == "reparent"),
        "{value:#}"
    );
}

#[test]
fn color_is_off_when_asked_and_on_when_forced() {
    assert!(!stdout(&diff("renamed_method", &["--color", "never"])).contains('\x1b'));
    assert!(stdout(&diff("renamed_method", &["--color", "always"])).contains('\x1b'));
}

/// Piping is the normal case, and a pipe is not a terminal, so `auto` must mean
/// "plain" here. This is the check that catches a colour default that only
/// looks right when a human runs it.
#[test]
fn color_auto_is_plain_when_stdout_is_not_a_terminal() {
    assert!(!stdout(&diff("renamed_method", &[])).contains('\x1b'));
}

#[test]
fn exit_code_is_opt_in() {
    assert!(diff("renamed_method", &[]).status.success());
    let out = diff("renamed_method", &["--exit-code"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "differing files with --exit-code"
    );

    let a = fixture("identical", "a");
    let out = run(&[
        "diff",
        a.to_str().unwrap(),
        a.to_str().unwrap(),
        "--exit-code",
    ]);
    assert!(out.status.success(), "identical files with --exit-code");
}

#[test]
fn an_unreadable_file_is_a_usage_error() {
    let a = fixture("identical", "a");
    let out = run(&["diff", a.to_str().unwrap(), "/nonexistent/Nope.java"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stdout(&out).is_empty(), "errors belong on stderr");
}

#[test]
fn an_unsupported_extension_is_a_usage_error() {
    let a = fixture("identical", "a");
    let out = run(&["diff", a.to_str().unwrap(), "Cargo.toml"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn both_profiles_are_accepted() {
    for profile in ["base", "strict"] {
        let out = diff("moved_and_edited_method", &["--profile", profile, "--stat"]);
        assert!(out.status.success(), "--profile {profile}");
    }
}
