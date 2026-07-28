//! `sm-eval report` — turn a replay directory into the numbers (SPEC.md §6.2,
//! §6.3).
//!
//! # The denominators, stated once
//!
//! Every rate here names its denominator, because that is the only part of an
//! evaluation a reader cannot reconstruct and the part that makes two published
//! numbers incomparable (docs/prior-art.md §8.3.1).
//!
//! * **G — gradeable.** Conflicted cases with base, ours, theirs *and* the
//!   human resolution, not flagged by the contamination filter. SPEC.md §6.2's
//!   rates are conditioned on this. 16,238 cases in the mined corpus.
//! * **G∩L — gradeable and the line merge also conflicted.** The corpus's
//!   "git conflicted" label comes from `git merge-tree` (merge-ort) at mining
//!   time; the driver falls back to `git merge-file`. They disagree on real
//!   cases. On a case where our own line merge is clean, the driver's fast path
//!   returns git's answer unchanged, so counting it as a "resolve" would be
//!   scoring git's work as ours. Both denominators are reported.
//! * **C — clean.** Cases git merged without conflict. The regression and
//!   divergence denominator.
//! * Contaminated, rename and incomplete cases are reported apart and are in no
//!   rate.
//!
//! # Pooled versus per-repo mean
//!
//! Eleven repositories hit the miner's 1,500-case cap, so the corpus is
//! project-imbalanced and a pooled rate is really a rate over the big
//! repositories. Every headline number is therefore reported twice: **pooled**
//! (every case counts once) and **per-repo mean** (every repository with at
//! least `MIN_REPO_CASES` gradeable cases counts once). Where the two disagree,
//! the disagreement is the finding.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::replay::{Arm, Bucket, ReplayRecord, RunInfo};

/// Repositories with fewer gradeable cases than this are pooled but left out of
/// the unweighted per-repo mean, where a 1-of-1 repository would otherwise
/// contribute a 0% or 100% with the same weight as a 1,000-case one.
pub const MIN_REPO_CASES: usize = 20;

/// Load every record in a replay directory, in file order.
pub fn load(dir: &Path) -> Result<Vec<ReplayRecord>> {
    let path = dir.join("records.jsonl");
    let file = std::fs::File::open(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(
            serde_json::from_str(&line).with_context(|| {
                format!("{}:{}: parsing a replay record", path.display(), i + 1)
            })?,
        );
    }
    Ok(out)
}

pub fn load_run(dir: &Path) -> Option<RunInfo> {
    let text = std::fs::read_to_string(dir.join("run.json")).ok()?;
    serde_json::from_str(&text).ok()
}

// ------------------------------------------------------------------ counting

/// The counts one population of gradeable cases produces. Every field is a
/// count; every rate in the report is a ratio of two of them, computed at
/// render time so the arithmetic is visible.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    /// Cases in the denominator.
    pub total: usize,
    /// Of those, the driver produced a conflict-free result.
    pub resolved: usize,
    /// Of `resolved`, output is AST-equal to the human resolution.
    pub correct_ast: usize,
    /// Of `resolved`, output is byte-identical to the human resolution.
    pub correct_bytes: usize,
    /// Of `resolved`, output differs from the human resolution under the AST
    /// notion. **The number SPEC.md §6.2 targets at under 1%.**
    pub incorrect: usize,
    /// Of `correct_ast`'s complement, the ones that agree on the code and
    /// differ only in comments. A sub-line of `incorrect`, not a separate
    /// bucket.
    pub comment_only: usize,
    /// The driver conflicted too. A correct decline.
    pub declined: usize,
    /// The driver exited 2 without writing anything.
    pub errored: usize,
    /// Of `resolved`, the output parses.
    pub parsable: usize,
    /// Of `resolved`, the output is universal (see `compare`'s docs).
    pub universal: usize,
}

impl Counts {
    fn add(&mut self, rec: &ReplayRecord) {
        self.total += 1;
        match rec.exit {
            Some(0) => {
                self.resolved += 1;
                if rec.ast_equal == Some(true) {
                    self.correct_ast += 1;
                } else {
                    self.incorrect += 1;
                    if rec.ast_equal_ignoring_comments == Some(true) {
                        self.comment_only += 1;
                    }
                }
                if rec.byte_exact == Some(true) {
                    self.correct_bytes += 1;
                }
                if rec.parsable == Some(true) {
                    self.parsable += 1;
                }
                if rec.universal == Some(true) {
                    self.universal += 1;
                }
            }
            Some(1) => self.declined += 1,
            _ => self.errored += 1,
        }
    }

