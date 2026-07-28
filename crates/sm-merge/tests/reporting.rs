//! What the merge reports about itself: fates, statistics, conflict summaries
//! and their serialised form.
//!
//! M5 histograms all of this and M6 walks the merged tree, so it is part of the
//! interface, not debug output.

mod support;

use sm_merge::{Fate, MergeConfig, MergedNode, Side, merge};
use support::{Run, java, parse};

#[test]
fn fates_are_reported_for_both_sides_over_every_base_node() {
    let run = Run::java(
        "class C {\n  void a() { x(); }\n\n  void b() {}\n}\n",
        "class C {\n  void a() { x2(); }\n\n  void b() {}\n}\n",
        "class C {\n  void b() {}\n}\n",
    );
    assert_eq!(run.outcome.ours_fates.len(), run.base.len());
    assert_eq!(run.outcome.theirs_fates.len(), run.base.len());

    // Non-participating nodes never get a fate.
    for id in run.base.ids() {
        let node = run.base.node(id);
        if !node.is_named || node.is_extra {
            assert_eq!(
                run.outcome.ours_fates[id.index()],
                Fate::NotApplicable,
                "{} should have no fate",
                node.kind
            );
        }
    }

    let (ours, theirs) = run.fates_of("method_declaration").expect("a method");
    assert_eq!(ours, Fate::Updated);
    assert_eq!(theirs, Fate::Deleted);
}

#[test]
fn the_fate_histogram_matches_the_fate_vector() {
    let run = Run::java(
        "class C {\n  void a() { x(); }\n\n  void b() {}\n}\n",
        "class C {\n  void a() { x2(); }\n\n  void b() {}\n}\n",
        "class C {\n  void b() {}\n}\n",
    );
    let counted = run
        .outcome
        .ours_fates
        .iter()
        .filter(|f| **f == Fate::Unchanged)
        .count();
    assert_eq!(counted, run.outcome.stats.ours_fates.unchanged);

    let total: usize = {
        let c = run.outcome.stats.ours_fates;
        c.unchanged + c.updated + c.moved + c.moved_and_updated + c.deleted
    };
    let applicable = run
        .outcome
        .ours_fates
        .iter()
        .filter(|f| **f != Fate::NotApplicable)
        .count();
    assert_eq!(total, applicable);
}

#[test]
fn every_fate_variant_is_reachable() {
    let mut seen = std::collections::BTreeSet::new();
    let cases = [
        // Unchanged / Updated / Deleted.
        (
            "class C {\n  void a() { x(); }\n\n  void b() {}\n}\n",
            "class C {\n  void a() { x2(); }\n\n  void b() {}\n}\n",
            "class C {\n  void b() {}\n}\n",
        ),
        // MovedAndUpdated: we both reorder and edit.
        (
            "class C {\n  void a() { x(); }\n\n  void b() { y(); }\n}\n",
            "class C {\n  void b() { y(); }\n\n  void a() { x2(); }\n}\n",
            "class C {\n  void a() { x(); }\n\n  void b() { y(); }\n}\n",
        ),
        // Moved: a pure reorder, with no content change anywhere.
        (
            "class C {\n  void a() { x(); }\n\n  void b() { y(); }\n}\n",
            "class C {\n  void b() { y(); }\n\n  void a() { x(); }\n}\n",
            "class C {\n  void a() { x(); }\n\n  void b() { y(); }\n}\n",
        ),
    ];
    for (b, o, t) in cases {
        let run = Run::java(b, o, t);
        seen.extend(run.outcome.ours_fates.iter().copied());
        seen.extend(run.outcome.theirs_fates.iter().copied());
    }
    for expected in [
        Fate::Unchanged,
        Fate::Updated,
        Fate::Moved,
        Fate::MovedAndUpdated,
        Fate::Deleted,
    ] {
        assert!(seen.contains(&expected), "{expected} never occurred");
    }
}

#[test]
fn statistics_agree_with_the_tree() {
    let run = Run::java(
        "class C {\n  void a() { x(); }\n\n  void b() {}\n}\n",
        "class C {\n  void a() { x2(); }\n\n  void b() {}\n\n  void c() {}\n}\n",
        "class C {\n  void a() { x3(); }\n\n  void b() {}\n}\n",
    );
    let stats = &run.outcome.stats;
    assert_eq!(stats.base_nodes, run.base.len());
    assert_eq!(stats.ours_nodes, run.ours.len());
    assert_eq!(stats.theirs_nodes, run.theirs.len());
    assert_eq!(stats.merged_nodes, run.outcome.tree.walk().count());
    assert_eq!(stats.conflicts, run.outcome.conflicts.len());

    let mut splices = 0;
    let mut rebuilds = 0;
    let mut conflicts = 0;
    for id in run.outcome.tree.walk() {
        match run.outcome.tree.node(id) {
            MergedNode::Splice { .. } => splices += 1,
            MergedNode::Rebuilt { .. } => rebuilds += 1,
            MergedNode::Conflict(_) => conflicts += 1,
        }
    }
    assert_eq!(
        (stats.splices, stats.rebuilds, stats.conflicts),
        (splices, rebuilds, conflicts)
    );
}

