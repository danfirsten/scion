//! `sm` — the `semantic-merge` command-line front end (SPEC.md §4.7).
//!
//! Six subcommands. `sm merge` is the one git runs and the only one that
//! writes to a file the user cares about; its contract, its exit codes and its
//! fallback ladder are documented in the `merge` module, which is the place to
//! start. `sm install-driver` registers it. The other four are inspection
//! tools: `sm parse` for the CST layer (M0), `sm match` for a matching (M2) —
//! which SPEC.md §4.3 requires because "matching bugs are almost invisible in
//! assertions and obvious visually" — `sm diff` for the edit script derived
//! from it (M3), and `sm check` for M6's semantic check over three revisions.

mod cmd_check;
mod cmd_diff;
mod cmd_install;
mod cmd_match;
mod merge;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand};

/// Exit code for a file we cannot read or a language we do not support.
const EXIT_USAGE: u8 = 2;

#[derive(Parser, Debug)]
#[command(
    name = "sm",
    version,
    about = "semantic-merge: an AST-aware three-way merge driver for git",
    long_about = "semantic-merge parses source with tree-sitter and merges on the tree \
                  rather than on lines.\n\n\
                  `sm install-driver` registers `sm merge` with git; git then calls it \
                  as `sm merge %O %A %B %L %P %S %X %Y`. It exits 0 when the file merged \
                  cleanly, 1 when conflict markers remain, and 2 only when it failed \
                  without touching the file. Anything it cannot merge confidently — an \
                  unparseable input, an oversized file, a timeout, an internal error — \
                  falls back to `git merge-file`.\n\n\
                  `sm parse`, `sm match` and `sm diff` are inspection tools for the \
                  layers underneath."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Three-way merge one file. This is what git runs.
    ///
    /// Positional arguments are git's `%O %A %B %L %P %S %X %Y`, in that
    /// order. The result is written to `%A`.
    Merge(Box<merge::MergeArgs>),
    /// Register `sm merge` as a git merge driver.
    InstallDriver(cmd_install::InstallArgs),
    /// Parse a file and print its concrete syntax tree.
    Parse(ParseArgs),
    /// Match two files structurally and show the result side by side.
    Match(cmd_match::MatchArgs),
    /// Diff two files structurally, reporting moves as moves.
    Diff(cmd_diff::DiffArgs),
    /// Merge three revisions of a file and report the names the merge broke.
    ///
    /// The same check `sm merge --semantic` runs, on files you name, with no
    /// git repository involved. Exits 1 when it finds something.
    Check(cmd_check::CheckArgs),
}

#[derive(clap::Args, Debug)]
struct ParseArgs {
    /// Source file to parse.
    file: PathBuf,

    /// Omit comments from the output.
    ///
    /// A display option only: the tree itself always retains every byte.
    #[arg(long)]
    no_trivia: bool,

    /// Emit the arena as JSON instead of an indented tree.
    #[arg(long)]
    json: bool,

    /// Maximum characters of source text to show per node.
    #[arg(long, default_value_t = 40, value_name = "N")]
    max_text: usize,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Merge(args) => merge::run(&args),
        Command::InstallDriver(args) => cmd_install::run(&args),
        Command::Parse(args) => run_parse(&args),
        Command::Match(args) => cmd_match::run(&args),
        Command::Diff(args) => cmd_diff::run(&args),
        Command::Check(args) => cmd_check::run(&args),
    }
}

/// Detect the language for `path`, read it and parse it.
///
/// Returns `None` after printing the reason; every caller turns that into
/// [`EXIT_USAGE`]. A file that *parses* but contains `ERROR` nodes is a
/// success here — the tree is still a faithful description of the bytes
/// (PROGRESS.md decision 8) — so callers that care must ask
/// [`sm_cst::SourceTree::has_errors`].
fn load(path: &Path) -> Option<Loaded> {
    let Some(lang) = sm_cst::languages::detect(path) else {
        let known: Vec<&str> = sm_cst::languages::all()
            .iter()
            .flat_map(|l| l.file_extensions().iter().copied())
            .collect();
        eprintln!(
            "sm: {}: unsupported file type (known extensions: {})",
            path.display(),
            known.join(", ")
        );
        return None;
    };

    let source = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("sm: {}: {err}", path.display());
            return None;
        }
    };

    let started = Instant::now();
    match sm_cst::parse(&source, lang) {
        Ok(tree) => Some(Loaded {
            lang,
            tree,
            parse_time: started.elapsed(),
        }),
        Err(err) => {
            eprintln!("sm: {}: {err}", path.display());
            None
        }
    }
}

/// A successfully parsed file. `parse_time` measures the parse alone, not the
/// read, because that is what the `sm parse` header claims to report.
struct Loaded {
    lang: &'static dyn sm_cst::Language,
    tree: sm_cst::SourceTree,
    parse_time: std::time::Duration,
}

fn run_parse(args: &ParseArgs) -> ExitCode {
    let path: &Path = &args.file;

    let Some(Loaded {
        lang,
        tree,
        parse_time,
    }) = load(path)
    else {
        return ExitCode::from(EXIT_USAGE);
    };

    let display_path = path.display().to_string();
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    let rendered = if args.json {
        // The warning belongs on stderr here so that stdout stays valid JSON.
        if tree.has_errors() {
            eprintln!("sm: {}", sm_cst::ERROR_WARNING);
        }
        match serde_json::to_string_pretty(&sm_cst::JsonTree::new(&tree)) {
            Ok(mut json) => {
                json.push('\n');
                json
            }
            Err(err) => {
                eprintln!("sm: {}: could not serialize tree: {err}", path.display());
                return ExitCode::from(EXIT_USAGE);
            }
        }
    } else {
        sm_cst::render_parse(
            &tree,
            lang,
            &sm_cst::RenderOptions {
                show_trivia: !args.no_trivia,
                max_text_len: args.max_text,
                header: Some(sm_cst::HeaderInfo {
                    path: &display_path,
                    parse_time: Some(parse_time),
                }),
            },
        )
    };

    if let Err(code) = write_all(&mut out, rendered.as_bytes()) {
        return code;
    }

    // Exit 0 even when the tree has errors: the parse succeeded and the tree is
    // a faithful description of the file. Only an unreadable file or an unknown
    // language is a usage failure.
    ExitCode::SUCCESS
}

/// Write everything out, treating a closed downstream pipe as success.
///
/// `sm parse Big.java | head` is the normal way to look at a large tree, and
/// Rust ignores `SIGPIPE`, so without this the common case ends in an error
/// message and a non-zero exit.
fn write_all(out: &mut impl std::io::Write, bytes: &[u8]) -> Result<(), ExitCode> {
    let result = out.write_all(bytes).and_then(|()| out.flush());
    match result {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(err) => {
            eprintln!("sm: write failed: {err}");
            Err(ExitCode::from(EXIT_USAGE))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::CommandFactory as _;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
