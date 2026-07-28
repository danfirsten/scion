//! `sm-eval stats` — the summary table SPEC M1 asks to be committed.
//!
//! Counts come from **two** sources and that is deliberate:
//!
//! * The cases are counted by walking the corpus directory and reading every
//!   `case.json`. This is the honest number — it is what a later milestone will
//!   actually replay.
//! * The merges walked and the exclusion histogram come from `manifest.json`,
//!   because a merge that produced nothing leaves no trace on disk.
//!
//! If the two disagree about a repository (say a case directory was deleted by
//! hand), the disk wins and the manifest number is still shown, so the
//! discrepancy is visible rather than reconciled away.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::mine::load_manifest;
use crate::model::{Case, CaseKind, Exclusion, RepoStatus};

#[derive(Debug, Default, Clone)]
pub struct RepoCounts {
    pub conflicted: u64,
    pub contaminated: u64,
    /// Conflicted cases that have base, ours *and* theirs, plus a human
    /// resolution — i.e. everything a three-way merge needs and everything
    /// grading it needs. This, minus the contaminated ones, is the denominator
    /// M5's resolve rate is conditioned on.
    pub replayable: u64,
    /// `replayable` and not contaminated.
    pub gradeable: u64,
    pub clean: u64,
    pub bytes: u64,
}

impl RepoCounts {
    pub fn usable(&self) -> u64 {
        self.conflicted.saturating_sub(self.contaminated)
    }
}

#[derive(Debug, Default)]
pub struct CorpusStats {
    pub per_repo: BTreeMap<String, RepoCounts>,
    pub shapes: BTreeMap<String, u64>,
    pub resolutions: BTreeMap<String, u64>,
    pub licenses: BTreeMap<String, u64>,
    pub total_bytes: u64,
    pub case_dirs: u64,
}

/// Walk the corpus and read every `case.json`.
pub fn scan(corpus: &Path) -> Result<CorpusStats> {
    let mut stats = CorpusStats::default();
    let entries = fs::read_dir(corpus)
        .with_context(|| format!("reading corpus directory {}", corpus.display()))?;
    let mut repo_dirs: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.path())
        .collect();
    repo_dirs.sort();

    for repo_dir in repo_dirs {
        let repo_key = repo_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .replace("__", "/");
        let counts = stats.per_repo.entry(repo_key).or_default();
        let mut pending = Vec::new();
        collect_cases(&repo_dir, &mut pending)?;
        for (case, bytes) in pending {
            stats.case_dirs += 1;
            stats.total_bytes += bytes;
            counts.bytes += bytes;
            match case.kind {
                CaseKind::Conflicted => {
                    counts.conflicted += 1;
                    if case.contaminated {
                        counts.contaminated += 1;
                    }
                    let complete = case.base.is_some()
                        && case.ours.is_some()
                        && case.theirs.is_some()
                        && case.resolved.is_some();
                    if complete {
                        counts.replayable += 1;
                        if !case.contaminated {
                            counts.gradeable += 1;
                        }
                    }
                }
                CaseKind::Clean => counts.clean += 1,
            }
            *stats
                .shapes
                .entry(case.shape.as_str().to_string())
                .or_insert(0) += 1;
            *stats
                .resolutions
                .entry(case.resolution_kind.as_str().to_string())
                .or_insert(0) += 1;
            *stats.licenses.entry(case.license.clone()).or_insert(0) += 1;
        }
    }
    Ok(stats)
}

fn collect_cases(dir: &Path, out: &mut Vec<(Case, u64)>) -> Result<()> {
    let mut children: Vec<_> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    children.sort();
    for child in children {
        if !child.is_dir() {
            continue;
        }
        let case_json = child.join("case.json");
        if case_json.is_file() {
            let bytes =
                fs::read(&case_json).with_context(|| format!("reading {}", case_json.display()))?;
            match serde_json::from_slice::<Case>(&bytes) {
                Ok(case) => {
                    let size = dir_size(&child)?;
                    out.push((case, size));
                }
                Err(e) => {
                    eprintln!("warning: {} is not a valid case: {e}", case_json.display());
                }
            }
        } else {
            // `clean/` and any future grouping directory.
            collect_cases(&child, out)?;
        }
    }
    Ok(())
}