#[test]
fn conflict_summaries_carry_spans_in_every_revision_that_has_one() {
    let run = Run::java(
        "class C {\n  int x = 1;\n}\n",
        "class C {\n  int x = 2;\n}\n",
        "class C {\n  int x = 3;\n}\n",
    );
    let summary = &run.outcome.conflicts[0];
    let base = summary.base.clone().expect("an ancestor span");
    let ours = summary.ours.clone().expect("our span");
    let theirs = summary.theirs.clone().expect("their span");
    assert_eq!(
        &run.base.source()[base.start as usize..base.end as usize],
        b"int x = 1;"
    );
    assert_eq!(
        &run.ours.source()[ours.start as usize..ours.end as usize],
        b"int x = 2;"
    );
    assert_eq!(
        &run.theirs.source()[theirs.start as usize..theirs.end as usize],
        b"int x = 3;"
    );
    assert_eq!(summary.kind, "field_declaration");
}

/// A conflict whose ancestor has nothing to show (both sides inserted) reports
/// `None` rather than an empty range.
#[test]
fn an_insertion_collision_has_no_ancestor_span() {
    let run = Run::java(
        "class C {\n  void a() {\n    p();\n    q();\n  }\n}\n",
        "class C {\n  void a() {\n    p();\n    ours();\n    q();\n  }\n}\n",
        "class C {\n  void a() {\n    p();\n    theirs();\n    q();\n  }\n}\n",
    );
    let summary = &run.outcome.conflicts[0];
    assert!(summary.base.is_none());
    assert!(summary.ours.is_some() && summary.theirs.is_some());
}

/// M6 walks the merged tree and re-resolves names over it, which means every
/// node it sees has to lead back to a scope in one of the three inputs.
#[test]
fn the_merged_tree_is_walkable_and_every_node_resolves_to_an_input() {
    let run = Run::java(
        "class C {\n  void a() { x(); }\n}\n",
        "class C {\n  void a() { x2(); }\n\n  void b() {}\n}\n",
        "class C {\n  void a() { x(); }\n\n  void c() {}\n}\n",
    );
    let mut count = 0;
    for id in run.outcome.tree.walk() {
        count += 1;
        match run.outcome.tree.node(id) {
            MergedNode::Conflict(_) => {}
            _ => {
                let (side, node) = run.outcome.tree.provenance(id).expect("provenance");
                let tree = run.tree(side);
                assert!(node.index() < tree.len(), "node id out of range for {side}");
            }
        }
        assert!(matches!(
            run.outcome.tree.lead(id),
            sm_merge::Gap::Copied { .. }
        ));
    }
    assert!(count > 1, "the walk must reach more than the root");
}

#[test]
fn the_outcome_round_trips_through_json() {
    let run = Run::java(
        "class C {\n  int x = 1;\n}\n",
        "class C {\n  int x = 2;\n}\n",
        "class C {\n  int x = 3;\n}\n",
    );
    let json = serde_json::to_string(&run.outcome).expect("serialize");
    let back: sm_merge::MergeOutcome = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, run.outcome);
}

#[test]
fn side_and_reason_tags_are_stable_strings() {
    assert_eq!(
        Side::ALL.map(Side::name),
        ["base", "ours", "theirs"],
        "these names appear in reports and in marker labels"
    );
    assert_eq!(
        sm_merge::ConflictReason::Unmergeable {
            detail: "why".into()
        }
        .to_string(),
        "unmergeable(why)"
    );
}

/// Two runs produce identical outcomes, including the arena layout — nothing in
/// the decision path may iterate a hash map.
#[test]
fn the_merge_is_deterministic() {
    let base = parse(
        "class C {\n  void a() { x(); }\n\n  void b() { y(); }\n}\n",
        java(),
    );
    let ours = parse(
        "class C {\n  void b() { y(); }\n\n  void a() { x(); }\n\n  void c() {}\n}\n",
        java(),
    );
    let theirs = parse(
        "class C {\n  void a() { x2(); }\n\n  void b() { y(); }\n\n  void d() {}\n}\n",
        java(),
    );
    let a = merge(&base, &ours, &theirs, java(), &MergeConfig::default());
    let b = merge(&base, &ours, &theirs, java(), &MergeConfig::default());
    assert_eq!(a, b);
}
