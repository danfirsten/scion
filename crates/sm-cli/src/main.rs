//! `sm` — the `semantic-merge` command-line front end (SPEC.md §4.7).
//!
//! In M0 the only subcommand is `sm parse`, which exists to make the CST layer
//! inspectable. The merge driver (`sm merge %O %A %B %L %P`), `sm diff` and
//! `sm match` land in M4, M3 and M2 respectively; none of them are registered
//! here yet, because a subcommand that exists and does nothing is worse than one
//! that does not exist.

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
                  Status: M0 — scaffold and CST layer. Only `sm parse` is implemented; \
                  the merge driver lands in M4."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Parse a file and print its concrete syntax tree.
    Parse(ParseArgs),
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
        Command::Parse(args) => run_parse(&args),
    }
}

fn run_parse(args: &ParseArgs) -> ExitCode {
    let path: &Path = &args.file;

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
        return ExitCode::from(EXIT_USAGE);
    };

    let source = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("sm: {}: {err}", path.display());
            return ExitCode::from(EXIT_USAGE);
        }
    };

    let started = Instant::now();
    let tree = match sm_cst::parse(&source, lang) {
        Ok(tree) => tree,
        Err(err) => {
            eprintln!("sm: {}: {err}", path.display());
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let elapsed = started.elapsed();

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
                    parse_time: Some(elapsed),
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
