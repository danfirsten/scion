//! The concrete syntax tree layer for `semantic-merge` (SPEC.md §4.2).
//!
//! This crate wraps tree-sitter in an arena that every later stage of the
//! pipeline works against:
//!
//! - [`SourceTree`] owns the source bytes and a `Vec<Node>` indexed by
//!   [`NodeId`]. IDs are assigned in preorder, so they are stable within a tree
//!   and a parent's ID is always smaller than any of its descendants'.
//! - Every byte of the input is recoverable. Tokens are nodes; the whitespace
//!   *between* tokens is not a node, but it is never discarded — it stays in the
//!   owned source buffer and is addressed by the gaps between sibling ranges.
//!   The [`invariants`] module turns that into an assertion the test suite runs
//!   over every fixture.
//! - [`Language`] abstracts the per-language configuration (ordered vs unordered
//!   child lists, scope-introducing kinds, identifier kinds, comment kinds) so
//!   that nothing above this crate mentions Java.
//!
//! # Milestone status
//!
//! M0 implements parsing, the arena, the invariant checks, the `Language` trait
//! with a Java implementation, and the tree renderer used by `sm parse`. The
//! *trivia attachment pass* — the heuristic that decides which comment belongs
//! to which node — is not implemented yet. The data model for it is in place:
//! see [`Node::leading_trivia`], [`Node::trailing_trivia`] and [`Attachment`],
//! which are respectively empty and [`Attachment::Floating`] until that pass
//! runs. Until then comments appear only at their structural position in the
//! tree as `extra` nodes, which is a complete and correct — just not yet
//! useful — description of the file.

mod arena;
pub mod invariants;
pub mod json;
mod language;
pub mod languages;
mod parse;
mod render;

pub use arena::{Attachment, Node, NodeId, SourceTree};
pub use json::JsonTree;
pub use language::{ChildListKind, Language};
pub use parse::{MAX_SOURCE_LEN, ParseError, parse};
pub use render::{ERROR_WARNING, HeaderInfo, RenderOptions, render_parse};
