//! End-to-end test of the miner against a git repository built here, in a
//! tempdir, with no network access.
//!
//! The point is to pin the *whole* pipeline — merge-base, the `git merge-tree`
//! replay, conflicted-path detection, four-version extraction, the case shapes,
//! the contamination filter and idempotence — against a repository whose
//! history we wrote ourselves and therefore know the right answers for.
//!
//! History built by [`synthetic_repo`]:
//!
//! ```text
//!            o---o  side    (Conflicted.java, Clean.java, Added.java, Doomed.java deleted)
//!           /     \
//!   base---o-------M
//!           \     /
//!            o---o  main    (Conflicted.java, Clean.java, Added.java, Doomed.java edited)
//! ```
//!
//! * `Conflicted.java` — both sides edit the same line. git conflicts. The
//!   merge commit resolves it by hand **and adds one line that appears in no
//!   input**, which is exactly what the contamination filter exists to catch.
//! * `Clean.java` — both sides edit *different* methods. git merges cleanly, so
//!   it is a clean case, not a conflicted one.
//! * `Added.java` — added on both sides with different content: add/add.
//! * `Doomed.java` — deleted on one side and edited on the other:
//!   delete/modify.
//! * `notes.txt` — conflicted too, and must be excluded as a non-Java path.

use std::path::Path;
use std::process::Command;

use sm_eval::git::Repo;
use sm_eval::mine::{MineOptions, RepoSpec, mine_repo};
use sm_eval::model::{Case, CaseKind, CaseShape, ResolutionKind};

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00+0000")
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00+0000")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", dir)
        .output()
        .expect("git should be on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write(dir: &Path, path: &str, contents: &str) {
    let full = dir.join(path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(full, contents).unwrap();
}

const CONFLICTED_BASE: &str = "\
class Conflicted {
    int value() {
        return 1;
    }
}
";

const CLEAN_BASE: &str = "\
class Clean {
    int left() {
        return 1;
    }

    int right() {
        return 2;
    }
}
";

const DOOMED_BASE: &str = "\
class Doomed {
    void go() {}
}
";

/// Build the repository described in the module docs and return its path.
fn synthetic_repo(dir: &Path) -> String {
    git(dir, &["init", "--quiet", "--initial-branch=main", "."]);
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "user.email", "test@example.invalid"]);
    // Keep the replay independent of the ambient merge configuration.
    git(dir, &["config", "merge.conflictStyle", "merge"]);

    write(dir, "src/Conflicted.java", CONFLICTED_BASE);
    write(dir, "src/Clean.java", CLEAN_BASE);
    write(dir, "src/Doomed.java", DOOMED_BASE);
    write(dir, "notes.txt", "one\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "--quiet", "-m", "base"]);

    // ---- ours (main) ----------------------------------------------------
    write(
        dir,
        "src/Conflicted.java",
        &CONFLICTED_BASE.replace("return 1;", "return 10;"),
    );
    write(
        dir,
        "src/Clean.java",
        &CLEAN_BASE.replace("return 1;", "return 11;"),
    );
    write(
        dir,
        "src/Doomed.java",
        &DOOMED_BASE.replace(
            "void go() {}",
            "void go() { System.out.println(\"ours\"); }",
        ),
    );
    write(
        dir,
        "src/Added.java",
        "class Added { int ours() { return 1; } }\n",
    );
    write(dir, "notes.txt", "one\nours\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "--quiet", "-m", "ours"]);

    // ---- theirs (side) --------------------------------------------------
    git(dir, &["checkout", "--quiet", "-b", "side", "HEAD~1"]);
    write(
        dir,
        "src/Conflicted.java",
        &CONFLICTED_BASE.replace("return 1;", "return 20;"),
    );
    write(
        dir,
        "src/Clean.java",
        &CLEAN_BASE.replace("return 2;", "return 22;"),
    );
    git(dir, &["rm", "--quiet", "src/Doomed.java"]);
    write(
        dir,
        "src/Added.java",
        "class Added { int theirs() { return 2; } }\n",
    );
    write(dir, "notes.txt", "one\ntheirs\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "--quiet", "-m", "theirs"]);

    // ---- the merge, resolved by hand ------------------------------------
    git(dir, &["checkout", "--quiet", "main"]);
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["merge", "--no-commit", "--no-ff", "side"])
        .env("HOME", dir)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "the synthetic merge was supposed to conflict"
    );

    // The human takes ours' number, keeps theirs' formatting, and — the point
    // of this fixture — writes one line that exists in no input.
    write(
        dir,
        "src/Conflicted.java",
        "\
class Conflicted {
    int value() {
        // reconciled during the merge
        return 10;
    }
}
",
    );
    write(
        dir,
        "src/Added.java",
        "class Added { int ours() { return 1; } }\n",
    );
    write(dir, "notes.txt", "one\nours\ntheirs\n");
    write(
        dir,
        "src/Doomed.java",
        &DOOMED_BASE.replace(
            "void go() {}",
            "void go() { System.out.println(\"ours\"); }",
        ),
    );
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "--quiet", "-m", "merge side into main"]);

    String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string()
}

