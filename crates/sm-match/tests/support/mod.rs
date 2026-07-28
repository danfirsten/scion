//! Shared fixture loading for the `sm-match` integration tests.
//!
//! Every integration test file is its own crate, so this module is compiled
//! once per test binary and each copy sees only the items that binary happens
//! to use. `dead_code` is therefore meaningless here.
#![allow(dead_code)]

use std::path::PathBuf;

use sm_cst::{Language, SourceTree, languages};
use sm_match::{MatchConfig, Matching, match_trees};

/// Every fixture pair, by directory name.
///
/// An explicit list rather than a directory scan, for the same reason
/// `sm-cst`'s test support module keeps one: adding a case should be a
/// deliberate act, and a *missing* case should fail loudly.
///
/// Each entry is a directory under `tests/fixtures/` holding `a.java` and
/// `b.java`, and each covers one edit shape the matcher has to survive.
pub const CASES: &[&str] = &[
    // Nothing changed: everything must match, nothing may be flagged a move.
    "identical",
    // A method's name changed and its body did not.
    "renamed_method",
    // A method moved within its class body, byte for byte.
    "moved_method",
    // A method moved *and* gained statements.
    "moved_and_edited_method",
    // Three statements got wrapped in an `if`, reindenting every line.
    "wrapped_in_if",
    // A subexpression became a local variable.
    "extracted_variable",
    // One import added, one removed.
    "import_churn",
    // Class members permuted with no textual change.
    "reordered_members",
    // A comment moved from one method to the next.
    "moved_comment",
    // Two files with nothing in common.
    "unrelated",
    // An empty file against a real one.
    "empty_vs_nonempty",
];

#[must_use]
pub fn case_path(case: &str, side: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(case)
        .join(format!("{side}.java"))
}

#[must_use]
pub fn java() -> &'static dyn Language {
    languages::detect(&case_path("identical", "a")).expect("java should be registered for .java")
}

#[must_use]
pub fn parse_source(source: &str) -> SourceTree {
    sm_cst::parse(source.as_bytes(), java()).expect("parsing a test snippet")
}

#[must_use]
pub fn parse_side(case: &str, side: &str) -> SourceTree {
    let path = case_path(case, side);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|err| panic!("reading fixture {}: {err}", path.display()));
    sm_cst::parse(&bytes, java())
        .unwrap_or_else(|err| panic!("parsing fixture {}: {err}", path.display()))
}

/// Parse both sides of a case.
#[must_use]
pub fn parse_case(case: &str) -> (SourceTree, SourceTree) {
    (parse_side(case, "a"), parse_side(case, "b"))
}

/// Match both sides of a case with the given profile.
#[must_use]
pub fn match_case(case: &str, cfg: &MatchConfig) -> (SourceTree, SourceTree, Matching) {
    let (a, b) = parse_case(case);
    let m = match_trees(&a, &b, java(), cfg);
    (a, b, m)
}

/// A named constant profile, as the tests iterate them.
pub type NamedProfile = (&'static str, fn() -> MatchConfig);

/// Both shipped profiles, with the names the snapshots use.
pub const PROFILES: &[NamedProfile] = &[
    ("base", MatchConfig::base_to_side),
    ("strict", MatchConfig::ours_to_theirs),
];
