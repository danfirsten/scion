//! `sm-eval replay` — run the merge driver over the corpus and record what it
//! did (SPEC.md §6.2, milestone M5).
//!
//! # What it runs
//!
//! One `sm merge` subprocess per case, with `--output` so the corpus is never
//! written to and `--debug-json` so the driver's own account of the invocation
//! comes back structured. Two arms:
//!
//! * **`sm`** — the driver as shipped. This is the measurement.
//! * **`line`** — the same binary with `--line-merge-only`, which makes it a
//!   pure `git merge-file` wrapper. This is the control: it is what the user
//!   would have got with no merge driver installed, graded on exactly the same
//!   inputs by exactly the same comparator.
//!
//! The control matters more than it looks. The corpus's "git conflicted here"
//! label comes from `git merge-tree` (merge-ort) at mining time; the driver's
//! fallback ladder uses `git merge-file` (the classic three-way line merge).
//! Those are different algorithms and they disagree on real cases, so a resolve
//! rate conditioned on the *mined* label would be quietly measuring a different
//! question than the one SPEC.md §6.2 asks. Running the control gives the
//! denominator "the line merge our own fallback would have used also
//! conflicted", which is the honest one, and it costs one extra subprocess.
//!
//! # What it writes
//!
//! `records.jsonl`, one [`ReplayRecord`] per (case, arm), plus `run.json`
//! describing the run. Outputs are retained only when they are worth looking at
//! — see [`ReplayOptions::keep_outputs`] — because 22,000 Java files is 400 MB
//! and the gallery needs maybe fifty of them.
//!
//! # Determinism, parallelism and resumption
//!
//! Cases are enumerated in sorted order and processed in fixed-size **chunks**.
//! Within a chunk the work is spread over a thread pool; the chunk's results are
//! sorted back into case order before anything is written. So the JSONL is
//! byte-identical across runs and across `--jobs` values, and a run that dies
//! mid-way leaves a file that ends on a chunk boundary. `--resume` reads the
//! case ids already present and skips them.
//!
//! The driver is a subprocess, so the pool is sized to the core count: the work
//! is not in our address space and threads here are just `wait()` slots.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::compare::{self, Comparison};
use crate::model::{Case, CaseKind};
use crate::util::fnv1a64;

/// Bumped when a field's meaning changes; see `model`'s docs for the rules,
/// which are the same ones.
pub const REPLAY_SCHEMA_VERSION: u32 = 1;

/// How many cases one thread pool dispatch covers. Big enough that the
/// per-chunk join is noise, small enough that a killed run loses little.
const CHUNK: usize = 256;

/// Cap on the fabricated-token sample kept per record.
const MAX_FABRICATED: usize = 8;

// ---------------------------------------------------------------- the buckets

/// Which population a case belongs to. The report's denominators are these.
///
/// SPEC.md §6.2's rates are conditioned on a case being *gradeable*: git
/// conflicted, the human resolution is available, and the human did not do
/// unrelated work while resolving. Everything else is still replayed — the
/// driver has to survive it — but is counted apart, because mixing populations
/// is how an evaluation ends up quoting a number nobody can reproduce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bucket {
    /// Conflicted, has base+ours+theirs+resolution, not contaminated.
    /// **The denominator.**
    Gradeable,
    /// The same, but the contamination filter flagged it.
    Contaminated,
    /// `add_none`: the miner recorded one side and a resolution because the
    /// *other* side renamed the file (docs/corpus-summary.md). These are rename
    /// conflicts, not three-way merges, and grading them as three-way merges
    /// would be measuring the wrong thing. Counted, not replayed.
    Rename,
    /// Conflicted, but missing an input or the resolution: add/add (no base),
    /// delete/modify, modify/delete, or a path the merge commit does not
    /// contain. Real merge situations the driver must survive; not gradeable
    /// against ground truth as a three-way merge.
    Incomplete,
    /// Git merged this path cleanly. The regression and divergence denominator.
    Clean,
}

impl Bucket {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gradeable => "gradeable",
            Self::Contaminated => "contaminated",
            Self::Rename => "rename",
            Self::Incomplete => "incomplete",
            Self::Clean => "clean",
        }
    }

    /// Whether the case can be fed to a three-way merge at all.
    #[must_use]
    pub const fn runnable(self) -> bool {
        !matches!(self, Self::Rename | Self::Incomplete)
    }
}

/// Classify a case. Pure, and the single definition of every denominator.
#[must_use]
pub fn bucket_of(case: &Case) -> Bucket {
    if case.kind == CaseKind::Clean {
        return Bucket::Clean;
    }
    if case.shape == crate::model::CaseShape::AddNone {
        return Bucket::Rename;
    }
    let complete = case.base.is_some()
        && case.ours.is_some()
        && case.theirs.is_some()
        && case.resolved.is_some();
    if !complete {
        return Bucket::Incomplete;
    }
    if case.contaminated {
        Bucket::Contaminated
    } else {
        Bucket::Gradeable
    }
}