struct Mined {
    corpus: tempfile::TempDir,
    report: sm_eval::model::RepoReport,
    merge_sha: String,
}

fn mine(clean_sample: u64) -> Mined {
    let work = tempfile::tempdir().unwrap();
    let merge_sha = synthetic_repo(work.path());

    let corpus = tempfile::tempdir().unwrap();
    let opts = MineOptions {
        out: corpus.path().to_path_buf(),
        scratch: corpus.path().join("scratch"),
        max_cases_per_repo: 0,
        clean_sample,
        max_merges_per_repo: 0,
        max_file_bytes: MineOptions::DEFAULT_MAX_FILE_BYTES,
        clone_timeout: std::time::Duration::from_secs(60),
        keep_clones: false,
        resume: false,
        verbose: false,
    };
    let spec = RepoSpec {
        name: "synthetic/repo".into(),
        url: "https://example.invalid/synthetic/repo.git".into(),
        license: "Apache-2.0".into(),
    };
    let repo = Repo::at(work.path());
    let report = mine_repo(&repo, &spec, &opts).expect("mining the synthetic repo");
    Mined {
        corpus,
        report,
        merge_sha,
    }
}

fn load_case(corpus: &Path, sub: &str, merge_sha: &str, path: &str) -> Case {
    let id = sm_eval::util::case_id(merge_sha, path);
    let dir = corpus.join("synthetic__repo").join(sub).join(&id);
    let bytes = std::fs::read(dir.join("case.json"))
        .unwrap_or_else(|e| panic!("no case.json at {}: {e}", dir.display()));
    serde_json::from_slice(&bytes).unwrap()
}

fn read_version(corpus: &Path, sub: &str, merge_sha: &str, path: &str, file: &str) -> String {
    let id = sm_eval::util::case_id(merge_sha, path);
    let dir = corpus.join("synthetic__repo").join(sub).join(&id);
    std::fs::read_to_string(dir.join(file)).unwrap()
}

#[test]
fn extracts_the_expected_counts() {
    let m = mine(100);
    let r = &m.report;

    assert_eq!(r.merges_found, 1);
    assert_eq!(r.merges_walked, 1);
    assert_eq!(r.merges_with_conflicts, 1);
    // Conflicted.java, Added.java and Doomed.java conflict; Clean.java does not.
    assert_eq!(r.conflicted_java_files, 3, "conflicted .java paths");
    assert_eq!(r.conflicted_cases, 3);
    assert_eq!(
        r.contaminated_cases, 1,
        "only Conflicted.java is contaminated"
    );
    // notes.txt conflicts too and must be excluded, loudly.
    assert_eq!(r.exclusions.get("non_java_path").copied(), Some(1));
    assert_eq!(r.clean_candidates, 1, "Clean.java changed on both sides");
    assert_eq!(r.clean_cases, 1);
    assert_eq!(r.status, sm_eval::model::RepoStatus::Ok);
}

