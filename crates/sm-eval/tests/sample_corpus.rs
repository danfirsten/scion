//! Guards for the small committed slice of the corpus.
//!
//! `tests/sample-corpus/` holds a handful of real conflicted cases, produced by
//! `sm-eval sample` from the full (gitignored) corpus. They exist so that M2's
//! matcher, M4's merge and M5's metrics all have real Java to work against
//! without anyone needing to re-mine gigabytes of history first.
//!
//! These tests assert the properties later milestones will *rely* on, so that a
//! careless regeneration of the sample cannot quietly break them:
//!
//! * every case directory is self-describing (`case.json` parses at the current
//!   schema version),
//! * provenance is complete — repository, merge sha and licence — because these
//!   files are redistributed inside this repository and the sample is
//!   restricted to permissive licences,
//! * every version named in `case.json` exists on disk with exactly the
//!   recorded byte length,
//! * the cases are genuine three-way conflicts and none of them is
//!   contaminated,
//! * and `index.json` agrees with the directories.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sm_eval::model::{Case, CaseKind, CaseShape, SCHEMA_VERSION};
use sm_eval::sample::PERMISSIVE;

fn sample_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/sample-corpus")
}

fn cases() -> Vec<(PathBuf, Case)> {
    let dir = sample_dir();
    let mut out = Vec::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("no sample corpus at {}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    entries.sort();
    for case_dir in entries {
        let json = case_dir.join("case.json");
        let bytes = std::fs::read(&json)
            .unwrap_or_else(|e| panic!("{} is missing case.json: {e}", case_dir.display()));
        let case: Case = serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("{} does not parse: {e}", json.display()));
        out.push((case_dir, case));
    }
    out
}

#[test]
fn the_sample_is_the_documented_size() {
    let cases = cases();
    assert!(
        cases.len() >= 15,
        "the sample should hold at least 15 cases, found {}",
        cases.len()
    );
}

#[test]
fn every_case_is_self_describing_and_permissively_licensed() {
    for (dir, case) in cases() {
        let at = dir.display();
        assert_eq!(case.schema_version, SCHEMA_VERSION, "{at}: schema version");
        assert_eq!(
            case.case_id,
            dir.file_name().unwrap().to_string_lossy(),
            "{at}: directory name and case id disagree"
        );
        assert_eq!(case.kind, CaseKind::Conflicted, "{at}");
        assert!(
            !case.contaminated,
            "{at}: contaminated cases are not sampled"
        );

        // Provenance. These files live in *our* repository, so who wrote them,
        // where they came from and under what terms all have to travel with
        // them.
        assert!(!case.repo.is_empty(), "{at}: no repository");
        assert!(case.repo_url.starts_with("https://"), "{at}: no clone url");
        assert_eq!(case.merge_commit.len(), 40, "{at}: merge sha");
        assert_eq!(case.base_commit.len(), 40, "{at}: base sha");
        assert!(case.parents.iter().all(|p| p.len() == 40), "{at}: parents");
        assert!(
            PERMISSIVE.contains(&case.license.as_str()),
            "{at}: {} is not a licence we may vendor from",
            case.license
        );
        assert!(case.path.ends_with(".java"), "{at}: {}", case.path);
    }
}

#[test]
fn every_recorded_version_is_on_disk_at_the_recorded_size() {
    for (dir, case) in cases() {
        let versions = [
            ("base", &case.base),
            ("ours", &case.ours),
            ("theirs", &case.theirs),
            ("resolved", &case.resolved),
        ];
        for (name, version) in versions {
            let v = version
                .as_ref()
                .unwrap_or_else(|| panic!("{}: no {name}", dir.display()));
            let path = dir.join(&v.file);
            let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(
                bytes.len() as u64,
                v.bytes,
                "{}: byte length",
                path.display()
            );
            assert_eq!(
                sm_eval::util::line_count(&bytes),
                v.lines,
                "{}: line count",
                path.display()
            );
            assert!(
                !sm_eval::util::looks_binary(&bytes),
                "{}: binary content in a Java fixture",
                path.display()
            );
        }
        // A conflicted case must not carry git's own conflict-marked output.
        assert!(case.git_result.is_none(), "{}", dir.display());
    }
}

#[test]
fn every_case_is_a_real_three_way_conflict() {
    for (dir, case) in cases() {
        let at = dir.display();
        assert_eq!(case.shape, CaseShape::ModifyModify, "{at}");
        let base = std::fs::read(dir.join("base.java")).unwrap();
        let ours = std::fs::read(dir.join("ours.java")).unwrap();
        let theirs = std::fs::read(dir.join("theirs.java")).unwrap();
        assert_ne!(base, ours, "{at}: ours did not change");
        assert_ne!(base, theirs, "{at}: theirs did not change");
        assert_ne!(ours, theirs, "{at}: the two sides are identical");
        // No conflict markers: these are the *inputs*, not git's output.
        for name in ["base.java", "ours.java", "theirs.java", "resolved.java"] {
            let text = std::fs::read_to_string(dir.join(name)).unwrap();
            assert!(
                !text.contains("<<<<<<<"),
                "{at}/{name} contains conflict markers"
            );
        }
    }
}

#[test]
fn the_contamination_flag_still_holds() {
    // Re-run the filter rather than trusting the recorded answer: this is the
    // one number in the corpus that an honest denominator depends on.
    for (dir, case) in cases() {
        let base = std::fs::read(dir.join("base.java")).unwrap();
        let ours = std::fs::read(dir.join("ours.java")).unwrap();
        let theirs = std::fs::read(dir.join("theirs.java")).unwrap();
        let resolved = std::fs::read(dir.join("resolved.java")).unwrap();
        let c = sm_eval::util::contamination(&resolved, &[&base, &ours, &theirs]);
        assert_eq!(
            c.count,
            case.contaminating_lines,
            "{}: recomputed contamination disagrees with case.json",
            dir.display()
        );
        assert!(!c.contaminated(), "{}", dir.display());
    }
}

#[test]
fn the_index_matches_the_directories() {
    let index_path = sample_dir().join("index.json");
    let bytes = std::fs::read(&index_path).unwrap();
    let index: Vec<Case> = serde_json::from_slice(&bytes).unwrap();

    let on_disk: BTreeSet<String> = cases().into_iter().map(|(_, c)| c.case_id).collect();
    let indexed: BTreeSet<String> = index.iter().map(|c| c.case_id.clone()).collect();
    assert_eq!(on_disk, indexed);
}

#[test]
fn the_sample_spans_several_repositories() {
    let repos: BTreeSet<String> = cases().into_iter().map(|(_, c)| c.repo).collect();
    assert!(
        repos.len() >= 4,
        "the sample should not inherit one project's house style, got {repos:?}"
    );
}