// ---------------------------------------------------------------- the record

/// Which configuration produced a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arm {
    /// `sm merge` as shipped.
    Sm,
    /// `sm merge --line-merge-only`: the `git merge-file` control.
    Line,
}

impl Arm {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sm => "sm",
            Self::Line => "line",
        }
    }
}

/// One replayed (case, arm) pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayRecord {
    pub schema_version: u32,
    pub case_id: String,
    pub repo: String,
    pub arm: Arm,
    pub bucket: Bucket,
    pub kind: String,
    pub shape: String,
    pub path: String,
    pub merge_commit: String,
    pub license: String,
    pub contaminated: bool,
    /// How the human's committed resolution relates to the two sides:
    /// `took_ours`, `took_theirs`, `merged` or `absent`. Carried through
    /// because it splits the incorrect-resolution population more sharply than
    /// anything the driver reports — a human who took one side wholesale and a
    /// human who hand-merged are two different grading questions.
    ///
    /// `#[serde(default)]` because the field was added after the schema was
    /// first published. Adding an optional field is a compatible change (see
    /// [`REPLAY_SCHEMA_VERSION`]), and a log written before it existed must
    /// still load rather than abort a report.
    #[serde(default)]
    pub resolution_kind: String,

    /// False when the case has no three inputs to merge; every field below is
    /// then absent. Counted, never silently dropped.
    pub ran: bool,
    /// Why not, when `ran` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_run_reason: Option<String>,
    /// The subprocess failed to start or was killed by a signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_error: Option<String>,

    /// The driver's exit code: 0 clean, 1 conflicts, 2 hard error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_taken: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    pub conflicts: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflict_reasons: Vec<String>,
    /// What the line merge inside the driver did, on every arm. This is what
    /// makes "git also conflicted here" a measured fact rather than a mining
    /// label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_merge_conflicts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_merge_parsed_ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthesized_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthesized_separators: Option<u64>,

    // ---- grading against the human resolution (absent when there is none)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_exact: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ast_equal: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ast_equal_ignoring_comments: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_parsable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_size: Option<u64>,

    // ---- ground-truth-free criteria (clean outputs only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parsable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub universal: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fabricated: Vec<String>,

    /// For a clean case: does our output equal what git committed to the index?
    /// A `false` here is SPEC.md §6.2's divergence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches_git_result: Option<bool>,

    /// Wall time of the whole subprocess, as the harness saw it.
    pub wall_ms: f64,
    /// The driver's own total, which excludes process start-up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_check_ms: Option<f64>,

    /// M6's check, verbatim from the driver record. `null` when it did not run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_check: Option<serde_json::Value>,

    /// Where the output was kept, relative to the replay directory. Absent when
    /// it was not worth keeping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_file: Option<String>,
}

/// `run.json`: what was replayed, with what, so a number can be traced back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunInfo {
    pub schema_version: u32,
    pub tool: String,
    pub sm_binary: String,
    pub sm_version: String,
    pub corpus: String,
    pub subset: String,
    pub jobs: usize,
    pub semantic_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_overrides: Option<serde_json::Value>,
    pub cases_selected: usize,
    pub records_written: usize,
    pub arms: Vec<String>,
    pub duration_secs: f64,
}

// ---------------------------------------------------------------- selection

/// Which cases to replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subset {
    /// Everything on disk.
    All,
    /// Only conflicted cases (every bucket but [`Bucket::Clean`]).
    Conflicted,
    /// Only clean cases.
    Clean,
    /// A single repository, `owner/repo`.
    Repo(String),
    /// A stratified sample of `n` gradeable conflicted cases, allocated across
    /// repositories in proportion to how many each has, chosen by a seeded hash
    /// of the case id. Deterministic and reproducible from `(n, seed)` alone.
    Sample { n: usize, seed: u64 },
}