#[test]
fn flags_the_contaminated_resolution() {
    let m = mine(0);
    let case = load_case(m.corpus.path(), "", &m.merge_sha, "src/Conflicted.java");

    assert_eq!(case.kind, CaseKind::Conflicted);
    assert_eq!(case.shape, CaseShape::ModifyModify);
    assert_eq!(case.repo, "synthetic/repo");
    assert_eq!(case.license, "Apache-2.0");
    assert_eq!(case.merge_commit, m.merge_sha);
    assert_eq!(case.path, "src/Conflicted.java");

    assert!(case.contaminated, "the merge added a line from nowhere");
    assert_eq!(case.contaminating_lines, 1);
    assert_eq!(
        case.contaminating_sample,
        vec!["        // reconciled during the merge".to_string()]
    );

    // The resolution took ours' value but is not byte-identical to it.
    assert_eq!(case.resolution_kind, ResolutionKind::Merged);

    // All four versions on disk, with the right contents.
    let base = read_version(
        m.corpus.path(),
        "",
        &m.merge_sha,
        "src/Conflicted.java",
        "base.java",
    );
    let ours = read_version(
        m.corpus.path(),
        "",
        &m.merge_sha,
        "src/Conflicted.java",
        "ours.java",
    );
    let theirs = read_version(
        m.corpus.path(),
        "",
        &m.merge_sha,
        "src/Conflicted.java",
        "theirs.java",
    );
    let resolved = read_version(
        m.corpus.path(),
        "",
        &m.merge_sha,
        "src/Conflicted.java",
        "resolved.java",
    );
    assert!(base.contains("return 1;"));
    assert!(ours.contains("return 10;"));
    assert!(theirs.contains("return 20;"));
    assert!(resolved.contains("return 10;"));
    assert!(resolved.contains("// reconciled during the merge"));

    // A conflicted case has no git_result: git's answer there is a file full of
    // conflict markers, which is not an input to anything.
    assert!(case.git_result.is_none());
    assert_eq!(case.base.as_ref().unwrap().lines, 5);
}

#[test]
fn records_add_add_and_delete_modify_shapes() {
    let m = mine(0);

    let added = load_case(m.corpus.path(), "", &m.merge_sha, "src/Added.java");
    assert_eq!(added.shape, CaseShape::AddAdd);
    assert!(added.base.is_none());
    assert!(added.ours.is_some() && added.theirs.is_some());
    // The human kept ours verbatim.
    assert_eq!(added.resolution_kind, ResolutionKind::TookOurs);
    assert!(!added.contaminated);

    let doomed = load_case(m.corpus.path(), "", &m.merge_sha, "src/Doomed.java");
    // `ours` is P1 = main, which edited the file; `theirs` = side, which deleted it.
    assert_eq!(doomed.shape, CaseShape::ModifyDelete);
    assert!(doomed.theirs.is_none());
    assert_eq!(doomed.resolution_kind, ResolutionKind::TookOurs);
    assert!(
        doomed
            .conflict_messages
            .iter()
            .any(|m| m.contains("modify/delete")),
        "merge-tree should have said modify/delete, got {:?}",
        doomed.conflict_messages
    );
}

#[test]
fn records_the_clean_case_with_gits_own_result() {
    let m = mine(100);
    let case = load_case(m.corpus.path(), "clean", &m.merge_sha, "src/Clean.java");

    assert_eq!(case.kind, CaseKind::Clean);
    assert_eq!(case.shape, CaseShape::ModifyModify);
    let git_result = case
        .git_result
        .as_ref()
        .expect("clean cases carry git's result");
    assert_eq!(git_result.file, "git_result.java");

    let merged = read_version(
        m.corpus.path(),
        "clean",
        &m.merge_sha,
        "src/Clean.java",
        "git_result.java",
    );
    assert!(merged.contains("return 11;"), "ours' edit survived");
    assert!(merged.contains("return 22;"), "theirs' edit survived");
    assert!(!merged.contains("<<<<"), "clean means clean");
}

