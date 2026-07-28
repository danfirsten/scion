//! SPEC.md §5's properties, over the whole scenario corpus.
//!
//! These are the assertions M4b will turn into `proptest` generators. Running
//! them over a hand-written table first means the generators start from a
//! passing baseline rather than from a bug hunt.

mod support;

use sm_emit::{ConflictStyle, EmitOptions, emit};
use sm_merge::{ConflictReason, Gap, MergeConfig, MergedNode, merge};
use support::{Merged, Scenario, java, run, run_scenario, scenarios, typescript};

// ------------------------------------------------------- the identity laws

/// `merge(B, X, B) == X`, byte for byte, over every scenario in the corpus.
#[test]
fn identity_on_their_side() {
    for sc in scenarios() {
        for text in [sc.ours, sc.theirs] {
            let m = run(sc.base, text, sc.base, sc.language());
            assert!(m.is_clean(), "{}: {:?}", sc.name, m.reasons());
            assert_eq!(
                m.text(),
                text,
                "{}: merge(B, X, B) must be byte-identical to X",
                sc.name
            );
        }
    }
}

/// `merge(B, B, Y) == Y`.
#[test]
fn identity_on_our_side() {
    for sc in scenarios() {
        for text in [sc.ours, sc.theirs] {
            let m = run(sc.base, sc.base, text, sc.language());
            assert!(m.is_clean(), "{}: {:?}", sc.name, m.reasons());
            assert_eq!(
                m.text(),
                text,
                "{}: merge(B, B, Y) must be byte-identical to Y",
                sc.name
            );
        }
    }
}

/// `merge(B, X, X) == X` — convergent edits, taken from our side.
#[test]
fn convergent_edits_are_byte_identical_to_the_shared_result() {
    for sc in scenarios() {
        for text in [sc.ours, sc.theirs] {
            let m = run(sc.base, text, text, sc.language());
            assert!(m.is_clean(), "{}: {:?}", sc.name, m.reasons());
            assert_eq!(m.text(), text, "{}: merge(B, X, X) must be X", sc.name);
        }
    }
}

/// `merge(B, B, B) == B`.
#[test]
fn merging_a_file_with_itself_changes_nothing() {
    for sc in scenarios() {
        for text in [sc.base, sc.ours, sc.theirs] {
            let m = run(text, text, text, sc.language());
            assert!(m.is_clean(), "{}", sc.name);
            assert_eq!(m.text(), text, "{}: merge(B, B, B) must be B", sc.name);
        }
    }
}

/// SPEC.md §4.6's headline invariant, stated directly: zero conflicts plus one
/// side entirely unchanged means the output *is* the other side's file.
///
/// The structural half matters as much as the byte half — the merged tree
/// should be a single splice, not a coincidentally-identical reconstruction.
#[test]
fn an_untouched_side_reduces_to_one_splice_of_the_other() {
    for sc in scenarios() {
        for (ours, theirs, expect) in [
            (sc.ours, sc.base, sm_merge::Side::Ours),
            (sc.base, sc.theirs, sm_merge::Side::Theirs),
        ] {
            let m = run(sc.base, ours, theirs, sc.language());
            let root = m.outcome.tree.root();
            assert!(
                matches!(m.outcome.tree.node(root), MergedNode::Splice { .. }),
                "{}: expected a single splice, got {:?}",
                sc.name,
                m.outcome.tree.node(root)
            );
            assert_eq!(
                m.outcome.tree.provenance(root).map(|(s, _)| s),
                Some(expect),
                "{}",
                sc.name
            );
        }
    }
}

// ------------------------------------------------------------------ symmetry

/// SPEC.md §5: `merge(B, X, Y)` conflicts ⟺ `merge(B, Y, X)` conflicts. The
/// labels and the marker order legitimately differ; the *decision* must not.
#[test]
fn the_conflict_decision_is_symmetric() {
    for sc in scenarios() {
        let forward = run_scenario(&sc);
        let mirrored = run_scenario(&sc.mirrored());
        assert_eq!(
            forward.is_clean(),
            mirrored.is_clean(),
            "{}: clean forward = {}, clean mirrored = {} ({:?} vs {:?})",
            sc.name,
            forward.is_clean(),
            mirrored.is_clean(),
            forward.reasons(),
            mirrored.reasons()
        );
    }
}

