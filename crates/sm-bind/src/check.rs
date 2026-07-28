//! The merge-time semantic check: [`check`] and [`SemanticConflict`].
//!
//! Everything here is *differential*. Nothing is reported because a name failed
//! to resolve; things are reported because a name that **did** resolve in the
//! branch it came from stopped resolving, or started resolving somewhere else,
//! once the merge put the two branches together. See the crate documentation.

use std::ops::Range;

use serde::{Deserialize, Serialize};
use sm_cst::{DeclKind, Language};
use sm_merge::{MergeOutcome, MergedId, Side};

use crate::scope::{Decl, DeclId, NodeRef, Reference, Resolution, Revisions, ScopeTree};

/// What went wrong.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticConflictKind {
    /// The reference resolved to a declaration in the revision it came from,
    /// and resolves to nothing in the merged result.
    ///
    /// The canonical cause is the one SPEC.md §1 opens with: one branch renamed
    /// or deleted a declaration, the other branch added a use of the old name,
    /// the two edits touch different lines, and the line merge is clean.
    BrokenReference,
    /// The reference resolved to one declaration in the revision it came from,
    /// and resolves to a **different** declaration in the merged result.
    ///
    /// The classic silent capture: one branch introduces a local that shadows a
    /// field the other branch's code was using.
    CapturedReference,
}

impl SemanticConflictKind {
    /// A short stable identifier, for histograms and snapshot tests.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::BrokenReference => "broken_reference",
            Self::CapturedReference => "captured_reference",
        }
    }
}

/// A byte range in one of the three input revisions.
///
/// # Why not the merged file's coordinates
///
/// The check runs on the merge *plan*, before anything is serialised, which is
/// what lets it run on a merge that still has conflicts in it (`sm-emit` would
/// write marker lines that do not parse). The plan addresses input nodes, so
/// that is what a span here is.
///
/// Every reference this reports is spliced into the merged output verbatim —
/// the only transform `sm-emit` applies is rewriting the *leading whitespace of
/// a line*, which cannot touch an identifier token. So the bytes named here are
/// byte-for-byte the bytes that end up in the merged file; only the offset
/// differs. Mapping the offset needs an emitter provenance map, which is the
/// one small thing the next wave should add to `sm-emit` if the driver wants to
/// print merged line numbers.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Span {
    /// Which revision these bytes live in.
    pub side: Side,
    /// The node.
    pub node: sm_cst::NodeId,
    /// Its byte range within that revision's source.
    pub range: Range<u32>,
}

impl Span {
    fn of(revs: Revisions<'_>, at: NodeRef) -> Self {
        Self {
            side: at.side,
            node: at.node,
            range: revs.span(at),
        }
    }
}

/// One declaration, described well enough to explain a conflict.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DeclInfo {
    /// What it declares.
    pub kind: DeclKind,
    /// Its name.
    pub name: String,
    /// The kind of the scope that owns it — `program`, `class_declaration`,
    /// `block`. Two declarations with the same name and kind in different
    /// scopes are the shape a capture takes, so this is what makes an
    /// explanation readable.
    pub scope_kind: String,
    /// Where its name node is.
    pub span: Span,
}

impl DeclInfo {
    fn of(revs: Revisions<'_>, tree: &ScopeTree, decl: &Decl) -> Self {
        Self {
            kind: decl.kind,
            name: String::from_utf8_lossy(&decl.name).into_owned(),
            scope_kind: tree.scope(decl.scope).kind.to_owned(),
            span: Span::of(revs, decl.name_node),
        }
    }
}

/// A name-binding disagreement the syntactic merge could not see.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SemanticConflict {
    /// Broken or captured.
    pub kind: SemanticConflictKind,
    /// The text of the reference.
    pub name: String,
    /// Where the reference is. Its `side` is the branch that contributed it.
    pub reference: Span,
    /// What it bound to in the revision it came from. Always `Some` — a
    /// reference that resolved to nothing there is never reported.
    pub origin_declaration: Option<DeclInfo>,
    /// What it binds to in the merged result. `None` for a
    /// [`SemanticConflictKind::BrokenReference`].
    pub merged_declaration: Option<DeclInfo>,
    /// The nearest enclosing node of the merge plan, for a caller that wants to
    /// locate this in [`sm_merge::MergedTree`].
    pub anchor: Option<MergedId>,
    /// A sentence a human can act on.
    pub explanation: String,
}

/// Everything [`check`] computed, including the counters that make the result
/// auditable.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct CheckStats {
    /// Lexically-resolved references found in the merged program.
    pub references: usize,
    /// Of those, how many resolved in the revision they came from. Only these
    /// can produce a report.
    pub resolved_in_origin: usize,
    /// How many were skipped because they resolved to nothing in their own
    /// revision — cross-file names, library names, inherited members.
    pub unresolved_in_origin: usize,
    /// References the merged program held but the revision they came from did
    /// not classify as references.
    ///
    /// Should always be `0` — the two walks run the same `Language` over the
    /// same arenas — and is reported rather than asserted so that a corpus scan
    /// surfaces a discrepancy instead of aborting a merge.
    pub origin_not_found: usize,
    /// Conflict regions the walk skipped.
    pub conflict_regions: usize,
    /// Reported conflicts.
    pub conflicts: usize,
}