impl Subset {
    /// Parse `--subset`. The grammar is deliberately tiny:
    /// `all`, `conflicted`, `clean`, `repo:<owner/name>`, `sample:<n>[:<seed>]`.
    pub fn parse(spec: &str) -> Result<Self> {
        let spec = spec.trim();
        if let Some(rest) = spec.strip_prefix("repo:") {
            return Ok(Self::Repo(rest.to_owned()));
        }
        if let Some(rest) = spec.strip_prefix("sample:") {
            let mut parts = rest.split(':');
            let n = parts
                .next()
                .unwrap_or_default()
                .parse::<usize>()
                .with_context(|| format!("sample size in --subset {spec:?}"))?;
            let seed = match parts.next() {
                Some(s) => s
                    .parse::<u64>()
                    .with_context(|| format!("seed in --subset {spec:?}"))?,
                None => 0,
            };
            if parts.next().is_some() {
                anyhow::bail!("--subset {spec:?}: expected sample:<n> or sample:<n>:<seed>");
            }
            return Ok(Self::Sample { n, seed });
        }
        match spec {
            "all" => Ok(Self::All),
            "conflicted" => Ok(Self::Conflicted),
            "clean" => Ok(Self::Clean),
            other => anyhow::bail!(
                "unknown --subset {other:?}: expected all, conflicted, clean, \
                 repo:<owner/name> or sample:<n>[:<seed>]"
            ),
        }
    }

    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::All => "all".to_owned(),
            Self::Conflicted => "conflicted".to_owned(),
            Self::Clean => "clean".to_owned(),
            Self::Repo(r) => format!("repo:{r}"),
            Self::Sample { n, seed } => format!("sample:{n}:{seed}"),
        }
    }
}

/// A case found on disk, before anything has been run.
#[derive(Debug, Clone)]
pub struct Selected {
    pub dir: PathBuf,
    pub case: Case,
    pub bucket: Bucket,
}

/// Walk the corpus and return every case, in a fixed order.
///
/// Sorted by `(repo, case id)`, which is what makes the replay log stable: the
/// filesystem's `read_dir` order is not.
pub fn enumerate(corpus: &Path) -> Result<Vec<Selected>> {
    let mut repo_dirs: Vec<PathBuf> = std::fs::read_dir(corpus)
        .with_context(|| format!("reading corpus directory {}", corpus.display()))?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    repo_dirs.sort();

    let mut out = Vec::new();
    for dir in repo_dirs {
        collect(&dir, &mut out)?;
    }
    out.sort_by(|a, b| {
        (&a.case.repo, &a.case.case_id, a.case.kind.as_str()).cmp(&(
            &b.case.repo,
            &b.case.case_id,
            b.case.kind.as_str(),
        ))
    });
    Ok(out)
}

fn collect(dir: &Path, out: &mut Vec<Selected>) -> Result<()> {
    let case_file = dir.join("case.json");
    if case_file.is_file() {
        let text = std::fs::read_to_string(&case_file)
            .with_context(|| format!("reading {}", case_file.display()))?;
        let case: Case = serde_json::from_str(&text)
            .with_context(|| format!("parsing {}", case_file.display()))?;
        out.push(Selected {
            bucket: bucket_of(&case),
            dir: dir.to_path_buf(),
            case,
        });
        return Ok(());
    }
    let mut children: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    children.sort();
    for child in children {
        collect(&child, out)?;
    }
    Ok(())
}

/// Apply a [`Subset`] to an enumeration.
///
/// The stratified sample is the interesting one. The corpus is
/// project-imbalanced — eleven repositories hit the 1,500-case mining cap — so
/// a uniform sample would be most of `spring-framework`. Allocating
/// proportionally reproduces the corpus's own repository mix in the subsample,
/// which is the right thing for a *sweep* (it should be tuned on the
/// distribution it will be measured on) and is stated as a limitation for
/// anything else.
#[must_use]
pub fn apply_subset(cases: Vec<Selected>, subset: &Subset) -> Vec<Selected> {
    match subset {
        Subset::All => cases,
        Subset::Conflicted => cases
            .into_iter()
            .filter(|c| c.bucket != Bucket::Clean)
            .collect(),
        Subset::Clean => cases
            .into_iter()
            .filter(|c| c.bucket == Bucket::Clean)
            .collect(),
        Subset::Repo(repo) => cases.into_iter().filter(|c| &c.case.repo == repo).collect(),
        Subset::Sample { n, seed } => stratified(cases, *n, *seed),
    }
}