/// And the *number* of conflicting regions is symmetric too, which is what
/// makes M5's conflict-count metric meaningful.
#[test]
fn the_conflict_count_is_symmetric() {
    for sc in scenarios() {
        let forward = run_scenario(&sc);
        let mirrored = run_scenario(&sc.mirrored());
        assert_eq!(
            forward.outcome.conflicts.len(),
            mirrored.outcome.conflicts.len(),
            "{}: {:?} vs {:?}",
            sc.name,
            forward.reasons(),
            mirrored.reasons()
        );
    }
}

/// Every reason has a mirror image, and mirroring a scenario produces it.
#[test]
fn conflict_reasons_mirror_exactly() {
    for sc in scenarios() {
        let forward = run_scenario(&sc);
        let mirrored = run_scenario(&sc.mirrored());
        let expected: Vec<String> = forward
            .outcome
            .conflicts
            .iter()
            .map(|c| c.reason.mirrored().tag().to_owned())
            .collect();
        let actual: Vec<String> = mirrored
            .outcome
            .conflicts
            .iter()
            .map(|c| c.reason.tag().to_owned())
            .collect();
        assert_eq!(expected, actual, "{}", sc.name);
    }
}

#[test]
fn mirroring_a_reason_twice_is_the_identity() {
    for reason in [
        ConflictReason::UpdateUpdate,
        ConflictReason::MoveMove,
        ConflictReason::DeleteUpdate,
        ConflictReason::UpdateDelete,
        ConflictReason::DeleteMove,
        ConflictReason::MoveDelete,
        ConflictReason::InsertInsert,
        ConflictReason::OrderedInsertCollision,
        ConflictReason::ReorderConflict,
        ConflictReason::KindClash,
        ConflictReason::CommentEdit,
        ConflictReason::RootMismatch,
        ConflictReason::Unmergeable { detail: "x".into() },
    ] {
        assert_eq!(reason.mirrored().mirrored(), reason);
    }
}

// --------------------------------------------------------------- determinism

/// Two runs over the same inputs produce identical bytes and identical
/// conflict reports. Nothing in the decision path may depend on hash iteration
/// order.
#[test]
fn two_runs_agree_byte_for_byte() {
    for sc in scenarios() {
        let a = run_scenario(&sc);
        let b = run_scenario(&sc);
        assert_eq!(a.result.bytes, b.result.bytes, "{}", sc.name);
        assert_eq!(a.outcome.conflicts, b.outcome.conflicts, "{}", sc.name);
        assert_eq!(a.outcome.stats, b.outcome.stats, "{}", sc.name);
    }
}

// -------------------------------------------------------------- idempotence

/// Merging a clean result against itself is a no-op.
#[test]
fn a_clean_result_is_a_fixed_point() {
    for sc in scenarios() {
        let first = run_scenario(&sc);
        if !first.is_clean() {
            continue;
        }
        let text = first.text();
        let again = run(&text, &text, &text, sc.language());
        assert!(again.is_clean(), "{}", sc.name);
        assert_eq!(again.text(), text, "{}: merge is not idempotent", sc.name);
    }
}

/// A stronger form: re-merging the result against the two inputs it came from
/// still yields the result. This is the shape M4b's driver needs when git
/// re-runs a merge over an already-merged file.
#[test]
fn remerging_a_clean_result_against_its_inputs_is_stable() {
    for sc in scenarios() {
        let first = run_scenario(&sc);
        if !first.is_clean() {
            continue;
        }
        let text = first.text();
        let again = run(&text, &text, sc.theirs, sc.language());
        assert!(again.is_clean(), "{}: {:?}", sc.name, again.reasons());
    }
}

// ----------------------------------------------------------- parse stability

/// Every conflict-free output reparses with no syntax errors.
///
/// This is SPEC.md §5's parse-stability property and docs/prior-art.md §8.3.3's
/// "parsable" metric: a ground-truth-free correctness signal that scales to the
/// whole corpus in M5.
#[test]
fn every_clean_output_reparses_without_errors() {
    let mut checked = 0usize;
    for sc in scenarios() {
        for case in [sc.mirrored(), sc] {
            let m = run_scenario(&case);
            if !m.is_clean() {
                continue;
            }
            let reparsed = m.reparse();
            assert!(
                !reparsed.has_errors(),
                "{}: clean output does not parse:\n{}",
                case.name,
                m.text()
            );
            checked += 1;
        }
    }
    assert!(checked > 20, "only {checked} clean outputs were checked");
}

