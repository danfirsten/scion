//! `--debug-json`: one machine-readable record per driver invocation.
//!
//! M5 replays the corpus by running this driver over every case and reading
//! these records back, so the shape here is an **interface**, not a debug dump.
//! The rules that go with `schema_version`:
//!
//! - Adding a field is a compatible change and does not bump the version.
//! - Removing a field, renaming one, or changing the meaning of an existing one
//!   bumps it.
//! - Every enum rendered as a string ([`PathTaken`], [`FallbackReason`]) may
//!   gain variants without a bump, so a consumer must treat an unknown string as
//!   "something else" rather than failing.
//! - Timings are milliseconds as `f64`, always present, and always measured —
//!   a stage that did not run reports `null`, never `0`.
//!
//! The file is written **after** `%A` has been dealt with and its failure never
//! changes the exit code: a debugging aid must not be able to turn a good merge
//! into a bad one.

use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;

/// Bumped only for an incompatible change; see the module docs.
pub const SCHEMA_VERSION: u32 = 1;

/// Which of the three routes through the driver produced the bytes in `%A`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathTaken {
    /// The line merge was clean and its result parsed. No tree was built.
    Fast,
    /// The three-way tree merge produced the output.
    Semantic,
    /// The line merge's output was used verbatim. See `fallback_reason`.
    Fallback,
    /// Nothing was written; `%A` is untouched. Exit code 2.
    Aborted,
}

/// Why the semantic path was not used. `None` on the fast and semantic paths.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackReason {
    /// `%P` has no extension we have a grammar for.
    UnknownLanguage,
    /// One of the three inputs is larger than `--max-bytes`.
    TooLarge,
    /// One of the three inputs has syntax errors (`SourceTree::has_errors`).
    ParseError,
    /// The semantic path did not finish within `--timeout-ms`.
    Timeout,
    /// The semantic path panicked and the panic was caught.
    Panic,
    /// The semantic path finished but its output failed a self-check.
    InvariantFailed,
    /// `--no-fast-path`'s counterpart: the semantic path was disabled.
    SemanticDisabled,
    /// The line merge itself could not be run. Always paired with
    /// [`PathTaken::Aborted`].
    LineMergeFailed,
    /// An input could not be read. Always paired with [`PathTaken::Aborted`].
    InputUnreadable,
    /// The output could not be written. Always paired with
    /// [`PathTaken::Aborted`].
    WriteFailed,
}

#[derive(Serialize)]
pub struct Record {
    pub schema_version: u32,
    /// `env!("CARGO_PKG_NAME")` and version of the binary that wrote this.
    pub tool: String,
    /// `%P`, the real pathname. `null` if git did not pass one.
    pub path: Option<String>,
    /// The language detected from `path`, e.g. `"java"`.
    pub language: Option<&'static str>,
    pub path_taken: PathTaken,
    pub fallback_reason: Option<FallbackReason>,
    /// The process's exit code: 0 clean, 1 conflicts, 2 hard error.
    pub exit_code: u8,
    /// Conflict regions in the file that was written, whichever path wrote it.
    pub conflicts: u32,
    pub inputs: Inputs,
    pub line_merge: Option<LineMergeRecord>,
    pub semantic: Option<SemanticRecord>,
    /// M6's name-binding check. `null` when it did not run: `--semantic=off`,
    /// or a path other than the semantic one, or a merge that was not clean.
    /// Added after `schema_version` 1 was published; a new object is a
    /// compatible change, so the version does not move (see the module docs).
    pub semantic_check: Option<SemanticCheckRecord>,
    pub timings_ms: Timings,
    /// Human-readable notes: a caught panic's message, a rejected fast path,
    /// an old-git label fallback. Never load-bearing; always worth reading.
    pub warnings: Vec<String>,
}

#[derive(Serialize)]
pub struct Inputs {
    pub base: InputRecord,
    pub ours: InputRecord,
    pub theirs: InputRecord,
    /// The effective marker size, after `%L` parsing and clamping.
    pub marker_size: usize,
    /// True when git passed the literal `"%S"`/`"%X"`/`"%Y"` because it is
    /// older than 2.44 (docs/prior-art.md §2.9) and we substituted defaults.
    pub old_git_labels: bool,
    pub labels: LabelRecord,
}

#[derive(Serialize)]
pub struct LabelRecord {
    pub ours: String,
    pub base: String,
    pub theirs: String,
}

#[derive(Serialize)]
pub struct InputRecord {
    pub bytes: u64,
    /// `null` until the input has been parsed, which only the semantic path
    /// does.
    pub parse_errors: Option<bool>,
}

#[derive(Serialize)]
pub struct LineMergeRecord {
    pub bytes: u64,
    pub conflict_hunks: u32,
    /// Whether the line merge's output parsed without errors. `null` when we
    /// did not check (no language, or the merge was not clean so the fast path
    /// never asked).
    pub parsed_ok: Option<bool>,
}

#[derive(Serialize)]
pub struct SemanticRecord {
    pub clean: bool,
    pub conflicts: u32,
    /// `reason@kind` for each conflict region, in output order.
    pub conflict_reasons: Vec<String>,
    pub output_bytes: u64,
    pub synthesized_bytes: u64,
    /// Of `synthesized_bytes`, how many were single spaces the emitter wrote to
    /// stop two adjacent tokens lexing as one. Normally zero — see `sm-emit`'s
    /// crate docs, "Token separation".
    pub synthesized_separators: u64,
    pub reindented_lines: u64,
    pub stats: StatsRecord,
}