fn stratified(cases: Vec<Selected>, n: usize, seed: u64) -> Vec<Selected> {
    let pool: Vec<Selected> = cases
        .into_iter()
        .filter(|c| c.bucket == Bucket::Gradeable)
        .collect();
    if pool.len() <= n {
        return pool;
    }

    // Group by repo, keeping the enumeration's order inside each group.
    let mut repos: Vec<String> = pool.iter().map(|c| c.case.repo.clone()).collect();
    repos.sort();
    repos.dedup();

    // Largest-remainder allocation: proportional shares, then the leftover
    // seats to the largest fractional parts. Deterministic, and it never gives
    // a repository more cases than it has.
    let total = pool.len() as f64;
    let mut quotas: Vec<(String, usize, f64)> = Vec::with_capacity(repos.len());
    for repo in &repos {
        let have = pool.iter().filter(|c| &c.case.repo == repo).count();
        let exact = have as f64 * n as f64 / total;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let floor = exact.floor() as usize;
        quotas.push((repo.clone(), floor.min(have), exact - exact.floor()));
    }
    let mut assigned: usize = quotas.iter().map(|(_, q, _)| *q).sum();
    let mut order: Vec<usize> = (0..quotas.len()).collect();
    order.sort_by(|&a, &b| {
        quotas[b]
            .2
            .partial_cmp(&quotas[a].2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| quotas[a].0.cmp(&quotas[b].0))
    });
    for &i in order.iter().cycle().take(quotas.len() * 4) {
        if assigned >= n {
            break;
        }
        let have = pool.iter().filter(|c| c.case.repo == quotas[i].0).count();
        if quotas[i].1 < have {
            quotas[i].1 += 1;
            assigned += 1;
        }
    }

    let mut out = Vec::with_capacity(n);
    for (repo, quota, _) in &quotas {
        let mut group: Vec<&Selected> = pool.iter().filter(|c| &c.case.repo == repo).collect();
        // Seeded, stable, and independent of how many cases the repo has.
        group.sort_by_key(|c| {
            fnv1a64(format!("{seed}:{}:{}", c.case.repo, c.case.case_id).as_bytes())
        });
        for c in group.into_iter().take(*quota) {
            out.push(c.clone());
        }
    }
    out.sort_by(|a, b| (&a.case.repo, &a.case.case_id).cmp(&(&b.case.repo, &b.case.case_id)));
    out
}

// ------------------------------------------------------------------- the run

/// Everything `replay` was asked to do.
#[derive(Debug, Clone)]
pub struct ReplayOptions {
    pub corpus: PathBuf,
    pub sm: PathBuf,
    pub out: PathBuf,
    pub jobs: usize,
    pub subset: Subset,
    /// A JSON `sm_merge::MergeConfig` (possibly partial) handed to the driver
    /// with `--merge-config`. `None` uses the shipped defaults.
    pub profile_overrides: Option<PathBuf>,
    /// `off`, `report` or `conflict` for `sm merge --semantic`.
    pub semantic: String,
    /// Also run the `git merge-file` control arm.
    pub control: bool,
    /// Skip cases already in `records.jsonl`.
    pub resume: bool,
    /// Keep the merged output for cases the report will want to show: anything
    /// graded incorrect, anything that diverged, anything that failed a
    /// ground-truth-free criterion.
    pub keep_outputs: bool,
    pub quiet: bool,
    pub timeout_ms: u64,
}

/// Run the replay. Returns the `run.json` it wrote.
pub fn replay(opts: &ReplayOptions) -> Result<RunInfo> {
    let started = std::time::Instant::now();
    std::fs::create_dir_all(&opts.out)
        .with_context(|| format!("creating {}", opts.out.display()))?;

    let all = enumerate(&opts.corpus)?;
    let selected = apply_subset(all, &opts.subset);

    let records_path = opts.out.join("records.jsonl");
    let done: BTreeSet<String> = if opts.resume && records_path.is_file() {
        read_done(&records_path)?
    } else {
        if records_path.exists() {
            std::fs::remove_file(&records_path)
                .with_context(|| format!("removing {}", records_path.display()))?;
        }
        BTreeSet::new()
    };

    let arms: Vec<Arm> = if opts.control {
        vec![Arm::Sm, Arm::Line]
    } else {
        vec![Arm::Sm]
    };

    let mut todo: Vec<(Selected, Arm)> = Vec::new();
    for case in &selected {
        for &arm in &arms {
            if !done.contains(&record_key(&case.case.repo, &case.case.case_id, arm)) {
                todo.push((case.clone(), arm));
            }
        }
    }

    let overrides = match &opts.profile_overrides {
        Some(p) => {
            let text =
                std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
            Some(
                serde_json::from_str::<serde_json::Value>(&text)
                    .with_context(|| format!("parsing {}", p.display()))?,
            )
        }
        None => None,
    };

    let scratch = opts.out.join("scratch");
    std::fs::create_dir_all(&scratch)?;
    if opts.keep_outputs {
        std::fs::create_dir_all(opts.out.join("outputs"))?;
    }

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&records_path)
        .with_context(|| format!("opening {}", records_path.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    let mut written = 0usize;

    let total = todo.len();
    for (chunk_index, chunk) in todo.chunks(CHUNK).enumerate() {
        let results = run_chunk(chunk, opts, &scratch);
        for record in results {
            serde_json::to_writer(&mut writer, &record)?;
            writer.write_all(b"\n")?;
            written += 1;
        }
        writer.flush()?;
        if !opts.quiet {
            let done_n = ((chunk_index + 1) * CHUNK).min(total);
            eprint!("\rreplay: {done_n}/{total} invocations");
            let _ = std::io::stderr().flush();
        }
    }
    if !opts.quiet && total > 0 {
        eprintln!();
    }
    let _ = std::fs::remove_dir_all(&scratch);

    let info = RunInfo {
        schema_version: REPLAY_SCHEMA_VERSION,
        tool: concat!("sm-eval ", env!("CARGO_PKG_VERSION")).to_owned(),
        sm_binary: opts.sm.display().to_string(),
        sm_version: sm_version(&opts.sm),
        corpus: opts.corpus.display().to_string(),
        subset: opts.subset.describe(),
        jobs: opts.jobs,
        semantic_mode: opts.semantic.clone(),
        profile_overrides: overrides,
        cases_selected: selected.len(),
        records_written: written + done.len(),
        arms: arms.iter().map(|a| a.as_str().to_owned()).collect(),
        duration_secs: started.elapsed().as_secs_f64(),
    };
    let info_path = opts.out.join("run.json");
    std::fs::write(&info_path, serde_json::to_string_pretty(&info)? + "\n")
        .with_context(|| format!("writing {}", info_path.display()))?;
    Ok(info)
}

fn record_key(repo: &str, case_id: &str, arm: Arm) -> String {
    format!("{repo}\u{1}{case_id}\u{1}{}", arm.as_str())
}

fn read_done(path: &Path) -> Result<BTreeSet<String>> {
    let file = std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut out = BTreeSet::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        // A truncated final line from a killed run is dropped rather than
        // fatal: the case simply gets replayed again.
        if let Ok(rec) = serde_json::from_str::<ReplayRecord>(&line) {
            out.insert(record_key(&rec.repo, &rec.case_id, rec.arm));
        }
    }
    Ok(out)
}

