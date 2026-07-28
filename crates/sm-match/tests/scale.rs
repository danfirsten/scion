//! A synthetic scale test.
//!
//! SPEC.md §6.2 budgets p99 < 1 s *per file* for the whole merge driver, of
//! which matching is one of four stages and runs three times. This test is not
//! a benchmark — the numbers on CI hardware are noise — it is a **regression
//! trip-wire** for accidental quadratic behaviour, with a budget loose enough
//! that only an algorithmic mistake can trip it.
//!
//! The generated file is deliberately *repetitive*: near-identical methods are
//! the worst case for the top-down phase's ambiguity handling, because every
//! subtree has many isomorphic candidates.

mod support;

use std::time::Instant;

use sm_match::{MatchConfig, TreeMetrics, match_trees};
use support::{java, parse_source};

/// Debug builds run this code roughly an order of magnitude slower than
/// release; the two budgets differ accordingly.
#[cfg(debug_assertions)]
const BUDGET_MS: u128 = 200;
#[cfg(not(debug_assertions))]
const BUDGET_MS: u128 = 50;

/// Valid Java with roughly `4 * count` declarations and a wide spread of node
/// kinds.
fn synth(count: usize, mutate: bool) -> String {
    let mut s = String::with_capacity(count * 400);
    s.push_str("package demo.generated;\n\n");
    s.push_str("import java.util.ArrayList;\nimport java.util.List;\nimport java.util.Map;\n\n");
    s.push_str("public class Generated {\n");

    for i in 0..count {
        // A rename on one field in every tenth block.
        let field = if mutate && i % 10 == 0 {
            format!("renamedField{i}")
        } else {
            format!("field{i}")
        };
        s.push_str(&format!("    private int {field} = {i};\n"));
        s.push_str(&format!(
            "    private final List<String> names{i} = new ArrayList<>();\n"
        ));

        // A body edit in every seventh block.
        let extra = if mutate && i % 7 == 0 {
            format!("        audit({i});\n")
        } else {
            String::new()
        };
        s.push_str(&format!(
            "    public int compute{i}(int a, int b) {{\n\
             \x20       int t = a * {i} + b;\n\
             {extra}\
             \x20       if (t > {field}) {{\n\
             \x20           t = t - {field};\n\
             \x20       }}\n\
             \x20       return t;\n\
             \x20   }}\n"
        ));
        s.push_str(&format!(
            "    public void record{i}(String name) {{\n\
             \x20       names{i}.add(name);\n\
             \x20   }}\n\n"
        ));
    }

    // A move: one method migrates from the front of the class to the back.
    if mutate {
        let moved = "    public void record0(String name) {\n        names0.add(name);\n    }\n\n";
        if let Some(pos) = s.find(moved) {
            s.replace_range(pos..pos + moved.len(), "");
            s.push_str(moved);
        }
    }

    s.push_str("}\n");
    s
}

#[test]
fn a_two_thousand_node_file_matches_within_budget() {
    // 60 blocks lands a little over 2 000 arena nodes; assert that rather than
    // trusting the arithmetic.
    let a_src = synth(60, false);
    let b_src = synth(60, true);

    let a = parse_source(&a_src);
    let b = parse_source(&b_src);
    assert!(!a.has_errors(), "the generated source must be valid Java");
    assert!(!b.has_errors(), "the mutated source must be valid Java");
    assert!(
        a.len() >= 2_000,
        "the synthetic file should be ~2 000 nodes, got {}",
        a.len()
    );

    let cfg = MatchConfig::base_to_side();
    let started = Instant::now();
    let m = match_trees(&a, &b, java(), &cfg);
    let elapsed = started.elapsed();

    let am = TreeMetrics::compute(&a, java());
    // A file that differs by a rename, a move and a few inserted statements
    // should still be overwhelmingly matched. This is the quality half of the
    // test: a matcher that got fast by giving up would pass the timing
    // assertion alone.
    assert!(
        m.len() * 10 >= am.matchable_count() * 9,
        "matched only {} of {} matchable nodes",
        m.len(),
        am.matchable_count()
    );

    assert!(
        elapsed.as_millis() <= BUDGET_MS,
        "matching {} vs {} nodes took {:?}, budget {BUDGET_MS} ms",
        a.len(),
        b.len(),
        elapsed
    );
}

/// The same file against itself, which is the top-down phase's worst case for
/// ambiguity: every subtree has an isomorphic twin at the same height.
#[test]
fn a_repetitive_file_against_itself_is_still_the_identity() {
    let src = synth(60, false);
    let tree = parse_source(&src);
    let metrics = TreeMetrics::compute(&tree, java());

    let started = Instant::now();
    let m = match_trees(&tree, &tree, java(), &MatchConfig::base_to_side());
    let elapsed = started.elapsed();

    assert_eq!(m.len(), metrics.matchable_count());
    for (a, b) in m.iter() {
        assert_eq!(a, b);
    }
    assert!(
        elapsed.as_millis() <= BUDGET_MS,
        "self-matching took {elapsed:?}, budget {BUDGET_MS} ms"
    );
}

/// A file of *byte-identical* methods — the worst case there is for the
/// top-down phase, and a regression test for a bug worth remembering.
///
/// With 300 identical methods, every hash group is far too large to rank
/// pairwise. The first version of the matcher deferred such groups to phase 2;
/// because the *children* of duplicated subtrees are duplicated too, the
/// ambiguity recurred at every height, nothing was matched top-down, phase 2
/// had no matched descendants to compute a dice from, and the whole file came
/// back with four matched nodes out of three thousand. Generated code and
/// boilerplate look exactly like this, so it is not a hypothetical.
#[test]
fn a_file_of_identical_methods_still_matches() {
    let method = "    public void run() {\n        helper();\n    }\n\n";
    let a_src = format!(
        "package p;\npublic class Dup {{\n{}}}\n",
        method.repeat(300)
    );
    let b_src = format!(
        "package p;\npublic class Dup {{\n{}    public void run() {{\n        helper();\n        extra();\n    }}\n\n}}\n",
        method.repeat(299)
    );

    let a = parse_source(&a_src);
    let b = parse_source(&b_src);
    assert!(!a.has_errors() && !b.has_errors());

    let started = Instant::now();
    let m = match_trees(&a, &b, java(), &MatchConfig::base_to_side());
    let elapsed = started.elapsed();

    let am = TreeMetrics::compute(&a, java());
    assert_eq!(
        m.len(),
        am.matchable_count(),
        "every node of `a` has an identical twin in `b`; matched {} of {}",
        m.len(),
        am.matchable_count()
    );

    // And the alignment must be the boring one: no method should look moved.
    let bm = TreeMetrics::compute(&b, java());
    let moves = sm_match::visualize::move_flags(&am, &bm, &m)
        .iter()
        .filter(|&&f| f)
        .count();
    assert_eq!(
        moves, 0,
        "aligning identical siblings invented {moves} moves"
    );

    assert!(
        elapsed.as_millis() <= BUDGET_MS,
        "matching 300 identical methods took {elapsed:?}, budget {BUDGET_MS} ms"
    );
}