fn dir_size(dir: &Path) -> Result<u64> {
    let mut total = 0;
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            total += meta.len();
        }
    }
    Ok(total)
}

/// The prose half of the summary.
///
/// It lives here rather than in a hand-written `docs/` file so that the
/// committed document stays regenerable: `sm-eval stats --corpus corpus --out
/// docs/corpus-summary.md` reproduces the whole thing, method section and all,
/// and the method can never drift away from the code that implements it.
const METHOD: &str = r#"## What is in here, and how it was obtained

Per repository, one at a time, with the clone deleted before the next one:

1. `git clone --bare --single-branch` — full history of the default branch.
2. `git log --merges --min-parents=2 --max-parents=2` for the merge commits `M`
   with parents `P1` (ours) and `P2` (theirs).
3. `git merge-base P1 P2` -> `B`. A merge with no base, or whose base *is* one
   of its parents, is excluded: that is a fast-forward recorded with `--no-ff`
   and it contains no three-way decision.
4. `git merge-tree --write-tree --name-only -z P1 P2` re-runs the merge with
   merge-ort **in memory** — no worktree, no checkout, no index — and reports
   the paths git conflicts on *today*. This is what makes walking tens of
   thousands of merges affordable, and it is also the honest question to ask,
   because SPEC §6.2's resolve rate is conditioned on git conflicting.
5. Every conflicted `.java` path becomes a case: `base = B:path`,
   `ours = P1:path`, `theirs = P2:path`, `resolved = M:path` — the last being
   the human's committed resolution.
6. Paths that changed on **both** sides relative to `B` and that git merged
   without conflict become *clean* cases, sampled deterministically (FNV-1a of
   merge sha and path) up to `clean_sample` per repository. These are the
   denominators for SPEC §6.2's regression and divergence rates; without them a
   tool that never merges anything would score perfectly.

### The contamination filter

SPEC §6.1 requires that cases where the human did unrelated work during the
merge be identified. A case is flagged `contaminated` when the resolution
contains at least one line, compared with trailing whitespace trimmed, that
appears in none of base, ours or theirs.

Flagged cases are **kept, not deleted**. M5 needs to be able to state its
denominator honestly, and "the human also changed something while merging" is a
finding about real merge history rather than a reason to make the corpus look
tidier. Each case records the count of such lines and a sample of them, so a
one-line copyright bump can be told apart from a resolution that was
substantially rewritten.

### Case shapes, and which cases are actually replayable

Not every conflict is modify/modify. Add/add, delete/modify, modify/delete and
"deleted during the resolution" all occur and are all recorded with a `shape`
and whichever of the four versions exist, rather than being quietly skipped.

Two consequences matter for M5:

* **`add_none` is, in practice, a rename conflict.** `git merge-tree` names the
  conflicted path as it exists on *one* side; when the other side moved the file
  the path resolves to nothing in base and nothing on that side, so the case is
  recorded with `ours` (or `theirs`) and the resolution only. The miner does not
  chase the counterpart path — following `git diff -M` rename detection would be
  a genuine improvement and is deliberately left for later.
* **A case with fewer than three inputs cannot be fed to a three-way merge**,
  and a case with no resolution cannot be graded against ground truth. The
  headline therefore reports a `gradeable` figure — conflicted, non-contaminated,
  and carrying base, ours, theirs *and* the human resolution. **That is the
  number SPEC §6.2's resolve rate should be conditioned on**, not the raw case
  count. The other cases are kept because delete/modify and rename conflicts are
  real merge situations that M4 has to survive; they simply belong in a
  different bucket.