/// The result of a check.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CheckReport {
    /// The conflicts, in merged-program order.
    pub conflicts: Vec<SemanticConflict>,
    /// Counters.
    pub stats: CheckStats,
}

/// Re-resolve every reference in a candidate merge and report the ones the
/// merge broke or captured.
///
/// The three trees must be the ones the merge ran on: the merge plan's
/// provenance pointers are [`sm_cst::NodeId`]s into them.
///
/// See the crate documentation for the rules, the guarantees and the
/// limitations. In one line: **a name is only reported when it resolved in the
/// branch it came from and resolves differently, or not at all, in the merged
/// result.**
///
/// # Cost
///
/// Four scope-tree builds (three revisions plus the merged program) and one
/// scope-chain walk per reference. No parsing, no matching, no I/O.
#[must_use]
pub fn check_report(
    outcome: &MergeOutcome,
    base: &sm_cst::SourceTree,
    ours: &sm_cst::SourceTree,
    theirs: &sm_cst::SourceTree,
    lang: &dyn Language,
) -> CheckReport {
    let revs = Revisions { base, ours, theirs };
    let origins = [
        ScopeTree::build(base, Side::Base, lang),
        ScopeTree::build(ours, Side::Ours, lang),
        ScopeTree::build(theirs, Side::Theirs, lang),
    ];
    let merged = ScopeTree::build_merged(&outcome.tree, revs, lang);

    let mut stats = CheckStats {
        conflict_regions: merged.conflict_regions(),
        ..CheckStats::default()
    };
    let mut conflicts = Vec::new();

    for reference in merged.references() {
        stats.references += 1;
        let origin = &origins[slot(reference.node.side)];
        // The same node, in the tree it came from. Provenance is the key: the
        // merge plan says these bytes are that revision's node, so the two
        // `NodeRef`s are equal by construction.
        let Some(origin_ref_id) = origin.reference_at(reference.node) else {
            stats.origin_not_found += 1;
            continue;
        };
        let origin_ref = origin.reference(origin_ref_id);

        let Resolution::Decl(origin_decl) = origin.resolve(origin_ref) else {
            // THE RULE THAT KEEPS THE FALSE-POSITIVE RATE NEAR ZERO. A name
            // that did not resolve in its own branch is a cross-file name, a
            // library name or an inherited member — none of which this
            // single-file analysis can see, and none of which the merge did
            // anything to.
            stats.unresolved_in_origin += 1;
            continue;
        };
        stats.resolved_in_origin += 1;

        match merged.resolve(reference) {
            Resolution::Unresolved => conflicts.push(broken(
                revs,
                reference,
                origin,
                origin_decl,
                &merged,
                &origins,
            )),
            Resolution::Decl(merged_decl) => {
                if !same_declaration(&merged, merged_decl, origin, origin_decl) {
                    conflicts.push(captured(
                        revs,
                        reference,
                        origin,
                        origin_decl,
                        &merged,
                        merged_decl,
                    ));
                }
            }
        }
    }

    // Already in merged-program order: `references()` is the virtual preorder.
    stats.conflicts = conflicts.len();
    CheckReport { conflicts, stats }
}

/// [`check_report`] without the counters.
///
/// The shape SPEC.md §7 asks for; use [`check_report`] when you want to report
/// *how much* was examined, which is what makes a corpus-scale number credible.
#[must_use]
pub fn check(
    outcome: &MergeOutcome,
    base: &sm_cst::SourceTree,
    ours: &sm_cst::SourceTree,
    theirs: &sm_cst::SourceTree,
    lang: &dyn Language,
) -> Vec<SemanticConflict> {
    check_report(outcome, base, ours, theirs, lang).conflicts
}

const fn slot(side: Side) -> usize {
    match side {
        Side::Base => 0,
        Side::Ours => 1,
        Side::Theirs => 2,
    }
}

/// Are these two declarations, in two different programs, the same declaration?
///
/// Provenance first: if the merged program kept the very node the origin
/// resolved to, there is nothing to discuss. Otherwise fall back to the
/// signature, whose deliberate coarseness is documented on
/// [`ScopeTree::signature`].
fn same_declaration(
    merged: &ScopeTree,
    merged_decl: DeclId,
    origin: &ScopeTree,
    origin_decl: DeclId,
) -> bool {
    if merged.decl(merged_decl).decl_node == origin.decl(origin_decl).decl_node {
        return true;
    }
    merged.signature(merged_decl) == origin.signature(origin_decl)
}

