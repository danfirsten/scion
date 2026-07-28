//! Conflict-marker rendering: size, labels, style and line discipline.

mod support;

use sm_emit::{ConflictStyle, EmitOptions};
use support::{java, run_with};

const BASE: &str = "class C {\n  void a() {\n    p();\n  }\n}\n";
const OURS: &str = "class C {\n  void a() {\n    ours();\n  }\n}\n";
const THEIRS: &str = "class C {\n  void a() {\n    theirs();\n  }\n}\n";

fn emitted(opts: &EmitOptions) -> String {
    let m = run_with(BASE, OURS, THEIRS, java(), opts);
    assert!(!m.is_clean());
    m.text()
}

#[test]
fn default_markers_are_seven_characters_with_ours_and_theirs_labels() {
    let text = emitted(&EmitOptions::default());
    insta::assert_snapshot!(text);
}

/// Git passes the marker size as `%L`. A file that legitimately contains a
/// seven-character marker line is merged with a longer one, and honouring that
/// is not optional.
#[test]
fn the_marker_size_is_honoured() {
    let text = emitted(&EmitOptions {
        marker_size: 12,
        ..EmitOptions::default()
    });
    assert!(text.contains("<<<<<<<<<<<< ours"), "{text}");
    assert!(text.contains("\n============\n"), "{text}");
    assert!(text.contains(">>>>>>>>>>>> theirs"), "{text}");
    assert!(!text.contains("<<<<<<<<<<<<<"), "exactly twelve:\n{text}");
}

/// Below git's minimum the size is clamped rather than obeyed: a five-character
/// marker is ambiguous with ordinary text and git itself never emits one.
#[test]
fn a_too_small_marker_size_is_clamped() {
    let text = emitted(&EmitOptions {
        marker_size: 2,
        ..EmitOptions::default()
    });
    assert!(text.contains("<<<<<<< ours"), "{text}");
}

/// `%X` / `%Y` / `%S`.
#[test]
fn labels_come_from_the_options() {
    let text = emitted(
        &EmitOptions {
            style: ConflictStyle::Diff3,
            ..EmitOptions::default()
        }
        .with_labels("HEAD", "feature/x", "merged common ancestors"),
    );
    insta::assert_snapshot!(text);
}

/// An empty label produces a bare marker line, which is what git does when it
/// has no name for a side.
#[test]
fn empty_labels_produce_bare_marker_lines() {
    let text = emitted(&EmitOptions::default().with_labels("", "", ""));
    assert!(text.contains("\n<<<<<<<\n"), "{text}");
    assert!(text.contains("\n>>>>>>>\n"), "{text}");
}

#[test]
fn diff3_style_includes_the_ancestor() {
    let text = emitted(&EmitOptions {
        style: ConflictStyle::Diff3,
        ..EmitOptions::default()
    });
    insta::assert_snapshot!(text);
}

/// A deleted side renders as an empty block rather than as a missing one, so
/// the marker structure is uniform whatever happened.
#[test]
fn a_deletion_renders_as_an_empty_side() {
    let m = run_with(
        "class C {\n  void a() { x(); }\n\n  void b() {}\n}\n",
        "class C {\n  void b() {}\n}\n",
        "class C {\n  void a() { x(); z(); }\n\n  void b() {}\n}\n",
        java(),
        &EmitOptions::default(),
    );
    let text = m.text();
    assert!(text.contains("<<<<<<< ours\n=======\n"), "{text}");
    insta::assert_snapshot!(text);
}

// -------------------------------------------------------- line discipline

/// Every marker line starts a line of its own, and none of them acquires the
/// indentation the gap in front of the conflict was about to place.
#[test]
fn marker_lines_start_at_column_zero() {
    let text = emitted(&EmitOptions::default());
    for line in text.lines() {
        if line.contains("<<<<<<<") || line.contains("=======") || line.contains(">>>>>>>") {
            assert!(
                line.starts_with('<') || line.starts_with('=') || line.starts_with('>'),
                "marker line is indented: {line:?}"
            );
        }
    }
}

/// No blank line is left behind after the closing marker.
#[test]
fn nothing_blank_follows_the_closing_marker() {
    let text = emitted(&EmitOptions::default());
    let lines: Vec<&str> = text.lines().collect();
    let close = lines
        .iter()
        .position(|l| l.starts_with(">>>>>>>"))
        .expect("a closing marker");
    assert!(
        lines.get(close + 1).is_some_and(|l| !l.trim().is_empty()),
        "a blank line follows the closing marker:\n{text}"
    );
}

/// A conflict at the very end of a file still terminates with a newline.
#[test]
fn a_trailing_conflict_ends_with_a_newline() {
    // No trailing newline anywhere, and the conflict is the last thing in the
    // file: the emitter has to supply the newline the marker line needs.
    let m = run_with(
        "class C {}",
        "class D {}",
        "class E {}",
        java(),
        &EmitOptions::default(),
    );
    assert!(!m.is_clean());
    let text = m.text();
    assert!(text.ends_with(">>>>>>> theirs\n"), "{text:?}");
    insta::assert_snapshot!(text);
}

/// Conflicting code keeps its own indentation inside the markers.
#[test]
fn conflicting_code_keeps_its_indentation() {
    let m = run_with(
        "class C {\n  void a() {\n    if (p) {\n      deep();\n    }\n  }\n}\n",
        "class C {\n  void a() {\n    if (p) {\n      ours();\n    }\n  }\n}\n",
        "class C {\n  void a() {\n    if (p) {\n      theirs();\n    }\n  }\n}\n",
        java(),
        &EmitOptions::default(),
    );
    let text = m.text();
    assert!(text.contains("\n      ours();\n"), "{text}");
    assert!(text.contains("\n      theirs();\n"), "{text}");
    insta::assert_snapshot!(text);
}

/// Two conflicts in one file do not run into each other.
#[test]
fn two_conflicts_in_one_file_are_both_well_formed() {
    let m = run_with(
        "class C {\n  int a = 1;\n\n  int b = 1;\n}\n",
        "class C {\n  int a = 2;\n\n  int b = 2;\n}\n",
        "class C {\n  int a = 3;\n\n  int b = 3;\n}\n",
        java(),
        &EmitOptions::default(),
    );
    assert_eq!(m.result.conflict_count, 2);
    let text = m.text();
    assert_eq!(text.matches("<<<<<<<").count(), 2);
    assert_eq!(text.matches(">>>>>>>").count(), 2);
    insta::assert_snapshot!(text);
}

/// `conflict_count` is what the driver's exit code is derived from, so it has
/// to agree with the merge's own count and with what is actually rendered.
#[test]
fn the_conflict_count_agrees_with_the_merge_and_with_the_text() {
    let m = run_with(
        "class C {\n  int a = 1;\n\n  int b = 1;\n}\n",
        "class C {\n  int a = 2;\n\n  int b = 2;\n}\n",
        "class C {\n  int a = 3;\n\n  int b = 3;\n}\n",
        java(),
        &EmitOptions::default(),
    );
    assert_eq!(m.result.conflict_count, m.outcome.conflicts.len());
    assert_eq!(m.result.conflict_count, m.text().matches("<<<<<<<").count());
}
