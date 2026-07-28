//! Edit-script derivation and structural diff rendering (SPEC.md §4.4, M3).
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use sm_diff::derive;
//! use sm_match::{MatchConfig, match_trees};
//!
//! let lang = sm_cst::languages::detect("Foo.java".as_ref()).expect("java");
//! let a = sm_cst::parse(&std::fs::read("a.java")?, lang)?;
//! let b = sm_cst::parse(&std::fs::read("b.java")?, lang)?;
//! let m = match_trees(&a, &b, lang, &MatchConfig::base_to_side());
//!
//! let script = derive(&a, &b, &m, lang);
//! println!("{} operations", script.len());
//! # Ok(()) }
//! ```
//!
//! # The model
//!
//! Four operations over stable [`sm_cst::NodeId`]s — [`EditOp::Insert`],
//! [`EditOp::Delete`], [`EditOp::Update`] and [`EditOp::Move`] — in a
//! documented deterministic order. `Move` is the one a line diff structurally
//! cannot express, and it is split into [`MoveKind::Reparent`] and
//! [`MoveKind::Reorder`], because those mean different things downstream: a
//! re-parent changes nesting depth and therefore needs reindentation, while a
//! reorder does not, and inside an unordered child list a reorder is not an edit
//! at all.
//!
//! Three properties the rest of the pipeline may assume, all of them tested:
//!
//! 1. **Minimal granularity.** `Insert` and `Delete` are reported at the
//!    highest unmatched node — a whole new method is one operation.
//!    [`participating_subtree`] expands one when a consumer wants the nodes.
//! 2. **Orthogonality.** Each fact is stated by exactly one kind of operation.
//!    See [`EditOp::Update`] for the precise definition of a *local* change and
//!    why inserting a child does not also mark its parent changed.
//! 3. **Minimal reorders.** Reordering is decided by a longest increasing
//!    subsequence over the matched siblings, so moving one element out of ten
//!    reports one `Move`, not nine.
//!
//! # The viewer
//!
//! [`render`] is the `sm diff` output and [`render_script`] the stable text
//! form the snapshot tests pin. See the [`render`](crate::render) module docs
//! for what the layout is trying to achieve; the short version is that a move,
//! a reindentation and a wrapped block each get a shape a line diff has no
//! vocabulary for.
//!
//! # Scope
//!
//! This crate derives and displays. It does not merge: `sm-merge` consumes a
//! [`Matching`](sm_match::Matching) directly and computes its own fate
//! classification, so the two can evolve in parallel (PROGRESS.md session 3).

mod lines;
mod lis;

pub mod json;
pub mod render;
pub mod script;

pub use json::DiffReport;
pub use lines::LineIndex;
pub use render::{DiffView, RenderOptions, Side, render, render_script, render_stat};
pub use script::{
    EditOp, EditScript, MoveKind, Summary, derive, derive_with_metrics, participating_subtree,
};
