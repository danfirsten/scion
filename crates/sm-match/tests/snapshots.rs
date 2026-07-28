//! Snapshot tests over hand-written Java pairs.
//!
//! SPEC.md §4.3 asks for `insta` snapshots on hand-written pairs, and §0.3 says
//! never to let correctness be unmeasurable. A matching is a set of ~200 node
//! pairs; an assertion about it that a human can read is not possible, but a
//! *diff* of it is. So each case is rendered in the stable text form
//! [`sm_match::render_pairs`] produces — `src_id:kind@range <-> dst_id:kind@range`,
//! sorted by source ID — and both shipped profiles go in one snapshot so the
//! permissive/strict difference shows up as a diff rather than as two files
//! nobody compares.
//!
//! When one of these changes, read the diff. A matcher change that improves one
//! case and silently wrecks another is the failure mode this file exists to
//! catch.

mod support;

use std::fmt::Write as _;

use sm_match::visualize::{
    MatchReport, Side, VisualizeOptions, render_side_by_side, render_summary, summarize,
};
use sm_match::{MatchConfig, TreeMetrics, match_trees, render_pairs};
use support::{CASES, PROFILES, java, match_case, parse_case};

/// One document per case: the header, then each profile's matching.
fn snapshot_body(case: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "case: {case}");

    for (name, make) in PROFILES {
        let cfg = make();
        let (a, b, m) = match_case(case, &cfg);
        let am = TreeMetrics::compute(&a, java());
        let bm = TreeMetrics::compute(&b, java());
        let summary = summarize(Side::new(&a, &am, "a"), Side::new(&b, &bm, "b"), &m);

        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "--- profile {name} (min_height={} min_dice={} max_size={}) ---",
            cfg.min_height, cfg.min_dice, cfg.max_size
        );
        let _ = writeln!(
            out,
            "matchable: a={} b={}   matched: {}   moves: {}",
            summary.src_matchable, summary.dst_matchable, summary.matched, summary.moved
        );
        out.push_str(&render_pairs(&m, &a, &b));
    }
    out
}

macro_rules! case_snapshot {
    ($name:ident, $case:literal) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!($case, snapshot_body($case));
        }
    };
}

case_snapshot!(identical, "identical");
case_snapshot!(renamed_method, "renamed_method");
case_snapshot!(moved_method, "moved_method");
case_snapshot!(moved_and_edited_method, "moved_and_edited_method");
case_snapshot!(wrapped_in_if, "wrapped_in_if");
case_snapshot!(extracted_variable, "extracted_variable");
case_snapshot!(import_churn, "import_churn");
case_snapshot!(reordered_members, "reordered_members");
case_snapshot!(moved_comment, "moved_comment");
case_snapshot!(unrelated, "unrelated");
case_snapshot!(empty_vs_nonempty, "empty_vs_nonempty");

/// Every declared case must have both sides on disk.
#[test]
fn every_case_has_both_sides() {
    for case in CASES {
        for side in ["a", "b"] {
            let path = support::case_path(case, side);
            assert!(path.exists(), "missing fixture {}", path.display());
        }
    }
}

/// Pin the eyeball tool's own output on one representative case.
///
/// This is the view a human uses to judge whether a matching is right
/// (SPEC.md §4.3), so its layout is part of the deliverable, not an
/// implementation detail. Colour is off: ANSI escapes in a committed snapshot
/// are unreadable, and the colour path adds no structure the plain path lacks.
#[test]
fn side_by_side_view_of_a_moved_method() {
    let (a, b) = parse_case("moved_method");
    let cfg = MatchConfig::base_to_side();
    let am = TreeMetrics::compute(&a, java());
    let bm = TreeMetrics::compute(&b, java());
    let m = match_trees(&a, &b, java(), &cfg);

    let side_a = Side::new(&a, &am, "moved_method/a.java");
    let side_b = Side::new(&b, &bm, "moved_method/b.java");
    let mut rendered = render_side_by_side(
        side_a,
        side_b,
        java(),
        &m,
        &VisualizeOptions {
            width: 120,
            color: false,
            max_text_len: 20,
        },
    );
    rendered.push_str(&render_summary(&summarize(side_a, side_b, &m), &cfg, false));

    insta::assert_snapshot!("side_by_side_moved_method", rendered);
}

/// The `--json` payload is a wire format other tools consume; pin its shape.
#[test]
fn json_report_shape_is_stable() {
    let (a, b) = parse_case("renamed_method");
    let cfg = MatchConfig::base_to_side();
    let am = TreeMetrics::compute(&a, java());
    let bm = TreeMetrics::compute(&b, java());
    let m = match_trees(&a, &b, java(), &cfg);

    let report = MatchReport::new(
        Side::new(&a, &am, "a.java"),
        Side::new(&b, &bm, "b.java"),
        &m,
        &cfg,
    );
    let json = serde_json::to_string_pretty(&report).expect("serialize");
    // Only the first pairs: the whole list is already covered by the matching
    // snapshots, and what this test is for is the *shape*.
    let value: serde_json::Value = serde_json::from_str(&json).expect("parse back");
    let mut trimmed = value.clone();
    trimmed["pairs"] = serde_json::Value::Array(
        value["pairs"]
            .as_array()
            .expect("pairs is an array")
            .iter()
            .take(3)
            .cloned()
            .collect(),
    );
    insta::assert_snapshot!(
        "json_report",
        serde_json::to_string_pretty(&trimmed).expect("re-serialize")
    );
}
