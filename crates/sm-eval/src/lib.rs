//! Corpus mining, replay harness and metrics (SPEC.md §6).
//!
//! This crate produces the deliverable the whole project exists for. Milestone
//! **M1** is the mining half, and it is implemented here:
//!
//! * [`mine`] walks `git log --merges` over a list of Java repositories,
//!   recovers each two-parent merge's `(base, ours, theirs, human_resolution)`
//!   tuples via an in-memory `git merge-tree` replay, flags cases contaminated
//!   by unrelated edits made during the merge, samples the clean merges that
//!   M5 needs as a regression denominator, and writes `corpus/manifest.json`.
//! * [`stats`] renders the committed summary of what was mined and what was
//!   excluded.
//! * [`sample`] carves a small permissively licensed slice out of the corpus
//!   for use as test fixtures.
//!
//! The replay half — re-running the merge driver over the corpus and reporting
//! resolve rate, correct- and incorrect-resolve rate, regression rate,
//! divergence and latency — is milestone M5 and is not here yet. Nothing in
//! this crate calls into it.

pub mod git;
pub mod mine;
pub mod model;
pub mod sample;
pub mod stats;
pub mod util;
