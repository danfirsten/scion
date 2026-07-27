//! Source-range splicing serializer (SPEC.md §4.6).
//!
//! This crate will render a merged tree back to bytes by *copying* the byte
//! ranges each node points at in base, ours or theirs, synthesizing text only
//! where structurally unavoidable. Whole-file pretty-printing is forbidden; the
//! only permitted rewriting is a controlled reindentation transform applied when
//! a node's nesting depth changes. Conflicts are rendered here as
//! `<<<<<<<`/`=======`/`>>>>>>>` markers at the smallest sensible declaration or
//! statement boundary, honouring the marker size git passes as `%L`.
//!
//! Scheduled for milestone **M4**. Empty in M0 — nothing calls into it.
