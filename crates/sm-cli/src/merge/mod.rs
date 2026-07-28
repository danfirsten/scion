//! `sm merge` — the git merge driver (SPEC.md §4.7, milestone M4).
//!
//! This module documentation **is** the driver's contract. Everything here is
//! load-bearing for data safety; SPEC.md §4.7's warning is the frame:
//! *"writing the result to the wrong path silently destroys work"* and *"a
//! panic degrades to line merge rather than data loss."*
//!
//! # 1. The argv, exactly
//!
//! ```text
//! sm merge %O %A %B %L %P %S %X %Y
//! ```
//!
//! ```text
//! %O  base      path to the merge base's content        (read only)
//! %A  ours      path to our content, AND THE DESTINATION (read, then replaced)
//! %B  theirs    path to their content                   (read only)
//! %L  marker size, an integer                           (optional)
//! %P  the real pathname in the worktree                 (optional)
//! %S  base label for conflict markers                   (optional)
//! %X  ours label                                        (optional)
//! %Y  theirs label                                      (optional)
//! ```
//!
//! `%A` is the destination. It is the *only* file this command writes, unless
//! `--output` is given, and it is written through [`atomic::replace`]. Nothing
//! in this module opens it for writing directly.
//!
//! `%P` — not `%A` — is what selects the language. Git hands the driver
//! temporaries named `.merge_file_kJ3nQx`, with no extension and no relation to
//! the file being merged, so detecting from `%A` would classify every file as
//! unknown. If `%P` is absent the language is unknown and the driver goes
//! straight to the fallback, which is correct rather than clever.
//!
//! ## Old git
//!
//! `%S`, `%X` and `%Y` arrived in git 2.44. An older git does not expand them
//! and passes the **literal strings** `"%S"`, `"%X"`, `"%Y"` through
//! (docs/prior-art.md §2.9). Each is compared against its own literal, exactly,
//! and a match means "git did not give me a label" — not "the user's branch is
//! called `%X`". `%L` gets the same treatment. Without this the driver writes
//! `>>>>>>> %Y` into people's files. The machine this was developed on runs git
//! 2.43, so the dogfood test exercises this path for real rather than by
//! simulation.
//!
//! # 2. Exit codes
//!
//! | code | meaning | state of `%A` |
//! |---|---|---|
//! | 0 | merged cleanly | the merged result |
//! | 1 | conflicts remain | the merged result, with conflict markers |
//! | 1 | `--semantic=conflict` found a name-binding conflict | the merged result, **with no markers** — see the [`semantic`] module |
//! | ≥2 | the driver failed | **untouched** — still valid "ours" for git |
//!
//! Exit 2 is the one that has to be right. Git treats any non-zero exit as
//! "conflicts remain" and keeps whatever is in `%A`, so leaving `%A` as our
//! original content on a hard error degrades to the same outcome as a merge
//! driver that was never installed.
//!
//! # 3. The fast path (docs/prior-art.md §2.8 / §8.2.1)
//!
//! **The line merge runs first, always.**
//!
//! 1. Run `git merge-file -p` over the three inputs.
//! 2. If it is conflict-free, parse its output. If that parses without errors,
//!    write it and exit 0. **The tree machinery is never invoked.**
//! 3. Otherwise fall through to the semantic path.
//!
//! Two things follow, and they are the reason this is not merely an
//! optimisation:
//!
//! - **"Git merged it clean and we broke it" becomes structurally impossible**
//!   for the whole clean population, which is the overwhelming majority of
//!   invocations. Our regression rate against git on that population is zero by
//!   construction, not by testing.
//! - **p50 latency is one `fork`/`exec` plus one parse** — no matching, no
//!   merge, no emit.
//!
//! The parse check in step 2 is what stops the fast path from rubber-stamping a
//! clean-but-broken line merge (the "false clean merge" of SPEC.md §1). When it
//! fires, the semantic path runs and may well produce a *conflict* where git
//! produced clean output. That is deliberate: git's answer has been shown not
//! to be a program, so a conflict is the better of the two. Every occurrence is
//! recorded in `--debug-json` as a `fast path rejected` warning so M5 can count
//! them.
//!
//! **How often can it fire?** Rarely, and the argument is worth stating because
//! it bounds the fast path's risk. Git merges cleanly only when the two sides'
//! hunks are disjoint, so the output is the ancestor with two independent
//! regions replaced. Brace balance is additive over disjoint regions, so if
//! both inputs parse, each side's edit is brace-neutral and the merged file is
//! brace-balanced too — the whole "unbalanced braces" family is *impossible*
//! here. What remains is non-brace syntax errors, and `tree-sitter-java` is
//! permissive about those (an `import` after a class declaration, a bare
//! statement at file scope: both parse without an `ERROR` node). A brute-force
//! search over ~1400 pairs of single-line edits found no example. The check
//! stays because it is one parse of a file already in memory and because
//! grammars differ, but it is insurance, not a branch we expect to take.
//!
//! # 4. The fallback ladder
//!
//! Checked in this order. Any rung sends the driver to git's line merge — the
//! same bytes the fast path already computed, so the fallback costs nothing
//! extra:
//!
//! 1. **An input cannot be read** → exit 2, `%A` untouched. (Not a fallback:
//!    without the inputs there is nothing to merge.)
//! 2. **`%P` has no known extension** → line merge.
//! 3. **An input exceeds `--max-bytes`** (default 5 MiB) → line merge. Applied
//!    to the largest of the three, before any parsing, because the budget
//!    exists to bound work we have not started yet.
//! 4. **Any of the three inputs has syntax errors** (`SourceTree::has_errors`)
//!    → line merge. This is the decision SPEC.md §4.7 mandates and it is the
//!    *driver's* to make: `sm_merge::merge` is total and will happily merge a
//!    broken tree, which is exactly what we do not want.
//! 5. **Timeout** (default 5000 ms; `0` disables) → line merge.
//! 6. **Panic** anywhere in parse, match, merge or emit → line merge.
//! 7. **An output self-check fails** → line merge. For a clean merge: zero
//!    conflict markers, no synthesized bytes other than token separators (see
//!    below), output reparses without errors, and **every token in the output
//!    is a token of one of the inputs**. For a conflicted merge: at least one
//!    marker. The token check is the one that earns its keep — see
//!    [`fabricated_token`], which explains the real bug it catches and why
//!    reparsing alone does not.
//! 8. **The line merge itself failed** → exit 2, `%A` untouched.
//!
//! # 5. Panic safety
//!
//! The semantic path runs inside [`std::panic::catch_unwind`]. `sm_merge::merge`
//! is documented as total and tested as such, but the wrapper covers everything
//! — the parser, the matcher, the emitter, and any future code — because the
//! cost is one catch per invocation and the alternative is a corrupted merge.
//!
//! A panic hook is installed for the duration of `sm merge` that prints **one
//! line** and no backtrace. A merge driver runs inside `git merge`, whose
//! output the user is reading; a multi-thousand-line backtrace in the middle of
//! it is its own kind of damage. The message is kept and reported in
//! `--debug-json`, which is where the detail belongs.
//!
//! # 6. The timeout, and why detaching the worker is sound
//!
//! Parse, match, merge and emit run on a worker thread; the main thread waits
//! on a channel with [`std::sync::mpsc::Receiver::recv_timeout`]. On timeout the
//! main thread takes the fallback and **does not join the worker** — it is
//! detached and dies when the process exits.
//!
//! That is sound, and specifically:
//!
//! - The worker owns everything it touches. It is handed its own `Vec<u8>` copy
//!   of each of the three inputs; the trees, the matchings, the merged tree and
//!   the output buffer are all allocated inside it. There is no shared mutable
//!   state to be left inconsistent.
//! - The worker performs **no I/O**. It cannot write `%A`, cannot create a
//!   temporary, and cannot touch the debug-JSON file. Only the main thread
//!   writes anything.
//! - Its result channel is a `sync_channel(1)`, so a late send neither blocks
//!   nor panics if the receiver is gone; it returns `Err` and is dropped.
//! - The process exits promptly afterwards — the main thread's remaining work
//!   is one atomic write and one small JSON file. A detached worker therefore
//!   lives for milliseconds, holding memory that the exit reclaims.
//!
//! The alternative — a cancellation flag polled inside the merge — would put a
//! check in every inner loop of code whose correctness is the point of the
//! project, to save a few megabytes for a few milliseconds. Not worth it.
//!
//! # 7. The semantic check
//!
//! `--semantic=off|report|conflict`, default `report`. It runs M6's
//! name-binding check ([`sm_bind`]) over the candidate merge, and only when the
//! semantic path produced a **clean** result. What each mode does, why the fast
//! path skips it, and why `conflict` exits 1 without writing markers, are all in
//! the [`semantic`] module's documentation — the exit-code decision in
//! particular is a UX choice with a real tradeoff and it is argued there rather
//! than summarised here.
//!
//! # 8. What this module deliberately does not do
//!
//! - **No per-subtree line fallback.** docs/prior-art.md §2.3's
//!   `LineBasedMerge` node type would let one bad method degrade while the rest
//!   of the file merges structurally. SPEC.md §4.7 asks for a whole-file
//!   fallback and that is what this is.
//! - **No matcher seeding from the conflicted line merge** (docs/prior-art.md
//!   §2.8 step 2). We have the three real revisions, so we do not need to
//!   reconstruct fictional ones; the seeding is a speed optimisation for a case
//!   we do not have.
//! - **No `mergiraf solve` equivalent.** Different feature, different milestone.