### Limits

Any version larger than `max_file_bytes`, anything containing a NUL byte, and
anything that is a symlink or a submodule gitlink is excluded and counted.
Reaching `max_cases_per_repo` stops that repository's walk, which is what the
`case_limit_reached` exclusion marks; those repositories' `merges walked` and
`conflicted .java` columns therefore describe the newest slice of their history
rather than all of it.

### Honesty note

The human resolution is ground truth for *what was committed*, not for *what
was correct* (SPEC §6.2). It is the best available oracle and it is not a
perfect one.

"#;

/// Render the committed summary (`docs/corpus-summary.md`).
pub fn render_markdown(corpus: &Path, stats: &CorpusStats) -> Result<String> {
    use std::fmt::Write as _;

    let manifest = load_manifest(&corpus.join("manifest.json"))?;
    let mut s = String::new();

    writeln!(s, "# Corpus summary")?;
    writeln!(s)?;
    if let Some(m) = &manifest {
        writeln!(
            s,
            "Generated by `sm-eval stats --corpus {}`. Schema version {}, {}, {}.",
            corpus.display(),
            m.schema_version,
            m.tool,
            m.git_version
        )?;
        writeln!(s)?;
        writeln!(
            s,
            "Settings: `max_cases_per_repo = {}`, `clean_sample = {}`, `max_merges_per_repo = {}`, `max_file_bytes = {}`. Zero means unlimited.",
            m.settings.max_cases_per_repo,
            m.settings.clean_sample,
            m.settings.max_merges_per_repo,
            m.settings.max_file_bytes
        )?;
        writeln!(s)?;
    }

    s.push_str(METHOD);

    // ---- headline ------------------------------------------------------
    let repos_with_cases = stats.per_repo.values().filter(|c| c.conflicted > 0).count();
    let conflicted: u64 = stats.per_repo.values().map(|c| c.conflicted).sum();
    let contaminated: u64 = stats.per_repo.values().map(|c| c.contaminated).sum();
    let clean: u64 = stats.per_repo.values().map(|c| c.clean).sum();

    writeln!(s, "## Headline")?;
    writeln!(s)?;
    writeln!(s, "| | |")?;
    writeln!(s, "|---|---:|")?;
    writeln!(
        s,
        "| Repositories with at least one case | {repos_with_cases} |"
    )?;
    writeln!(s, "| Conflicted `.java` cases | {conflicted} |")?;
    writeln!(
        s,
        "| — of those, contaminated (flagged, kept) | {contaminated} ({:.1}%) |",
        pct(contaminated, conflicted)
    )?;
    writeln!(
        s,
        "| — **non-contaminated conflicted cases** | **{}** |",
        conflicted.saturating_sub(contaminated)
    )?;
    let replayable: u64 = stats.per_repo.values().map(|c| c.replayable).sum();
    let gradeable: u64 = stats.per_repo.values().map(|c| c.gradeable).sum();
    writeln!(
        s,
        "| — with all of base, ours, theirs *and* a resolution | {replayable} |"
    )?;
    writeln!(
        s,
        "| — **replayable and non-contaminated (M5's denominator)** | **{gradeable}** |"
    )?;
    writeln!(
        s,
        "| Clean cases (both sides changed, git merged) | {clean} |"
    )?;
    writeln!(s, "| Case directories on disk | {} |", stats.case_dirs)?;
    writeln!(
        s,
        "| Corpus size on disk | {} |",
        human_bytes(stats.total_bytes)
    )?;
    if let Some(m) = &manifest {
        let walked: u64 = m.repos.iter().map(|r| r.merges_walked).sum();
        let found: u64 = m.repos.iter().map(|r| r.merges_found).sum();
        let conflicting: u64 = m.repos.iter().map(|r| r.merges_with_conflicts).sum();
        let secs: f64 = m.repos.iter().map(|r| r.duration_secs).sum();
        // A merge is either replayed, excluded by one of the two merge-base
        // rules, or never reached because the repository hit its case cap and
        // the walk stopped. Spelling the third one out keeps `found` and
        // `walked` from looking like an unexplained shortfall.
        let merge_level_exclusions: u64 = m
            .repos
            .iter()
            .flat_map(|r| {
                [Exclusion::NoMergeBase, Exclusion::MergeBaseIsParent]
                    .into_iter()
                    .filter_map(|e| r.exclusions.get(e.as_str()).copied())
            })
            .sum();
        let unreached = found
            .saturating_sub(walked)
            .saturating_sub(merge_level_exclusions);
        writeln!(s, "| Two-parent merge commits found | {found} |")?;
        writeln!(s, "| Merges replayed with `git merge-tree` | {walked} |")?;
        writeln!(s, "| — of those, producing ≥1 conflict | {conflicting} |")?;
        writeln!(
            s,
            "| Merges excluded as fast-forwards or without a merge base | {merge_level_exclusions} |"
        )?;
        writeln!(
            s,
            "| Merges never reached (walk stopped at `max_cases_per_repo`) | {unreached} |"
        )?;
        writeln!(s, "| Repositories attempted | {} |", m.repos.len())?;
        writeln!(s, "| Total mining wall clock | {} |", human_secs(secs))?;
    }
    writeln!(s)?;

    // ---- per repo ------------------------------------------------------
    writeln!(s, "## Per repository")?;
    writeln!(s)?;
    writeln!(
        s,
        "| repo | license | status | merges found | merges walked | conflicted .java | cases | contaminated | usable | gradeable | clean sampled | size |"
    )?;
    writeln!(
        s,
        "|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|"
    )?;

    let empty = RepoCounts::default();
    match &manifest {
        Some(m) => {
            for r in &m.repos {
                let c = stats.per_repo.get(&r.name).unwrap_or(&empty);
                writeln!(
                    s,
                    "| `{}` | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                    r.name,
                    r.license,
                    status_str(r.status),
                    r.merges_found,
                    r.merges_walked,
                    r.conflicted_java_files,
                    c.conflicted,
                    c.contaminated,
                    c.usable(),
                    c.gradeable,
                    c.clean,
                    human_bytes(c.bytes),
                )?;
            }
        }
        None => {
            for (name, c) in &stats.per_repo {
                writeln!(
                    s,
                    "| `{name}` | — | — | — | — | — | {} | {} | {} | {} | {} | {} |",
                    c.conflicted,
                    c.contaminated,
                    c.usable(),
                    c.gradeable,
                    c.clean,
                    human_bytes(c.bytes),
                )?;
            }
        }
    }
    writeln!(s)?;

    // ---- exclusions ----------------------------------------------------
    writeln!(s, "## Exclusions")?;
    writeln!(s)?;
    writeln!(
        s,
        "Every candidate that did not become a case, and why. Nothing is dropped silently."
    )?;
    writeln!(s)?;
    if let Some(m) = &manifest {
        let mut totals: BTreeMap<&str, u64> = BTreeMap::new();
        for r in &m.repos {
            for (reason, n) in &r.exclusions {
                *totals.entry(reason.as_str()).or_insert(0) += n;
            }
        }
        writeln!(s, "| reason | count | meaning |")?;
        writeln!(s, "|---|---:|---|")?;
        for reason in Exclusion::ALL {
            let n = totals.get(reason.as_str()).copied().unwrap_or(0);
            writeln!(
                s,
                "| `{}` | {n} | {} |",
                reason.as_str(),
                exclusion_meaning(reason)
            )?;
        }
        for (reason, n) in &totals {
            if !Exclusion::ALL.iter().any(|e| e.as_str() == *reason) {
                writeln!(s, "| `{reason}` | {n} | (unknown to this build) |")?;
            }
        }
    } else {
        writeln!(
            s,
            "_No manifest found; exclusions are not recoverable from disk._"
        )?;
    }
    writeln!(s)?;

    // ---- distributions -------------------------------------------------
    writeln!(s, "## Case shapes")?;
    writeln!(s)?;
    writeln!(s, "| shape | count |")?;
    writeln!(s, "|---|---:|")?;
    for (k, v) in &stats.shapes {
        writeln!(s, "| `{k}` | {v} |")?;
    }
    writeln!(s)?;

    writeln!(s, "## How the human resolved")?;
    writeln!(s)?;
    writeln!(s, "| resolution | count |")?;
    writeln!(s, "|---|---:|")?;
    for (k, v) in &stats.resolutions {
        writeln!(s, "| `{k}` | {v} |")?;
    }
    writeln!(s)?;

    writeln!(s, "## Licenses")?;
    writeln!(s)?;
    writeln!(s, "| license | cases |")?;
    writeln!(s, "|---|---:|")?;
    for (k, v) in &stats.licenses {
        writeln!(s, "| {k} | {v} |")?;
    }
    writeln!(s)?;

    if let Some(m) = &manifest {
        let failures: Vec<_> = m
            .repos
            .iter()
            .filter(|r| r.status != RepoStatus::Ok)
            .collect();
        if !failures.is_empty() {
            writeln!(s, "## Repositories that produced nothing")?;
            writeln!(s)?;
            writeln!(s, "| repo | status | detail |")?;
            writeln!(s, "|---|---|---|")?;
            for r in failures {
                let detail = r
                    .error
                    .as_deref()
                    .unwrap_or("—")
                    .replace('\n', " ")
                    .replace('|', "\\|");
                let detail: String = detail.chars().take(160).collect();
                writeln!(
                    s,
                    "| `{}` | {} | {} |",
                    r.name,
                    status_str(r.status),
                    detail
                )?;
            }
            writeln!(s)?;
        }
    }

    Ok(s)
}