fn sm_version(sm: &Path) -> String {
    Command::new(sm)
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

/// Run one chunk across the thread pool and return its records **in the chunk's
/// own order**, which is what keeps the log deterministic.
fn run_chunk(chunk: &[(Selected, Arm)], opts: &ReplayOptions, scratch: &Path) -> Vec<ReplayRecord> {
    let slots: Vec<Mutex<Option<ReplayRecord>>> =
        (0..chunk.len()).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let jobs = opts.jobs.clamp(1, chunk.len().max(1));

    std::thread::scope(|scope| {
        for worker in 0..jobs {
            let slots = &slots;
            let next = &next;
            scope.spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= chunk.len() {
                        break;
                    }
                    let (case, arm) = &chunk[i];
                    let record = run_one(case, *arm, opts, scratch, worker);
                    *slots[i].lock().expect("replay slot poisoned") = Some(record);
                }
            });
        }
    });

    slots
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .expect("replay slot poisoned")
                .expect("every slot is filled before the scope ends")
        })
        .collect()
}

fn blank(case: &Selected, arm: Arm) -> ReplayRecord {
    ReplayRecord {
        schema_version: REPLAY_SCHEMA_VERSION,
        case_id: case.case.case_id.clone(),
        repo: case.case.repo.clone(),
        arm,
        bucket: case.bucket,
        kind: case.case.kind.as_str().to_owned(),
        shape: case.case.shape.as_str().to_owned(),
        path: case.case.path.clone(),
        merge_commit: case.case.merge_commit.clone(),
        license: case.case.license.clone(),
        contaminated: case.case.contaminated,
        resolution_kind: case.case.resolution_kind.as_str().to_owned(),
        ran: false,
        not_run_reason: None,
        harness_error: None,
        exit: None,
        path_taken: None,
        fallback_reason: None,
        conflicts: 0,
        conflict_reasons: Vec::new(),
        line_merge_conflicts: None,
        line_merge_parsed_ok: None,
        warnings: Vec::new(),
        output_bytes: None,
        synthesized_bytes: None,
        synthesized_separators: None,
        byte_exact: None,
        ast_equal: None,
        ast_equal_ignoring_comments: None,
        resolved_parsable: None,
        diff_size: None,
        parsable: None,
        universal: None,
        fabricated: Vec::new(),
        matches_git_result: None,
        wall_ms: 0.0,
        driver_ms: None,
        semantic_check_ms: None,
        semantic_check: None,
        output_file: None,
    }
}

