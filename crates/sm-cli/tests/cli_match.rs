//! End-to-end tests for `sm match`.
//!
//! These drive the real binary through `CARGO_BIN_EXE_sm`, because the things
//! worth testing here are the things the library cannot get wrong on its own:
//! argument parsing, exit codes, and which stream each kind of output lands on.

use std::path::PathBuf;
use std::process::{Command, Output};

fn sm() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sm"))
}

/// A fixture pair from `sm-match`'s corpus. Reusing them keeps the CLI tests
/// from inventing a second, divergent set of Java snippets.
fn fixture(case: &str, side: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../sm-match/tests/fixtures")
        .join(case)
        .join(format!("{side}.java"))
}

fn run(args: &[&str]) -> Output {
    sm().args(args).output().expect("running sm")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn match_renders_two_columns_and_a_summary() {
    let out = run(&[
        "match",
        fixture("moved_method", "a").to_str().unwrap(),
        fixture("moved_method", "b").to_str().unwrap(),
        "--color",
        "never",
        "--width",
        "120",
    ]);
    assert!(out.status.success(), "exit status {:?}", out.status);

    let text = stdout(&out);
    assert!(text.contains("method_declaration"), "{text}");
    assert!(text.contains(" | "), "no column separator in:\n{text}");
    assert!(text.contains("moves: 3"), "{text}");
    assert!(
        text.contains("matched: 52 pairs"),
        "unexpected summary in:\n{text}"
    );
    // A move must be flagged where a human will see it.
    assert!(text.contains(" M "), "no move marker in:\n{text}");
}

#[test]
fn summary_only_suppresses_the_columns() {
    let out = run(&[
        "match",
        fixture("moved_method", "a").to_str().unwrap(),
        fixture("moved_method", "b").to_str().unwrap(),
        "--color",
        "never",
        "--summary-only",
    ]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(!text.contains("method_declaration"), "{text}");
    assert!(text.contains("matched: 52 pairs"), "{text}");
}

#[test]
fn the_two_profiles_are_selectable_and_differ() {
    let a = fixture("unrelated", "a").to_str().unwrap().to_owned();
    let b = fixture("unrelated", "b").to_str().unwrap().to_owned();

    let base = run(&["match", &a, &b, "--color", "never", "--profile", "base"]);
    let strict = run(&["match", &a, &b, "--color", "never", "--profile", "strict"]);
    assert!(base.status.success() && strict.status.success());
    assert!(stdout(&base).contains("min_height=1 min_dice=0.4"));
    assert!(stdout(&strict).contains("min_height=2 min_dice=0.6"));
}

#[test]
fn json_output_is_valid_json_and_carries_the_pairs() {
    let out = run(&[
        "match",
        fixture("renamed_method", "a").to_str().unwrap(),
        fixture("renamed_method", "b").to_str().unwrap(),
        "--json",
    ]);
    assert!(out.status.success());

    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["config"]["min_height"], 1);
    assert_eq!(value["summary"]["matched"], 33);
    let pairs = value["pairs"].as_array().expect("pairs array");
    assert_eq!(pairs.len(), 33);
    assert_eq!(pairs[0]["kind"], "program");
    assert_eq!(pairs[0]["src_id"], 0);
}

#[test]
fn an_unreadable_file_exits_two_and_says_why() {
    let out = run(&["match", "does/not/exist.java", "also/missing.java"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("does/not/exist.java"), "{err}");
}

#[test]
fn an_unknown_extension_exits_two_and_lists_the_known_ones() {
    let out = run(&["match", "a.txt", "b.txt"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("unsupported file type"), "{err}");
    assert!(err.contains("java"), "{err}");
}

/// `sm parse` must keep working exactly as M0 left it.
#[test]
fn parse_still_works() {
    let out = run(&["parse", fixture("identical", "a").to_str().unwrap()]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("class_declaration"));
}