pub mod atomic;
pub mod linemerge;
pub mod record;
pub mod semantic;

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use clap::Args;
use sm_cst::{Language, SourceTree};
use sm_emit::{ConflictStyle, EmitOptions, EmitResult, emit};
use sm_merge::{MergeConfig, MergeOutcome};

use record::{
    FallbackReason, InputRecord, LabelRecord, LineMergeRecord, PathTaken, Record,
    SemanticCheckRecord, SemanticRecord, StatsRecord, ms,
};
use semantic::SemanticMode;

/// Merged cleanly.
pub const EXIT_CLEAN: u8 = 0;
/// Merged, conflicts remain in `%A`.
pub const EXIT_CONFLICTS: u8 = 1;
/// The driver failed and `%A` was left as valid input for git to handle.
pub const EXIT_ERROR: u8 = 2;

/// Default budget for the whole semantic path, in milliseconds.
///
/// SPEC.md §4.7 asks for "around 5s"; docs/prior-art.md §2.9 records Mergiraf
/// using 5000 ms for its fast mode and 10000 ms for the full structured merge.
/// Ours is a fast mode — the line merge has already run and the semantic path
/// only sees files git could not merge — so 5000 ms is the like-for-like
/// number.
const DEFAULT_TIMEOUT_MS: u64 = 5_000;

/// Default per-input size budget.
///
/// The largest file in the M1 corpus is under 400 KiB. 5 MiB is not a
/// performance tuning knob, it is a guard against a generated or vendored file
/// where the quadratic parts of the matcher would blow the timeout anyway; it
/// just fails faster and more predictably than the timeout would.
const DEFAULT_MAX_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Args, Debug)]
pub struct MergeArgs {
    /// `%O` — the merge base's content.
    #[arg(value_name = "BASE")]
    base: PathBuf,
    /// `%A` — our content, and the path the result is written to.
    #[arg(value_name = "OURS")]
    ours: PathBuf,
    /// `%B` — their content.
    #[arg(value_name = "THEIRS")]
    theirs: PathBuf,
    /// `%L` — conflict marker size.
    #[arg(value_name = "MARKER_SIZE")]
    marker_size: Option<String>,
    /// `%P` — the real pathname, which is what selects the language.
    #[arg(value_name = "PATHNAME")]
    pathname: Option<String>,
    /// `%S` — the conflict-marker label for the base.
    #[arg(value_name = "BASE_LABEL")]
    base_label: Option<String>,
    /// `%X` — the conflict-marker label for our side.
    #[arg(value_name = "OURS_LABEL")]
    ours_label: Option<String>,
    /// `%Y` — the conflict-marker label for their side.
    #[arg(value_name = "THEIRS_LABEL")]
    theirs_label: Option<String>,

