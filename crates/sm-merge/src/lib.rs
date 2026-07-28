//! Three-way tree merge and the conflict model (SPEC.md §4.5, milestone M4).
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use sm_merge::{MergeConfig, merge};
//!
//! let lang = sm_cst::languages::detect("Foo.java".as_ref()).expect("java");
//! let base = sm_cst::parse(&std::fs::read("base.java")?, lang)?;
//! let ours = sm_cst::parse(&std::fs::read("ours.java")?, lang)?;
//! let theirs = sm_cst::parse(&std::fs::read("theirs.java")?, lang)?;
//!
//! let outcome = merge(&base, &ours, &theirs, lang, &MergeConfig::for_language(lang));
//! if outcome.is_clean() {
//!     // hand `outcome.tree` to `sm-emit`
//! }
//! # Ok(()) }
//! ```
//!
//! This module documentation **is** the design record. Everything below is a
//! decision that can produce a wrong merge if it is wrong, so each one is
//! stated with its reasoning. SPEC.md §0.4 governs all of them: *a conflict is
//! always an acceptable answer, a wrong merge never is.*
//!
//! # 1. What the algorithm actually is
//!
//! Two matchings, `base → ours` and `base → theirs`, composed **through base**.
//! There is no third matching at all (SPEC.md §4.5). The one question that
//! would need one — "are these two insertions the same insertion?" — is
//! answered exactly instead, by comparing structural hashes and confirming
//! them with [`sm_match::structurally_equal`]. An approximate answer there can
//! only produce false positives, and a false positive between the two sides
//! feeds straight into the output (docs/prior-art.md §2.2).
//!
//! From the three roots down, at every triple `(b, o, t)` of matched nodes:
//!
//! 1. If one side's bytes are identical to the ancestor's, the other side's
//!    bytes are the answer. **This is the whole of SPEC.md §4.6's invariant**:
//!    if one side is entirely unchanged, the root triple takes this branch and
//!    the merged tree is a single [`MergedNode::Splice`] of the other side.
//! 2. Otherwise, if the two sides converged, take ours (see §3).
//! 3. Otherwise, if one side changed only formatting, take the other side.
//! 4. Otherwise recurse: align the three **content sequences** and merge them.
//!
//! A node's content sequence is its children with comments and `MISSING`
//! repairs removed, **anonymous tokens kept**. Keeping the tokens is what lets
//! one sequence merge handle both list edits and token edits: `{ a; b; }` →
//! `{ a; b; c; }` inserts an element between two unchanged tokens, and
//! `x = 1` → `x += 1` replaces a token between two unchanged elements. Without
//! them, a merge that recursed into `x += 1` versus `x -= 1` would emit
//! whichever operator its container frame happened to come from, silently.
//!
//! # 2. Fate classification
//!
//! [`Fate`] gives SPEC.md §4.5's per-node classification —
//! `Unchanged | Updated | Moved | MovedAndUpdated | Deleted` — with the exact
//! definitions in that module's documentation. It is computed for both sides
//! and reported in [`MergeOutcome::ours_fates`] / [`MergeOutcome::theirs_fates`]
//! so M5 can histogram it and the tests can assert on it. The merge itself runs
//! on finer distinctions than the five-valued summary can express; see the
//! `Fate` docs for why.
//!
//! # 3. Side preference — why "take x" means "take *our* x"
//!
//! SPEC.md §4.5's `Updated(x) | Updated(x) → take x` row is not implementable
//! as written, because there is no such thing as *the* bytes of `x`: the two
//! sides converged on the same code, but they may have written it with
//! different indentation and different comments, and SPEC.md §4.6 requires the
//! output to be a splice of a *specific* input range.
//!
//! **The rule: when both sides converge, splice ours.** Reasons, in order:
//!
//! - It is deterministic and it is written down, which is what makes M5's
//!   divergence analysis possible at all.
//! - `%A` is the file git overwrites in place, so preferring it minimises the
//!   diff the user sees against their own working tree.
//! - It composes with the identity laws: `merge(B, X, X) == X` byte-exactly,
//!   one of SPEC.md §5's required properties, and false for any rule that
//!   sometimes preferred theirs.
//!
//! The same preference breaks every other tie in the crate: the framing
//! revision of a rebuilt container, the order of set-merged additions, and
//! which side a deduplicated insertion is spliced from.
//!
//! ## Three grades of "changed", not two
//!
//! | grade | test | meaning |
//! |---|---|---|
//! | `None` | extent bytes equal | untouched, comments and whitespace included |
//! | `Formatting` | bytes differ, but [`sm_match::TreeMetrics::hash`] *and* the comment hash agree | reindented, nothing else |
//! | `Content` | otherwise | a reader would notice |
//!
//! "Extent" means the node's byte range widened over the comments the trivia
//! pass attached to it, so a comment edit is a content change on its owner.
//! Formatting loses to content, because a byte splicer cannot reprint one
//! side's code in the other side's layout, and conflicting over a reindent
//! would be a regression against git on a change git does not notice.
//!
//! # 4. The expanded decision table
//!
//! `x`, `y` are content; `p`, `q` are positions. `Moved` and `Updated`
//! compose: [`Fate::MovedAndUpdated`] is resolved as its two components
//! independently, which is what makes the headline row work.
//!
//! | ours | theirs | result | why |
//! |---|---|---|---|
//! | Unchanged | anything | take theirs verbatim | nothing of ours to preserve |
//! | anything | Unchanged | take ours verbatim | symmetric |
//! | Updated(x) | Updated(x) | take **our** bytes | §3 |
//! | Updated(x) | Updated(y), x≠y | recurse; conflict where the disagreement bottoms out | conflicts land at the smallest boundary, §6 |
//! | Formatting | Updated(y) | take theirs | a reindent cannot be replayed onto other bytes |
//! | Updated(x) | Formatting | take ours | symmetric |
//! | Moved(p) | Updated(x) | **both apply**: content from them, position from us | the headline win |
//! | Updated(x) | Moved(q) | both apply: content from us, position from them | symmetric |
//! | Moved(p) | Moved(p) | move once, to p | convergent |
//! | Moved(p) | Moved(q), p≠q | **conflict** `MoveMove` | the interleaving is not ours to guess |
//! | Moved(p)+Updated(x) | Updated(y), x≠y | position p, **conflict** on content | components resolved independently |
//! | Moved(p)+Updated(x) | Moved(q), p≠q | **conflict** `MoveMove` | the position component decides first |
//! | Moved(p)+Updated(x) | Moved(p)+Updated(x) | position p, content x, spliced from us | convergent |
//! | Moved(p)+Updated(x) | Unchanged | move and update, both from us | one side did nothing |
//! | Deleted | Unchanged | delete | |
//! | Deleted | Formatting | delete | a reindent is not worth resurrecting a node over |
//! | Deleted | Updated | **conflict** `DeleteUpdate` | classic delete/modify |
//! | Updated | Deleted | **conflict** `UpdateDelete` | symmetric |
//! | Deleted | Moved | **conflict** `DeleteMove`, raised at the destination | |
//! | Moved | Deleted | **conflict** `MoveDelete`, raised at the destination | |
//! | Deleted | Deleted | delete | |
//! | Deleted (a whole region) | Moved *out of* that region | delete the region, keep their node at its destination | the covering rule, §7 |
//!
//! Position is settled by the **parent's** child-list merge, and content by the
//! recursion into the node. That factorisation is exactly why
//! `Moved(p) | Updated(x)` applies both: the parent list places the element
//! where the mover put it, and the recursion fills that slot with the updater's
//! bytes. Nothing special-cases it.
//!
//! # 5. Child lists
//!
//! ## Ordered lists
//!
//! A diff3 chunker over the three content sequences, with **identity = node
//! matching, not position and not text** (SPEC.md §4.5). Consequences:
//!
//! - An element edited in place still aligns with its ancestor, so an edit and
//!   an insertion elsewhere in the same list are disjoint changes.
//! - Two textually identical statements are never confused for one another.
//! - A reorder on one side and an edit on the other merge cleanly: the sequence
//!   merge takes the reorderer's order, and the content merge fills each slot.
//! - Insertions from both sides at the same anchor **conflict**
//!   ([`ConflictReason::OrderedInsertCollision`]) — unless they are the same
//!   insertion, in which case they are deduplicated and applied once.
//! - A deletion on one side and a content edit on the other **conflicts**. The
//!   sequence merge cannot see this on its own (at the level of identities,
//!   "kept it" and "kept it and rewrote it" look the same), so there is an
//!   explicit escalation pass over every element a hunk drops.
//!
//! ## Unordered lists, as commutative *groups*
//!
//! `Language::child_list_kind` reports `Unordered` or `PartiallyUnordered`.
//! Rather than merging such a list as a flat set — which would let an import
//! float past a class declaration (docs/prior-art.md §2.4) — the ordered
//! machinery runs unchanged and only the **resolution of a conflicting region**
//! differs: if every element in the region, on all three sides, is of a
//! commutable kind, the region is resolved by set union instead of conflicting.
//!
//! That one change buys the group discipline. A region containing a class
//! declaration is not resolvable by union, so the alignment keeps the class
//! where it was and the imports around it commute only among themselves.
//!
//! The union takes **our** list, drops what they deleted, and appends the
//! elements they added that we do not already have — so new elements land at
//! the end of the region they were inserted into, ours before theirs on a tie.
//! Because the union is applied per aligned region rather than per whole list,
//! an addition still lands next to the ancestor element it was written next to.
//!
//! ### Separators
//!
//! Some commutative groups put a token between their elements — TypeScript's
//! `{ a, b }` import clause, Java's `throws A, B`. Removing an element from
//! such a run also means removing the right comma, and getting that wrong
//! produces code that does not parse. So a region containing tokens is unioned
//! in exactly one shape: when it is a **pure insertion on both sides**, where
//! nothing is removed and each side's run can be taken whole, separators
//! included. Everything else conflicts. That is what makes "both branches
//! imported one more name" a clean merge in TypeScript while keeping the rule
//! provably safe.
//!
//! ## Atomic sets
//!
//! Kinds in [`MergeConfig::atomic_set_kinds`] — `import_declaration` by
//! default — are keyed by their own **whitespace-stripped source text** and are
//! never merged internally. This overrides the matcher inside them on purpose:
//! `sm-match`'s recovery pass will pair `import java.util.HashMap;` with
//! `import java.util.Optional;` because they are the same shape, and reading
//! that as an update is a semantically wrong merge. Under text keying it is a
//! deletion plus an insertion, which is what it is.
//!
//! # 6. Conflict granularity and promotion
//!
//! SPEC.md §4.6 requires conflicts at the smallest *sensible* boundary —
//! statement or declaration, never expression. Two mechanisms:
//!
//! - **A conflict raised in a child list stays where it is** if every node in
//!   the region is a named node of a boundary kind. A collision between two
//!   inserted statements is already at statement granularity.
//! - **Otherwise it is promoted.** It is materialised where it arose, and a
//!   flag travels up the recursion; the first enclosing node whose kind is a
//!   boundary ([`MergeConfig::is_boundary`]) replaces itself with a single
//!   conflict over its own three nodes and clears the flag. If nothing above is
//!   a boundary, the conflict stays where it was raised.
//!
//! A boundary is a kind ending in `_declaration`, `_statement` or `_definition`
//! plus a short explicit list. That is a property of how tree-sitter grammars
//! are conventionally named rather than a per-language table, and it covers
//! every statement and member kind in `tree-sitter-java` and
//! `tree-sitter-typescript`. `block`, `class_body` and `program` are
//! deliberately *not* boundaries: they are containers whose *elements* are the
//! boundaries, and promoting to them would turn one bad statement into a
//! whole-method or whole-file conflict.
//!
//! # 7. Moves across containers
//!
//! Each base node is assigned one **home** — the container it will be emitted
//! under — before the descent begins, so that it is emitted exactly once:
//!
//! - neither side reparented it → its ancestor's container;
//! - one side did → that side's container;
//! - both did, to corresponding containers → that container;
//! - both did, to different containers → [`ConflictReason::MoveMove`], raised
//!   at our destination.
//!
//! The container a node left simply does not list it, which is what makes the
//! covering rule fall out: "we deleted the region, they moved an element out of
//! it" is not a delete/modify conflict, because their element is emitted at its
//! destination with their changes intact (docs/prior-art.md §2.5).
//!
//! The same mechanism handles the wrapped-block case, which is the one that
//! makes tree merging worth doing. If we wrap a block in `if (…) { }`, the
//! matcher pairs the ancestor's block with the *inner* block, so the ancestor's
//! statements have a home inside a subtree that exists only on our side. That
//! subtree is therefore not spliced verbatim: it is descended into, and at the
//! inner block the ordinary three-way decision runs, so their edit to a
//! statement inside it applies — reindented by `sm-emit` to its new depth.
//!
//! # 8. Comments
//!
//! Comments are not matched (`sm-match` excludes them by design) and they are
//! not in the content sequence. They ride along instead:
//!
//! - A comment the trivia pass attached to a node is inside that node's
//!   **extent**, so it moves, splices and deletes with its owner.
//! - A *floating* comment lies in the gap between two elements, and a gap is
//!   copied with the element that follows it. It therefore travels with its
//!   following neighbour, and is dropped if that neighbour is.
//! - A comment edit is a content change on its owner, so a comment-only change
//!   on one side merges cleanly against anything on the other.
//! - If **both** sides changed the comments attached to the same node, and
//!   changed them differently, that is [`ConflictReason::CommentEdit`] on the
//!   owner. Conservative on purpose: reconciling two prose edits is not
//!   something a merge algorithm should attempt.
//! - Otherwise, when both sides changed a node's content, the container frame —
//!   and with it the node's own attached comments — comes from whichever side
//!   edited the comments, defaulting to ours.
//!
//! # 9. Whitespace ownership
//!
//! Stated once, because everything about the output's readability follows from
//! it. For a container emitted as [`MergedNode::Rebuilt`] from revision `S`:
//!
//! - The container's **head** (its leading comments, up to its own first byte)
//!   and **tail** (from its last content item to the end of its extent) come
//!   from `S`. In practice that is the `{` and the `}`.
//! - Every emitted child brings a **leading gap**, recorded in
//!   [`MergedTree::lead`]: the bytes from the previous item's extent end to its
//!   own extent start, *in some revision*. Which revision is the rule:
//!   1. if the child comes from `S`, the gap comes from `S`;
//!   2. otherwise, if `S` has a slot with the same identity, the gap comes from
//!      `S` — so a statement whose *content* is theirs but whose *slot* is ours
//!      is indented the way our file indents it;
//!   3. otherwise the gap comes from the child's own revision, which is where
//!      it had a preceding sibling to measure against.
//! - A conflict's gap comes from ours, then the ancestor, then theirs.
//!
//! Nothing else touches whitespace. The only bytes this crate can produce that
//! are in no input are [`Gap::Synthesized`], which the merge does not currently
//! emit at all; the variant exists so SPEC.md §5's byte-preservation property is
//! stated over a real distinction rather than an assumption.
//!
//! # 10. Known limitations
//!
//! - **No per-subtree line-based fallback.** docs/prior-art.md §2.3 records
//!   Mergiraf's `LineBasedMerge` node type, which lets one gnarly method
//!   degrade to diff3 while the rest of the file merges structurally.
//!   [`MergedNode`] has room for it; M4b's driver has only the whole-file
//!   fallback SPEC.md §4.7 mandates.
//! - **No signature-keyed duplicate post-pass.** Two sides adding methods with
//!   the same erasure but different bodies at different anchors both apply,
//!   where Spork and Mergiraf would conflict (docs/prior-art.md §2.6).
//!   Textually identical additions *are* deduplicated.
//! - **Multi-line leaves are atomic.** A block comment or text block edited on
//!   both sides conflicts wholesale rather than merging line by line
//!   (docs/prior-art.md §8.2.7).
//! - **Both sides wrapping the same region differently** conflicts over the
//!   whole region rather than reporting the two wrappers. That is the
//!   conservative answer, and it is the one SPEC.md §0.4 asks for.
//! - **A conflict among separator-bearing elements is promoted to the enclosing
//!   declaration.** Two branches adding a different enum constant conflict over
//!   the whole `enum_declaration` rather than over the two constants, because a
//!   marker block whose first line is a bare `,` is not a region a reader can
//!   act on. Bigger region, coherent region.
//! - **A restructuring the matcher reads as a container replacement is
//!   conservative.** If they unwrap an `if` and we edit a statement in the
//!   block that surrounded it, the matcher pairs the ancestor's *inner* block
//!   with their method body, the ancestor's method body has no counterpart on
//!   their side, and the answer is a conflict rather than a guess.
//! - **A floating comment is dropped with the element that follows it.** The
//!   alternative — orphaned banners left at deletion sites — was judged worse;
//!   the trivia pass already attaches anything within one newline of a
//!   declaration, so a floating comment is one separated by a blank line.
//! - **The convergence and dedup tests trust a 64-bit hash for comment text.**
//!   Code equality is confirmed structurally; comment equality is not. A
//!   collision would swap one comment for an equally-hashing one, never change
//!   code.