/// The reference no longer resolves. Work out whether a rename explains it, and
/// say so.
fn broken(
    revs: Revisions<'_>,
    reference: &Reference,
    origin: &ScopeTree,
    origin_decl: DeclId,
    merged: &ScopeTree,
    origins: &[ScopeTree; 3],
) -> SemanticConflict {
    let decl = origin.decl(origin_decl);
    let name = String::from_utf8_lossy(&reference.name).into_owned();
    let from = reference.node.side;

    let explanation = match rename_hint(merged, origin, origin_decl) {
        Some(new_decl) => {
            let new_name = String::from_utf8_lossy(&new_decl.name).into_owned();
            let culprit = new_decl.decl_node.side;
            format!(
                "`{culprit}` renamed {kind} `{name}` to `{new_name}`; the reference to `{name}` \
                 from `{from}` still says `{name}`, which no longer resolves in the merged result.",
                kind = decl.kind.tag(),
            )
        }
        None => {
            let culprit = deleting_side(origins, from, origin, origin_decl);
            match culprit {
                Some(side) => format!(
                    "`{side}` removed {kind} `{name}`; the reference to `{name}` from `{from}` \
                     no longer resolves in the merged result.",
                    kind = decl.kind.tag(),
                ),
                None => format!(
                    "the reference to `{name}` from `{from}` resolved there to {kind} `{name}`, \
                     and resolves to nothing in the merged result — the other branch renamed or \
                     removed it.",
                    kind = decl.kind.tag(),
                ),
            }
        }
    };

    SemanticConflict {
        kind: SemanticConflictKind::BrokenReference,
        name,
        reference: Span::of(revs, reference.node),
        origin_declaration: Some(DeclInfo::of(revs, origin, decl)),
        merged_declaration: None,
        anchor: reference.anchor,
        explanation,
    }
}

/// The reference resolves, but somewhere else.
fn captured(
    revs: Revisions<'_>,
    reference: &Reference,
    origin: &ScopeTree,
    origin_decl: DeclId,
    merged: &ScopeTree,
    merged_decl: DeclId,
) -> SemanticConflict {
    let was = origin.decl(origin_decl);
    let now = merged.decl(merged_decl);
    let name = String::from_utf8_lossy(&reference.name).into_owned();
    let from = reference.node.side;
    let culprit = now.decl_node.side;

    let explanation = format!(
        "the reference to `{name}` from `{from}` bound to the {was_kind} `{name}` declared in \
         `{was_scope}` there; in the merged result it binds instead to the {now_kind} `{name}` \
         that `{culprit}` declared in `{now_scope}` — a silent capture that both the line merge \
         and the tree merge accept.",
        was_kind = was.kind.tag(),
        now_kind = now.kind.tag(),
        was_scope = origin.scope(was.scope).kind,
        now_scope = merged.scope(now.scope).kind,
    );

    SemanticConflict {
        kind: SemanticConflictKind::CapturedReference,
        name,
        reference: Span::of(revs, reference.node),
        origin_declaration: Some(DeclInfo::of(revs, origin, was)),
        merged_declaration: Some(DeclInfo::of(revs, merged, now)),
        anchor: reference.anchor,
        explanation,
    }
}

/// Did a rename explain the disappearance?
///
/// Look in the merged program for a declaration of the same kind, in the same
/// shape of scope, whose name did not exist in that position in the origin
/// revision. Exactly one such candidate is a rename; zero or several is not
/// something to assert, and the explanation says "renamed or removed" instead.
fn rename_hint<'m>(
    merged: &'m ScopeTree,
    origin: &ScopeTree,
    origin_decl: DeclId,
) -> Option<&'m Decl> {
    let want = origin.signature(origin_decl);
    // Names already present in that position on the origin side. A rename
    // introduces a name that was not.
    let known: Vec<&[u8]> = origin
        .decls()
        .iter()
        .enumerate()
        .filter(|(i, d)| {
            d.kind == want.kind && origin.signature(DeclId(*i as u32)).scope_path == want.scope_path
        })
        .map(|(_, d)| d.name.as_slice())
        .collect();

    let mut found: Option<&Decl> = None;
    for (i, d) in merged.decls().iter().enumerate() {
        if d.kind != want.kind || d.name == want.name {
            continue;
        }
        if merged.signature(DeclId(i as u32)).scope_path != want.scope_path {
            continue;
        }
        if known.contains(&d.name.as_slice()) {
            continue;
        }
        if found.is_some() {
            return None; // ambiguous; do not assert a rename
        }
        found = Some(d);
    }
    found
}

/// Which branch dropped the declaration, if exactly one of them did.
///
/// The reference came from one branch and resolved there. If the *other*
/// branch's own scope tree has no declaration with that signature, that branch
/// is the one that removed it.
fn deleting_side(
    origins: &[ScopeTree; 3],
    origin_side: Side,
    origin: &ScopeTree,
    origin_decl: DeclId,
) -> Option<Side> {
    let want = origin.signature(origin_decl);
    let mut culprit = None;
    for side in [Side::Ours, Side::Theirs] {
        if side == origin_side {
            continue;
        }
        let tree = &origins[slot(side)];
        let has = tree
            .decls()
            .iter()
            .enumerate()
            .any(|(i, _)| tree.signature(DeclId(i as u32)) == want);
        if !has {
            if culprit.is_some() {
                return None;
            }
            culprit = Some(side);
        }
    }
    culprit
}
