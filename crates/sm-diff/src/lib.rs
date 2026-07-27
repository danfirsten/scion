//! Edit-script derivation from a node matching (SPEC.md §4.4).
//!
//! This crate will turn a matching produced by `sm-match` into an ordered list
//! of operations over stable node IDs: `Insert`, `Delete`, `Update` (same node,
//! changed text) and — critically — `Move` (matched node, different parent or
//! different position). `Move` is the operation a line diff structurally cannot
//! express, and it is the source of most of this project's wins over `git
//! merge`.
//!
//! Scheduled for milestone **M3**. Empty in M0 — nothing calls into it.