mod config;
mod engine;
mod fate;
pub mod layout;
mod model;
mod seq;
mod tables;

pub use config::MergeConfig;
pub use fate::Fate;
pub use model::{
    Conflict, ConflictReason, ConflictSummary, FateCounts, Gap, MergeOutcome, MergeStats, MergedId,
    MergedNode, MergedTree, Side,
};

use sm_cst::{Language, SourceTree};
use sm_match::{Matching, TreeMetrics, match_trees_with_metrics};

use crate::tables::SideTables;

/// Merge three revisions of one file.
///
/// All three trees must have been parsed with `lang`; structural hashes fold in
/// tree-sitter kind IDs, which are only comparable within one grammar.
///
/// # Guarantees
///
/// - **Deterministic.** Two runs on the same inputs produce an identical
///   outcome. Nothing in the decision path iterates a hash map.
/// - **Every leaf carries provenance.** [`MergedTree::provenance`] resolves any
///   non-conflict node to a `(side, NodeId)`, i.e. to a byte range in an input.
/// - **Total.** There is no input for which this returns an error: a file that
///   will not parse still produces a tree, and an unresolvable situation
///   produces a conflict. Falling back to a line merge belongs to the driver
///   (SPEC.md §4.7), which has the file-level context to decide it.
///
/// # Cost
///
/// Two matchings, then one pass over the three trees. The child-list alignment
/// is quadratic in the list length, bounded by
/// [`MergeConfig::alignment_budget`].
#[must_use]
pub fn merge(
    base: &SourceTree,
    ours: &SourceTree,
    theirs: &SourceTree,
    lang: &dyn Language,
    cfg: &MergeConfig,
) -> MergeOutcome {
    debug_assert_eq!(base.language_name(), ours.language_name());
    debug_assert_eq!(base.language_name(), theirs.language_name());

    let base_metrics = TreeMetrics::compute(base, lang);
    let ours_metrics = TreeMetrics::compute(ours, lang);
    let theirs_metrics = TreeMetrics::compute(theirs, lang);

    let m_ours = match_trees_with_metrics(
        base,
        &base_metrics,
        ours,
        &ours_metrics,
        lang,
        &cfg.base_to_ours,
    );
    let m_theirs = match_trees_with_metrics(
        base,
        &base_metrics,
        theirs,
        &theirs_metrics,
        lang,
        &cfg.base_to_theirs,
    );

    let base_tables = SideTables::with_metrics(base, base_metrics, lang);
    engine::Engine::new(base_tables, ours, theirs, &m_ours, &m_theirs, lang, cfg).run()
}

/// [`merge`], with the two base matchings supplied by the caller.
///
/// M4b's driver computes matchings for its own reporting and M5's sweep varies
/// them; neither should pay for them twice. `m_ours` must be a `base → ours`
/// matching and `m_theirs` a `base → theirs` one.
#[must_use]
pub fn merge_with_matchings(
    base: &SourceTree,
    ours: &SourceTree,
    theirs: &SourceTree,
    m_ours: &Matching,
    m_theirs: &Matching,
    lang: &dyn Language,
    cfg: &MergeConfig,
) -> MergeOutcome {
    let base_tables = SideTables::build(base, lang);
    engine::Engine::new(base_tables, ours, theirs, m_ours, m_theirs, lang, cfg).run()
}
