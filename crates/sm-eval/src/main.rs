//! `sm-eval` — the corpus miner and, later, the replay harness.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use sm_eval::mine::{MineOptions, mine_all, parse_repos};
use sm_eval::sample::{SampleOptions, build_sample};
use sm_eval::stats;

#[derive(Parser, Debug)]
#[command(
    name = "sm-eval",
    about = "Mine three-way merge cases from real Java history (semantic-merge, SPEC §6.1)",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Clone each repository, replay its merges and extract cases.
    Mine {
        /// List of `<clone-url> [<spdx-license>]` lines; `#` starts a comment.
        #[arg(long, value_name = "FILE")]
        repos: PathBuf,
        /// Corpus directory to write (created if missing).
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
        /// Stop after this many conflicted cases per repository. 0 = unlimited.
        #[arg(long, default_value_t = 0, value_name = "N")]
        max_cases_per_repo: u64,
        /// Clean cases to sample per repository. 0 disables clean mining.
        #[arg(long, default_value_t = 100, value_name = "N")]
        clean_sample: u64,
        /// Walk at most this many merge commits per repository, newest first.
        /// 0 = unlimited.
        #[arg(long, default_value_t = 0, value_name = "N")]
        max_merges_per_repo: u64,
        /// Skip any version of a file larger than this.
        #[arg(long, default_value_t = 1024 * 1024, value_name = "BYTES")]
        max_file_bytes: u64,
        /// Per-repository clone budget in seconds.
        #[arg(long, default_value_t = 900, value_name = "SECS")]
        clone_timeout: u64,
        /// Keep the bare clones instead of deleting each one after use.
        #[arg(long)]
        keep_clones: bool,
        /// Scratch directory for the clones.
        #[arg(long, value_name = "DIR")]
        scratch: Option<PathBuf>,
        /// Skip repositories already recorded as `ok` in the manifest.
        #[arg(long)]
        resume: bool,
        /// Print nothing but the final summary.
        #[arg(long)]
        quiet: bool,
    },

    /// Print the corpus summary table.
    Stats {
        #[arg(long, value_name = "DIR")]
        corpus: PathBuf,
        /// Write the markdown to a file as well as stdout.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
    },

    /// Copy a small, permissively licensed slice of the corpus somewhere it can
    /// be committed as test fixtures.
    Sample {
        #[arg(long, value_name = "DIR")]
        corpus: PathBuf,
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
        #[arg(long, default_value_t = 15, value_name = "N")]
        count: usize,
        #[arg(long, default_value_t = 2, value_name = "N")]
        max_per_repo: usize,
        #[arg(long, default_value_t = 24 * 1024, value_name = "BYTES")]
        max_version_bytes: u64,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sm-eval: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Mine {
            repos,
            out,
            max_cases_per_repo,
            clean_sample,
            max_merges_per_repo,
            max_file_bytes,
            clone_timeout,
            keep_clones,
            scratch,
            resume,
            quiet,
        } => {
            let text = std::fs::read_to_string(&repos)
                .with_context(|| format!("reading {}", repos.display()))?;
            let specs = parse_repos(&text);
            if specs.is_empty() {
                anyhow::bail!("{} lists no repositories", repos.display());
            }
            let scratch = scratch.unwrap_or_else(|| std::env::temp_dir().join("sm-eval-mining"));
            let opts = MineOptions {
                out: out.clone(),
                scratch,
                max_cases_per_repo,
                clean_sample,
                max_merges_per_repo,
                max_file_bytes,
                clone_timeout: Duration::from_secs(clone_timeout),
                keep_clones,
                resume,
                verbose: !quiet,
            };
            let manifest = mine_all(&specs, &opts)?;
            eprintln!(
                "mined {} repositories into {}",
                manifest.repos.len(),
                out.display()
            );
            let s = stats::scan(&out)?;
            print!("{}", stats::render_markdown(&out, &s)?);
            Ok(())
        }

        Command::Stats { corpus, out } => {
            let s = stats::scan(&corpus)?;
            let md = stats::render_markdown(&corpus, &s)?;
            if let Some(path) = out {
                std::fs::write(&path, &md)
                    .with_context(|| format!("writing {}", path.display()))?;
            }
            print!("{md}");
            Ok(())
        }

        Command::Sample {
            corpus,
            out,
            count,
            max_per_repo,
            max_version_bytes,
        } => {
            let opts = SampleOptions {
                count,
                max_per_repo,
                max_version_bytes,
            };
            let chosen = build_sample(&corpus, &out, &opts)?;
            println!("wrote {} cases to {}", chosen.len(), out.display());
            for c in &chosen {
                println!("  {}  {}  {}  {}", c.case_id, c.repo, c.license, c.path);
            }
            Ok(())
        }
    }
}
