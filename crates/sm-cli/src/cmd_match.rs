//! `sm match <a> <b>` — the matching visualiser SPEC.md §4.3 mandates.
//!
//! *"Build the eyeball tool — matching bugs are almost invisible in assertions
//! and obvious visually."* The rendering itself lives in
//! [`sm_match::visualize`] so that it is snapshot-testable without a
//! subprocess; this module is argument parsing, file loading and exit codes.

use std::io::IsTerminal as _;
use std::path::PathBuf;
use std::process::ExitCode;

use sm_match::visualize::{
    MatchReport, Side, VisualizeOptions, render_side_by_side, render_summary,
};
use sm_match::{MatchConfig, TreeMetrics, match_trees_with_metrics};

use crate::{EXIT_USAGE, load, write_all};

/// Which of the two tuned constant profiles to use.
///
/// The names are deliberately the *roles*, not the numbers: a caller should
/// pick by what they are matching, and let M5's sweep move the constants
/// underneath them.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Profile {
    /// Permissive — what a base↔ours or base↔theirs matching uses.
    Base,
    /// Strict — what an ours↔theirs matching uses.
    Strict,
}

impl Profile {
    pub fn config(self) -> MatchConfig {
        match self {
            Self::Base => MatchConfig::base_to_side(),
            Self::Strict => MatchConfig::ours_to_theirs(),
        }
    }
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(clap::Args, Debug)]
pub struct MatchArgs {
    /// The source file — "a", the left column.
    a: PathBuf,
    /// The destination file — "b", the right column.
    b: PathBuf,

    /// Constant profile (see `MatchConfig` in `sm-match`).
    #[arg(long, value_enum, default_value_t = Profile::Base)]
    profile: Profile,

    /// Emit the pair list as JSON instead of the side-by-side view.
    #[arg(long)]
    json: bool,

    /// Print only the summary footer.
    #[arg(long, conflicts_with = "json")]
    summary_only: bool,

    /// When to colourise the side-by-side view.
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto)]
    color: ColorChoice,

    /// Total width of the two columns plus the separator.
    #[arg(long, default_value_t = 160, value_name = "N")]
    width: usize,

    /// Maximum characters of node text to show per node.
    #[arg(long, default_value_t = 24, value_name = "N")]
    max_text: usize,
}

pub fn run(args: &MatchArgs) -> ExitCode {
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
            "sm: cannot match {} ({}) against {} ({}): different languages",
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

    let label_a = args.a.display().to_string();
    let label_b = args.b.display().to_string();
    let side_a = Side::new(&tree_a, &metrics_a, &label_a);
    let side_b = Side::new(&tree_b, &metrics_b, &label_b);

    let rendered = if args.json {
        let report = MatchReport::new(side_a, side_b, &matching, &cfg);
        match serde_json::to_string_pretty(&report) {
            Ok(mut json) => {
                json.push('\n');
                json
            }
            Err(err) => {
                eprintln!("sm: could not serialize the matching: {err}");
                return ExitCode::from(EXIT_USAGE);
            }
        }
    } else {
        let color = match args.color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => std::io::stdout().is_terminal(),
        };
        let mut out = String::new();
        if !args.summary_only {
            out.push_str(&render_side_by_side(
                side_a,
                side_b,
                lang_a,
                &matching,
                &VisualizeOptions {
                    width: args.width,
                    color,
                    max_text_len: args.max_text,
                },
            ));
        }
        let summary = sm_match::visualize::summarize(side_a, side_b, &matching);
        out.push_str(&render_summary(&summary, &cfg, color));
        out
    };

    for (path, tree) in [(&args.a, &tree_a), (&args.b, &tree_b)] {
        if tree.has_errors() {
            eprintln!(
                "sm: {}: parsed with ERROR/MISSING nodes; the matching may be poor",
                path.display()
            );
        }
    }

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    if let Err(code) = write_all(&mut out, rendered.as_bytes()) {
        return code;
    }
    ExitCode::SUCCESS
}