fn exclusion_meaning(e: Exclusion) -> &'static str {
    match e {
        Exclusion::NoMergeBase => "the two parents share no common ancestor",
        Exclusion::MergeBaseIsParent => {
            "the merge base is one of the parents — a fast-forward, no three-way decision"
        }
        Exclusion::MergeTreeFailed => "`git merge-tree` could not replay the merge",
        Exclusion::NonJavaPath => "a conflicted path that is not a `.java` file",
        Exclusion::FileTooLarge => "some version exceeds the 1 MiB extraction limit",
        Exclusion::BinaryContent => "some version contains a NUL byte",
        Exclusion::NotARegularFile => "the path is a symlink or a submodule gitlink",
        Exclusion::NoContent => "the file is absent from both sides",
        Exclusion::BlobReadFailed => "git listed a blob it then could not produce",
        Exclusion::CaseLimitReached => "`--max-cases-per-repo` was already met",
    }
}

fn status_str(s: RepoStatus) -> &'static str {
    match s {
        RepoStatus::Ok => "ok",
        RepoStatus::CloneFailed => "clone failed",
        RepoStatus::NoJava => "no java",
        RepoStatus::Error => "error",
    }
}

fn pct(n: u64, d: u64) -> f64 {
    if d == 0 {
        0.0
    } else {
        100.0 * n as f64 / d as f64
    }
}

pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

pub fn human_secs(secs: f64) -> String {
    let total = secs.round() as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sizes_and_durations() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(1024 * 1024 * 3 / 2), "1.5 MiB");
        assert_eq!(human_secs(5.0), "5s");
        assert_eq!(human_secs(65.0), "1m 5s");
        assert_eq!(human_secs(3725.0), "1h 2m 5s");
    }

    #[test]
    fn percentages_do_not_divide_by_zero() {
        assert_eq!(pct(0, 0), 0.0);
        assert!((pct(1, 4) - 25.0).abs() < 1e-9);
    }
}
