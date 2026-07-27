//! Three-way tree merge and the conflict model (SPEC.md §4.5).
//!
//! Given the matchings `base -> ours` and `base -> theirs`, this crate will
//! compose through base to relate ours against theirs, classify every base
//! node's fate on each side (`Unchanged | Updated | Moved | Deleted |
//! MovedAndUpdated`), and combine child lists — sequence-merging the lists that
//! `sm_cst::Language` reports as ordered, and set-merging the ones it reports as
//! unordered (imports, class members). Conflicts are represented structurally as
//! `Conflict { base, ours, theirs, span, reason }`; text markers are the
//! emitter's job, not this crate's.
//!
//! Scheduled for milestone **M4**. Empty in M0 — nothing calls into it.