/// M6's name-binding check, as it appears in the record.
///
/// The conflicts are carried through as [`sm_bind::SemanticConflict`] values —
/// they already derive `Serialize` and their shape is `sm-bind`'s to define, so
/// re-flattening them here would create a second definition to keep in step. The
/// counters *are* flattened, for the same reason [`StatsRecord`] is: this file
/// is a wire format and M5 reads it.
#[derive(Serialize)]
pub struct SemanticCheckRecord {
    /// `off`, `report` or `conflict` — what the driver was told to do with the
    /// result. Present even when there are no conflicts, so a corpus scan can
    /// tell "checked, found nothing" from "not checked".
    pub mode: &'static str,
    /// How many conflicts were reported.
    pub conflicts: u32,
    /// One `kind` tag per conflict, in merged-program order.
    pub kinds: Vec<&'static str>,
    /// The findings themselves.
    pub findings: Vec<sm_bind::SemanticConflict>,
    pub stats: CheckStatsRecord,
}

/// A flattened copy of [`sm_bind::CheckStats`].
#[derive(Serialize)]
pub struct CheckStatsRecord {
    pub references: u64,
    pub resolved_in_origin: u64,
    pub unresolved_in_origin: u64,
    pub origin_not_found: u64,
    pub conflict_regions: u64,
    pub conflicts: u64,
}

impl SemanticCheckRecord {
    pub fn from_report(report: &sm_bind::CheckReport, mode: &'static str) -> Self {
        let s = &report.stats;
        Self {
            mode,
            conflicts: u32::try_from(report.conflicts.len()).unwrap_or(u32::MAX),
            kinds: report.conflicts.iter().map(|c| c.kind.tag()).collect(),
            findings: report.conflicts.clone(),
            stats: CheckStatsRecord {
                references: s.references as u64,
                resolved_in_origin: s.resolved_in_origin as u64,
                unresolved_in_origin: s.unresolved_in_origin as u64,
                origin_not_found: s.origin_not_found as u64,
                conflict_regions: s.conflict_regions as u64,
                conflicts: s.conflicts as u64,
            },
        }
    }
}

/// A flattened copy of [`sm_merge::MergeStats`]. Flattened on purpose: this
/// file is a wire format and should not change shape when an internal struct
/// is refactored.
#[derive(Serialize)]
pub struct StatsRecord {
    pub base_nodes: u64,
    pub ours_nodes: u64,
    pub theirs_nodes: u64,
    pub merged_nodes: u64,
    pub splices: u64,
    pub rebuilds: u64,
    pub reparented: u64,
    pub set_merged_regions: u64,
    pub deduplicated_insertions: u64,
    pub covered_deletions: u64,
}

impl StatsRecord {
    pub fn from_stats(s: &sm_merge::MergeStats) -> Self {
        Self {
            base_nodes: s.base_nodes as u64,
            ours_nodes: s.ours_nodes as u64,
            theirs_nodes: s.theirs_nodes as u64,
            merged_nodes: s.merged_nodes as u64,
            splices: s.splices as u64,
            rebuilds: s.rebuilds as u64,
            reparented: s.reparented as u64,
            set_merged_regions: s.set_merged_regions as u64,
            deduplicated_insertions: s.deduplicated_insertions as u64,
            covered_deletions: s.covered_deletions as u64,
        }
    }
}

/// Per-stage wall time. `null` means the stage did not run.
#[derive(Default, Serialize)]
pub struct Timings {
    pub read: Option<f64>,
    pub line_merge: Option<f64>,
    pub fast_path_verify: Option<f64>,
    pub parse: Option<f64>,
    pub merge: Option<f64>,
    pub emit: Option<f64>,
    pub verify: Option<f64>,
    /// M6's name-binding check. `null` when it did not run.
    pub semantic_check: Option<f64>,
    pub write: Option<f64>,
    pub total: f64,
}

pub fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

impl Record {
    pub fn new(path: Option<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            tool: format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
            path,
            language: None,
            path_taken: PathTaken::Aborted,
            fallback_reason: None,
            exit_code: crate::merge::EXIT_ERROR,
            conflicts: 0,
            inputs: Inputs {
                base: InputRecord {
                    bytes: 0,
                    parse_errors: None,
                },
                ours: InputRecord {
                    bytes: 0,
                    parse_errors: None,
                },
                theirs: InputRecord {
                    bytes: 0,
                    parse_errors: None,
                },
                marker_size: 0,
                old_git_labels: false,
                labels: LabelRecord {
                    ours: String::new(),
                    base: String::new(),
                    theirs: String::new(),
                },
            },
            line_merge: None,
            semantic: None,
            semantic_check: None,
            timings_ms: Timings::default(),
            warnings: Vec::new(),
        }
    }

    pub fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }

    /// Write the record. Errors are reported on stderr and swallowed: see the
    /// module docs.
    pub fn write_to(&mut self, path: &Path, started: Instant) {
        // `total` is only knowable once everything else has happened, which is
        // why it is stamped here rather than by the caller.
        self.timings_ms.total = ms(started.elapsed());
        match serde_json::to_string(self) {
            Ok(mut json) => {
                json.push('\n');
                if let Err(err) = std::fs::write(path, json) {
                    eprintln!("sm merge: {}: {err}", path.display());
                }
            }
            Err(err) => eprintln!("sm merge: could not serialize the debug record: {err}"),
        }
    }
}
