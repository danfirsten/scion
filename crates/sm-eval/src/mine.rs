//! The corpus miner (SPEC §6.1, milestone M1).
//!
//! ## Procedure
//!
//! Per repository, one at a time (the clone is deleted before the next one, so
//! peak disk is one repository, not the whole list):
//!
//! 1. `git clone --bare --single-branch` into a scratch directory.
//! 2. `git log --merges --min-parents=2 --max-parents=2` for the merge commits.
//! 3. `git merge-base P1 P2` -> `B`. No base, or `B == P1`/`B == P2`, is
//!    excluded: a fast-forward has no three-way decision in it.
//! 4. `git merge-tree --write-tree --name-only -z P1 P2` replays the merge with
//!    merge-ort **in memory** — no worktree, no checkout, no index. This is the
//!    single decision that makes mining forty thousand merges affordable.
//! 5. Each conflicted `.java` path becomes a case: `base = B:path`,
//!    `ours = P1:path`, `theirs = P2:path`, `resolved = M:path`.
//! 6. Paths changed on *both* sides that git merged cleanly become clean cases,
//!    sampled deterministically. These are the regression and divergence
//!    denominators in SPEC §6.2 — without them "we never conflict" would score
//!    perfectly.
//! 7. The contamination filter flags, and does not delete.
//!
//! ## Idempotence
//!
//! A case directory that already contains a readable `case.json` is left alone
//! and counted. Re-running the miner over the same repository state therefore
//! writes the same bytes and produces the same counts.
//!
//! ## What `--max-cases-per-repo` does to the numbers
//!
//! Reaching the cap **stops the merge walk** for that repository rather than
//! merely declining to write more cases. That is what keeps a single very
//! merge-heavy project from consuming the whole run, but it means a capped
//! repository's `merges_walked` and `conflicted_java_files` describe only the
//! newest slice of its history, not all of it. The `case_limit_reached`
//! exclusion is present exactly when this happened, and the cap itself is
//! recorded in the manifest's `settings`, so a truncated denominator is always
//! visible rather than inferred. Repositories that never reach the cap are
//! walked in full.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::git::{MergeTreeResult, Repo, bare_clone};
use crate::model::{
    Case, CaseKind, CaseShape, Exclusion, Manifest, MineSettings, RepoReport, RepoStatus,
    ResolutionKind, SCHEMA_VERSION, Version,
};
use crate::util::{
    case_id, contamination, fnv1a64, line_count, looks_binary, repo_dir_name, repo_name_from_url,
};

/// One line of `repos.txt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSpec {
    pub name: String,
    pub url: String,
    pub license: String,
}

/// Parse `repos.txt`: `<clone-url> [<spdx-license>]`, `#` comments, blank lines
/// ignored. The license is recorded in every case so the committed sample can
/// be restricted to permissively licensed sources.
pub fn parse_repos(text: &str) -> Vec<RepoSpec> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = match raw.split_once('#') {
            Some((before, _)) => before,
            None => raw,
        }
        .trim();
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(url) = fields.next() else { continue };
        let license = fields.next().unwrap_or("UNKNOWN").to_string();
        out.push(RepoSpec {
            name: repo_name_from_url(url),
            url: url.to_string(),
            license,
        });
    }
    out
}

/// Everything that changes what lands in the corpus.
#[derive(Debug, Clone)]
pub struct MineOptions {
    pub out: PathBuf,
    pub scratch: PathBuf,
    /// 0 = unlimited.
    pub max_cases_per_repo: u64,
    /// Clean cases sampled per repository.
    pub clean_sample: u64,
    /// 0 = unlimited. Newest merges first when capped.
    pub max_merges_per_repo: u64,
    pub max_file_bytes: u64,
    pub clone_timeout: Duration,
    pub keep_clones: bool,
    /// Skip repositories already recorded as `ok` in the manifest.
    pub resume: bool,
    pub verbose: bool,
}

impl MineOptions {
    pub const DEFAULT_MAX_FILE_BYTES: u64 = 1024 * 1024;

    pub fn settings(&self) -> MineSettings {
        MineSettings {
            max_cases_per_repo: self.max_cases_per_repo,
            clean_sample: self.clean_sample,
            max_merges_per_repo: self.max_merges_per_repo,
            max_file_bytes: self.max_file_bytes,
        }
    }
}