    /// Budget for the semantic path, in milliseconds. 0 disables it.
    #[arg(long, value_name = "N", default_value_t = DEFAULT_TIMEOUT_MS)]
    timeout_ms: u64,

    /// Skip the semantic path for inputs larger than this many bytes.
    #[arg(long, value_name = "N", default_value_t = DEFAULT_MAX_BYTES)]
    max_bytes: u64,

    /// The command used for the line merge. Must accept `git merge-file`'s
    /// arguments.
    #[arg(long, value_name = "CMD", default_value = "git")]
    fallback_cmd: String,

    /// Write a machine-readable record of this invocation. See
    /// `merge::record`'s docs for the schema.
    #[arg(long, value_name = "PATH")]
    debug_json: Option<PathBuf>,

    /// Write the result here instead of to `%A`, which is then only read.
    ///
    /// The driver contract requires writing to `%A`; this is for inspecting
    /// what the driver would do without touching anything, which is how the
    /// corpus replay runs.
    #[arg(long, short = 'o', value_name = "PATH")]
    output: Option<PathBuf>,

    /// Never take the fast path: always run the semantic merge.
    ///
    /// The fast path hides the tree merge from anything git can already do, so
    /// measuring or testing the tree merge through the driver needs a way off
    /// it.
    #[arg(long)]
    no_fast_path: bool,

    /// Never run the semantic merge: behave as a pure `git merge-file` wrapper.
    ///
    /// The control condition for M5's evaluation.
    #[arg(long, conflicts_with = "no_fast_path")]
    line_merge_only: bool,

    /// Include the ancestor's text in conflict markers (git's `diff3` style).
    #[arg(long)]
    diff3: bool,

    /// What to do with M6's name-binding check (see the `merge::semantic`
    /// module docs).
    ///
    /// It runs only when the semantic path produced a **clean** merge, which is
    /// where the dangerous case lives: a file that already has conflict markers
    /// in it is going to be read anyway. `report` prints findings and leaves
    /// the exit code alone; `conflict` additionally exits 1, leaving the clean
    /// merged text in `%A` and the file unmerged for git.
    #[arg(
        long = "semantic",
        value_enum,
        default_value_t = SemanticMode::Report,
        value_name = "MODE"
    )]
    semantic_check: SemanticMode,

    /// Suppress the one-line summary on stderr.
    #[arg(long, short = 'q')]
    quiet: bool,
}

/// The last caught panic's message, for the debug record.
static PANIC_MESSAGE: Mutex<Option<String>> = Mutex::new(None);

pub fn run(args: &MergeArgs) -> ExitCode {
    let started = Instant::now();
    install_panic_hook();

    let mut rec = Record::new(args.pathname.as_ref().and_then(|p| {
        // The literal "%P" means git did not expand it.
        (p != "%P").then(|| p.clone())
    }));

    let code = drive(args, &mut rec);
    rec.exit_code = code;

    if let Some(path) = &args.debug_json {
        rec.write_to(path, started);
    }
    ExitCode::from(code)
}

// ---------------------------------------------------------------- the ladder

