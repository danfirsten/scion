//! Properties the check must hold whatever the input, in the spirit of
//! SPEC.md §5.
//!
//! The scenario suites say "this input produces that report". These say things
//! that must be true of *every* input, and they are what catches a class of bug
//! a hand-written scenario never would.

mod support;

use support::{java, run, typescript};

/// A gallery of triples, each exercising a different corner: identity, both
/// sides idle, convergent edits, a conflict, a rename, a capture, a broken
/// file.
fn corpus() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    vec![
        (
            "identity",
            "class C { int a() { return 1; } }\n",
            "class C { int a() { return 1; } }\n",
            "class C { int a() { return 1; } }\n",
        ),
        (
            "one side idle",
            "class C {\n    int f;\n    int a() { return f; }\n}\n",
            "class C {\n    int f;\n    int a() { return f; }\n}\n",
            "class C {\n    int f;\n    int a() { return f; }\n    int b() { return f + 1; }\n}\n",
        ),
        (
            "convergent",
            "class C {\n    int a() { return 1; }\n}\n",
            "class C {\n    int a() { return 2; }\n}\n",
            "class C {\n    int a() { return 2; }\n}\n",
        ),
        (
            "conflict",
            "class C {\n    int a() { return 1; }\n}\n",
            "class C {\n    int a() { return 2; }\n}\n",
            "class C {\n    int a() { return 3; }\n}\n",
        ),
        (
            "rename versus new call",
            "class C {\n    int helper() { return 1; }\n    int a() { return helper(); }\n}\n",
            "class C {\n    int renamed() { return 1; }\n    int a() { return renamed(); }\n}\n",
            "class C {\n    int helper() { return 1; }\n    int a() { return helper(); }\n    int b() { return helper(); }\n}\n",
        ),
        (
            "capture",
            "class C {\n    int v;\n    void m() {\n        int q = 0;\n    }\n}\n",
            "class C {\n    int v;\n    void m() {\n        int q = 0;\n        use(v);\n    }\n}\n",
            "class C {\n    int v;\n    void m() {\n        int v = 9;\n        int q = 0;\n    }\n}\n",
        ),
        (
            "unparseable",
            "class C {\n    int a() { return 1; }\n}\n",
            "class C {\n    int a() { return 1 }\n",
            "class C {\n    int a() { return 1; }\n    int b() { return 2; }\n}\n",
        ),
        (
            "empty ancestor",
            "",
            "class C { int a() { return 1; } }\n",
            "class C { int b() { return 2; } }\n",
        ),
    ]
}

/// **`merge(B, X, B)` reports nothing.** One side did nothing, so nothing the
/// merge did can have changed what a name means: the result is `X` verbatim and
/// every reference resolves exactly as it did there.
#[test]
fn a_merge_with_one_side_unchanged_reports_nothing() {
    for (label, base, ours, _) in corpus() {
        if base.is_empty() {
            continue;
        }
        let s = run(java(), base, ours, base);
        assert!(
            s.report.conflicts.is_empty(),
            "{label} (B, X, B) reported:\n{}",
            s.render()
        );
        let s = run(java(), base, base, ours);
        assert!(
            s.report.conflicts.is_empty(),
            "{label} (B, B, X) reported:\n{}",
            s.render()
        );
    }
}

/// **`merge(B, X, X)` reports nothing.** Both branches converged on the same
/// program, so the merged program is that program.
#[test]
fn convergent_edits_report_nothing() {
    for (label, base, ours, _) in corpus() {
        let s = run(java(), base, ours, ours);
        assert!(
            s.report.conflicts.is_empty(),
            "{label} (B, X, X) reported:\n{}",
            s.render()
        );
    }
}