    #[must_use]
    pub fn resolve_rate(&self) -> f64 {
        rate(self.resolved, self.total)
    }
    /// Correct as a fraction of the resolved ones — SPEC.md's "of those".
    #[must_use]
    pub fn correct_of_resolved(&self) -> f64 {
        rate(self.correct_ast, self.resolved)
    }
    /// **The target metric.** Incorrect as a fraction of the resolved ones.
    #[must_use]
    pub fn incorrect_of_resolved(&self) -> f64 {
        rate(self.incorrect, self.resolved)
    }
    /// Incorrect as a fraction of everything gradeable — the risk per conflicted
    /// file, which is what a user experiences.
    #[must_use]
    pub fn incorrect_of_total(&self) -> f64 {
        rate(self.incorrect, self.total)
    }
    #[must_use]
    pub fn decline_rate(&self) -> f64 {
        rate(self.declined, self.total)
    }
}

fn rate(num: usize, den: usize) -> f64 {
    if den == 0 {
        0.0
    } else {
        num as f64 * 100.0 / den as f64
    }
}

/// Counts over the clean population: SPEC.md §6.2's regression and divergence.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanCounts {
    pub total: usize,
    /// The driver produced conflict markers where git produced none.
    pub regressions: usize,
    /// Both were clean and the bytes differ.
    pub divergences: usize,
    /// Both were clean and the bytes are identical.
    pub identical: usize,
    /// The driver exited 2.
    pub errored: usize,
    /// How many took the fast path, i.e. returned git's own answer.
    pub fast_path: usize,

    /// Of `total`, the ones where the driver's own line merge (`git merge-file`)
    /// was *also* clean.
    ///
    /// The corpus labels a case clean because `git merge-tree` — merge-ort —
    /// merged it at mining time. The driver's fallback is `git merge-file`, the
    /// classic three-way line merge, and the two disagree: ort has rename
    /// detection and a different hunk model. On a case where merge-file
    /// conflicts, "we conflicted where git did not" is a statement about ort,
    /// not about the tool the driver would otherwise have handed the user. Both
    /// readings are reported; see the crate docs.
    pub line_clean: usize,
    /// Regressions among `line_clean` — the reading of SPEC.md §6.2 that holds
    /// the line merge fixed. The fast path makes this **structurally** zero.
    pub regressions_line_clean: usize,
    /// Divergences among `line_clean`.
    pub divergences_line_clean: usize,
}

impl CleanCounts {
    fn add(&mut self, rec: &ReplayRecord) {
        self.total += 1;
        if rec.path_taken.as_deref() == Some("fast") {
            self.fast_path += 1;
        }
        let line_clean = rec.line_merge_conflicts == Some(0);
        if line_clean {
            self.line_clean += 1;
        }
        match rec.exit {
            Some(0) => match rec.matches_git_result {
                Some(true) => self.identical += 1,
                Some(false) => {
                    self.divergences += 1;
                    if line_clean {
                        self.divergences_line_clean += 1;
                    }
                }
                None => {}
            },
            Some(1) => {
                self.regressions += 1;
                if line_clean {
                    self.regressions_line_clean += 1;
                }
            }
            _ => self.errored += 1,
        }
    }

    #[must_use]
    pub fn regression_rate(&self) -> f64 {
        rate(self.regressions, self.total)
    }
    #[must_use]
    pub fn divergence_rate(&self) -> f64 {
        rate(self.divergences, self.total)
    }
    /// Regressions against the line merge the driver actually falls back to.
    #[must_use]
    pub fn regression_rate_line_clean(&self) -> f64 {
        rate(self.regressions_line_clean, self.line_clean)
    }
    #[must_use]
    pub fn divergence_rate_line_clean(&self) -> f64 {
        rate(self.divergences_line_clean, self.line_clean)
    }
}

/// Latency quantiles, in milliseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Latency {
    pub n: usize,
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
    pub max: f64,
    pub mean: f64,
}

/// Nearest-rank quantiles over an owned sample. Nearest-rank rather than
/// interpolated because a latency budget is a statement about an observation
/// that happened, not about an average of two.
#[must_use]
pub fn latency(mut samples: Vec<f64>) -> Latency {
    if samples.is_empty() {
        return Latency::default();
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pick = |q: f64| -> f64 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let rank = (q * samples.len() as f64).ceil() as usize;
        samples[rank.clamp(1, samples.len()) - 1]
    };
    Latency {
        n: samples.len(),
        p50: pick(0.50),
        p90: pick(0.90),
        p99: pick(0.99),
        max: *samples.last().expect("non-empty"),
        mean: samples.iter().sum::<f64>() / samples.len() as f64,
    }
}