fn drive(args: &MergeArgs, rec: &mut Record) -> u8 {
    // ---- 1. read the three inputs. A failure here is a hard error: with no
    //         inputs there is nothing to merge and %A must stay as it is.
    let t = Instant::now();
    let inputs = match read_inputs(args) {
        Ok(inputs) => inputs,
        Err((path, err)) => {
            eprintln!("sm merge: {}: {err}", path.display());
            rec.fallback_reason = Some(FallbackReason::InputUnreadable);
            return EXIT_ERROR;
        }
    };
    rec.timings_ms.read = Some(ms(t.elapsed()));
    rec.inputs.base = InputRecord {
        bytes: inputs.base.len() as u64,
        parse_errors: None,
    };
    rec.inputs.ours = InputRecord {
        bytes: inputs.ours.len() as u64,
        parse_errors: None,
    };
    rec.inputs.theirs = InputRecord {
        bytes: inputs.theirs.len() as u64,
        parse_errors: None,
    };

    // ---- 2. resolve everything git told us about markers.
    let marker_size = resolve_marker_size(args.marker_size.as_deref());
    let labels = resolve_labels(args, rec);
    rec.inputs.marker_size = marker_size;
    rec.inputs.labels = LabelRecord {
        ours: labels.ours.clone(),
        base: labels.base.clone(),
        theirs: labels.theirs.clone(),
    };

    let destination: &Path = args.output.as_deref().unwrap_or(&args.ours);

    // ---- 3. the line merge, always, first. Its result is both the fast path's
    //         candidate answer and every fallback rung's answer.
    let t = Instant::now();
    let line_merge = linemerge::run(
        &args.fallback_cmd,
        &args.ours,
        &args.base,
        &args.theirs,
        marker_size,
        &linemerge::Labels {
            ours: &labels.ours,
            base: &labels.base,
            theirs: &labels.theirs,
        },
        args.diff3,
    );
    rec.timings_ms.line_merge = Some(ms(t.elapsed()));
    if let Ok(lm) = &line_merge {
        rec.line_merge = Some(LineMergeRecord {
            bytes: lm.bytes.len() as u64,
            conflict_hunks: lm.conflict_hunks,
            parsed_ok: None,
        });
    }

    let lang = args
        .pathname
        .as_deref()
        .filter(|p| *p != "%P")
        .and_then(|p| sm_cst::languages::detect(Path::new(p)));
    rec.language = lang.map(Language::name);

    if args.line_merge_only {
        return finish_fallback(
            args,
            rec,
            destination,
            line_merge,
            FallbackReason::SemanticDisabled,
        );
    }

    // ---- 4. the fast path.
    if let Ok(lm) = &line_merge
        && lm.is_clean()
        && !args.no_fast_path
    {
        match lang {
            None => {
                // Nothing to verify against, but git's answer is clean and it is
                // the answer the user would have got anyway.
                return finish_fallback(
                    args,
                    rec,
                    destination,
                    line_merge,
                    FallbackReason::UnknownLanguage,
                );
            }
            Some(lang) => {
                let t = Instant::now();
                let parsed_ok = parses_cleanly(&lm.bytes, lang);
                rec.timings_ms.fast_path_verify = Some(ms(t.elapsed()));
                if let Some(l) = &mut rec.line_merge {
                    l.parsed_ok = Some(parsed_ok);
                }
                if parsed_ok {
                    rec.path_taken = PathTaken::Fast;
                    rec.conflicts = 0;
                    return write_out(rec, destination, &lm.bytes, EXIT_CLEAN);
                }
                rec.warn(
                    "fast path rejected: git's line merge was clean but its output does not parse",
                );
            }
        }
    }

    // ---- 5. the gates on the semantic path.
    let Some(lang) = lang else {
        return finish_fallback(
            args,
            rec,
            destination,
            line_merge,
            FallbackReason::UnknownLanguage,
        );
    };
    let largest = inputs
        .base
        .len()
        .max(inputs.ours.len())
        .max(inputs.theirs.len()) as u64;
    if largest > args.max_bytes {
        rec.warn(format!(
            "largest input is {largest} bytes, over the --max-bytes budget of {}",
            args.max_bytes
        ));
        return finish_fallback(args, rec, destination, line_merge, FallbackReason::TooLarge);
    }

    // ---- 6. the semantic path, on a worker thread, inside catch_unwind.
    let opts = EmitOptions {
        marker_size,
        style: if args.diff3 {
            ConflictStyle::Diff3
        } else {
            ConflictStyle::Merge
        },
        ..EmitOptions::default()
    }
    .with_labels(&labels.ours, &labels.theirs, &labels.base);

    match run_semantic(inputs, lang, opts, args.timeout_ms, args.semantic_check) {
        Ok(done) => {
            rec.inputs.base.parse_errors = Some(done.parse_errors[0]);
            rec.inputs.ours.parse_errors = Some(done.parse_errors[1]);
            rec.inputs.theirs.parse_errors = Some(done.parse_errors[2]);
            rec.timings_ms.parse = Some(done.timings[0]);
            rec.timings_ms.merge = Some(done.timings[1]);
            rec.timings_ms.emit = Some(done.timings[2]);
            rec.timings_ms.verify = Some(done.timings[3]);
            rec.semantic = Some(SemanticRecord {
                clean: done.clean,
                conflicts: done.conflict_count,
                conflict_reasons: done.conflict_reasons,
                output_bytes: done.bytes.len() as u64,
                synthesized_bytes: done.synthesized_bytes,
                synthesized_separators: done.synthesized_separators,
                reindented_lines: done.reindented_lines,
                stats: done.stats,
            });
            if done.synthesized_separators != 0 {
                rec.warn(format!(
                    "the emitter wrote {} token separator(s): two spliced items met with \
                     nothing between them and no revision had a usable gap",
                    done.synthesized_separators
                ));
            }
            rec.path_taken = PathTaken::Semantic;
            rec.conflicts = done.conflict_count;
            let mut code = if done.clean {
                EXIT_CLEAN
            } else {
                EXIT_CONFLICTS
            };
            if let Some(check) = &done.check {
                rec.timings_ms.semantic_check = Some(done.timings[4]);
                rec.semantic_check = Some(SemanticCheckRecord::from_report(
                    check,
                    args.semantic_check.as_str(),
                ));
                if !args.quiet {
                    semantic::report(check, &display_path(args), args.semantic_check);
                }
                if args.semantic_check == SemanticMode::Conflict && !check.conflicts.is_empty() {
                    // The text merged; the names did not. `%A` gets the clean
                    // merge and git is told the path is unmerged. The `semantic`
                    // module documents why this is the least-surprising answer
                    // and what it costs.
                    code = EXIT_CONFLICTS;
                }
            }
            write_out(rec, destination, &done.bytes, code)
        }
        Err(failure) => {
            for (i, errs) in failure.parse_errors.iter().enumerate() {
                let slot = match i {
                    0 => &mut rec.inputs.base,
                    1 => &mut rec.inputs.ours,
                    _ => &mut rec.inputs.theirs,
                };
                slot.parse_errors = *errs;
            }
            if let Some(note) = failure.note {
                rec.warn(note);
            }
            finish_fallback(args, rec, destination, line_merge, failure.reason)
        }
    }
}

/// Write the line merge's bytes, or abort with `%A` untouched.
fn finish_fallback(
    args: &MergeArgs,
    rec: &mut Record,
    destination: &Path,
    line_merge: Result<linemerge::LineMerge, linemerge::LineMergeError>,
    reason: FallbackReason,
) -> u8 {
    rec.fallback_reason = Some(reason);
    match line_merge {
        Ok(lm) => {
            rec.path_taken = PathTaken::Fallback;
            rec.conflicts = lm.conflict_hunks;
            let code = if lm.is_clean() {
                EXIT_CLEAN
            } else {
                EXIT_CONFLICTS
            };
            if !args.quiet {
                eprintln!(
                    "sm merge: {}: falling back to git's line merge ({})",
                    display_path(args),
                    describe(reason)
                );
            }
            write_out(rec, destination, &lm.bytes, code)
        }
        Err(err) => {
            // The bottom rung failed too. %A is untouched and is still valid
            // "ours", which is exactly what git needs to carry on.
            eprintln!("sm merge: {}: {err}", display_path(args));
            rec.fallback_reason = Some(FallbackReason::LineMergeFailed);
            rec.path_taken = PathTaken::Aborted;
            EXIT_ERROR
        }
    }
}

fn write_out(rec: &mut Record, destination: &Path, bytes: &[u8], code: u8) -> u8 {
    let t = Instant::now();
    match atomic::replace(destination, bytes) {
        Ok(()) => {
            rec.timings_ms.write = Some(ms(t.elapsed()));
            code
        }
        Err(err) => {
            eprintln!("sm merge: {}: {err}", destination.display());
            rec.fallback_reason = Some(FallbackReason::WriteFailed);
            rec.path_taken = PathTaken::Aborted;
            EXIT_ERROR
        }
    }
}

