//! `sm check <base> <ours> <theirs>` — M6's semantic check, standalone.
//!
//! The same [`sm_bind::check_report`] the merge driver runs, on three files you
//! name, without a git repository anywhere in sight. Two reasons it exists as
//! its own subcommand rather than only as a driver flag:
//!
//! - **M5's corpus scan drives it.** The mined corpus is triples of files on
//!   disk; measuring how often merge-time reference breakage actually happens is
//!   the deliverable of M6 (`sm-bind`'s crate docs say so), and `--json` here is
//!   the interface that measurement reads.
//! - **It makes the feature demonstrable.** "One branch renames, the other adds
//!   a call, both merges are clean and the build breaks" is three files and one
//!   command, not a repository, two branches and a driver installation.
//!
//! # Exit codes
//!
//! | code | meaning |
//! |---|---|
//! | 0 | the check ran and found nothing |
//! | 1 | the check ran and found something |
//! | 2 | a file could not be read, or its type is not supported |
//!
//! Exit 1 for "found something" is what makes this usable in a script or a CI
//! step, and it is the same convention `sm merge --semantic=conflict` follows.
//! It is *not* affected by the tree merge conflicting: the check walks the merge
//! plan and simply skips conflict regions (`sm-bind`'s `scope` module docs), so
//! a partly-conflicted merge still gets checked everywhere else, and the number
//! of regions skipped is reported so the answer can be read honestly.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use sm_bind::{CheckReport, SemanticConflict};
use sm_cst::{Language, SourceTree};
use sm_merge::{MergeConfig, MergeOutcome, Side, merge};

use crate::{EXIT_USAGE, load, write_all};

/// Found at least one semantic conflict.
const EXIT_FOUND: u8 = 1;

#[derive(clap::Args, Debug)]
pub struct CheckArgs {
    /// The merge base.
    #[arg(value_name = "BASE")]
    base: PathBuf,
    /// Our revision.
    #[arg(value_name = "OURS")]
    ours: PathBuf,
    /// Their revision.
    #[arg(value_name = "THEIRS")]
    theirs: PathBuf,

    /// Emit the report as JSON instead of prose.
    ///
    /// The object is `{ "schema_version", "language", "tree_merge_clean",
    /// "conflicts": [...], "stats": {...} }`; the conflicts are `sm-bind`'s own
    /// `SemanticConflict` serialization, so the shape has one definition.
    #[arg(long)]
    json: bool,

    /// Exit 0 even when conflicts are found.
    #[arg(long)]
    exit_zero: bool,
}

/// The JSON document, versioned for the same reason `--debug-json` is.
#[derive(serde::Serialize)]
struct JsonReport<'a> {
    schema_version: u32,
    tool: String,
    language: &'static str,
    /// Whether the *tree* merge was conflict-free. A semantic finding means
    /// something quite different depending on this.
    tree_merge_clean: bool,
    /// Conflict regions the check walked past.
    conflicts: &'a [SemanticConflict],
    stats: &'a sm_bind::CheckStats,
}

const SCHEMA_VERSION: u32 = 1;

pub fn run(args: &CheckArgs) -> ExitCode {
    let Some((lang, base, ours, theirs)) = load_three(&args.base, &args.ours, &args.theirs) else {
        return ExitCode::from(EXIT_USAGE);
    };

    for (path, tree) in [
        (&args.base, &base),
        (&args.ours, &ours),
        (&args.theirs, &theirs),
    ] {
        if tree.has_errors() {
            eprintln!(
                "sm: {}: parsed with ERROR/MISSING nodes; the check may be poor",
                path.display()
            );
        }
    }

    let outcome: MergeOutcome = merge(
        &base,
        &ours,
        &theirs,
        lang,
        &MergeConfig::for_language(lang),
    );
    let report = sm_bind::check_report(&outcome, &base, &ours, &theirs, lang);

    let rendered = if args.json {
        let doc = JsonReport {
            schema_version: SCHEMA_VERSION,
            tool: format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
            language: lang.name(),
            tree_merge_clean: outcome.is_clean(),
            conflicts: &report.conflicts,
            stats: &report.stats,
        };
        match serde_json::to_string_pretty(&doc) {
            Ok(mut json) => {
                json.push('\n');
                json
            }
            Err(err) => {
                eprintln!("sm: could not serialize the report: {err}");
                return ExitCode::from(EXIT_USAGE);
            }
        }
    } else {
        render(&report, &outcome, [&base, &ours, &theirs])
    };

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    if let Err(code) = write_all(&mut out, rendered.as_bytes()) {
        return code;
    }

    if report.conflicts.is_empty() || args.exit_zero {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_FOUND)
    }
}

/// Load three files and insist they are the same language.
fn load_three(
    base: &Path,
    ours: &Path,
    theirs: &Path,
) -> Option<(&'static dyn Language, SourceTree, SourceTree, SourceTree)> {
    let b = load(base)?;
    let o = load(ours)?;
    let t = load(theirs)?;
    if b.lang.name() != o.lang.name() || b.lang.name() != t.lang.name() {
        eprintln!(
            "sm: cannot check three files of different languages: {} ({}), {} ({}), {} ({})",
            base.display(),
            b.lang.name(),
            ours.display(),
            o.lang.name(),
            theirs.display(),
            t.lang.name()
        );
        return None;
    }
    Some((b.lang, b.tree, o.tree, t.tree))
}

/// The prose form.
///
/// Deliberately close to `sm-bind`'s own test rendering, so that what a reader
/// sees here and what the snapshots pin are the same description of the same
/// finding.
fn render(report: &CheckReport, outcome: &MergeOutcome, trees: [&SourceTree; 3]) -> String {
    let text = |side: Side, range: &std::ops::Range<u32>| -> String {
        let tree = trees[match side {
            Side::Base => 0,
            Side::Ours => 1,
            Side::Theirs => 2,
        }];
        String::from_utf8_lossy(&tree.source()[range.start as usize..range.end as usize])
            .into_owned()
    };

    let mut out = String::new();
    let s = &report.stats;
    writeln!(
        out,
        "tree merge: {}",
        if outcome.is_clean() {
            "clean".to_owned()
        } else {
            format!("{} conflict region(s)", outcome.conflicts.len())
        }
    )
    .unwrap();
    writeln!(
        out,
        "references: {} (resolved in origin {}, unresolved in origin {}), \
         skipped conflict regions: {}",
        s.references, s.resolved_in_origin, s.unresolved_in_origin, s.conflict_regions
    )
    .unwrap();
    writeln!(out, "semantic conflicts: {}", report.conflicts.len()).unwrap();

    for c in &report.conflicts {
        writeln!(out).unwrap();
        writeln!(out, "  [{}] `{}`", c.kind.tag(), c.name).unwrap();
        writeln!(
            out,
            "  reference: {} {:?}",
            c.reference.side,
            text(c.reference.side, &c.reference.range)
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

    if report.conflicts.is_empty() {
        writeln!(
            out,
            "\nNothing to report. Remember what this can see: one file, no \
             inheritance, no types (sm-bind's crate docs)."
        )
        .unwrap();
    }
    out
}