fn run_one(
    case: &Selected,
    arm: Arm,
    opts: &ReplayOptions,
    scratch: &Path,
    worker: usize,
) -> ReplayRecord {
    let mut rec = blank(case, arm);

    if !case.bucket.runnable() {
        rec.not_run_reason = Some(match case.bucket {
            Bucket::Rename => "rename conflict: no three-way input (see the rename bucket)".into(),
            _ => "the case is missing base, ours or theirs".to_owned(),
        });
        return rec;
    }
    let (Some(base), Some(ours), Some(theirs)) =
        (&case.case.base, &case.case.ours, &case.case.theirs)
    else {
        rec.not_run_reason = Some("the case is missing base, ours or theirs".to_owned());
        return rec;
    };

    let out_path = scratch.join(format!("out-{worker}.java"));
    let json_path = scratch.join(format!("rec-{worker}.json"));
    let _ = std::fs::remove_file(&out_path);
    let _ = std::fs::remove_file(&json_path);

    let mut cmd = Command::new(&opts.sm);
    cmd.arg("merge")
        .arg(case.dir.join(&base.file))
        .arg(case.dir.join(&ours.file))
        .arg(case.dir.join(&theirs.file))
        .arg("7")
        .arg(&case.case.path)
        .arg("--output")
        .arg(&out_path)
        .arg("--debug-json")
        .arg(&json_path)
        .arg("--timeout-ms")
        .arg(opts.timeout_ms.to_string())
        .arg("--semantic")
        .arg(&opts.semantic)
        .arg("--quiet")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if arm == Arm::Line {
        cmd.arg("--line-merge-only");
    }
    if let Some(p) = &opts.profile_overrides {
        cmd.arg("--merge-config").arg(p);
    }

    let t = std::time::Instant::now();
    let status = cmd.status();
    rec.wall_ms = t.elapsed().as_secs_f64() * 1000.0;
    rec.ran = true;

    match status {
        Ok(s) => rec.exit = s.code(),
        Err(err) => {
            rec.harness_error = Some(format!("could not run the driver: {err}"));
            return rec;
        }
    }
    if rec.exit.is_none() {
        rec.harness_error = Some("the driver was killed by a signal".to_owned());
        return rec;
    }

    absorb_driver_record(&mut rec, &json_path);
    grade(&mut rec, case, &out_path, opts);
    rec
}