fn display_path(args: &MergeArgs) -> String {
    args.pathname
        .as_deref()
        .filter(|p| *p != "%P")
        .map_or_else(|| args.ours.display().to_string(), ToOwned::to_owned)
}

const fn describe(reason: FallbackReason) -> &'static str {
    match reason {
        FallbackReason::UnknownLanguage => "no grammar for this file type",
        FallbackReason::TooLarge => "input over the size budget",
        FallbackReason::ParseError => "an input does not parse",
        FallbackReason::Timeout => "timed out",
        FallbackReason::Panic => "internal error",
        FallbackReason::InvariantFailed => "output failed a self-check",
        FallbackReason::SemanticDisabled => "--line-merge-only",
        FallbackReason::LineMergeFailed => "the line merge failed",
        FallbackReason::InputUnreadable => "an input could not be read",
        FallbackReason::WriteFailed => "the output could not be written",
    }
}

// ------------------------------------------------------------------- reading

struct Inputs {
    base: Vec<u8>,
    ours: Vec<u8>,
    theirs: Vec<u8>,
}

fn read_inputs(args: &MergeArgs) -> Result<Inputs, (&Path, std::io::Error)> {
    Ok(Inputs {
        base: read(&args.base).map_err(|e| (args.base.as_path(), e))?,
        ours: read(&args.ours).map_err(|e| (args.ours.as_path(), e))?,
        theirs: read(&args.theirs).map_err(|e| (args.theirs.as_path(), e))?,
    })
}

/// `std::fs::read`, but tolerating a path that is a pipe or `/dev/null`.
fn read(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut buf)?;
    Ok(buf)
}

// -------------------------------------------------------- markers and labels

/// Git's own default and minimum. Below this the markers stop being reliably
/// distinguishable from ordinary source.
const DEFAULT_MARKER_SIZE: usize = 7;

/// `%L` → a marker size.
///
/// Git passes a decimal integer, widened past 7 when the file itself contains
/// something that looks like a marker line. Anything we cannot read as a number
/// — including the literal `"%L"` from a git too old to expand it — means "git
/// did not tell me", so we use git's own default.
///
/// The result is clamped up to 7 and then used for **both** our emitter and the
/// `git merge-file` fallback, so the two paths cannot disagree about marker
/// width. `sm_emit::EmitOptions::effective_marker_size` clamps identically; the
/// clamp is done here as well so the number written to `--debug-json` is the
/// number actually used.
fn resolve_marker_size(arg: Option<&str>) -> usize {
    arg.and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MARKER_SIZE)
        .max(DEFAULT_MARKER_SIZE)
}

struct Labels {
    ours: String,
    base: String,
    theirs: String,
}

/// `%X` / `%S` / `%Y` → conflict-marker labels.
///
/// The literal-string test is exact and per-argument: git 2.44+ expands all
/// three, an older git expands none, but a user invoking `sm merge` by hand may
/// pass any subset. Comparing each against its own placeholder is the only rule
/// that is right in all three situations.
fn resolve_labels(args: &MergeArgs, rec: &mut Record) -> Labels {
    let unexpanded = |value: &Option<String>, literal: &str| -> Option<String> {
        match value {
            Some(v) if v == literal => None,
            Some(v) => Some(v.clone()),
            None => None,
        }
    };
    let ours = unexpanded(&args.ours_label, "%X");
    let base = unexpanded(&args.base_label, "%S");
    let theirs = unexpanded(&args.theirs_label, "%Y");

    let old_git = args.ours_label.as_deref() == Some("%X")
        || args.base_label.as_deref() == Some("%S")
        || args.theirs_label.as_deref() == Some("%Y");
    if old_git {
        rec.inputs.old_git_labels = true;
        rec.warn(
            "git did not expand %S/%X/%Y (it is older than 2.44); using default marker labels",
        );
    }

    let defaults = EmitOptions::default();
    Labels {
        ours: ours.unwrap_or(defaults.ours_label),
        base: base.unwrap_or(defaults.base_label),
        theirs: theirs.unwrap_or(defaults.theirs_label),
    }
}

// ------------------------------------------------------------ the panic hook

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_owned());
        let location = info
            .location()
            .map_or_else(String::new, |l| format!(" at {}:{}", l.file(), l.line()));
        let message = format!("{payload}{location}");
        // One line. A merge driver runs inside `git merge`, in the middle of
        // output the user is reading.
        eprintln!("sm merge: internal error: {message}");
        if let Ok(mut slot) = PANIC_MESSAGE.lock() {
            *slot = Some(message);
        }
    }));
}

// ---------------------------------------------------------- the semantic path

/// Everything the semantic path produced.
struct Done {
    bytes: Vec<u8>,
    clean: bool,
    conflict_count: u32,
    conflict_reasons: Vec<String>,
    synthesized_bytes: u64,
    synthesized_separators: u64,
    reindented_lines: u64,
    stats: StatsRecord,
    /// M6's name-binding check, when it ran. `None` means it was switched off
    /// or the merge was not clean; see the `semantic` module.
    check: Option<sm_bind::CheckReport>,
    parse_errors: [bool; 3],
    /// parse, merge, emit, verify, semantic check — in milliseconds.
    timings: [f64; 5],
}

struct Failure {
    reason: FallbackReason,
    note: Option<String>,
    parse_errors: [Option<bool>; 3],
}

/// What the worker thread sends back.
enum Outcome {
    Done(Box<Done>),
    Failed(Failure),
}

/// Why a worker did not hand back a value.
#[derive(Debug, PartialEq, Eq)]
enum WorkerFailure {
    /// The closure panicked; the payload, if the hook recorded one.
    Panicked(Option<String>),
    /// It did not finish in time. The thread is still running and abandoned.
    TimedOut,
    /// The thread could not be created at all.
    NotStarted(String),
}