/// Conflicting output is *not* expected to parse — it has marker lines in it —
/// but it must still contain every marker, in order, at the start of a line.
#[test]
fn conflicting_output_is_well_formed_even_though_it_does_not_parse() {
    for sc in scenarios() {
        let m = run_scenario(&sc);
        if m.is_clean() {
            continue;
        }
        let text = m.text();
        let opens = text.lines().filter(|l| l.starts_with("<<<<<<<")).count();
        let seps = text.lines().filter(|l| *l == "=======").count();
        let closes = text.lines().filter(|l| l.starts_with(">>>>>>>")).count();
        assert_eq!(opens, m.result.conflict_count, "{}", sc.name);
        assert_eq!(opens, seps, "{}", sc.name);
        assert_eq!(opens, closes, "{}", sc.name);
        assert!(text.ends_with('\n'), "{}: no final newline", sc.name);
    }
}

// ------------------------------------------------------- byte preservation

/// Nothing in a clean merge is synthesised: every byte came from an input.
#[test]
fn a_clean_merge_synthesises_nothing() {
    for sc in scenarios() {
        let m = run_scenario(&sc);
        if !m.is_clean() {
            continue;
        }
        assert_eq!(
            m.result.synthesized_bytes, 0,
            "{}: clean output must be a pure splice",
            sc.name
        );
        for id in m.outcome.tree.walk() {
            assert!(
                matches!(m.outcome.tree.lead(id), Gap::Copied { .. }),
                "{}: a gap was synthesised",
                sc.name
            );
        }
    }
}

/// Every non-conflict node in the merged tree resolves to a `(side, node)`, and
/// therefore to a byte range in an input.
#[test]
fn every_merged_node_carries_provenance() {
    for sc in scenarios() {
        let m = run_scenario(&sc);
        for id in m.outcome.tree.walk() {
            match m.outcome.tree.node(id) {
                MergedNode::Conflict(_) => {}
                _ => assert!(
                    m.outcome.tree.provenance(id).is_some(),
                    "{}: node {id} has no provenance",
                    sc.name
                ),
            }
        }
    }
}

// ------------------------------------------------------------ line endings

/// A CRLF file survives byte-exactly on untouched regions, and the merged
/// result keeps CRLF throughout.
#[test]
fn crlf_files_keep_their_line_endings() {
    let base = support::crlf("class C {\n  int a = 1;\n  int b = 2;\n}\n");
    let ours = support::crlf("class C {\n  int a = 11;\n  int b = 2;\n}\n");
    let theirs = support::crlf("class C {\n  int a = 1;\n  int b = 22;\n}\n");
    let m = support::run_bytes(&base, &ours, &theirs, java(), &EmitOptions::default());
    assert!(m.is_clean(), "{:?}", m.reasons());
    let text = m.text();
    assert!(text.contains("int a = 11;"), "{text:?}");
    assert!(text.contains("int b = 22;"), "{text:?}");
    assert_eq!(
        text.matches('\n').count(),
        text.matches("\r\n").count(),
        "every newline must still be a CRLF:\n{text:?}"
    );
}

/// The identity law holds for CRLF too — this is where a normalise-on-input
/// implementation would silently rewrite the whole file.
#[test]
fn crlf_identity_is_byte_exact() {
    let base = support::crlf("class C {\n  int a = 1;\n}\n");
    let ours = support::crlf("class C {\n  int a = 2;\n}\n");
    let m = support::run_bytes(&base, &ours, &base, java(), &EmitOptions::default());
    assert_eq!(m.result.bytes, ours);
}

// -------------------------------------------------------------- robustness