/// Clone and mine every repository in `specs`, updating the manifest as it
/// goes so a run that is interrupted still leaves a usable corpus behind.
pub fn mine_all(specs: &[RepoSpec], opts: &MineOptions) -> Result<Manifest> {
    fs::create_dir_all(&opts.out)
        .with_context(|| format!("creating corpus directory {}", opts.out.display()))?;
    fs::create_dir_all(&opts.scratch)
        .with_context(|| format!("creating scratch directory {}", opts.scratch.display()))?;

    let git_version = crate::git::run_git_in(None, ["--version"])
        .map(|o| o.stdout_trimmed())
        .unwrap_or_else(|_| "unknown".into());

    let manifest_path = opts.out.join("manifest.json");
    let mut manifest = match load_manifest(&manifest_path) {
        Ok(Some(mut m)) => {
            m.settings = opts.settings();
            m.git_version.clone_from(&git_version);
            m
        }
        _ => Manifest::new(git_version, opts.settings()),
    };

    for (i, spec) in specs.iter().enumerate() {
        if opts.resume
            && manifest
                .repos
                .iter()
                .any(|r| r.name == spec.name && r.status == RepoStatus::Ok)
        {
            log(
                opts,
                &format!(
                    "[{}/{}] {} — skipped (resume)",
                    i + 1,
                    specs.len(),
                    spec.name
                ),
            );
            continue;
        }

        log(
            opts,
            &format!("[{}/{}] {} — cloning", i + 1, specs.len(), spec.name),
        );
        let started = Instant::now();
        let clone_dir = opts
            .scratch
            .join(format!("{}.git", repo_dir_name(&spec.name)));
        let _ = fs::remove_dir_all(&clone_dir);

        let report = match bare_clone(&spec.url, &clone_dir, opts.clone_timeout) {
            Err(e) => {
                log(opts, &format!("    clone failed: {e}"));
                RepoReport::failed(
                    &spec.name,
                    &spec.url,
                    &spec.license,
                    RepoStatus::CloneFailed,
                    e.to_string(),
                )
            }
            Ok(()) => {
                let repo = Repo::at(&clone_dir);
                match mine_repo(&repo, spec, opts) {
                    Ok(mut r) => {
                        r.duration_secs = started.elapsed().as_secs_f64();
                        log(
                            opts,
                            &format!(
                                "    {} merges walked, {} conflicted .java, {} cases ({} contaminated), {} clean, {:.0}s",
                                r.merges_walked,
                                r.conflicted_java_files,
                                r.conflicted_cases,
                                r.contaminated_cases,
                                r.clean_cases,
                                r.duration_secs
                            ),
                        );
                        r
                    }
                    Err(e) => {
                        log(opts, &format!("    mining failed: {e}"));
                        RepoReport::failed(
                            &spec.name,
                            &spec.url,
                            &spec.license,
                            RepoStatus::Error,
                            e.to_string(),
                        )
                    }
                }
            }
        };

        manifest.upsert(report);
        write_manifest(&manifest_path, &manifest)?;

        if !opts.keep_clones {
            let _ = fs::remove_dir_all(&clone_dir);
        }
    }

    Ok(manifest)
}

