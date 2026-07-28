//! Structural node matching between two CSTs — GumTree, in two phases
//! (SPEC.md §4.3, milestone M2).
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use sm_match::{MatchConfig, match_trees};
//!
//! let lang = sm_cst::languages::detect("Foo.java".as_ref()).expect("java");
//! let base = sm_cst::parse(&std::fs::read("base.java")?, lang)?;
//! let ours = sm_cst::parse(&std::fs::read("ours.java")?, lang)?;
//!
//! let m = match_trees(&base, &ours, lang, &MatchConfig::base_to_side());
//! for (b, o) in m.iter() {
//!     println!("{b} <-> {o}");
//! }
//! # Ok(()) }
//! ```
//!
//! # The two phases
//!
//! **Phase 1, top-down** matches *identical* subtrees, tallest first, using
//! height-indexed priority queues and a per-node structural hash. An isomorphic
//! pair is mapped node for node. Ambiguity — several equally isomorphic
//! candidates — is resolved by parent context and position, and any genuine tie
//! is deferred to phase 2.
//!
//! **Phase 2, bottom-up** matches the *containers* those subtrees live in:
//! post-order over the source tree, candidates restricted to unmatched
//! same-kind ancestors of already-matched destinations, winner by dice
//! coefficient over shared matched descendants. Each newly matched pair gets a
//! cheap histogram recovery pass over its small unmatched children.
//!
//! # Two profiles, not one set of constants
//!
//! [`MatchConfig::base_to_side`] is permissive and
//! [`MatchConfig::ours_to_theirs`] is strict, because a miss against base costs
//! a spurious conflict while a false positive between the two sides corrupts
//! the merge. This asymmetry is the highest-value finding in
//! docs/prior-art.md (§2.2, §8.1.2) and SPEC.md §4.3's single constant profile
//! does not have it. All the numbers are pre-tuning; M5 sweeps them.
//!
//! # What "matched" means — read this before writing M3 or M4
//!
//! A [`Matching`] contains **only participating nodes**: named, non-comment,
//! non-`MISSING`. Anonymous tokens (`{`, `;`, `public`) and comments are never
//! in a pair, and their absence is not a signal that anything changed. See the
//! [`metrics`] module docs for the reasoning and for what to use instead
//! (the matched *parent* for punctuation; [`sm_cst::Node::leading_trivia`] and
//! [`sm_cst::Node::trailing_trivia`] for comments).

mod bottom_up;
mod config;
mod context;
mod matching;
pub mod metrics;
mod top_down;
pub mod visualize;

pub use config::MatchConfig;
pub use matching::{Matching, render_pairs};
pub use metrics::{ContentItem, TreeMetrics, structurally_equal};

use sm_cst::{Language, SourceTree};

/// Match two trees.
///
/// `src` and `dst` must have been parsed with `lang`; the structural hash folds
/// in tree-sitter kind IDs, which are only comparable within one grammar.
///
/// # Guarantees
///
/// - **Deterministic.** No hash-map iteration order reaches a decision point;
///   every group, candidate list and tie-break is ordered.
/// - **Injective.** No node appears in more than one pair.
/// - **Kind-equal.** Both members of every pair have the same `kind`.
/// - **Roots match when their kinds agree**, whatever else does or does not.
///
/// # Cost
///
/// Roughly linear in the two trees for the side tables and phase 1; phase 2 is
/// `O(|matched descendants| × |candidates|)` per unmatched container, which is
/// the usual GumTree shape. There is no timeout here — SPEC.md §4.7 puts the
/// budget in the driver, where it can fall back to a line merge.
#[must_use]
pub fn match_trees(
    src: &SourceTree,
    dst: &SourceTree,
    lang: &dyn Language,
    cfg: &MatchConfig,
) -> Matching {
    debug_assert_eq!(
        src.language_name(),
        dst.language_name(),
        "match_trees requires both trees to come from the same language"
    );

    let sm = TreeMetrics::compute(src, lang);
    let dm = TreeMetrics::compute(dst, lang);
    match_trees_with_metrics(src, &sm, dst, &dm, lang, cfg)
}

/// [`match_trees`], reusing side tables the caller has already built.
///
/// A three-way merge matches base against two sides, so base's
/// [`TreeMetrics`] would otherwise be computed twice.
#[must_use]
pub fn match_trees_with_metrics(
    src: &SourceTree,
    src_metrics: &TreeMetrics,
    dst: &SourceTree,
    dst_metrics: &TreeMetrics,
    lang: &dyn Language,
    cfg: &MatchConfig,
) -> Matching {
    let mut ctx = context::Ctx {
        src,
        dst,
        sm: src_metrics,
        dm: dst_metrics,
        lang,
        cfg,
        matching: Matching::new(src.len(), dst.len()),
    };

    top_down::run(&mut ctx);
    bottom_up::run(&mut ctx);

    debug_assert!(
        ctx.matching
            .iter()
            .all(|(a, b)| src.node(a).kind == dst.node(b).kind),
        "every matched pair must be kind-equal"
    );
    ctx.matching
}