/// Everything the report needs, computed once.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metrics {
    /// The `sm` arm over G.
    pub gradeable: Counts,
    /// The `sm` arm over G∩L — the line merge also conflicted.
    pub gradeable_line_conflicted: Counts,
    /// The `git merge-file` control arm over the same G∩L.
    pub control_line_conflicted: Counts,
    /// The `sm` arm over contaminated cases.
    pub contaminated: Counts,
    pub clean: CleanCounts,

    /// Gradeable cases where the driver's own line merge was clean, so the fast
    /// path returned git's answer. Not a resolve of ours.
    pub fast_path_on_gradeable: usize,
    /// Cases never replayed, by bucket.
    pub not_run: BTreeMap<String, usize>,
    /// Harness failures — the driver did not start, or wrote no record.
    pub harness_errors: usize,

    pub latency_all: Latency,
    pub latency_semantic: Latency,
    pub latency_fast: Latency,

    /// The gradeable population split by how the human resolved. The single
    /// most informative cut of the incorrect-resolution number, because a human
    /// who took one parent wholesale was answering a different question than a
    /// merge algorithm was.
    pub per_resolution_kind: BTreeMap<String, Counts>,

    pub per_repo: BTreeMap<String, Counts>,
    /// Unweighted mean over repositories with at least [`MIN_REPO_CASES`]
    /// gradeable cases.
    pub per_repo_mean_resolve: f64,
    pub per_repo_mean_correct: f64,
    pub per_repo_mean_incorrect: f64,
    pub per_repo_counted: usize,

    pub conflict_reasons: BTreeMap<String, usize>,
    pub path_taken: BTreeMap<String, usize>,
    pub fallback_reasons: BTreeMap<String, usize>,

    /// The semantic scan: clean merges checked, and findings by kind.
    pub semantic_checked: usize,
    pub semantic_with_findings: usize,
    pub semantic_findings: BTreeMap<String, usize>,
}

/// Fold a replay log into [`Metrics`].
#[must_use]
pub fn compute(records: &[ReplayRecord]) -> Metrics {
    let mut m = Metrics::default();
    let mut latencies = Vec::new();
    let mut semantic_lat = Vec::new();
    let mut fast_lat = Vec::new();

    for rec in records {
        if !rec.ran {
            // Once per case, not once per arm: a case that cannot be fed to a
            // three-way merge cannot be fed to either arm, and counting it
            // twice would make the populations table disagree with the corpus.
            if rec.arm == Arm::Sm {
                *m.not_run.entry(rec.bucket.as_str().to_owned()).or_default() += 1;
            }
            continue;
        }
        if rec.harness_error.is_some() {
            m.harness_errors += 1;
        }

        if rec.arm == Arm::Sm {
            latencies.push(rec.wall_ms);
            match rec.path_taken.as_deref() {
                Some("fast") => fast_lat.push(rec.wall_ms),
                Some("semantic") => semantic_lat.push(rec.wall_ms),
                _ => {}
            }
            if let Some(p) = &rec.path_taken {
                *m.path_taken.entry(p.clone()).or_default() += 1;
            }
            if let Some(f) = &rec.fallback_reason {
                *m.fallback_reasons.entry(f.clone()).or_default() += 1;
            }
            for reason in &rec.conflict_reasons {
                *m.conflict_reasons.entry(reason.clone()).or_default() += 1;
            }
            if let Some(check) = &rec.semantic_check {
                m.semantic_checked += 1;
                let kinds = check["kinds"].as_array().cloned().unwrap_or_default();
                if !kinds.is_empty() {
                    m.semantic_with_findings += 1;
                }
                for k in kinds {
                    if let Some(k) = k.as_str() {
                        *m.semantic_findings.entry(k.to_owned()).or_default() += 1;
                    }
                }
            }
        }

        // `line_merge_conflicts` is recorded on every arm because the driver
        // always runs the line merge first, so this condition means the same
        // thing on both.
        let line_conflicted = rec.line_merge_conflicts.is_none_or(|n| n > 0);

        match (rec.bucket, rec.arm) {
            (Bucket::Gradeable, Arm::Sm) => {
                m.gradeable.add(rec);
                m.per_repo.entry(rec.repo.clone()).or_default().add(rec);
                m.per_resolution_kind
                    .entry(rec.resolution_kind.clone())
                    .or_default()
                    .add(rec);
                if line_conflicted {
                    m.gradeable_line_conflicted.add(rec);
                } else {
                    m.fast_path_on_gradeable += 1;
                }
            }
            (Bucket::Gradeable, Arm::Line) => {
                if line_conflicted {
                    m.control_line_conflicted.add(rec);
                }
            }
            (Bucket::Contaminated, Arm::Sm) => m.contaminated.add(rec),
            (Bucket::Clean, Arm::Sm) => m.clean.add(rec),
            _ => {}
        }
    }

    m.latency_all = latency(latencies);
    m.latency_semantic = latency(semantic_lat);
    m.latency_fast = latency(fast_lat);

    let big: Vec<&Counts> = m
        .per_repo
        .values()
        .filter(|c| c.total >= MIN_REPO_CASES)
        .collect();
    m.per_repo_counted = big.len();
    if !big.is_empty() {
        let n = big.len() as f64;
        m.per_repo_mean_resolve = big.iter().map(|c| c.resolve_rate()).sum::<f64>() / n;
        m.per_repo_mean_correct = big.iter().map(|c| c.correct_of_resolved()).sum::<f64>() / n;
        m.per_repo_mean_incorrect = big.iter().map(|c| c.incorrect_of_resolved()).sum::<f64>() / n;
    }
    m
}