/// A file that will not parse still merges to *something* without panicking.
/// The decision to fall back to a line merge belongs to M4b's driver, which
/// reads `SourceTree::has_errors`; this crate must not be the thing that
/// crashes first.
#[test]
fn syntactically_broken_input_does_not_panic() {
    let lang = java();
    let base = sm_cst::parse(b"class C { void a() { } }\n", lang).expect("parse");
    let ours = sm_cst::parse(b"class C { void a() { if ( } }\n", lang).expect("parse");
    let theirs = sm_cst::parse(b"class C { void a() { x(); } }\n", lang).expect("parse");
    assert!(ours.has_errors(), "the fixture must actually be broken");
    let outcome = merge(&base, &ours, &theirs, lang, &MergeConfig::default());
    let result = emit(
        &outcome.tree,
        &base,
        &ours,
        &theirs,
        lang,
        &EmitOptions::default(),
    );
    assert!(!result.bytes.is_empty());
}

/// Empty and comment-only files are real inputs.
#[test]
fn degenerate_files_round_trip() {
    for (b, o, t, expected) in [
        ("", "", "", ""),
        ("", "class C {}\n", "", "class C {}\n"),
        ("class C {}\n", "class C {}\n", "", ""),
        ("// c\n", "// c\n", "// c\n", "// c\n"),
    ] {
        let m = run(b, o, t, java());
        assert!(m.is_clean(), "{b:?} {o:?} {t:?}: {:?}", m.reasons());
        assert_eq!(m.text(), expected, "{b:?} {o:?} {t:?}");
    }
}

/// The two conflict styles differ only in the ancestor block, and both keep the
/// marker discipline.
#[test]
fn diff3_style_adds_the_ancestor_block() {
    let sc = Scenario {
        name: "x",
        lang: "java",
        base: "class C {\n  int x = 1;\n}\n",
        ours: "class C {\n  int x = 2;\n}\n",
        theirs: "class C {\n  int x = 3;\n}\n",
    };
    let merge_style = run_scenario(&sc);
    let diff3 = support::run_with(
        sc.base,
        sc.ours,
        sc.theirs,
        sc.language(),
        &EmitOptions {
            style: ConflictStyle::Diff3,
            ..EmitOptions::default()
        },
    );
    assert!(!merge_style.text().contains("|||||||"));
    assert!(diff3.text().contains("||||||| base"));
    assert!(diff3.text().contains("int x = 1;"), "{}", diff3.text());
}

/// TypeScript rides the same machinery, which is the claim SPEC.md §3 makes.
#[test]
fn typescript_satisfies_the_identity_laws_too() {
    let base = "export function f(): number {\n  return 1;\n}\n";
    let ours = "export function f(): number {\n  return 2;\n}\n";
    let m = run(base, ours, base, typescript());
    assert_eq!(m.text(), ours);
    let m = run(base, base, ours, typescript());
    assert_eq!(m.text(), ours);
}

/// A scenario's emitted output for a clean merge must contain the changes both
/// sides made. Weak, but it is the property M5 reports as "universality"
/// (docs/prior-art.md §8.3.3) and it catches a whole class of silent drops.
#[test]
fn a_clean_merge_keeps_both_sides_distinctive_text() {
    let checks: &[(&str, &[&str])] = &[
        (
            "disjoint_edits_in_one_method",
            &["int i = 11;", "int j = 22;"],
        ),
        (
            "both_add_a_different_method",
            &["void ours()", "void theirs()"],
        ),
        (
            "both_add_an_import_at_the_same_spot",
            &["import java.util.Map;", "import java.util.Set;"],
        ),
        ("we_move_a_method_they_edit_its_body", &["z();"]),
        ("wrapped_block_with_an_inner_edit", &["if (ok)", "y2();"]),
        (
            "comment_edited_on_one_side_code_on_the_other",
            &["// new", "z();"],
        ),
        ("typescript_class_members", &["b()", "c()"]),
    ];
    let all: Vec<Merged> = scenarios().iter().map(run_scenario).collect();
    let names: Vec<&str> = scenarios().iter().map(|s| s.name).collect();
    for (name, needles) in checks {
        let idx = names
            .iter()
            .position(|n| n == name)
            .expect("known scenario");
        let m = &all[idx];
        assert!(m.is_clean(), "{name}: {:?}", m.reasons());
        for needle in *needles {
            assert!(
                m.text().contains(needle),
                "{name}: lost {needle:?}\n{}",
                m.text()
            );
        }
    }
}