/// Run `work` on a detached worker thread, catching panics and giving up after
/// `timeout_ms` (0 means "wait forever").
///
/// This is the mechanism the module docs describe in §5 and §6, factored out so
/// it can be tested against a closure that panics and a closure that hangs —
/// neither of which the real merge will do on demand. The only thing that
/// factoring leaves untested is the single call in [`run_semantic`] that hands
/// it the real work.
///
/// `work` must be `'static` and own everything it touches, which is the
/// property that makes abandoning it safe.
fn in_worker<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    timeout_ms: u64,
) -> Result<T, WorkerFailure> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<T>(1);

    std::thread::Builder::new()
        .name("sm-merge".to_owned())
        .spawn(move || {
            if let Ok(value) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)) {
                // The receiver may already be gone, because we timed out. The
                // channel has a slot, so this neither blocks nor panics; it
                // returns `Err` and the value is dropped.
                let _ = tx.send(value);
            }
            // On a panic, nothing is sent. The receiver sees a disconnect, which
            // it distinguishes from a timeout below.
        })
        .map_err(|err| WorkerFailure::NotStarted(err.to_string()))?;

    let received = if timeout_ms == 0 {
        rx.recv().map_err(|_| RecvEnd::Disconnected)
    } else {
        rx.recv_timeout(Duration::from_millis(timeout_ms))
            .map_err(|err| match err {
                std::sync::mpsc::RecvTimeoutError::Timeout => RecvEnd::Timeout,
                std::sync::mpsc::RecvTimeoutError::Disconnected => RecvEnd::Disconnected,
            })
    };

    match received {
        Ok(value) => Ok(value),
        Err(RecvEnd::Timeout) => Err(WorkerFailure::TimedOut),
        Err(RecvEnd::Disconnected) => Err(WorkerFailure::Panicked(
            PANIC_MESSAGE.lock().ok().and_then(|m| m.clone()),
        )),
    }
}

enum RecvEnd {
    Timeout,
    Disconnected,
}

fn run_semantic(
    inputs: Inputs,
    lang: &'static dyn Language,
    opts: EmitOptions,
    timeout_ms: u64,
    mode: SemanticMode,
) -> Result<Done, Failure> {
    // The worker takes ownership of the three input buffers, so nothing it
    // touches is visible here except through the channel (module docs §6).
    let outcome = in_worker(move || semantic(&inputs, lang, &opts, mode), timeout_ms);
    match outcome {
        Ok(Outcome::Done(done)) => Ok(*done),
        Ok(Outcome::Failed(failure)) => Err(failure),
        Err(WorkerFailure::TimedOut) => Err(Failure {
            reason: FallbackReason::Timeout,
            note: Some(format!(
                "the semantic merge did not finish in {timeout_ms} ms"
            )),
            parse_errors: [None; 3],
        }),
        Err(WorkerFailure::Panicked(message)) => Err(Failure {
            reason: FallbackReason::Panic,
            note: Some(message.map_or_else(
                || "caught a panic with no message".to_owned(),
                |m| format!("caught panic: {m}"),
            )),
            parse_errors: [None; 3],
        }),
        Err(WorkerFailure::NotStarted(err)) => Err(Failure {
            reason: FallbackReason::Panic,
            note: Some(format!("could not start the merge thread: {err}")),
            parse_errors: [None; 3],
        }),
    }
}

/// Parse, merge, emit and self-check. Runs on the worker thread; does no I/O.
fn semantic(
    inputs: &Inputs,
    lang: &'static dyn Language,
    opts: &EmitOptions,
    mode: SemanticMode,
) -> Outcome {
    let t = Instant::now();
    let trees: [Result<SourceTree, sm_cst::ParseError>; 3] = [
        sm_cst::parse(&inputs.base, lang),
        sm_cst::parse(&inputs.ours, lang),
        sm_cst::parse(&inputs.theirs, lang),
    ];
    let parse_ms = ms(t.elapsed());

    let mut parsed = Vec::with_capacity(3);
    for tree in trees {
        match tree {
            Ok(tree) => parsed.push(tree),
            Err(err) => {
                return Outcome::Failed(Failure {
                    reason: FallbackReason::ParseError,
                    note: Some(format!("parse failed: {err}")),
                    parse_errors: [None; 3],
                });
            }
        }
    }
    let parse_errors = [
        parsed[0].has_errors(),
        parsed[1].has_errors(),
        parsed[2].has_errors(),
    ];

    // Fallback rung 4. `sm_merge::merge` would happily merge these; the whole
    // point of the ladder is that the driver decides not to let it.
    if parse_errors.iter().any(|e| *e) {
        let which: Vec<&str> = ["base", "ours", "theirs"]
            .iter()
            .zip(parse_errors)
            .filter_map(|(name, bad)| bad.then_some(*name))
            .collect();
        return Outcome::Failed(Failure {
            reason: FallbackReason::ParseError,
            note: Some(format!("syntax errors in: {}", which.join(", "))),
            parse_errors: parse_errors.map(Some),
        });
    }

    let (base, ours, theirs) = (&parsed[0], &parsed[1], &parsed[2]);

    let t = Instant::now();
    let outcome: MergeOutcome =
        sm_merge::merge(base, ours, theirs, lang, &MergeConfig::for_language(lang));
    let merge_ms = ms(t.elapsed());

    let t = Instant::now();
    let result: EmitResult = emit(&outcome.tree, base, ours, theirs, lang, opts);
    let emit_ms = ms(t.elapsed());

    let t = Instant::now();
    if let Err(note) = self_check(&outcome, &result, [base, ours, theirs], lang) {
        return Outcome::Failed(Failure {
            reason: FallbackReason::InvariantFailed,
            note: Some(note),
            parse_errors: parse_errors.map(Some),
        });
    }
    let verify_ms = ms(t.elapsed());

    // M6's name-binding check. Inside the worker, so it is covered by the same
    // timeout and the same `catch_unwind` as everything else, and so its cost
    // is on the semantic path's budget rather than added to it afterwards. Only
    // for a clean merge — see the `semantic` module's docs.
    let t = Instant::now();
    let check = (mode != SemanticMode::Off && outcome.is_clean())
        .then(|| sm_bind::check_report(&outcome, base, ours, theirs, lang));
    let check_ms = ms(t.elapsed());

    Outcome::Done(Box::new(Done {
        clean: outcome.is_clean(),
        conflict_count: u32::try_from(result.conflict_count).unwrap_or(u32::MAX),
        conflict_reasons: outcome
            .conflicts
            .iter()
            .map(|c| format!("{}@{}", c.reason.tag(), c.kind))
            .collect(),
        synthesized_bytes: result.synthesized_bytes as u64,
        synthesized_separators: result.synthesized_separators as u64,
        reindented_lines: result.reindented_lines as u64,
        stats: StatsRecord::from_stats(&outcome.stats),
        check,
        bytes: result.bytes,
        parse_errors,
        timings: [parse_ms, merge_ms, emit_ms, verify_ms, check_ms],
    }))
}

