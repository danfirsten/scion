//! Structural node matching between two CSTs (SPEC.md §4.3).
//!
//! This crate will implement the GumTree algorithm (Falleri et al., ASE 2014) in
//! two phases: a top-down greedy pass that matches the tallest isomorphic
//! subtrees first, and a bottom-up pass that recovers containers by maximising
//! the dice coefficient over already-matched descendants. The tuning constants
//! (`MIN_HEIGHT`, `MIN_DICE`, `MAX_SIZE`) will be config-driven so M5 can sweep
//! them against the mined corpus.
//!
//! Scheduled for milestone **M2**. Empty in M0 — nothing calls into it.
