//! `sm diff <a> <b>` — the structural diff viewer (SPEC.md §4.7, milestone M3).
//!
//! The rendering itself lives in [`sm_diff::render`] so that it is
//! snapshot-testable without a subprocess; this module is argument parsing,
//! file loading, colour policy and exit codes. Deliberately shaped like
//! [`crate::cmd_match`]: the same `--profile`, `--json` and `--color` flags,
//! with the same meanings, because two subcommands over the same pair of files
//! that disagree about their own flags are a small betrayal every time.

use std::io::IsTerminal as _;
use std::path::PathBuf;
use std::process::ExitCode;

use sm_diff::{DiffReport, DiffView, RenderOptions, Side, render, render_stat};
use sm_match::{TreeMetrics, match_trees_with_metrics};

use crate::cmd_match::{ColorChoice, Profile};
use crate::{EXIT_USAGE, load, write_all};

/// Exit code for `--exit-code` when the two files differ structurally.
const EXIT_DIFFERENT: u8 = 1;

#[derive(clap::Args, Debug)]
pub struct DiffArgs {
    /// The source file — "a", the left-hand side.
    a: PathBuf,
    /// The destination file — "b", the right-hand side.
    b: PathBuf,

    /// Matcher constant profile (see `MatchConfig` in `sm-match`).
    #[arg(long, value_enum, default_value_t = Profile::Base)]
    profile: Profile,

    /// Emit the edit script as JSON instead of the rendered diff.
    #[arg(long)]
    json: bool,

    /// Print only the one-line summary of operation counts.
    #[arg(long, conflicts_with = "json")]
    stat: bool,

    /// When to colourise the rendered diff.
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto)]
    color: ColorChoice,

    /// Maximum source lines shown per hunk before the rest is elided.
    #[arg(long, default_value_t = 40, value_name = "N")]
    max_lines: usize,

    /// Show every move, including unchanged one- and two-node subtrees that are
    /// normally folded into a single summary line.
    #[arg(long)]
    all_moves: bool,

    /// Exit 1 when the two files differ structurally, like `git diff
    /// --exit-code`. Off by default so that a plain `sm diff` in a pipeline
    /// does not read as a failure.
    #[arg(long)]
    exit_code: bool,
}

pub fn run(args: &DiffArgs) -> ExitCode {
    let Some(a) = load(&args.a) else {
        return ExitCode::from(EXIT_USAGE);
    };
    let Some(b) = load(&args.b) else {
        return ExitCode::from(EXIT_USAGE);
    };
    let (lang_a, tree_a) = (a.lang, a.tree);
    let (lang_b, tree_b) = (b.lang, b.tree);
    if lang_a.name() != lang_b.name() {
        eprintln!(
            "sm: cannot diff {} ({}) against {} ({}): different languages",
            args.a.display(),
            lang_a.name(),
            args.b.display(),
            lang_b.name()
        );
        return ExitCode::from(EXIT_USAGE);
    }

    let cfg = args.profile.config();
    let metrics_a = TreeMetrics::compute(&tree_a, lang_a);
    let metrics_b = TreeMetrics::compute(&tree_b, lang_b);
    let matching = match_trees_with_metrics(&tree_a, &metrics_a, &tree_b, &metrics_b, lang_a, &cfg);
    let script =
        sm_diff::derive_with_metrics(&tree_a, &metrics_a, &tree_b, &metrics_b, &matching, lang_a);

    let label_a = args.a.display().to_string();
    let label_b = args.b.display().to_string();
    let view = DiffView {
        src: Side::new(&tree_a, &metrics_a, &label_a),
        dst: Side::new(&tree_b, &metrics_b, &label_b),
        matching: &matching,
        lang: lang_a,
        script: &script,
    };

    let rendered = if args.json {
        match serde_json::to_string_pretty(&DiffReport::new(&view)) {
            Ok(mut json) => {
                json.push('\n');
                json
            }
            Err(err) => {
                eprintln!("sm: could not serialize the edit script: {err}");
                return ExitCode::from(EXIT_USAGE);
            }
        }
    } else if args.stat {
        format!("{}\n", render_stat(&script.summary))
    } else {
        let color = match args.color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => std::io::stdout().is_terminal(),
        };
        render(
            &view,
            &RenderOptions {
                color,
                max_hunk_lines: args.max_lines,
                fold_moves_below: if args.all_moves { 0 } else { 3 },
                ..RenderOptions::default()
            },
        )
    };

    for (path, tree) in [(&args.a, &tree_a), (&args.b, &tree_b)] {
        if tree.has_errors() {
            eprintln!(
                "sm: {}: parsed with ERROR/MISSING nodes; the diff may be poor",
                path.display()
            );
        }
    }

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    if let Err(code) = write_all(&mut out, rendered.as_bytes()) {
        return code;
    }

    if args.exit_code && !script.is_empty() {
        return ExitCode::from(EXIT_DIFFERENT);
    }
    ExitCode::SUCCESS
}