/// Fallback rung 7: refuse to ship output that contradicts what the merge said
/// about itself.
///
/// Everything checked here is something the libraries already guarantee and
/// test. It is checked *again*, at the last moment before the bytes reach a
/// user's file, because a guarantee that has been violated is exactly the
/// situation where the guarantee's own tests were not enough.
fn self_check(
    outcome: &MergeOutcome,
    result: &EmitResult,
    inputs: [&SourceTree; 3],
    lang: &dyn Language,
) -> Result<(), String> {
    if outcome.is_clean() {
        if result.conflict_count != 0 {
            return Err(format!(
                "the merge reported no conflicts but the emitter wrote {} marker regions",
                result.conflict_count
            ));
        }
        // Byte preservation, with the one exception SPEC.md §5 allows.
        //
        // `sm-emit` may write a single space between two items whose tokens
        // would otherwise lex as one (its crate docs, "Token separation"), and
        // counts those bytes in both counters. Everything else — invented
        // *layout*, i.e. a `Gap::Synthesized` — is still refused outright, and
        // the two are told apart by comparing the counters rather than by
        // trusting a flag. A separator is safe on its own terms: it is one
        // U+0020 between two tokens that came from real inputs, it cannot
        // change what the program means, and the token check below still runs
        // over the result. It is reported as a warning because it means the
        // merge's own gap repair had nothing to work with, which is worth
        // knowing at corpus scale.
        if result.synthesized_bytes != result.synthesized_separators {
            return Err(format!(
                "a clean merge synthesized {} bytes, {} of them token separators; \
                 SPEC.md §5 requires a pure splice",
                result.synthesized_bytes, result.synthesized_separators
            ));
        }
        // Parse stability (SPEC.md §5). Costs one parse of a file already
        // parsed three times, and only on the semantic path.
        let Ok(reparsed) = sm_cst::parse(&result.bytes, lang) else {
            return Err("a clean merge produced output that could not be reparsed".to_owned());
        };
        if reparsed.has_errors() {
            return Err("a clean merge produced output that does not parse".to_owned());
        }
        if let Some(token) = fabricated_token(&reparsed, inputs) {
            return Err(format!(
                "a clean merge produced the token {token:?}, which is in none of the three inputs"
            ));
        }
    } else if result.conflict_count == 0 {
        return Err(format!(
            "the merge reported {} conflicts but the emitter wrote no markers",
            outcome.conflicts.len()
        ));
    }
    Ok(())
}

fn parses_cleanly(bytes: &[u8], lang: &dyn Language) -> bool {
    sm_cst::parse(bytes, lang).is_ok_and(|tree| !tree.has_errors())
}

