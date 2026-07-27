//! Scope tree construction and name resolution (SPEC.md §4, §7 M6).
//!
//! This crate is where the project's novel contribution lives. It will build a
//! scope tree per version — seeded by the kinds `sm_cst::Language::
//! is_scope_introducing` reports — and resolve every identifier to a
//! declaration. After a candidate merge is produced, each reference in the
//! merged tree is re-resolved: a reference that resolves to nothing, or to a
//! *different* declaration than it did in the branch it came from, is a
//! **semantic conflict**. That catches the rename-on-one-side /
//! new-call-on-the-other case that line merge and purely syntactic structural
//! merge both miss.
//!
//! Scheduled for milestone **M6**. Empty in M0 — nothing calls into it.