#[test]
fn clean_sample_zero_disables_clean_mining() {
    // clean_sample = 0 disables clean mining entirely.
    let m = mine(0);
    assert_eq!(m.report.clean_candidates, 0);
    assert_eq!(m.report.clean_cases, 0);
    assert!(
        !m.corpus
            .path()
            .join("synthetic__repo")
            .join("clean")
            .exists()
    );
}

#[test]
fn mining_twice_is_idempotent() {
    let work = tempfile::tempdir().unwrap();
    let merge_sha = synthetic_repo(work.path());
    let corpus = tempfile::tempdir().unwrap();
    let opts = MineOptions {
        out: corpus.path().to_path_buf(),
        scratch: corpus.path().join("scratch"),
        max_cases_per_repo: 0,
        clean_sample: 100,
        max_merges_per_repo: 0,
        max_file_bytes: MineOptions::DEFAULT_MAX_FILE_BYTES,
        clone_timeout: std::time::Duration::from_secs(60),
        keep_clones: false,
        resume: false,
        verbose: false,
    };
    let spec = RepoSpec {
        name: "synthetic/repo".into(),
        url: "https://example.invalid/synthetic/repo.git".into(),
        license: "Apache-2.0".into(),
    };
    let repo = Repo::at(work.path());

    let first = mine_repo(&repo, &spec, &opts).unwrap();
    let snapshot = snapshot_tree(corpus.path());
    let second = mine_repo(&repo, &spec, &opts).unwrap();

    assert_eq!(first.conflicted_cases, second.conflicted_cases);
    assert_eq!(first.contaminated_cases, second.contaminated_cases);
    assert_eq!(first.clean_cases, second.clean_cases);
    assert_eq!(first.exclusions, second.exclusions);
    assert_eq!(
        snapshot,
        snapshot_tree(corpus.path()),
        "a second run rewrote the corpus"
    );

    // And the case ids are what the documented scheme says they are.
    let id = sm_eval::util::case_id(&merge_sha, "src/Conflicted.java");
    assert!(id.starts_with(&merge_sha[..12]));
    assert!(id.ends_with("src_Conflicted.java"));
}

/// Every file under `root`, with its contents, so two runs can be compared.
fn snapshot_tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().display().to_string();
                out.push((rel, std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

#[test]
fn a_repo_with_no_java_is_reported_as_such() {
    let work = tempfile::tempdir().unwrap();
    let dir = work.path();
    git(dir, &["init", "--quiet", "--initial-branch=main", "."]);
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "user.email", "test@example.invalid"]);
    write(dir, "README.md", "hello\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "--quiet", "-m", "only prose"]);

    let corpus = tempfile::tempdir().unwrap();
    let opts = MineOptions {
        out: corpus.path().to_path_buf(),
        scratch: corpus.path().join("scratch"),
        max_cases_per_repo: 0,
        clean_sample: 100,
        max_merges_per_repo: 0,
        max_file_bytes: MineOptions::DEFAULT_MAX_FILE_BYTES,
        clone_timeout: std::time::Duration::from_secs(60),
        keep_clones: false,
        resume: false,
        verbose: false,
    };
    let spec = RepoSpec {
        name: "synthetic/prose".into(),
        url: "https://example.invalid/synthetic/prose.git".into(),
        license: "Apache-2.0".into(),
    };
    let report = mine_repo(&Repo::at(dir), &spec, &opts).unwrap();
    assert_eq!(report.status, sm_eval::model::RepoStatus::NoJava);
    assert_eq!(report.merges_found, 0);
}
