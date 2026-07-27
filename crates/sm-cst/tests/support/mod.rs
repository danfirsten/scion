//! Shared fixture loading for the `sm-cst` integration tests.
//!
//! Every integration test file is its own crate, so this module is compiled once
//! per test binary and each copy sees only the items that binary happens to use.
//! `dead_code` is therefore meaningless here.
#![allow(dead_code)]

use std::path::PathBuf;

use sm_cst::{Language, SourceTree, languages};

/// Every fixture, by base name (without the `.java` extension).
///
/// Kept as an explicit list rather than a directory scan so that adding a
/// fixture is a deliberate act and a *missing* fixture fails loudly.
pub const FIXTURES: &[&str] = &[
    "typical",
    "enum_annotations",
    "generics_lambdas",
    "unicode",
    "empty",
    "only_comment",
    "broken",
    "trivia_gallery",
];

/// Fixtures that are expected to contain syntax errors.
pub const BROKEN_FIXTURES: &[&str] = &["broken"];

#[must_use]
pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.java"))
}

#[must_use]
pub fn fixture_bytes(name: &str) -> Vec<u8> {
    let path = fixture_path(name);
    std::fs::read(&path).unwrap_or_else(|err| panic!("reading fixture {}: {err}", path.display()))
}

#[must_use]
pub fn java() -> &'static dyn Language {
    languages::detect(&fixture_path("typical")).expect("java should be registered for .java")
}

/// Parse a fixture, asserting only that parsing itself succeeded.
#[must_use]
pub fn parse_fixture(name: &str) -> SourceTree {
    let source = fixture_bytes(name);
    sm_cst::parse(&source, java()).unwrap_or_else(|err| panic!("parsing fixture {name}: {err}"))
}