/// **Byte preservation at token granularity**: the first token of the output
/// that appears in none of the three inputs, if there is one.
///
/// # Why parse stability is not enough
///
/// A splicing emitter's characteristic failure is not producing garbage, it is
/// producing two spliced ranges with nothing between them, so that the last
/// token of one and the first of the next lex as a single token. M4b's corpus
/// dry run found this happening for real, and the reduced case is alarming:
///
/// ```text
/// ours:   int a = 2;          theirs: static int a = 1;
/// merged: staticint a = 2;
/// ```
///
/// `staticint a = 2;` **parses without error** — `tree-sitter-java` reads
/// `staticint` as a type name — so the reparse check above waves it through.
/// It is a silently wrong merge of exactly the kind SPEC.md §0.4 says is never
/// acceptable, and it would have reached a user's file.
///
/// That bug is fixed — `sm-merge` validates the gap it copies and `sm-emit`
/// carries a lexical backstop, both documented in their crates and pinned by
/// `crates/sm-emit/tests/token_fusion.rs`. **This check stays anyway.** It is
/// the only one here that does not take the libraries' word for anything: it
/// re-derives the answer from the four trees rather than from a counter the
/// emitter maintained, so it is exactly the check that still works when the fix
/// above has a hole in it. The class of bug it guards is one whose entire
/// symptom is output that looks fine.
///
/// The invariant that does catch it is a direct reading of SPEC.md §5's byte
/// preservation, applied to tokens: **`sm-emit` only ever copies byte ranges,
/// so every token it writes must have been a token in one of the inputs.** A
/// fused token is not; neither is a split one. One check catches both, needs no
/// knowledge of merge internals, and costs a walk of four trees we have already
/// built.
///
/// Two exclusions, both because the emitter is allowed to rewrite these bytes:
///
/// - **Comments** — the reindenter adjusts the leading whitespace of a block
///   comment's interior lines, so its text legitimately differs from every
///   input.
/// - **Tokens containing a newline** — the same reason, for any multi-line
///   leaf. Fusion cannot produce one of these, so nothing is lost by skipping
///   them.
///
/// Whitespace-only and empty leaves are skipped too: they are `MISSING`
/// repairs or artefacts, never something a reader would see.
fn fabricated_token(output: &SourceTree, inputs: [&SourceTree; 3]) -> Option<String> {
    let mut known: std::collections::HashSet<&[u8]> = std::collections::HashSet::new();
    for tree in inputs {
        for id in tree.leaves() {
            known.insert(tree.node_bytes(id));
        }
    }
    for id in output.leaves() {
        let node = output.node(id);
        if node.is_extra || node.kind.contains("comment") {
            continue;
        }
        let text = output.node_bytes(id);
        if text.is_empty()
            || text.contains(&b'\n')
            || text.iter().all(u8::is_ascii_whitespace)
            || known.contains(text)
        {
            continue;
        }
        return Some(String::from_utf8_lossy(text).into_owned());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_MARKER_SIZE, WorkerFailure, fabricated_token, in_worker, install_panic_hook,
        resolve_marker_size,
    };

    /// **The token check still catches a fused token.**
    ///
    /// The bug that motivated it is fixed — `sm-merge` validates the gap it
    /// copies and `sm-emit` carries a lexical backstop — so no input reaches it
    /// any more, which is precisely why it is worth testing directly instead of
    /// deleting. It guards a class of failure whose only symptom is output that
    /// looks fine, and it re-derives its answer from the four trees rather than
    /// trusting a counter, so it is the rung that still works when the fix above
    /// develops a hole.
    ///
    /// The "output" here is hand-built: the exact bytes the corpus case used to
    /// produce, against the inputs it used to produce them from.
    #[test]
    fn the_token_check_catches_a_fused_token_that_parses() {
        let lang = sm_cst::languages::detect(std::path::Path::new("x.java")).expect("java");
        let parse = |src: &str| sm_cst::parse(src.as_bytes(), lang).expect("parse");

        let base = parse("class C {\n    int a = 1;\n}\n");
        let ours = parse("class C {\n    int a = 2;\n}\n");
        let theirs = parse("class C {\n    static int a = 1;\n}\n");

        let fused = parse("class C {\n    staticint a = 2;\n}\n");
        assert!(
            !fused.has_errors(),
            "the fused form must parse, or this test proves nothing"
        );
        assert_eq!(
            fabricated_token(&fused, [&base, &ours, &theirs]).as_deref(),
            Some("staticint")
        );

        // And it does not fire on the output the merge now actually produces.
        let good = parse("class C {\n    static int a = 2;\n}\n");
        assert_eq!(fabricated_token(&good, [&base, &ours, &theirs]), None);
    }

    /// The other half of the same invariant: a **split** token is caught too.
    #[test]
    fn the_token_check_catches_a_split_token() {
        let lang = sm_cst::languages::detect(std::path::Path::new("x.java")).expect("java");
        let parse = |src: &str| sm_cst::parse(src.as_bytes(), lang).expect("parse");

        let base = parse("class C {\n    int counter = 1;\n}\n");
        let split = parse("class C {\n    int coun ter = 1;\n}\n");
        assert_eq!(
            fabricated_token(&split, [&base, &base, &base]).as_deref(),
            Some("coun")
        );
    }

    #[test]
    fn marker_size_comes_from_git_when_git_gave_one() {
        assert_eq!(resolve_marker_size(Some("11")), 11);
        assert_eq!(resolve_marker_size(Some("7")), 7);
    }

    #[test]
    fn an_unexpanded_or_unreadable_percent_l_means_the_default() {
        for arg in ["%L", "", "seven", "-3", "9999999999999999999999"] {
            assert_eq!(
                resolve_marker_size(Some(arg)),
                DEFAULT_MARKER_SIZE,
                "for {arg:?}"
            );
        }
        assert_eq!(resolve_marker_size(None), DEFAULT_MARKER_SIZE);
    }

    #[test]
    fn a_marker_size_below_gits_minimum_is_clamped_up() {
        // Nothing in git passes one, but a hand invocation can, and our markers
        // and the fallback's have to stay the same width.
        assert_eq!(resolve_marker_size(Some("3")), DEFAULT_MARKER_SIZE);
        assert_eq!(resolve_marker_size(Some("0")), DEFAULT_MARKER_SIZE);
    }

    // ------------------------------------------------- the worker mechanism
    //
    // The fallback ladder's two hardest rungs — panic and timeout — cannot be
    // triggered on demand through the real merge, so they are tested here
    // against the mechanism that implements them.

    #[test]
    fn a_worker_hands_back_its_value() {
        assert_eq!(in_worker(|| 6 * 7, 5_000), Ok(42));
    }

    #[test]
    fn a_worker_that_finishes_is_not_reported_as_a_timeout() {
        // A generous budget and instant work: the timeout must not fire on a
        // race between `send` and `recv_timeout`.
        for _ in 0..50 {
            assert_eq!(in_worker(|| "done", 5_000), Ok("done"));
        }
    }

    /// **A panic in the worker is caught and reported, not propagated.**
    ///
    /// This is SPEC.md §4.7's "a panic degrades to line merge rather than data
    /// loss" at the level where it is implemented. The caller turns
    /// `Panicked` into `FallbackReason::Panic`, which takes the line merge.
    #[test]
    fn a_panicking_worker_is_caught_and_its_message_recorded() {
        // Installing the driver's own hook keeps this test's deliberate panic
        // to one line instead of a backtrace, and lets us check that the hook
        // captures the message the debug record reports.
        install_panic_hook();
        let result: Result<u32, WorkerFailure> =
            in_worker(|| panic!("deliberate test panic"), 5_000);
        match result {
            Err(WorkerFailure::Panicked(Some(message))) => {
                assert!(
                    message.contains("deliberate test panic"),
                    "message was {message:?}"
                );
                assert!(message.contains("merge/mod.rs"), "no location: {message:?}");
            }
            other => panic!("expected a caught panic, got {other:?}"),
        }
        let _ = std::panic::take_hook();
    }

    /// **A worker that never finishes is abandoned, not waited on.**
    #[test]
    fn a_slow_worker_times_out_and_the_caller_continues() {
        let started = std::time::Instant::now();
        let result: Result<(), WorkerFailure> = in_worker(
            || std::thread::sleep(std::time::Duration::from_secs(30)),
            50,
        );
        assert_eq!(result, Err(WorkerFailure::TimedOut));
        // The caller came back promptly — it did not join the sleeping thread.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the caller waited {:?}",
            started.elapsed()
        );
        // And the abandoned thread is still running here, holding its own
        // allocations, which is exactly the state the module docs argue is
        // safe: it shares nothing and writes nothing.
    }

    #[test]
    fn a_zero_timeout_waits_for_the_worker() {
        let result = in_worker(
            || {
                std::thread::sleep(std::time::Duration::from_millis(120));
                "slow but finished"
            },
            0,
        );
        assert_eq!(result, Ok("slow but finished"));
    }
}
