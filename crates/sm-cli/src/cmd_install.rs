//! `sm install-driver` — registering the driver with git.
//!
//! Two halves, because git needs two unrelated things and only one of them can
//! be automated safely:
//!
//! 1. **`.git/config` or `~/.gitconfig`** learns *how to run* the driver. This
//!    is written for you, through `git config` — never by editing the file. A
//!    config file is INI-ish with rules about section merging, include
//!    directives and conditional includes that a hand-written appender gets
//!    wrong the first time someone has a `[includeIf]` in theirs.
//! 2. **`.gitattributes`** decides *which files* it applies to. This is a
//!    tracked, shared, reviewable file and its contents are a project decision,
//!    so the default is to print the lines and let you commit them.
//!    `--write-attributes <path>` appends them for you when you would rather
//!    not type them.
//!
//! # `recursive =`
//!
//! `merge.<driver>.recursive` names the low-level driver git uses when the
//! recursive strategy merges **the merge bases themselves** — the criss-cross
//! case, where there are several merge bases and git merges them into one
//! virtual ancestor before doing the real merge.
//!
//! SPEC.md §4.7's sketch says `binary`. docs/prior-art.md §2.9 quotes
//! Mergiraf's config, which does not set the key at all. Neither settles it, so
//! it is decided here, with the reasoning written down (SPEC.md §0.7):
//!
//! - **`text` (chosen).** The virtual ancestor is merged with git's built-in
//!   line merge. If that conflicts, the ancestor contains conflict markers,
//!   which do not parse, which sends *our* driver down fallback rung 4 for the
//!   real merge. The bad case degrades into the safe case automatically.
//! - **`binary` (rejected).** Tells git the inner merge is impossible, so it
//!   takes one base as the ancestor and discards the other. That silently
//!   throws away half the ancestry information in exactly the situation where
//!   ancestry is hardest, and a three-way merge against a wrong ancestor is how
//!   you get a confidently wrong result.
//! - **`semantic` (rejected as a default, available via `--recursive`).**
//!   Recursing into ourselves for the virtual ancestor is defensible and may
//!   turn out to be better, but the merged-base merge is a case with no test
//!   coverage and no corpus evidence yet, and it multiplies driver invocations
//!   on the merges that are already the slowest. M5 can revisit it with data.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::{Args, ValueEnum};

/// The driver line git will run. `%A` is both an input and the destination.
pub const DRIVER_ARGS: &str = "merge %O %A %B %L %P %S %X %Y";

/// The `[merge "…"]` section name.
pub const DRIVER_NAME: &str = "semantic";

const EXIT_USAGE: u8 = 2;

#[derive(Copy, Clone, PartialEq, Eq, Debug, ValueEnum)]
pub enum Recursive {
    /// Merge virtual ancestors with git's line merge. The default; see the
    /// module docs.
    Text,
    /// Treat virtual ancestors as unmergeable.
    Binary,
    /// Merge virtual ancestors with this driver too.
    Semantic,
}

impl Recursive {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Binary => "binary",
            Self::Semantic => DRIVER_NAME,
        }
    }
}

#[derive(Args, Debug)]
pub struct InstallArgs {
    /// Write to this repository's `.git/config`. The default.
    #[arg(long, conflicts_with = "global")]
    local: bool,

    /// Write to the user's `~/.gitconfig` instead.
    #[arg(long)]
    global: bool,

    /// File types to print `.gitattributes` lines for.
    #[arg(
        long,
        value_name = "EXTS",
        value_delimiter = ',',
        default_value = "java,ts,tsx"
    )]
    langs: Vec<String>,

    /// Append the `.gitattributes` lines to this file instead of printing them.
    #[arg(long, value_name = "PATH")]
    write_attributes: Option<PathBuf>,

    /// The command git should run. Defaults to this binary's absolute path.
    #[arg(long, value_name = "PATH")]
    driver_path: Option<String>,

    /// What to use for merging virtual ancestors. See the module docs.
    #[arg(long, value_enum, default_value_t = Recursive::Text)]
    recursive: Recursive,

    /// Remove the driver's config section instead of adding it.
    #[arg(long, conflicts_with_all = ["langs", "write_attributes", "driver_path"])]
    uninstall: bool,

    /// Print what would be run and change nothing.
    #[arg(long)]
    dry_run: bool,
}

