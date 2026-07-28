//! Shared fixture loading for the `sm-diff` integration tests.
//!
//! Every integration test file is its own crate, so this module is compiled
//! once per test binary and each copy sees only the items that binary uses.
//! `dead_code` is therefore meaningless here.
#![allow(dead_code)]

use std::path::PathBuf;

use sm_cst::{Language, SourceTree, languages};
use sm_diff::{DiffView, EditScript, Side, derive};
use sm_match::{MatchConfig, Matching, TreeMetrics, match_trees};

/// Every fixture pair, as `(directory, extension)`.
///
/// An explicit list rather than a directory scan, for the reason `sm-cst` and
/// `sm-match` both keep one: adding a case should be a deliberate act, and a
/// missing case should fail loudly. `sm-diff` keeps its own copies of the four
/// shapes it shares with `sm-match` so that a snapshot here can never be
/// invalidated by a change made for the matcher's benefit.
pub const CASES: &[(&str, &str)] = &[
    // No change at all: the script must be empty.
    ("identical", "java"),
    // A method's name changed and nothing else: one Update, no Insert/Delete.
    ("renamed_method", "java"),
    // A method moved within its class body, byte for byte. `class_body` is
    // Unordered, so this must derive to *nothing at all*.
    ("moved_method", "java"),
    // A method moved from one class to another: one Reparent.
    ("moved_across_classes", "java"),
    // Statements permuted inside a block, which is Ordered: one Reorder, and
    // the LIS is what keeps it to one.
    ("reordered_statements", "java"),
    // A method moved *and* gained statements.
    ("moved_and_edited_method", "java"),
    // Three statements wrapped in an `if`, reindenting every line.
    ("wrapped_in_if", "java"),
    // A subexpression became a local variable.
    ("extracted_variable", "java"),
    // Imports permuted and nothing else. `program` is PartiallyUnordered over
    // `import_declaration`, so this must be an empty script.
    ("import_shuffle", "java"),
    // A whole method added: one Insert, not one per node.
    ("insert_method", "java"),
    // A whole method removed: one Delete.
    ("delete_method", "java"),
    // A body wrapped in an `if` *and* indented by eight rather than four.
    ("reindented_method", "java"),
    // Everything at once: imports churned, methods moved, renamed, wrapped,
    // one deleted, one statement extracted.
    ("stress", "java"),
    // The same shapes in TypeScript, to prove nothing here knows about Java.
    ("ts_class_edit", "ts"),
];

#[must_use]
pub fn case_path(case: &str, ext: &str, side: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(case)
        .join(format!("{side}.{ext}"))
}

#[must_use]
pub fn language_for(ext: &str) -> &'static dyn Language {
    languages::detect(std::path::Path::new(&format!("x.{ext}")))
        .unwrap_or_else(|| panic!("no language registered for .{ext}"))
}

#[must_use]
pub fn parse_source(source: &str, ext: &str) -> SourceTree {
    sm_cst::parse(source.as_bytes(), language_for(ext)).expect("parsing a test snippet")
}

#[must_use]
pub fn parse_side(case: &str, ext: &str, side: &str) -> SourceTree {
    let path = case_path(case, ext, side);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|err| panic!("reading fixture {}: {err}", path.display()));
    sm_cst::parse(&bytes, language_for(ext))
        .unwrap_or_else(|err| panic!("parsing fixture {}: {err}", path.display()))
}

/// Everything one case needs, kept alive together so the borrows in
/// [`Loaded::view`] have somewhere to point.
pub struct Loaded {
    pub lang: &'static dyn Language,
    pub a: SourceTree,
    pub b: SourceTree,
    pub am: TreeMetrics,
    pub bm: TreeMetrics,
    pub matching: Matching,
    pub script: EditScript,
    pub a_label: String,
    pub b_label: String,
}

impl Loaded {
    #[must_use]
    pub fn view(&self) -> DiffView<'_> {
        DiffView {
            src: Side::new(&self.a, &self.am, &self.a_label),
            dst: Side::new(&self.b, &self.bm, &self.b_label),
            matching: &self.matching,
            lang: self.lang,
            script: &self.script,
        }
    }
}

/// Parse, match and derive one fixture case under `cfg`.
#[must_use]
pub fn load_case(case: &str, ext: &str, cfg: &MatchConfig) -> Loaded {
    let lang = language_for(ext);
    let a = parse_side(case, ext, "a");
    let b = parse_side(case, ext, "b");
    load_trees(
        lang,
        a,
        b,
        cfg,
        format!("{case}/a.{ext}"),
        format!("{case}/b.{ext}"),
    )
}

/// Parse, match and derive from two in-memory snippets.
#[must_use]
pub fn load_snippets(a: &str, b: &str, ext: &str, cfg: &MatchConfig) -> Loaded {
    let lang = language_for(ext);
    let ta = parse_source(a, ext);
    let tb = parse_source(b, ext);
    load_trees(lang, ta, tb, cfg, "a".to_owned(), "b".to_owned())
}

fn load_trees(
    lang: &'static dyn Language,
    a: SourceTree,
    b: SourceTree,
    cfg: &MatchConfig,
    a_label: String,
    b_label: String,
) -> Loaded {
    let am = TreeMetrics::compute(&a, lang);
    let bm = TreeMetrics::compute(&b, lang);
    let matching = match_trees(&a, &b, lang, cfg);
    let script = derive(&a, &b, &matching, lang);
    Loaded {
        lang,
        a,
        b,
        am,
        bm,
        matching,
        script,
        a_label,
        b_label,
    }
}

/// A named constant profile, as the tests iterate them.
pub type NamedProfile = (&'static str, fn() -> MatchConfig);

/// Both shipped profiles, with the names the snapshots use.
pub const PROFILES: &[NamedProfile] = &[
    ("base", MatchConfig::base_to_side),
    ("strict", MatchConfig::ours_to_theirs),
];
