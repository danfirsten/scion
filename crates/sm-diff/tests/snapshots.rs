//! Snapshot tests over the edit scripts derived from hand-written pairs.
//!
//! One document per case, holding the stable one-operation-per-line dump
//! [`sm_diff::render_script`] produces. When one of these changes, read the
//! diff: a derivation change that improves one case and silently wrecks another
//! is exactly what this file exists to catch (the same argument `sm-match`'s
//! snapshot suite makes).
//!
//! Only the permissive `base` profile is snapshotted. The strict profile
//! changes the *matching*, which `sm-match` already pins case by case; what is
//! interesting here is the derivation on top of it, and duplicating every
//! document would halve the signal in a diff.

mod support;

use sm_diff::{RenderOptions, render, render_script};
use sm_match::MatchConfig;
use support::{CASES, load_case};

fn script_snapshot(case: &str, ext: &str) -> String {
    let loaded = load_case(case, ext, &MatchConfig::base_to_side());
    format!("case: {case}\n\n{}", render_script(&loaded.view()))
}

macro_rules! case_snapshot {
    ($name:ident, $case:literal, $ext:literal) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!($case, script_snapshot($case, $ext));
        }
    };
}

case_snapshot!(identical, "identical", "java");
case_snapshot!(renamed_method, "renamed_method", "java");
case_snapshot!(moved_method, "moved_method", "java");
case_snapshot!(moved_across_classes, "moved_across_classes", "java");
case_snapshot!(reordered_statements, "reordered_statements", "java");
case_snapshot!(moved_and_edited_method, "moved_and_edited_method", "java");
case_snapshot!(wrapped_in_if, "wrapped_in_if", "java");
case_snapshot!(extracted_variable, "extracted_variable", "java");
case_snapshot!(import_shuffle, "import_shuffle", "java");
case_snapshot!(insert_method, "insert_method", "java");
case_snapshot!(delete_method, "delete_method", "java");
case_snapshot!(reindented_method, "reindented_method", "java");
case_snapshot!(stress, "stress", "java");
case_snapshot!(ts_class_edit, "ts_class_edit", "ts");

/// Every declared case must have both sides on disk.
#[test]
fn every_case_has_both_sides() {
    for &(case, ext) in CASES {
        for side in ["a", "b"] {
            let path = support::case_path(case, ext, side);
            assert!(path.exists(), "missing fixture {}", path.display());
        }
    }
}

/// Every case must be valid source. A fixture with a syntax error would be
/// diffed against a broken tree and the snapshot would pin nonsense.
#[test]
fn every_fixture_parses_without_errors() {
    for &(case, ext) in CASES {
        for side in ["a", "b"] {
            let tree = support::parse_side(case, ext, side);
            assert!(
                !tree.has_errors(),
                "{case}/{side}.{ext} does not parse cleanly"
            );
        }
    }
}

/// The viewer's own output on the case M3 exists to win: three statements
/// wrapped in an `if`.
///
/// `git diff` calls this seven changed lines. What it should say — and what
/// this snapshot pins — is that two lines of wrapper were inserted and the
/// block moved into them unchanged. Colour is off: ANSI escapes in a committed
/// snapshot are unreadable, and the colour path adds no structure.
#[test]
fn rendered_view_of_a_wrapped_block() {
    let loaded = load_case("wrapped_in_if", "java", &MatchConfig::base_to_side());
    insta::assert_snapshot!(
        "render_wrapped_in_if",
        render(&loaded.view(), &RenderOptions::default())
    );
}

/// The other headline case: a method that moved to another class and did not
/// otherwise change. `git diff` reports it as six deleted lines plus six added
/// ones.
#[test]
fn rendered_view_of_a_moved_method() {
    let loaded = load_case("moved_across_classes", "java", &MatchConfig::base_to_side());
    insta::assert_snapshot!(
        "render_moved_across_classes",
        render(&loaded.view(), &RenderOptions::default())
    );
}

/// Moving a method *within* a class is not an edit at all: `class_body` is an
/// unordered child list, so the viewer says so instead of printing eight
/// changed lines.
#[test]
fn rendered_view_of_a_reordered_class_body() {
    let loaded = load_case("moved_method", "java", &MatchConfig::base_to_side());
    insta::assert_snapshot!(
        "render_moved_method",
        render(&loaded.view(), &RenderOptions::default())
    );
}

/// A statement permutation inside an *ordered* block, where the LIS is what
/// keeps the report to one operation.
#[test]
fn rendered_view_of_reordered_statements() {
    let loaded = load_case("reordered_statements", "java", &MatchConfig::base_to_side());
    insta::assert_snapshot!(
        "render_reordered_statements",
        render(&loaded.view(), &RenderOptions::default())
    );
}

/// A rename, where the interesting part is the word-level attribution.
#[test]
fn rendered_view_of_a_rename() {
    let loaded = load_case("renamed_method", "java", &MatchConfig::base_to_side());
    insta::assert_snapshot!(
        "render_renamed_method",
        render(&loaded.view(), &RenderOptions::default())
    );
}

/// Everything at once, as a guard on the hunk nesting.
#[test]
fn rendered_view_of_the_stress_case() {
    let loaded = load_case("stress", "java", &MatchConfig::base_to_side());
    insta::assert_snapshot!(
        "render_stress",
        render(&loaded.view(), &RenderOptions::default())
    );
}

/// The `--json` payload is a wire format other tools consume; pin its shape.
#[test]
fn json_report_shape_is_stable() {
    let loaded = load_case(
        "moved_and_edited_method",
        "java",
        &MatchConfig::base_to_side(),
    );
    let report = sm_diff::DiffReport::new(&loaded.view());
    insta::assert_snapshot!(
        "json_report",
        serde_json::to_string_pretty(&report).expect("serializing the report")
    );
}