/// Mine one already-cloned bare repository. No network access.
pub fn mine_repo(repo: &Repo, spec: &RepoSpec, opts: &MineOptions) -> Result<RepoReport> {
    let mut report = RepoReport {
        name: spec.name.clone(),
        url: spec.url.clone(),
        license: spec.license.clone(),
        status: RepoStatus::Ok,
        error: None,
        head: repo.rev_parse("HEAD")?,
        merges_found: 0,
        merges_walked: 0,
        merges_with_conflicts: 0,
        conflicted_java_files: 0,
        conflicted_cases: 0,
        contaminated_cases: 0,
        clean_candidates: 0,
        clean_cases: 0,
        exclusions: BTreeMap::new(),
        duration_secs: 0.0,
    };

    let repo_dir = opts.out.join(repo_dir_name(&spec.name));
    let clean_dir = repo_dir.join("clean");

    let limit = (opts.max_merges_per_repo > 0).then_some(opts.max_merges_per_repo as usize);
    let merges = repo.two_parent_merges(limit)?;
    report.merges_found = merges.len() as u64;

    // Clean candidates are collected across the whole repository first, then
    // sampled, so the sample does not depend on where a run happened to stop.
    let mut clean_candidates: Vec<CleanCandidate> = Vec::new();

    for merge in &merges {
        if opts.max_cases_per_repo > 0 && report.conflicted_cases >= opts.max_cases_per_repo {
            bump(&mut report, Exclusion::CaseLimitReached);
            break;
        }

        let parents = repo.parents(merge)?;
        if parents.len() != 2 {
            continue;
        }
        let (p1, p2) = (parents[0].clone(), parents[1].clone());

        let Some(base) = repo.merge_base(&p1, &p2)? else {
            bump(&mut report, Exclusion::NoMergeBase);
            continue;
        };
        if base == p1 || base == p2 {
            bump(&mut report, Exclusion::MergeBaseIsParent);
            continue;
        }

        let merged = match repo.merge_tree(&p1, &p2)? {
            MergeTreeResult::Merged {
                tree,
                conflicts,
                messages,
            } => (tree, conflicts, messages),
            MergeTreeResult::Unusable { .. } => {
                bump(&mut report, Exclusion::MergeTreeFailed);
                continue;
            }
        };
        let (result_tree, conflicts, messages) = merged;
        report.merges_walked += 1;
        if !conflicts.is_empty() {
            report.merges_with_conflicts += 1;
        }

        // ---- conflicted cases -------------------------------------------
        for path in &conflicts {
            if !is_java(path) {
                bump(&mut report, Exclusion::NonJavaPath);
                continue;
            }
            report.conflicted_java_files += 1;
            if opts.max_cases_per_repo > 0 && report.conflicted_cases >= opts.max_cases_per_repo {
                bump(&mut report, Exclusion::CaseLimitReached);
                continue;
            }

            let msgs: Vec<String> = messages
                .iter()
                .filter(|m| m.paths.iter().any(|p| p == path))
                .map(|m| m.kind.clone())
                .collect();

            let ctx = CaseContext {
                spec,
                merge: merge.as_str(),
                p1: &p1,
                p2: &p2,
                base: &base,
                path,
                kind: CaseKind::Conflicted,
                git_result_tree: None,
                conflict_messages: msgs,
            };
            match extract_case(repo, &ctx, opts, &repo_dir)? {
                Extracted::Written { contaminated } => {
                    report.conflicted_cases += 1;
                    if contaminated {
                        report.contaminated_cases += 1;
                    }
                }
                Extracted::Excluded(reason) => bump(&mut report, reason),
            }
        }

        // ---- clean candidates -------------------------------------------
        if opts.clean_sample > 0 {
            let ours_changed = repo.changed_paths(&base, &p1, "*.java")?;
            if !ours_changed.is_empty() {
                let theirs_changed = repo.changed_paths(&base, &p2, "*.java")?;
                for path in ours_changed {
                    if !theirs_changed.contains(&path) {
                        continue;
                    }
                    if conflicts.contains(&path) {
                        continue;
                    }
                    report.clean_candidates += 1;
                    clean_candidates.push(CleanCandidate {
                        key: fnv1a64(format!("{merge}\0{path}").as_bytes()),
                        merge: merge.clone(),
                        p1: p1.clone(),
                        p2: p2.clone(),
                        base: base.clone(),
                        result_tree: result_tree.clone(),
                        path,
                    });
                }
            }
        }
    }

    // ---- deterministic clean sample --------------------------------------
    clean_candidates.sort_by(|a, b| {
        a.key
            .cmp(&b.key)
            .then_with(|| a.merge.cmp(&b.merge))
            .then_with(|| a.path.cmp(&b.path))
    });
    for cand in clean_candidates.iter().take(opts.clean_sample as usize) {
        let ctx = CaseContext {
            spec,
            merge: &cand.merge,
            p1: &cand.p1,
            p2: &cand.p2,
            base: &cand.base,
            path: &cand.path,
            kind: CaseKind::Clean,
            git_result_tree: Some(&cand.result_tree),
            conflict_messages: Vec::new(),
        };
        match extract_case(repo, &ctx, opts, &clean_dir)? {
            Extracted::Written { .. } => report.clean_cases += 1,
            Extracted::Excluded(reason) => bump(&mut report, reason),
        }
    }

    if report.merges_walked == 0 && report.conflicted_cases == 0 && report.clean_cases == 0 {
        // Not an error, but worth being able to see in the manifest.
        if repo
            .run(["ls-tree", "-r", "--name-only", "HEAD"])?
            .stdout_trimmed()
            .lines()
            .all(|l| !is_java(l))
        {
            report.status = RepoStatus::NoJava;
        }
    }

    Ok(report)
}