// ------------------------------------------------------------------ rendering

fn pct(v: f64) -> String {
    format!("{v:.2}%")
}

/// The headline table, as it appears at the top of the report and in the
/// README. One function so the two cannot drift.
#[must_use]
pub fn headline_table(m: &Metrics) -> String {
    let g = &m.gradeable;
    let l = &m.gradeable_line_conflicted;
    let c = &m.clean;
    let mut s = String::new();
    s.push_str("| Metric | Value | Denominator | SPEC §6.2 target |\n");
    s.push_str("|---|---:|---|---|\n");
    s.push_str(&format!(
        "| **Resolve rate** | {} | {} gradeable conflicted cases | maximize |\n",
        pct(g.resolve_rate()),
        g.total
    ));
    s.push_str(&format!(
        "| **Correct-resolve rate** (AST-equal) | {} | {} clean results | maximize |\n",
        pct(g.correct_of_resolved()),
        g.resolved
    ));
    s.push_str(&format!(
        "| **Correct-resolve rate** (byte-exact) | {} | {} clean results | — |\n",
        pct(rate(g.correct_bytes, g.resolved)),
        g.resolved
    ));
    s.push_str(&format!(
        "| **Incorrect-resolve rate** | **{}** | {} clean results | **< 1%** |\n",
        pct(g.incorrect_of_resolved()),
        g.resolved
    ));
    s.push_str(&format!(
        "| — of those, differing only in comments | {} | {} incorrect | — |\n",
        pct(rate(g.comment_only, g.incorrect)),
        g.incorrect
    ));
    s.push_str(&format!(
        "| Incorrect per conflicted file | {} | {} gradeable cases | — |\n",
        pct(g.incorrect_of_total()),
        g.total
    ));
    s.push_str(&format!(
        "| **Correct decline** | {} | {} gradeable cases | acceptable |\n",
        pct(g.decline_rate()),
        g.total
    ));
    s.push_str(&format!(
        "| **Regression rate** vs `git merge-file` | {} | {} clean cases the line merge also merged | ≈ 0 |\n",
        pct(c.regression_rate_line_clean()),
        c.line_clean
    ));
    s.push_str(&format!(
        "| Regression rate vs `git merge-tree` | {} | {} clean cases | — |\n",
        pct(c.regression_rate()),
        c.total
    ));
    s.push_str(&format!(
        "| **Divergence** vs `git merge-file` | {} | {} clean cases the line merge also merged | ≈ 0 — investigate every instance |\n",
        pct(c.divergence_rate_line_clean()),
        c.line_clean
    ));
    s.push_str(&format!(
        "| Divergence vs `git merge-tree` | {} | {} clean cases | — |\n",
        pct(c.divergence_rate()),
        c.total
    ));
    s.push_str(&format!(
        "| **Latency** p50 / p90 / p99 | {:.0} / {:.0} / {:.0} ms | {} invocations | p99 < 1000 ms |\n",
        m.latency_all.p50, m.latency_all.p90, m.latency_all.p99, m.latency_all.n
    ));
    s.push_str(&format!(
        "| Parsable (ASE 2025) | {} | {} clean results | — |\n",
        pct(rate(g.parsable, g.resolved)),
        g.resolved
    ));
    s.push_str(&format!(
        "| Universal (ASE 2025) | {} | {} clean results | — |\n",
        pct(rate(g.universal, g.resolved)),
        g.resolved
    ));
    s.push_str(&format!(
        "\nConditioned instead on the driver's own line merge also conflicting \
         ({} of {} gradeable cases): resolve {}, incorrect-resolve {}.\n",
        l.total,
        g.total,
        pct(l.resolve_rate()),
        pct(l.incorrect_of_resolved()),
    ));
    s
}

