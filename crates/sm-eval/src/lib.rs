//! Corpus mining, replay harness and metrics (SPEC.md §6).
//!
//! This crate produces the deliverable the whole project exists for. It will
//! walk `git log --merges` over a list of Java repositories, recover each merge
//! commit's `(base, ours, theirs, human_resolution)` tuples, filter out cases
//! contaminated by unrelated edits made during the merge, and write a manifest
//! to `corpus/`. The replay half re-runs the merge driver over the corpus and
//! reports resolve rate, correct- and incorrect-resolve rate, regression rate,
//! divergence and latency — on both byte-exact and AST-modulo-formatting
//! equality.
//!
//! Mining is scheduled for milestone **M1** and the evaluation for **M5**.
//! Empty in M0 — nothing calls into it.