/// Copy the fields we keep out of the driver's `--debug-json`.
fn absorb_driver_record(rec: &mut ReplayRecord, json_path: &Path) {
    let Ok(text) = std::fs::read_to_string(json_path) else {
        rec.harness_error = Some("the driver wrote no --debug-json record".to_owned());
        return;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        rec.harness_error = Some("the driver's --debug-json record did not parse".to_owned());
        return;
    };
    rec.path_taken = v["path_taken"].as_str().map(ToOwned::to_owned);
    rec.fallback_reason = v["fallback_reason"].as_str().map(ToOwned::to_owned);
    #[allow(clippy::cast_possible_truncation)]
    {
        rec.conflicts = v["conflicts"].as_u64().unwrap_or(0) as u32;
    }
    rec.warnings = v["warnings"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if let Some(lm) = v.get("line_merge").filter(|x| !x.is_null()) {
        #[allow(clippy::cast_possible_truncation)]
        {
            rec.line_merge_conflicts = lm["conflict_hunks"].as_u64().map(|n| n as u32);
        }
        rec.line_merge_parsed_ok = lm["parsed_ok"].as_bool();
    }
    if let Some(sem) = v.get("semantic").filter(|x| !x.is_null()) {
        rec.conflict_reasons = sem["conflict_reasons"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(ToOwned::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        rec.synthesized_bytes = sem["synthesized_bytes"].as_u64();
        rec.synthesized_separators = sem["synthesized_separators"].as_u64();
    }
    rec.driver_ms = v["timings_ms"]["total"].as_f64();
    rec.semantic_check_ms = v["timings_ms"]["semantic_check"].as_f64();
    if let Some(check) = v.get("semantic_check").filter(|x| !x.is_null()) {
        rec.semantic_check = Some(check.clone());
    }
}

/// Compare the merged output with the human resolution and with git's own
/// result, and compute the two ground-truth-free criteria.
fn grade(rec: &mut ReplayRecord, case: &Selected, out_path: &Path, opts: &ReplayOptions) {
    let Ok(output) = std::fs::read(out_path) else {
        // Exit 2 leaves the destination untouched, which is the contract; there
        // is simply nothing to grade.
        if rec.exit != Some(2) {
            rec.harness_error = Some("the driver exited without writing an output".to_owned());
        }
        return;
    };
    rec.output_bytes = Some(output.len() as u64);

    let lang = sm_cst::languages::detect(Path::new(&case.case.path));

    // Against the human resolution.
    if let Some(resolved) = &case.case.resolved
        && let Ok(bytes) = std::fs::read(case.dir.join(&resolved.file))
    {
        let cmp = lang.map_or_else(
            || Comparison {
                byte_exact: output == bytes,
                ast_equal: output == bytes,
                ast_equal_ignoring_comments: output == bytes,
                ..Comparison::default()
            },
            |lang| compare::compare(&output, &bytes, lang),
        );
        rec.byte_exact = Some(cmp.byte_exact);
        rec.ast_equal = Some(cmp.ast_equal);
        rec.ast_equal_ignoring_comments = Some(cmp.ast_equal_ignoring_comments);
        rec.resolved_parsable = Some(cmp.resolved_parsable);
        rec.diff_size = Some(compare::line_diff_size(&output, &bytes));
    }

    // Against git's committed merge, for a clean case. Divergence, SPEC §6.2.
    if let Some(git_result) = &case.case.git_result
        && let Ok(bytes) = std::fs::read(case.dir.join(&git_result.file))
    {
        rec.matches_git_result = Some(output == bytes);
    }

    // The ground-truth-free criteria, only where they mean anything: a
    // conflict-free output. See `compare`'s docs.
    if rec.exit == Some(0)
        && let Some(lang) = lang
    {
        let parsed = sm_cst::parse(&output, lang)
            .ok()
            .filter(|t| !t.has_errors());
        rec.parsable = Some(parsed.is_some());
        if let Some(out_tree) = parsed {
            let inputs: Vec<sm_cst::SourceTree> =
                [&case.case.base, &case.case.ours, &case.case.theirs]
                    .into_iter()
                    .flatten()
                    .filter_map(|v| std::fs::read(case.dir.join(&v.file)).ok())
                    .filter_map(|b| sm_cst::parse(&b, lang).ok())
                    .collect();
            let refs: Vec<&sm_cst::SourceTree> = inputs.iter().collect();
            rec.fabricated = compare::fabricated_tokens(&out_tree, &refs, MAX_FABRICATED);
            let no_synth = rec
                .synthesized_bytes
                .zip(rec.synthesized_separators)
                .is_none_or(|(all, sep)| all == sep);
            rec.universal = Some(rec.fabricated.is_empty() && no_synth);
        } else {
            rec.universal = Some(false);
        }
    }

    if opts.keep_outputs && worth_keeping(rec) {
        let name = format!(
            "{}__{}__{}.java",
            case.case.repo.replace('/', "__"),
            case.case.case_id,
            rec.arm.as_str()
        );
        let rel = Path::new("outputs").join(&name);
        if std::fs::write(opts.out.join(&rel), &output).is_ok() {
            rec.output_file = Some(rel.display().to_string());
        }
    }
}

/// The cases the report will want to open: everything that is a finding.
fn worth_keeping(rec: &ReplayRecord) -> bool {
    let incorrect = rec.exit == Some(0) && rec.ast_equal == Some(false);
    let diverged = rec.matches_git_result == Some(false);
    let unsound = rec.parsable == Some(false) || rec.universal == Some(false);
    incorrect || diverged || unsound || rec.harness_error.is_some()
}

#[cfg(test)]
mod tests {
    use super::{Arm, Bucket, ReplayRecord, Subset, bucket_of, stratified};
    use crate::model::{Case, CaseKind, CaseShape, ResolutionKind, SCHEMA_VERSION, Version};

    fn version(file: &str) -> Version {
        Version {
            file: file.to_owned(),
            blob: "0".repeat(40),
            bytes: 1,
            lines: 1,
        }
    }

    fn case(repo: &str, id: &str, shape: CaseShape, kind: CaseKind) -> Case {
        Case {
            schema_version: SCHEMA_VERSION,
            case_id: id.to_owned(),
            kind,
            repo: repo.to_owned(),
            repo_url: String::new(),
            license: "Apache-2.0".to_owned(),
            merge_commit: "a".repeat(40),
            parents: ["b".repeat(40), "c".repeat(40)],
            base_commit: "d".repeat(40),
            path: "A.java".to_owned(),
            shape,
            resolution_kind: ResolutionKind::Merged,
            contaminated: false,
            contaminating_lines: 0,
            contaminating_sample: Vec::new(),
            base: Some(version("base.java")),
            ours: Some(version("ours.java")),
            theirs: Some(version("theirs.java")),
            resolved: Some(version("resolved.java")),
            git_result: None,
            conflict_messages: Vec::new(),
        }
    }

    #[test]
    fn buckets_split_the_populations_the_report_uses() {
        let full = case("r", "1", CaseShape::ModifyModify, CaseKind::Conflicted);
        assert_eq!(bucket_of(&full), Bucket::Gradeable);

        let mut dirty = full.clone();
        dirty.contaminated = true;
        assert_eq!(bucket_of(&dirty), Bucket::Contaminated);

        let mut rename = full.clone();
        rename.shape = CaseShape::AddNone;
        rename.base = None;
        rename.theirs = None;
        assert_eq!(bucket_of(&rename), Bucket::Rename);
        assert!(!Bucket::Rename.runnable());

        let mut add_add = full.clone();
        add_add.shape = CaseShape::AddAdd;
        add_add.base = None;
        assert_eq!(bucket_of(&add_add), Bucket::Incomplete);

        let mut no_resolution = full.clone();
        no_resolution.resolved = None;
        assert_eq!(bucket_of(&no_resolution), Bucket::Incomplete);

        let mut clean = full;
        clean.kind = CaseKind::Clean;
        assert_eq!(bucket_of(&clean), Bucket::Clean);
        assert!(Bucket::Clean.runnable());
    }

    #[test]
    fn subset_specs_parse_and_round_trip_through_describe() {
        for spec in [
            "all",
            "conflicted",
            "clean",
            "repo:apache/kafka",
            "sample:100:7",
        ] {
            let parsed = Subset::parse(spec).expect("parses");
            assert_eq!(parsed.describe(), spec);
        }
        assert_eq!(
            Subset::parse("sample:50").expect("parses"),
            Subset::Sample { n: 50, seed: 0 }
        );
        assert!(Subset::parse("nonsense").is_err());
        assert!(Subset::parse("sample:x").is_err());
        assert!(Subset::parse("sample:1:2:3").is_err());
    }

    fn pool(sizes: &[(&str, usize)]) -> Vec<super::Selected> {
        let mut out = Vec::new();
        for (repo, n) in sizes {
            for i in 0..*n {
                let c = case(
                    repo,
                    &format!("case{i:04}"),
                    CaseShape::ModifyModify,
                    CaseKind::Conflicted,
                );
                out.push(super::Selected {
                    dir: std::path::PathBuf::from("/nonexistent"),
                    bucket: bucket_of(&c),
                    case: c,
                });
            }
        }
        out
    }

    #[test]
    fn the_stratified_sample_is_proportional_deterministic_and_exact() {
        let cases = pool(&[("a", 600), ("b", 300), ("c", 100)]);
        let picked = stratified(cases.clone(), 100, 7);
        assert_eq!(picked.len(), 100);
        let count = |repo: &str| picked.iter().filter(|c| c.case.repo == repo).count();
        assert_eq!((count("a"), count("b"), count("c")), (60, 30, 10));

        let again = stratified(cases.clone(), 100, 7);
        assert_eq!(
            picked
                .iter()
                .map(|c| c.case.case_id.clone())
                .collect::<Vec<_>>(),
            again
                .iter()
                .map(|c| c.case.case_id.clone())
                .collect::<Vec<_>>(),
            "the sample must be reproducible from (n, seed)"
        );

        let other_seed = stratified(cases, 100, 8);
        assert_ne!(
            picked
                .iter()
                .map(|c| c.case.case_id.clone())
                .collect::<Vec<_>>(),
            other_seed
                .iter()
                .map(|c| c.case.case_id.clone())
                .collect::<Vec<_>>(),
            "a different seed must choose differently"
        );
    }

    #[test]
    fn a_sample_larger_than_the_pool_is_the_pool() {
        let cases = pool(&[("a", 5), ("b", 3)]);
        assert_eq!(stratified(cases, 100, 1).len(), 8);
    }

    #[test]
    fn largest_remainder_allocation_fills_every_seat() {
        // 3 repos, 10 seats, shares 3.33/3.33/3.33: the leftover seat must go
        // somewhere rather than being lost.
        let cases = pool(&[("a", 10), ("b", 10), ("c", 10)]);
        assert_eq!(stratified(cases, 10, 3).len(), 10);
    }

    #[test]
    fn the_record_round_trips_through_json() {
        let mut rec = super::blank(
            &super::Selected {
                dir: std::path::PathBuf::from("/x"),
                bucket: Bucket::Gradeable,
                case: case("r", "1", CaseShape::ModifyModify, CaseKind::Conflicted),
            },
            Arm::Sm,
        );
        rec.ran = true;
        rec.exit = Some(0);
        rec.ast_equal = Some(true);
        rec.parsable = Some(true);
        rec.universal = Some(true);
        rec.wall_ms = 12.5;
        let json = serde_json::to_string(&rec).expect("serialize");
        let back: ReplayRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.case_id, rec.case_id);
        assert_eq!(back.arm, Arm::Sm);
        assert_eq!(back.bucket, Bucket::Gradeable);
        assert_eq!(back.exit, Some(0));
        assert_eq!(back.ast_equal, Some(true));
        assert!((back.wall_ms - 12.5).abs() < f64::EPSILON);
    }
}
