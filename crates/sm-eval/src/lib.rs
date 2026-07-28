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
//! Milestone **M5** is the replay half, and it is here too:
//!
//! * [`replay`] runs the `sm merge` binary over every case — and over a
//!   `--line-merge-only` control arm — recording one JSONL record per
//!   invocation.
//! * [`compare`] is how a merged file is graded: byte-exact, AST-equal modulo
//!   formatting, and the two ground-truth-free criteria (parsable, universal)
//!   from Mori & Hashimoto (docs/prior-art.md §8.3.3).
//! * [`report`] folds a replay log into SPEC.md §6.2's table, the per-repository
//!   breakdown, the histograms and the incorrect-resolution gallery.

pub mod compare;
pub mod git;
pub mod mine;
pub mod model;
pub mod replay;
pub mod report;
pub mod sample;
pub mod stats;
pub mod util;