/// The per-repository table.
#[must_use]
pub fn per_repo_table(m: &Metrics) -> String {
    let mut s = String::new();
    s.push_str(
        "| repo | gradeable | resolved | resolve % | correct % | incorrect % | declined |\n",
    );
    s.push_str("|---|---:|---:|---:|---:|---:|---:|\n");
    for (repo, c) in &m.per_repo {
        s.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} | {} |\n",
            repo,
            c.total,
            c.resolved,
            pct(c.resolve_rate()),
            pct(c.correct_of_resolved()),
            pct(c.incorrect_of_resolved()),
            c.declined
        ));
    }
    s.push_str(&format!(
        "\nUnweighted mean over the {} repositories with at least {MIN_REPO_CASES} gradeable \
         cases: resolve {}, correct {}, incorrect {}. Pooled: resolve {}, correct {}, \
         incorrect {}.\n",
        m.per_repo_counted,
        pct(m.per_repo_mean_resolve),
        pct(m.per_repo_mean_correct),
        pct(m.per_repo_mean_incorrect),
        pct(m.gradeable.resolve_rate()),
        pct(m.gradeable.correct_of_resolved()),
        pct(m.gradeable.incorrect_of_resolved()),
    ));
    s
}

fn histogram(title: &str, counts: &BTreeMap<String, usize>, limit: usize) -> String {
    let mut rows: Vec<(&String, &usize)> = counts.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let total: usize = counts.values().sum();
    let mut s = format!("| {title} | count | share |\n|---|---:|---:|\n");
    for (name, n) in rows.iter().take(limit) {
        s.push_str(&format!(
            "| `{}` | {} | {} |\n",
            name,
            n,
            pct(rate(**n, total))
        ));
    }
    if rows.len() > limit {
        let rest: usize = rows.iter().skip(limit).map(|(_, n)| **n).sum();
        s.push_str(&format!(
            "| _{} more_ | {} | {} |\n",
            rows.len() - limit,
            rest,
            pct(rate(rest, total))
        ));
    }
    s
}

/// A case the gallery should show, with everything needed to write it up.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GalleryEntry {
    pub repo: String,
    pub case_id: String,
    pub path: String,
    pub merge_commit: String,
    pub diff_size: u64,
    pub comment_only: bool,
    pub conflict_reasons: Vec<String>,
    pub warnings: Vec<String>,
    pub output_file: Option<String>,
}

/// The incorrect resolutions, smallest difference first — the small ones are
/// the ones a human can actually diagnose, and a taxonomy is built from
/// diagnosed cases.
#[must_use]
pub fn gallery(records: &[ReplayRecord], limit: usize) -> Vec<GalleryEntry> {
    let mut out: Vec<GalleryEntry> = records
        .iter()
        .filter(|r| {
            r.arm == Arm::Sm
                && r.bucket == Bucket::Gradeable
                && r.exit == Some(0)
                && r.ast_equal == Some(false)
        })
        .map(|r| GalleryEntry {
            repo: r.repo.clone(),
            case_id: r.case_id.clone(),
            path: r.path.clone(),
            merge_commit: r.merge_commit.clone(),
            diff_size: r.diff_size.unwrap_or(u64::MAX),
            comment_only: r.ast_equal_ignoring_comments == Some(true),
            conflict_reasons: r.conflict_reasons.clone(),
            warnings: r.warnings.clone(),
            output_file: r.output_file.clone(),
        })
        .collect();
    out.sort_by(|a, b| {
        a.diff_size
            .cmp(&b.diff_size)
            .then_with(|| (&a.repo, &a.case_id).cmp(&(&b.repo, &b.case_id)))
    });
    out.truncate(limit);
    out
}

