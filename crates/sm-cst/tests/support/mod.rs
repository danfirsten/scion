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

/// Every TypeScript fixture, by *full* file name.
///
/// Unlike [`FIXTURES`] these carry their extension, because the extension is
/// what selects the grammar: `.ts` and `.tsx` are two different parsers, and a
/// TypeScript fixture list that hid that would be testing the wrong thing.
pub const TS_FIXTURES: &[&str] = &[
    "ts_typical.ts",
    "ts_react.tsx",
    "ts_unicode.ts",
    "ts_empty.ts",
    "ts_only_comment.ts",
    "ts_broken.ts",
];

/// TypeScript fixtures that are expected to contain syntax errors.
pub const BROKEN_TS_FIXTURES: &[&str] = &["ts_broken.ts"];

#[must_use]
pub fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[must_use]
pub fn fixture_path(name: &str) -> PathBuf {
    fixture_dir().join(format!("{name}.java"))
}

#[must_use]
pub fn fixture_bytes(name: &str) -> Vec<u8> {
    read_fixture(&fixture_path(name))
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

// ------------------------------------------------------------ TypeScript --

#[must_use]
pub fn ts_fixture_path(file_name: &str) -> PathBuf {
    fixture_dir().join(file_name)
}

#[must_use]
pub fn ts_fixture_bytes(file_name: &str) -> Vec<u8> {
    read_fixture(&ts_fixture_path(file_name))
}

/// The language the registry picks for a fixture file name.
///
/// Deliberately routed through [`languages::detect`] rather than naming
/// `TypeScriptLanguage` directly: every TypeScript test then also asserts that
/// the registry wiring works, which is half of what "the abstraction holds"
/// means.
#[must_use]
pub fn ts_lang(file_name: &str) -> &'static dyn Language {
    languages::detect(&ts_fixture_path(file_name))
        .unwrap_or_else(|| panic!("no language registered for {file_name}"))
}

/// The plain `.ts` language, for inline snippets that have no file of their own.
#[must_use]
pub fn typescript() -> &'static dyn Language {
    languages::by_name("typescript").expect("typescript should be registered")
}

/// The `.tsx` language.
#[must_use]
pub fn tsx() -> &'static dyn Language {
    languages::by_name("tsx").expect("tsx should be registered")
}

#[must_use]
pub fn parse_ts_fixture(file_name: &str) -> SourceTree {
    let source = ts_fixture_bytes(file_name);
    sm_cst::parse(&source, ts_lang(file_name))
        .unwrap_or_else(|err| panic!("parsing fixture {file_name}: {err}"))
}

fn read_fixture(path: &std::path::Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|err| panic!("reading fixture {}: {err}", path.display()))
}
