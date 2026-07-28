//! `sm-eval sample` — carve a small, committable slice out of the corpus.
//!
//! The corpus itself is gitignored (it is hundreds of megabytes and most of it
//! is other people's code under a mix of licences). But later milestones need
//! *some* real cases in the repository: M2 wants pairs to match, M4 wants merge
//! fixtures, and the miner's own tests want a case whose fields are known.
//!
//! So this picks a handful under three rules:
//!
//! * **Permissive licences only** — Apache-2.0, EPL-2.0, MIT, BSD. Each
//!   `case.json` already records the repository, the merge sha and the licence,
//!   so provenance travels with the file.
//! * **Small** — a fixture nobody can read is not a fixture.
//! * **Spread** — at most a couple per repository, so the sample does not
//!   inherit one project's house style.
//!
//! Selection is deterministic (FNV of the case id), so re-running it on the
//! same corpus reproduces the same sample.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::model::{Case, CaseKind, CaseShape};
use crate::util::fnv1a64;

/// Licences we are willing to vendor a few files from.
pub const PERMISSIVE: [&str; 6] = [
    "Apache-2.0",
    "EPL-2.0",
    "EPL-1.0",
    "MIT",
    "BSD-3-Clause",
    "BSD-2-Clause",
];

#[derive(Debug, Clone)]
pub struct SampleOptions {
    pub count: usize,
    pub max_per_repo: usize,
    /// Largest any single version may be, in bytes.
    pub max_version_bytes: u64,
}

impl Default for SampleOptions {
    fn default() -> Self {
        Self {
            count: 15,
            max_per_repo: 2,
            max_version_bytes: 24 * 1024,
        }
    }
}

/// Copy the selected cases into `out`, replacing whatever was there.
pub fn build_sample(corpus: &Path, out: &Path, opts: &SampleOptions) -> Result<Vec<Case>> {
    let mut candidates: Vec<(u64, std::path::PathBuf, Case)> = Vec::new();
    let mut repo_dirs: Vec<_> = fs::read_dir(corpus)
        .with_context(|| format!("reading {}", corpus.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    repo_dirs.sort();

    for repo_dir in repo_dirs {
        let mut case_dirs: Vec<_> = fs::read_dir(&repo_dir)
            .with_context(|| format!("reading {}", repo_dir.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        case_dirs.sort();
        for case_dir in case_dirs {
            let case_json = case_dir.join("case.json");
            if !case_json.is_file() {
                continue; // `clean/` — never sampled.
            }
            let bytes = fs::read(&case_json)?;
            let Ok(case) = serde_json::from_slice::<Case>(&bytes) else {
                continue;
            };
            if !is_eligible(&case, opts) {
                continue;
            }
            candidates.push((fnv1a64(case.case_id.as_bytes()), case_dir, case));
        }
    }

    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.case_id.cmp(&b.2.case_id)));

    let mut per_repo: BTreeMap<String, usize> = BTreeMap::new();
    let mut chosen = Vec::new();
    // Two passes: the first honours the per-repository cap, the second fills
    // any shortfall so a small corpus still produces the requested count.
    for pass in 0..2 {
        for (_, dir, case) in &candidates {
            if chosen.len() >= opts.count {
                break;
            }
            if chosen
                .iter()
                .any(|(_, c): &(_, Case)| c.case_id == case.case_id)
            {
                continue;
            }
            let n = per_repo.entry(case.repo.clone()).or_insert(0);
            if pass == 0 && *n >= opts.max_per_repo {
                continue;
            }
            *n += 1;
            chosen.push((dir.clone(), case.clone()));
        }
    }

    if chosen.is_empty() {
        bail!("no eligible cases found in {}", corpus.display());
    }

    if out.exists() {
        fs::remove_dir_all(out).with_context(|| format!("clearing {}", out.display()))?;
    }
    fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;

    let mut written = Vec::new();
    for (dir, case) in &chosen {
        let dest = out.join(&case.case_id);
        fs::create_dir_all(&dest)?;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.metadata()?.is_file() {
                fs::copy(entry.path(), dest.join(entry.file_name()))?;
            }
        }
        written.push(case.clone());
    }
    written.sort_by(|a, b| a.case_id.cmp(&b.case_id));

    let index = serde_json::to_string_pretty(&written)? + "\n";
    fs::write(out.join("index.json"), index)?;
    fs::write(out.join("README.md"), readme(&written))?;
    Ok(written)
}

/// Provenance for the vendored files, written next to them.
///
/// This is generated rather than hand-maintained because the sample is
/// regenerated by wiping and rewriting the directory: a hand-written note would
/// be deleted on the first `sm-eval sample` re-run, and a stale attribution
/// list is worse than none.
fn readme(cases: &[Case]) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    s.push_str(
        "# Sample corpus\n\
         \n\
         **Generated — do not edit by hand.** Reproduce with:\n\
         \n\
         ```\n\
         sm-eval sample --corpus corpus --out crates/sm-eval/tests/sample-corpus\n\
         ```\n\
         \n\
         Fifteen real three-way merge conflicts mined from public Java history\n\
         (SPEC §6.1, milestone M1). The full corpus is gitignored; this slice is\n\
         committed so that later milestones have real Java to work against\n\
         without re-mining gigabytes of history. `crates/sm-eval/tests/sample_corpus.rs`\n\
         asserts its shape.\n\
         \n\
         Each directory holds `base.java`, `ours.java`, `theirs.java`,\n\
         `resolved.java` (the human's committed resolution) and a `case.json`\n\
         recording the repository, the merge commit, both parents, the merge\n\
         base and the licence. `index.json` is every `case.json` in one file.\n\
         \n\
         ## Provenance\n\
         \n\
         These files are unmodified excerpts of the upstream projects, taken at\n\
         the commits named below, and are redistributed under each project's own\n\
         licence. Only permissively licensed projects are sampled.\n\
         \n",
    );
    let _ = writeln!(s, "| case | project | licence | merge commit | path |");
    let _ = writeln!(s, "|---|---|---|---|---|");
    for c in cases {
        let _ = writeln!(
            s,
            "| `{}` | [{}]({}) | {} | `{}` | `{}` |",
            c.case_id,
            c.repo,
            c.repo_url.trim_end_matches(".git"),
            c.license,
            &c.merge_commit[..12],
            c.path,
        );
    }
    s
}

fn is_eligible(case: &Case, opts: &SampleOptions) -> bool {
    if case.kind != CaseKind::Conflicted || case.contaminated {
        return false;
    }
    if !PERMISSIVE.contains(&case.license.as_str()) {
        return false;
    }
    // A fixture needs all four versions or it cannot exercise a three-way merge
    // plus its ground truth.
    if case.shape != CaseShape::ModifyModify {
        return false;
    }
    let versions = [&case.base, &case.ours, &case.theirs, &case.resolved];
    if versions.iter().any(|v| v.is_none()) {
        return false;
    }
    versions
        .iter()
        .filter_map(|v| v.as_ref())
        .all(|v| v.bytes > 0 && v.bytes <= opts.max_version_bytes)
}