struct CleanCandidate {
    key: u64,
    merge: String,
    p1: String,
    p2: String,
    base: String,
    result_tree: String,
    path: String,
}

struct CaseContext<'a> {
    spec: &'a RepoSpec,
    merge: &'a str,
    p1: &'a str,
    p2: &'a str,
    base: &'a str,
    path: &'a str,
    kind: CaseKind,
    git_result_tree: Option<&'a str>,
    conflict_messages: Vec<String>,
}

enum Extracted {
    Written { contaminated: bool },
    Excluded(Exclusion),
}

/// A file version read out of the object database, plus where it came from.
struct Loaded {
    blob: String,
    bytes: Vec<u8>,
}

/// Read `<rev>:<path>`, applying the extraction limits.
fn load(
    repo: &Repo,
    rev: &str,
    path: &str,
    opts: &MineOptions,
) -> Result<std::result::Result<Option<Loaded>, Exclusion>> {
    let Some((mode, oid)) = repo.tree_entry(rev, path)? else {
        return Ok(Ok(None));
    };
    // 100644 / 100755 are the only regular-file modes. 120000 is a symlink and
    // 160000 a submodule gitlink; neither has mergeable Java in it.
    if !mode.starts_with("100") {
        return Ok(Err(Exclusion::NotARegularFile));
    }
    match repo.blob_size(&oid)? {
        Some(n) if n > opts.max_file_bytes => return Ok(Err(Exclusion::FileTooLarge)),
        Some(_) => {}
        None => return Ok(Err(Exclusion::BlobReadFailed)),
    }
    let Some(bytes) = repo.blob(&oid)? else {
        return Ok(Err(Exclusion::BlobReadFailed));
    };
    if looks_binary(&bytes) {
        return Ok(Err(Exclusion::BinaryContent));
    }
    Ok(Ok(Some(Loaded { blob: oid, bytes })))
}

macro_rules! load_or_exclude {
    ($repo:expr, $rev:expr, $path:expr, $opts:expr) => {
        match load($repo, $rev, $path, $opts)? {
            Ok(v) => v,
            Err(reason) => return Ok(Extracted::Excluded(reason)),
        }
    };
}

