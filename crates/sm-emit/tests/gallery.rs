//! The scenario gallery: one snapshot of every case's conflict reasons and
//! emitted output, plus the mirrored run of each.
//!
//! A single snapshot rather than one per case, because the thing worth
//! reviewing is the *set* of answers: a change to the merge shows up as a diff
//! across every case it affects, in one place.

mod support;

use std::fmt::Write as _;

use support::{run_scenario, scenarios};

fn gallery(mirrored: bool) -> String {
    let mut out = String::new();
    for sc in scenarios() {
        let case = if mirrored { sc.mirrored() } else { sc };
        let m = run_scenario(&case);
        let _ = writeln!(out, "=== {} [{}]", case.name, case.lang);
        let reasons = m.reasons();
        let _ = writeln!(
            out,
            "conflicts: {}",
            if reasons.is_empty() {
                "none".to_owned()
            } else {
                reasons.join(", ")
            }
        );
        let _ = writeln!(
            out,
            "reindented lines: {}  synthesized bytes: {}",
            m.result.reindented_lines, m.result.synthesized_bytes
        );
        let _ = writeln!(out, "---");
        out.push_str(&m.text());
        if !out.ends_with('\n') {
            out.push_str("<no trailing newline>\n");
        }
        let _ = writeln!(out);
    }
    out
}

#[test]
fn scenario_gallery() {
    insta::assert_snapshot!(gallery(false));
}

#[test]
fn scenario_gallery_mirrored() {
    insta::assert_snapshot!(gallery(true));
}