/// The full markdown report body, minus the hand-written sections.
#[must_use]
pub fn render(m: &Metrics, run: Option<&RunInfo>, records: &[ReplayRecord]) -> String {
    let mut s = String::new();
    s.push_str("## Headline\n\n");
    s.push_str(&headline_table(m));

    s.push_str("\n## Populations\n\n");
    s.push_str("| bucket | cases | replayed |\n|---|---:|---:|\n");
    let mut buckets: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for rec in records.iter().filter(|r| r.arm == Arm::Sm) {
        let e = buckets.entry(rec.bucket.as_str()).or_default();
        e.0 += 1;
        if rec.ran {
            e.1 += 1;
        }
    }
    for (name, (total, ran)) in &buckets {
        s.push_str(&format!("| `{name}` | {total} | {ran} |\n"));
    }

    s.push_str("\n## By how the human resolved\n\n");
    s.push_str(
        "| human resolution | gradeable | resolved | resolve % | incorrect % of resolved |\n",
    );
    s.push_str("|---|---:|---:|---:|---:|\n");
    for (kind, c) in &m.per_resolution_kind {
        s.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            kind,
            c.total,
            c.resolved,
            pct(c.resolve_rate()),
            pct(c.incorrect_of_resolved())
        ));
    }

    s.push_str("\n## Per repository\n\n");
    s.push_str(&per_repo_table(m));

    s.push_str("\n## Conflict reasons\n\n");
    s.push_str(&histogram("reason@kind", &m.conflict_reasons, 25));

    s.push_str("\n## Path taken\n\n");
    s.push_str(&histogram("path", &m.path_taken, 10));
    if !m.fallback_reasons.is_empty() {
        s.push_str("\n### Fallback reasons\n\n");
        s.push_str(&histogram("reason", &m.fallback_reasons, 10));
    }

    s.push_str("\n## Latency\n\n");
    s.push_str(
        "| population | n | p50 | p90 | p99 | max | mean |\n|---|---:|---:|---:|---:|---:|---:|\n",
    );
    for (name, l) in [
        ("all invocations", &m.latency_all),
        ("fast path", &m.latency_fast),
        ("semantic path", &m.latency_semantic),
    ] {
        s.push_str(&format!(
            "| {} | {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} |\n",
            name, l.n, l.p50, l.p90, l.p99, l.max, l.mean
        ));
    }
    s.push_str("\nMilliseconds, wall clock, including process start-up. ");
    s.push_str("Measured on the machine described under Limitations.\n");

    s.push_str("\n## Clean cases (regression and divergence)\n\n");
    let c = &m.clean;
    s.push_str(&format!(
        "| outcome | count | share |\n|---|---:|---:|\n\
         | identical to git's result | {} | {} |\n\
         | diverged (both clean, bytes differ) | {} | {} |\n\
         | regressed (we conflicted, git did not) | {} | {} |\n\
         | driver error | {} | {} |\n\
         | — of the above, took the fast path | {} | {} |\n\
         | — the driver\'s own line merge was also clean | {} | {} |\n\
         | — — regressions among those | {} | {} |\n\
         | — — divergences among those | {} | {} |\n",
        c.identical,
        pct(rate(c.identical, c.total)),
        c.divergences,
        pct(rate(c.divergences, c.total)),
        c.regressions,
        pct(rate(c.regressions, c.total)),
        c.errored,
        pct(rate(c.errored, c.total)),
        c.fast_path,
        pct(rate(c.fast_path, c.total)),
        c.line_clean,
        pct(rate(c.line_clean, c.total)),
        c.regressions_line_clean,
        pct(rate(c.regressions_line_clean, c.line_clean)),
        c.divergences_line_clean,
        pct(rate(c.divergences_line_clean, c.line_clean)),
    ));

    s.push_str("\n## Semantic check (M6) scan\n\n");
    s.push_str(&format!(
        "Clean semantic-path merges checked: **{}**. Merges with at least one finding: \
         **{}** ({}).\n\n",
        m.semantic_checked,
        m.semantic_with_findings,
        pct(rate(m.semantic_with_findings, m.semantic_checked))
    ));
    if !m.semantic_findings.is_empty() {
        s.push_str(&histogram("finding", &m.semantic_findings, 10));
    }

    if let Some(run) = run {
        s.push_str("\n## Provenance\n\n");
        s.push_str(&format!(
            "| | |\n|---|---|\n| binary | `{}` |\n| version | `{}` |\n| corpus | `{}` |\n\
             | subset | `{}` |\n| arms | {} |\n| jobs | {} |\n| semantic mode | `{}` |\n\
             | invocations | {} |\n| wall clock | {:.0} s |\n",
            run.sm_binary,
            run.sm_version,
            run.corpus,
            run.subset,
            run.arms.join(", "),
            run.jobs,
            run.semantic_mode,
            run.records_written,
            run.duration_secs
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::{CleanCounts, Counts, Metrics, compute, gallery, latency};
    use crate::replay::{Arm, Bucket, REPLAY_SCHEMA_VERSION, ReplayRecord};

    fn rec(bucket: Bucket, exit: Option<i32>) -> ReplayRecord {
        ReplayRecord {
            schema_version: REPLAY_SCHEMA_VERSION,
            case_id: "c".to_owned(),
            repo: "r/r".to_owned(),
            arm: Arm::Sm,
            bucket,
            kind: "conflicted".to_owned(),
            shape: "modify_modify".to_owned(),
            path: "A.java".to_owned(),
            merge_commit: "a".repeat(40),
            license: "Apache-2.0".to_owned(),
            contaminated: bucket == Bucket::Contaminated,
            resolution_kind: "merged".to_owned(),
            ran: true,
            not_run_reason: None,
            harness_error: None,
            exit,
            path_taken: Some("semantic".to_owned()),
            fallback_reason: None,
            conflicts: 0,
            conflict_reasons: Vec::new(),
            line_merge_conflicts: Some(1),
            line_merge_parsed_ok: None,
            warnings: Vec::new(),
            output_bytes: Some(10),
            synthesized_bytes: Some(0),
            synthesized_separators: Some(0),
            byte_exact: None,
            ast_equal: None,
            ast_equal_ignoring_comments: None,
            resolved_parsable: Some(true),
            diff_size: Some(0),
            parsable: None,
            universal: None,
            fabricated: Vec::new(),
            matches_git_result: None,
            wall_ms: 10.0,
            driver_ms: Some(8.0),
            semantic_check_ms: None,
            semantic_check: None,
            output_file: None,
        }
    }

    fn resolved(ast_equal: bool, byte_exact: bool) -> ReplayRecord {
        let mut r = rec(Bucket::Gradeable, Some(0));
        r.ast_equal = Some(ast_equal);
        r.ast_equal_ignoring_comments = Some(ast_equal);
        r.byte_exact = Some(byte_exact);
        r.parsable = Some(true);
        r.universal = Some(true);
        r
    }

    #[test]
    fn counts_partition_the_gradeable_population() {
        let mut c = Counts::default();
        c.add(&resolved(true, true));
        c.add(&resolved(true, false));
        c.add(&resolved(false, false));
        c.add(&rec(Bucket::Gradeable, Some(1)));
        c.add(&rec(Bucket::Gradeable, Some(2)));
        assert_eq!(c.total, 5);
        assert_eq!(c.resolved + c.declined + c.errored, c.total);
        assert_eq!(c.correct_ast + c.incorrect, c.resolved);
        assert_eq!(c.correct_bytes, 1);
        assert!((c.resolve_rate() - 60.0).abs() < 1e-9);
        assert!((c.correct_of_resolved() - 200.0 / 3.0).abs() < 1e-9);
        assert!((c.incorrect_of_resolved() - 100.0 / 3.0).abs() < 1e-9);
        assert!((c.incorrect_of_total() - 20.0).abs() < 1e-9);
        assert!((c.decline_rate() - 20.0).abs() < 1e-9);
    }

    #[test]
    fn a_comment_only_difference_is_still_incorrect_and_is_counted_apart() {
        let mut c = Counts::default();
        let mut r = resolved(false, false);
        r.ast_equal_ignoring_comments = Some(true);
        c.add(&r);
        assert_eq!(
            c.incorrect, 1,
            "comments count: this is not a correct resolve"
        );
        assert_eq!(c.comment_only, 1);
        assert_eq!(c.correct_ast, 0);
    }

    #[test]
    fn empty_denominators_are_zero_not_nan() {
        let c = Counts::default();
        assert!(c.resolve_rate() == 0.0 && c.correct_of_resolved() == 0.0);
        assert!(c.incorrect_of_resolved() == 0.0 && c.incorrect_of_total() == 0.0);
        assert_eq!(latency(Vec::new()).n, 0);
    }

    #[test]
    fn clean_counts_separate_regression_from_divergence() {
        let mut c = CleanCounts::default();
        let mut ok = rec(Bucket::Clean, Some(0));
        ok.matches_git_result = Some(true);
        ok.path_taken = Some("fast".to_owned());
        c.add(&ok);
        let mut diverged = rec(Bucket::Clean, Some(0));
        diverged.matches_git_result = Some(false);
        c.add(&diverged);
        c.add(&rec(Bucket::Clean, Some(1)));
        assert_eq!(
            (c.total, c.identical, c.divergences, c.regressions),
            (3, 1, 1, 1)
        );
        assert_eq!(c.fast_path, 1);
        assert!((c.regression_rate() - 100.0 / 3.0).abs() < 1e-9);
        assert!((c.divergence_rate() - 100.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn a_regression_only_counts_against_the_line_merge_that_was_also_clean() {
        // The corpus calls a case clean because merge-ort merged it. When the
        // driver's own `git merge-file` conflicts, our conflict is not a
        // regression against the tool we would otherwise have handed the user.
        let mut c = CleanCounts::default();
        let mut ort_only = rec(Bucket::Clean, Some(1));
        ort_only.line_merge_conflicts = Some(2);
        c.add(&ort_only);
        let mut both = rec(Bucket::Clean, Some(1));
        both.line_merge_conflicts = Some(0);
        c.add(&both);
        let mut ok = rec(Bucket::Clean, Some(0));
        ok.line_merge_conflicts = Some(0);
        ok.matches_git_result = Some(true);
        c.add(&ok);

        assert_eq!((c.total, c.line_clean), (3, 2));
        assert_eq!(c.regressions, 2);
        assert_eq!(c.regressions_line_clean, 1);
        assert!((c.regression_rate() - 200.0 / 3.0).abs() < 1e-9);
        assert!((c.regression_rate_line_clean() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn the_gradeable_population_is_also_split_by_how_the_human_resolved() {
        let mut took_ours = resolved(true, true);
        took_ours.resolution_kind = "took_ours".to_owned();
        let mut hand_merged = resolved(false, false);
        hand_merged.resolution_kind = "merged".to_owned();
        let m = compute(&[took_ours, hand_merged]);
        assert_eq!(m.per_resolution_kind["took_ours"].correct_ast, 1);
        assert_eq!(m.per_resolution_kind["merged"].incorrect, 1);
        assert_eq!(
            m.per_resolution_kind
                .values()
                .map(|c| c.total)
                .sum::<usize>(),
            m.gradeable.total
        );
    }

    #[test]
    fn nearest_rank_quantiles() {
        let l = latency((1..=100).map(f64::from).collect());
        assert!((l.p50 - 50.0).abs() < 1e-9);
        assert!((l.p90 - 90.0).abs() < 1e-9);
        assert!((l.p99 - 99.0).abs() < 1e-9);
        assert!((l.max - 100.0).abs() < 1e-9);
        // A single sample is its own every quantile.
        let one = latency(vec![7.0]);
        assert!((one.p50 - 7.0).abs() < 1e-9 && (one.p99 - 7.0).abs() < 1e-9);
    }

    #[test]
    fn compute_separates_the_arms_and_the_buckets() {
        let mut records = vec![resolved(true, true), resolved(false, false)];
        records.push(rec(Bucket::Contaminated, Some(0)));
        let mut clean = rec(Bucket::Clean, Some(0));
        clean.matches_git_result = Some(true);
        records.push(clean);
        // The control arm must not land in the `sm` numbers.
        let mut control = resolved(false, false);
        control.arm = Arm::Line;
        records.push(control);
        // A case the fast path handled: the line merge was clean, so it is not
        // one of ours to claim.
        let mut fastpath = resolved(true, true);
        fastpath.line_merge_conflicts = Some(0);
        records.push(fastpath);

        let m: Metrics = compute(&records);
        assert_eq!(m.gradeable.total, 3);
        assert_eq!(m.gradeable_line_conflicted.total, 2);
        assert_eq!(m.fast_path_on_gradeable, 1);
        assert_eq!(m.control_line_conflicted.total, 1);
        assert_eq!(m.contaminated.total, 1);
        assert_eq!(m.clean.total, 1);
        assert_eq!(m.per_repo["r/r"].total, 3);
    }

    #[test]
    fn not_run_cases_are_counted_and_excluded_from_every_rate() {
        let mut skipped = rec(Bucket::Rename, None);
        skipped.ran = false;
        let records = vec![skipped, resolved(true, true)];
        let m = compute(&records);
        assert_eq!(m.not_run["rename"], 1);
        assert_eq!(m.gradeable.total, 1);
        assert_eq!(m.latency_all.n, 1, "a case that never ran has no latency");
    }

    #[test]
    fn the_gallery_holds_only_incorrect_resolves_and_is_ordered_by_difference() {
        let mut big = resolved(false, false);
        big.diff_size = Some(40);
        big.case_id = "big".to_owned();
        let mut small = resolved(false, false);
        small.diff_size = Some(2);
        small.case_id = "small".to_owned();
        let records = vec![
            big,
            small,
            resolved(true, true),
            rec(Bucket::Gradeable, Some(1)),
        ];
        let g = gallery(&records, 10);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].case_id, "small");
        assert_eq!(g[1].case_id, "big");
    }

    #[test]
    fn the_headline_table_is_rendered_from_the_metrics_it_is_given() {
        let m = compute(&[resolved(true, true), resolved(false, false)]);
        let table = super::headline_table(&m);
        assert!(table.contains("| **Resolve rate** | 100.00% |"), "{table}");
        assert!(
            table.contains("| **Incorrect-resolve rate** | **50.00%** |"),
            "{table}"
        );
    }
}
