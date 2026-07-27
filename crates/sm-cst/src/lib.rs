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
//!   that nothing above this crate mentions Java. Java and TypeScript (both the
//!   `.ts` and `.tsx` grammars) are implemented behind it; see
//!   [`languages`].
//!
//! - The trivia attachment pass ([`attach_trivia`], configured by
//!   [`TriviaConfig`]) decides which node each comment belongs to, recording
//!   the answer in [`Node::leading_trivia`], [`Node::trailing_trivia`] and
//!   [`Attachment`]. It runs as part of [`parse`], so every tree in the system
//!   arrives attached. The comment nodes stay where they are in the tree —
//!   attachment annotates, it never restructures.
//!
//! # Milestone status
//!
//! M0 is complete: parsing, the arena, the invariant checks, the `Language`
//! trait with a Java implementation, the trivia attachment pass and the tree
//! renderer used by `sm parse`.
//!
//! TypeScript was pulled forward from M5 to prove the `Language` abstraction
//! holds (SPEC.md §3). It required no change to the arena, the parser, the
//! trivia pass, the invariant checks, the renderer or the JSON schema — only a
//! new [`languages`] module and a registry entry. Where TypeScript wants
//! something the trait cannot express, that is recorded as a limitation rather
//! than worked around; see `languages::TypeScriptLanguage`.

mod arena;
pub mod invariants;
pub mod json;
mod language;
pub mod languages;
mod parse;
mod render;
mod trivia;

pub use arena::{Attachment, Node, NodeId, SourceTree};
pub use json::JsonTree;
pub use language::{ChildListKind, Language};
pub use parse::{MAX_SOURCE_LEN, ParseError, parse, parse_with_trivia_config};
pub use render::{ERROR_WARNING, HeaderInfo, RenderOptions, render_parse};
pub use trivia::{TriviaConfig, attach_trivia};
