//! Scenario harness: three source strings in, a rendered semantic-conflict
//! report out, through the **real** pipeline.
//!
//! Nothing here fakes a merge. Each scenario parses three revisions with
//! `sm-cst`, merges them with `sm-merge` (which runs `sm-match` inside), emits
//! the result with `sm-emit`, and only then runs `sm_bind::check_report`. A test
//! that passed against a hand-built `MergedTree` would prove nothing about
//! whether the check fires on merges the pipeline actually produces.

#![allow(dead_code)]

use std::fmt::Write as _;

use sm_bind::CheckReport;
use sm_cst::{Language, SourceTree};
use sm_emit::{EmitOptions, emit};
use sm_merge::{MergeConfig, MergeOutcome, merge};

/// Everything one scenario produced.
pub struct Scenario {
    pub lang: &'static dyn Language,
    pub base: SourceTree,
    pub ours: SourceTree,
    pub theirs: SourceTree,
    pub outcome: MergeOutcome,
    /// The bytes `sm-emit` wrote, so a test can assert what the user would see.
    pub merged_text: String,
    pub report: CheckReport,
}

pub fn java() -> &'static dyn Language {
    sm_cst::languages::detect(std::path::Path::new("X.java")).expect("java is registered")
}

pub fn typescript() -> &'static dyn Language {
    sm_cst::languages::detect(std::path::Path::new("x.ts")).expect("typescript is registered")
}

/// Run one three-way merge and check its names.
pub fn run(lang: &'static dyn Language, base: &str, ours: &str, theirs: &str) -> Scenario {
    let base_tree = sm_cst::parse(base.as_bytes(), lang).expect("base parses");
    let ours_tree = sm_cst::parse(ours.as_bytes(), lang).expect("ours parses");
    let theirs_tree = sm_cst::parse(theirs.as_bytes(), lang).expect("theirs parses");

    let outcome = merge(
        &base_tree,
        &ours_tree,
        &theirs_tree,
        lang,
        &MergeConfig::for_language(lang),
    );
    let merged = emit(
        &outcome.tree,
        &base_tree,
        &ours_tree,
        &theirs_tree,
        lang,
        &EmitOptions::default(),
    );
    let report = sm_bind::check_report(&outcome, &base_tree, &ours_tree, &theirs_tree, lang);

    Scenario {
        lang,
        base: base_tree,
        ours: ours_tree,
        theirs: theirs_tree,
        outcome,
        merged_text: String::from_utf8_lossy(&merged.bytes).into_owned(),
        report,
    }
}

impl Scenario {
    /// Whether the *tree* merge produced no conflicts.
    pub fn tree_merge_is_clean(&self) -> bool {
        self.outcome.is_clean()
    }

    /// A stable, human-readable rendering of the semantic conflicts, for
    /// `insta`.
    pub fn render(&self) -> String {
        let mut out = String::new();
        writeln!(
            out,
            "tree merge: {}",
            if self.tree_merge_is_clean() {
                "clean"
            } else {
                "conflicted"
            }
        )
        .unwrap();
        let s = &self.report.stats;
        writeln!(
            out,
            "references: {} (resolved in origin {}, unresolved in origin {}), \
             skipped conflict regions: {}",
            s.references, s.resolved_in_origin, s.unresolved_in_origin, s.conflict_regions
        )
        .unwrap();
        writeln!(out, "semantic conflicts: {}", self.report.conflicts.len()).unwrap();
        for c in &self.report.conflicts {
            writeln!(out).unwrap();
            writeln!(out, "  [{}] `{}`", c.kind.tag(), c.name).unwrap();
            writeln!(
                out,
                "  reference: {} {:?}",
                c.reference.side,
                self.text(c.reference.side, &c.reference.range)
            )
            .unwrap();
            if let Some(d) = &c.origin_declaration {
                writeln!(
                    out,
                    "  was: {} `{}` in {} scope `{}`",
                    d.kind.tag(),
                    d.name,
                    d.span.side,
                    d.scope_kind
                )
                .unwrap();
            }
            if let Some(d) = &c.merged_declaration {
                writeln!(
                    out,
                    "  now: {} `{}` in {} scope `{}`",
                    d.kind.tag(),
                    d.name,
                    d.span.side,
                    d.scope_kind
                )
                .unwrap();
            }
            writeln!(out, "  {}", c.explanation).unwrap();
        }
        out
    }

    fn text(&self, side: sm_merge::Side, range: &std::ops::Range<u32>) -> String {
        let tree = match side {
            sm_merge::Side::Base => &self.base,
            sm_merge::Side::Ours => &self.ours,
            sm_merge::Side::Theirs => &self.theirs,
        };
        String::from_utf8_lossy(&tree.source()[range.start as usize..range.end as usize])
            .into_owned()
    }