/// **Symmetry.** SPEC.md §5: swapping the two sides may change the labels but
/// must not change the decision. Here the decision is *which names* are
/// reported and *why*.
#[test]
fn the_decision_is_symmetric() {
    for (label, base, ours, theirs) in corpus() {
        let forward = run(java(), base, ours, theirs);
        let mirror = run(java(), base, theirs, ours);
        let key = |s: &support::Scenario| {
            let mut v: Vec<(String, &'static str)> = s
                .report
                .conflicts
                .iter()
                .map(|c| (c.name.clone(), c.kind.tag()))
                .collect();
            v.sort();
            v
        };
        assert_eq!(
            key(&forward),
            key(&mirror),
            "{label} is asymmetric:\nforward:\n{}\nmirror:\n{}",
            forward.render(),
            mirror.render()
        );
    }
}

/// **Determinism.** Two runs on the same input produce byte-identical reports.
/// `sm_merge::merge` guarantees this and the check must not lose it — a report
/// that reordered itself between runs would make a corpus scan unrepeatable.
#[test]
fn the_report_is_deterministic() {
    for (label, base, ours, theirs) in corpus() {
        let a = run(java(), base, ours, theirs);
        let b = run(java(), base, ours, theirs);
        assert_eq!(a.report, b.report, "{label} is not deterministic");
    }
}

/// **Totality.** No input panics, including one that does not parse and one
/// whose merge is entirely a conflict.
#[test]
fn nothing_in_the_corpus_panics() {
    for (_, base, ours, theirs) in corpus() {
        let _ = run(java(), base, ours, theirs);
        let _ = run(java(), theirs, base, ours);
        let _ = run(java(), ours, theirs, base);
    }
}

/// Every reported conflict is well-formed: it names something, it points at a
/// declaration it used to resolve to, and it explains itself.
#[test]
fn every_report_is_well_formed() {
    let mut total = 0usize;
    for (base, ours, theirs) in [
        (
            "class C {\n    int helper() { return 1; }\n    int a() { return helper(); }\n}\n",
            "class C {\n    int renamed() { return 1; }\n    int a() { return renamed(); }\n}\n",
            "class C {\n    int helper() { return 1; }\n    int a() { return helper(); }\n    int b() { return helper(); }\n}\n",
        ),
        (
            "class C {\n    int v;\n    void m() {\n        int q = 0;\n    }\n}\n",
            "class C {\n    int v;\n    void m() {\n        int q = 0;\n        use(v);\n    }\n}\n",
            "class C {\n    int v;\n    void m() {\n        int v = 9;\n        int q = 0;\n    }\n}\n",
        ),
    ] {
        let s = run(java(), base, ours, theirs);
        for c in &s.report.conflicts {
            total += 1;
            assert!(!c.name.is_empty());
            assert!(c.explanation.ends_with('.'), "{}", c.explanation);
            assert!(
                c.explanation.contains(&c.name),
                "the explanation must name the reference: {}",
                c.explanation
            );
            assert!(c.origin_declaration.is_some(), "origin must be known");
            assert!(c.reference.range.start < c.reference.range.end);
            match c.kind {
                sm_bind::SemanticConflictKind::BrokenReference => {
                    assert!(c.merged_declaration.is_none());
                }
                sm_bind::SemanticConflictKind::CapturedReference => {
                    assert!(c.merged_declaration.is_some());
                }
            }
        }
    }
    assert!(total >= 2, "the fixtures above must actually report");
}

/// The report round-trips through JSON, so `sm-eval` can persist a corpus scan.
#[test]
fn the_report_round_trips_through_json() {
    let s = run(
        java(),
        "class C {\n    int helper() { return 1; }\n    int a() { return helper(); }\n}\n",
        "class C {\n    int renamed() { return 1; }\n    int a() { return renamed(); }\n}\n",
        "class C {\n    int helper() { return 1; }\n    int a() { return helper(); }\n    int b() { return helper(); }\n}\n",
    );
    assert!(!s.report.conflicts.is_empty());
    let json = serde_json::to_string(&s.report).expect("serialize");
    let back: sm_bind::CheckReport = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, s.report);
}

/// The origin lookup must never fail: the merged walk and the per-revision walk
/// run the same `Language` over the same arenas, so a reference in one is a
/// reference in the other.
#[test]
fn every_merged_reference_is_found_in_its_own_revision() {
    for (label, base, ours, theirs) in corpus() {
        let s = run(java(), base, ours, theirs);
        assert_eq!(s.report.stats.origin_not_found, 0, "{label}");
    }
    // And once through the other language, since the property is about the
    // two walks agreeing rather than about Java.
    let s = run(
        typescript(),
        "export const a = 1;\nexport function f() { return a; }\n",
        "export const a = 2;\nexport function f() { return a; }\n",
        "export const a = 1;\nexport function f() { return a; }\nexport function g() { return a; }\n",
    );
    assert_eq!(s.report.stats.origin_not_found, 0, "typescript");
}

/// A file with syntax errors still produces an answer rather than a panic, and
/// the answer stays quiet — the recovered tree is the same on both sides of the
/// comparison, so its weirdness cancels.
#[test]
fn an_unparseable_revision_does_not_produce_noise() {
    let s = run(
        java(),
        "class C {\n    int a() { return 1; }\n}\n",
        "class C {\n    int a() { return 1 }\n",
        "class C {\n    int a() { return 1; }\n    int b() { return 2; }\n}\n",
    );
    assert!(s.ours.has_errors(), "the fixture must actually be broken");
    s.assert_silent();
}