pub fn run(args: &InstallArgs) -> ExitCode {
    let scope = if args.global { "--global" } else { "--local" };

    if args.uninstall {
        return uninstall(scope, args.dry_run);
    }

    // An absolute path by default. `driver = sm merge …` needs `sm` on the
    // PATH of every process that runs a merge, which is not the same set as the
    // shell the user typed this into — notably not a GUI client, a hook, or a
    // rebase run from an editor.
    let driver_path = args.driver_path.clone().unwrap_or_else(default_driver_path);
    let driver = format!("{} {DRIVER_ARGS}", shell_quote(&driver_path));

    let settings = [
        (
            format!("merge.{DRIVER_NAME}.name"),
            "semantic-merge: AST-aware three-way merge".to_owned(),
        ),
        (format!("merge.{DRIVER_NAME}.driver"), driver.clone()),
        (
            format!("merge.{DRIVER_NAME}.recursive"),
            args.recursive.as_str().to_owned(),
        ),
    ];

    for (key, value) in &settings {
        if args.dry_run {
            println!("git config {scope} {key} {}", shell_quote(value));
            continue;
        }
        if let Err(code) = git_config(&["config", scope, key, value]) {
            return code;
        }
    }

    let attribute_lines = attribute_lines(&args.langs);

    if !args.dry_run {
        println!("Installed merge driver \"{DRIVER_NAME}\" into git config ({scope}).");
        println!("  driver    = {driver}");
        println!("  recursive = {}", args.recursive.as_str());
        println!();
    }

    match &args.write_attributes {
        Some(path) if !args.dry_run => match append_attributes(path, &attribute_lines) {
            Ok(added) if added.is_empty() => {
                println!("{}: already had every line.", path.display());
            }
            Ok(added) => {
                println!("Appended to {}:", path.display());
                for line in added {
                    println!("  {line}");
                }
            }
            Err(err) => {
                eprintln!("sm: {}: {err}", path.display());
                return ExitCode::from(EXIT_USAGE);
            }
        },
        Some(path) => {
            println!("Would append to {}:", path.display());
            for line in &attribute_lines {
                println!("  {line}");
            }
        }
        None => {
            println!("Now add these lines to .gitattributes and commit them:");
            println!();
            for line in &attribute_lines {
                println!("    {line}");
            }
            println!();
            println!("(or re-run with --write-attributes .gitattributes)");
        }
    }

    if !args.dry_run {
        println!();
        println!("To remove it again: sm install-driver {scope} --uninstall");
    }
    ExitCode::SUCCESS
}

fn uninstall(scope: &str, dry_run: bool) -> ExitCode {
    let args = [
        "config",
        scope,
        "--remove-section",
        &format!("merge.{DRIVER_NAME}"),
    ];
    if dry_run {
        println!("git {}", args.join(" "));
        return ExitCode::SUCCESS;
    }
    match Command::new("git").args(args).output() {
        Ok(output) if output.status.success() => {
            println!("Removed the \"{DRIVER_NAME}\" merge driver from git config ({scope}).");
            println!("Remember to remove the `merge={DRIVER_NAME}` lines from .gitattributes.");
            ExitCode::SUCCESS
        }
        Ok(output) => {
            // git exits non-zero when the section was not there, which is not
            // an error worth failing on for an uninstall.
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!(
                "No \"{DRIVER_NAME}\" section to remove ({scope}){}",
                if stderr.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", stderr.trim())
                }
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("sm: could not run git: {err}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

fn git_config(args: &[&str]) -> Result<(), ExitCode> {
    match Command::new("git").args(args).output() {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            eprintln!(
                "sm: `git {}` failed ({}): {}",
                args.join(" "),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
            Err(ExitCode::from(EXIT_USAGE))
        }
        Err(err) => {
            eprintln!("sm: could not run git: {err}");
            Err(ExitCode::from(EXIT_USAGE))
        }
    }
}

fn default_driver_path() -> String {
    std::env::current_exe().map_or_else(|_| "sm".to_owned(), |p| p.to_string_lossy().into_owned())
}

/// One `.gitattributes` line per extension, deduplicated, in the order given.
#[must_use]
pub fn attribute_lines(langs: &[String]) -> Vec<String> {
    let mut seen = Vec::new();
    for lang in langs {
        let ext = lang.trim().trim_start_matches('.');
        if ext.is_empty() {
            continue;
        }
        let line = format!("*.{ext} merge={DRIVER_NAME}");
        if !seen.contains(&line) {
            seen.push(line);
        }
    }
    seen
}

/// Append the lines that are not already present, returning the ones added.
///
/// Reads first so that running the command twice is a no-op — a user who
/// re-installs after an upgrade should not end up with the rules duplicated.
fn append_attributes(path: &Path, lines: &[String]) -> std::io::Result<Vec<String>> {
    use std::io::Write as _;

    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    let present: Vec<&str> = existing.lines().map(str::trim).collect();
    let missing: Vec<String> = lines
        .iter()
        .filter(|l| !present.contains(&l.as_str()))
        .cloned()
        .collect();
    if missing.is_empty() {
        return Ok(missing);
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    if !existing.is_empty() && !existing.ends_with('\n') {
        file.write_all(b"\n")?;
    }
    for line in &missing {
        writeln!(file, "{line}")?;
    }
    file.flush()?;
    Ok(missing)
}

/// Quote a path for a git config value, which git splits like a shell.
fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./+=:@".contains(&b))
    {
        return value.to_owned();
    }
    format!("\"{}\"", value.replace('\\', r"\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::{DRIVER_ARGS, DRIVER_NAME, attribute_lines, shell_quote};

    #[test]
    fn the_driver_line_matches_the_positional_arguments_sm_merge_declares() {
        // If these ever drift, git silently passes the wrong file as the
        // destination. That is the failure SPEC.md §4.7 warns about, so it gets
        // an assertion rather than a comment.
        assert_eq!(DRIVER_ARGS, "merge %O %A %B %L %P %S %X %Y");
    }

    #[test]
    fn attribute_lines_are_deduplicated_and_normalised() {
        let langs = ["java".to_owned(), ".ts".to_owned(), " java ".to_owned()];
        assert_eq!(
            attribute_lines(&langs),
            vec![
                format!("*.java merge={DRIVER_NAME}"),
                format!("*.ts merge={DRIVER_NAME}"),
            ]
        );
    }

    #[test]
    fn a_path_with_spaces_is_quoted() {
        assert_eq!(shell_quote("/usr/local/bin/sm"), "/usr/local/bin/sm");
        assert_eq!(
            shell_quote("/home/a b/target/release/sm"),
            "\"/home/a b/target/release/sm\""
        );
        assert_eq!(shell_quote(r#"/x/"q"/sm"#), r#""/x/\"q\"/sm""#);
    }
}