    /// Assert this scenario reported nothing, with a message that says what it
    /// did report.
    pub fn assert_silent(&self) {
        assert!(
            self.report.conflicts.is_empty(),
            "expected no semantic conflicts, got:\n{}",
            self.render()
        );
    }
}

/// Ask git whether the *line* merge is clean, so a scenario can claim "git
/// merges this without complaint" rather than assert it by eye.
///
/// Returns `None` when git is not available, so the suite still runs in an
/// environment without it.
pub fn git_line_merge_is_clean(base: &str, ours: &str, theirs: &str) -> Option<bool> {
    use std::io::Write as _;
    use std::process::Command;

    let dir = std::env::temp_dir().join(format!(
        "sm-bind-linemerge-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).ok()?;
    for (name, text) in [("base", base), ("ours", ours), ("theirs", theirs)] {
        let mut f = std::fs::File::create(dir.join(name)).ok()?;
        f.write_all(text.as_bytes()).ok()?;
    }
    let status = Command::new("git")
        .arg("merge-file")
        .arg("-p")
        .arg(dir.join("ours"))
        .arg(dir.join("base"))
        .arg(dir.join("theirs"))
        .output()
        .ok()?;
    std::fs::remove_dir_all(&dir).ok();
    // `git merge-file` exits 0 when there were no conflicts, and with the
    // number of conflicts otherwise.
    status.status.code().map(|c| c == 0)
}

// ------------------------------------------------------- scope-tree helpers

use sm_bind::{Resolution, ScopeTree};
use sm_merge::Side;

/// A one-line description of what a name binds to, for assertions that have to
/// name a *specific* declaration rather than just "something".
///
/// `"local variable `x` in block @ line 5"`, or `"unresolved"`.
pub fn binding_of(lang: &'static dyn Language, src: &str, name: &str, occurrence: usize) -> String {
    let tree = sm_cst::parse(src.as_bytes(), lang).expect("parses");
    let scopes = ScopeTree::build(&tree, Side::Ours, lang);
    let reference = scopes
        .references()
        .iter()
        .filter(|r| r.name == name.as_bytes())
        .nth(occurrence)
        .unwrap_or_else(|| {
            panic!(
                "no reference `{name}` #{occurrence} in:\n{src}\n(references seen: {:?})",
                scopes
                    .references()
                    .iter()
                    .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                    .collect::<Vec<_>>()
            )
        });
    match scopes.resolve(reference) {
        Resolution::Unresolved => "unresolved".to_owned(),
        Resolution::Decl(d) => {
            let decl = scopes.decl(d);
            format!(
                "{} `{}` in {} @ line {}",
                decl.kind.tag(),
                String::from_utf8_lossy(&decl.name),
                scopes.scope(decl.scope).kind,
                line_of(
                    src,
                    tree.node(decl.name_node.node).byte_range.start as usize
                ),
            )
        }
    }
}

/// Every reference in the file and what it binds to, one per line — the
/// broad-coverage view that catches a regression nobody wrote an assertion for.
pub fn bindings_dump(lang: &'static dyn Language, src: &str) -> String {
    let tree = sm_cst::parse(src.as_bytes(), lang).expect("parses");
    let scopes = ScopeTree::build(&tree, Side::Ours, lang);
    let mut out = String::new();
    for r in scopes.references() {
        let what = match scopes.resolve(r) {
            Resolution::Unresolved => "unresolved".to_owned(),
            Resolution::Decl(d) => {
                let decl = scopes.decl(d);
                format!(
                    "{} `{}` @ line {}",
                    decl.kind.tag(),
                    String::from_utf8_lossy(&decl.name),
                    line_of(
                        src,
                        tree.node(decl.name_node.node).byte_range.start as usize
                    ),
                )
            }
        };
        writeln!(
            out,
            "line {:>3}  {:<8} {:<16} -> {what}",
            line_of(src, tree.node(r.node.node).byte_range.start as usize),
            r.role.tag(),
            String::from_utf8_lossy(&r.name),
        )
        .unwrap();
    }
    out
}

/// Every declaration in the file, with the scope that owns it.
pub fn decls_dump(lang: &'static dyn Language, src: &str) -> String {
    let tree = sm_cst::parse(src.as_bytes(), lang).expect("parses");
    let scopes = ScopeTree::build(&tree, Side::Ours, lang);
    let mut out = String::new();
    for d in scopes.decls() {
        writeln!(
            out,
            "line {:>3}  {:<16} {:<20} in {}",
            line_of(src, tree.node(d.name_node.node).byte_range.start as usize),
            d.kind.tag(),
            String::from_utf8_lossy(&d.name),
            scopes.scope(d.scope).kind,
        )
        .unwrap();
    }
    out
}

fn line_of(src: &str, offset: usize) -> usize {
    src[..offset].bytes().filter(|&b| b == b'\n').count() + 1
}