fn extract_case(
    repo: &Repo,
    ctx: &CaseContext<'_>,
    opts: &MineOptions,
    parent_dir: &Path,
) -> Result<Extracted> {
    let id = case_id(ctx.merge, ctx.path);
    let dir = parent_dir.join(&id);
    if dir.join("case.json").is_file() {
        // Idempotence: a case already on disk is a case, not a rewrite.
        let existing = fs::read(dir.join("case.json"))?;
        if let Ok(c) = serde_json::from_slice::<Case>(&existing) {
            return Ok(Extracted::Written {
                contaminated: c.contaminated,
            });
        }
    }

    let base = load_or_exclude!(repo, ctx.base, ctx.path, opts);
    let ours = load_or_exclude!(repo, ctx.p1, ctx.path, opts);
    let theirs = load_or_exclude!(repo, ctx.p2, ctx.path, opts);
    let resolved = load_or_exclude!(repo, ctx.merge, ctx.path, opts);
    let git_result = match ctx.git_result_tree {
        Some(tree) => load_or_exclude!(repo, tree, ctx.path, opts),
        None => None,
    };

    if ours.is_none() && theirs.is_none() {
        return Ok(Extracted::Excluded(Exclusion::NoContent));
    }

    let shape = match (base.is_some(), ours.is_some(), theirs.is_some()) {
        (true, true, true) => CaseShape::ModifyModify,
        (false, true, true) => CaseShape::AddAdd,
        (true, false, true) => CaseShape::DeleteModify,
        (true, true, false) => CaseShape::ModifyDelete,
        (true, false, false) => CaseShape::DeleteDelete,
        (false, _, _) => CaseShape::AddNone,
    };

    let resolution_kind = match &resolved {
        None => ResolutionKind::Absent,
        Some(r) => {
            if ours.as_ref().is_some_and(|o| o.bytes == r.bytes) {
                ResolutionKind::TookOurs
            } else if theirs.as_ref().is_some_and(|t| t.bytes == r.bytes) {
                ResolutionKind::TookTheirs
            } else {
                ResolutionKind::Merged
            }
        }
    };

    let contam = match &resolved {
        None => crate::util::Contamination {
            count: 0,
            sample: Vec::new(),
        },
        Some(r) => {
            let inputs: Vec<&[u8]> = [&base, &ours, &theirs]
                .iter()
                .filter_map(|v| v.as_ref().map(|l| l.bytes.as_slice()))
                .collect();
            contamination(&r.bytes, &inputs)
        }
    };

    fs::create_dir_all(&dir)
        .with_context(|| format!("creating case directory {}", dir.display()))?;

    let ext = extension_of(ctx.path);
    let write = |name: &str, l: &Option<Loaded>| -> Result<Option<Version>> {
        let Some(l) = l else { return Ok(None) };
        let file = format!("{name}.{ext}");
        fs::write(dir.join(&file), &l.bytes)
            .with_context(|| format!("writing {}", dir.join(&file).display()))?;
        Ok(Some(Version {
            file,
            blob: l.blob.clone(),
            bytes: l.bytes.len() as u64,
            lines: line_count(&l.bytes),
        }))
    };

    let case = Case {
        schema_version: SCHEMA_VERSION,
        case_id: id.clone(),
        kind: ctx.kind,
        repo: ctx.spec.name.clone(),
        repo_url: ctx.spec.url.clone(),
        license: ctx.spec.license.clone(),
        merge_commit: ctx.merge.to_string(),
        parents: [ctx.p1.to_string(), ctx.p2.to_string()],
        base_commit: ctx.base.to_string(),
        path: ctx.path.to_string(),
        shape,
        resolution_kind,
        contaminated: contam.contaminated(),
        contaminating_lines: contam.count,
        contaminating_sample: contam.sample,
        base: write("base", &base)?,
        ours: write("ours", &ours)?,
        theirs: write("theirs", &theirs)?,
        resolved: write("resolved", &resolved)?,
        git_result: write("git_result", &git_result)?,
        conflict_messages: ctx.conflict_messages.clone(),
    };

    let json = serde_json::to_string_pretty(&case)? + "\n";
    fs::write(dir.join("case.json"), json)
        .with_context(|| format!("writing {}", dir.join("case.json").display()))?;

    Ok(Extracted::Written {
        contaminated: case.contaminated,
    })
}

fn extension_of(path: &str) -> &str {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("txt")
}

fn is_java(path: &str) -> bool {
    path.ends_with(".java")
}

fn bump(report: &mut RepoReport, reason: Exclusion) {
    *report
        .exclusions
        .entry(reason.as_str().to_string())
        .or_insert(0) += 1;
}

fn log(opts: &MineOptions, msg: &str) {
    if opts.verbose {
        println!("{msg}");
    }
}

pub fn load_manifest(path: &Path) -> Result<Option<Manifest>> {
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let m =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(m))
}

pub fn write_manifest(path: &Path, manifest: &Manifest) -> Result<()> {
    let json = serde_json::to_string_pretty(manifest)? + "\n";
    fs::write(path, json).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repos_txt() {
        let text = "\
# a comment
https://github.com/apache/kafka.git Apache-2.0

https://github.com/eclipse-jetty/jetty.project.git EPL-2.0  # inline
https://github.com/x/y.git
";
        let specs = parse_repos(text);
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].name, "apache/kafka");
        assert_eq!(specs[0].license, "Apache-2.0");
        assert_eq!(specs[1].name, "eclipse-jetty/jetty.project");
        assert_eq!(specs[1].license, "EPL-2.0");
        assert_eq!(specs[2].license, "UNKNOWN");
    }

    #[test]
    fn recognises_java_paths() {
        assert!(is_java("a/B.java"));
        assert!(!is_java("a/B.kt"));
        assert!(!is_java("a/java"));
        assert_eq!(extension_of("a/B.java"), "java");
        assert_eq!(extension_of("Makefile"), "txt");
    }
}
