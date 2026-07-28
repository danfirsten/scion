//! End-to-end tests for `sm-eval replay` and `sm-eval report` over a synthetic
//! corpus and a synthetic driver.
//!
//! The driver is a shell script rather than the real `sm` binary, and that is
//! deliberate: Cargo only exports `CARGO_BIN_EXE_*` for the package's own
//! binaries, and — more usefully — a stub lets a test *choose* what the driver
//! did, so the harness's own behaviour (enumeration order, chunking,
//! resumption, grading, the exit-2 contract) can be pinned without depending on
//! how well the merge happens to work today. `crates/sm-cli/tests/cli_merge.rs`
//! is where the real binary is exercised.

#![cfg(unix)]

use std::path::{Path, PathBuf};

use sm_eval::replay::{Arm, Bucket, ReplayOptions, ReplayRecord, Subset, replay};
use sm_eval::report;

// ------------------------------------------------------------ the fake driver

/// A `sm merge` stub. It reads the *base* file's first line to decide what to
/// do, which is how each synthetic case picks its own outcome:
///
/// * `//CLEAN`     — copy `theirs` to `--output`, exit 0.
/// * `//WRONG`     — copy `ours` to `--output`, exit 0.
/// * `//CONFLICT`  — write a marker file, exit 1.
/// * `//ERROR`     — write nothing, exit 2.
fn write_fake_driver(path: &Path) {
    std::fs::write(
        path,
        r#"#!/bin/sh
base=$2; ours=$3; theirs=$4
out=""; json=""; prev=""
for a in "$@"; do
  case "$prev" in
    --output) out="$a" ;;
    --debug-json) json="$a" ;;
  esac
  prev="$a"
done
mode=$(head -n 1 "$base")
code=0
case "$mode" in
  *CLEAN*)    cp "$theirs" "$out" ;;
  *WRONG*)    cp "$ours" "$out" ;;
  *CONFLICT*) printf 'class C {\n<<<<<<< ours\n=======\n>>>>>>> theirs\n}\n' > "$out"; code=1 ;;
  *ERROR*)    code=2 ;;
esac
if [ -n "$json" ]; then
  cat > "$json" <<EOF
{"schema_version":1,"path_taken":"semantic","fallback_reason":null,"conflicts":$code,
 "warnings":[],"line_merge":{"bytes":1,"conflict_hunks":1,"parsed_ok":null},
 "semantic":{"clean":true,"conflicts":0,"conflict_reasons":["update_update@method_declaration"],
             "output_bytes":1,"synthesized_bytes":0,"synthesized_separators":0},
 "timings_ms":{"total":1.5,"semantic_check":null}}
EOF
fi
exit $code
"#,
    )
    .expect("write driver");
    let mut perms = std::fs::metadata(path).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(path, perms).expect("chmod");
}

// ------------------------------------------------------------ the fake corpus

/// Which population a synthetic case is built to land in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Gradeable,
    Contaminated,
    Rename,
    Clean,
}

struct Corpus {
    dir: tempfile::TempDir,
}

impl Corpus {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// A plain conflicted, non-contaminated, modify/modify case — the shape
    /// that ends up in the gradeable bucket. `mode` is the marker the fake
    /// driver reads.
    fn case(&self, repo: &str, id: &str, mode: &str) -> PathBuf {
        self.write(repo, id, mode, Kind::Gradeable)
    }

    fn write(&self, repo: &str, id: &str, mode: &str, kind: Kind) -> PathBuf {
        let mut dir = self.dir.path().join(repo.replace('/', "__"));
        if kind == Kind::Clean {
            dir = dir.join("clean");
        }
        let dir = dir.join(id);
        let contaminated = kind == Kind::Contaminated;
        let shape = if kind == Kind::Rename {
            "add_none"
        } else {
            "modify_modify"
        };
        let kind_name = if kind == Kind::Clean {
            "clean"
        } else {
            "conflicted"
        };
        std::fs::create_dir_all(&dir).expect("case dir");

        let base = format!("//{mode}\nclass C {{ int a = 0; }}\n");
        let ours = format!("//{mode}\nclass C {{ int a = 1; }}\n");
        let theirs = format!("//{mode}\nclass C {{ int a = 2; }}\n");
        // The "human resolution" is `theirs`, so a driver that copies theirs is
        // correct and one that copies ours is not.
        std::fs::write(dir.join("base.java"), &base).expect("base");
        std::fs::write(dir.join("ours.java"), &ours).expect("ours");
        std::fs::write(dir.join("theirs.java"), &theirs).expect("theirs");
        std::fs::write(dir.join("resolved.java"), &theirs).expect("resolved");
        if kind == Kind::Clean {
            std::fs::write(dir.join("git_result.java"), &theirs).expect("git_result");
        }

        let version = |file: &str, text: &str| {
            serde_json::json!({
                "file": file, "blob": "0".repeat(40),
                "bytes": text.len(), "lines": text.lines().count()
            })
        };
        let mut case = serde_json::json!({
            "schema_version": 1,
            "case_id": id,
            "kind": kind_name,
            "repo": repo,
            "repo_url": "",
            "license": "Apache-2.0",
            "merge_commit": "a".repeat(40),
            "parents": ["b".repeat(40), "c".repeat(40)],
            "base_commit": "d".repeat(40),
            "path": "A.java",
            "shape": shape,
            "resolution_kind": "merged",
            "contaminated": contaminated,
            "contaminating_lines": u64::from(contaminated),
            "base": version("base.java", &base),
            "ours": version("ours.java", &ours),
            "theirs": version("theirs.java", &theirs),
            "resolved": version("resolved.java", &theirs),
        });
        if kind == Kind::Clean {
            case["git_result"] = version("git_result.java", &theirs);
        }
        if kind == Kind::Rename {
            // A rename conflict as the miner records it: one side and the
            // resolution, nothing else.
            case["base"] = serde_json::Value::Null;
            case["theirs"] = serde_json::Value::Null;
        }
        std::fs::write(
            dir.join("case.json"),
            serde_json::to_string_pretty(&case).expect("case json"),
        )
        .expect("write case.json");
        dir
    }
}

/// The corpus every test below shares: five gradeable cases across two
/// repositories with known outcomes, one contaminated, one rename, one clean.
fn build() -> Corpus {
    let c = Corpus::new();
    c.case("a/one", "c1-correct", "CLEAN");
    c.case("a/one", "c2-correct", "CLEAN");
    c.case("a/one", "c3-wrong", "WRONG");
    c.case("a/one", "c4-declined", "CONFLICT");
    c.case("b/two", "c5-errored", "ERROR");
    c.write("b/two", "c6-dirty", "CLEAN", Kind::Contaminated);
    c.write("b/two", "c7-rename", "CLEAN", Kind::Rename);
    c.write("b/two", "c8-clean", "CLEAN", Kind::Clean);
    c
}

fn options(corpus: &Corpus, driver: &Path, out: &Path) -> ReplayOptions {
    ReplayOptions {
        corpus: corpus.path().to_path_buf(),
        sm: driver.to_path_buf(),
        out: out.to_path_buf(),
        jobs: 3,
        subset: Subset::All,
        profile_overrides: None,
        semantic: "report".to_owned(),
        control: false,
        resume: false,
        keep_outputs: true,
        quiet: true,
        timeout_ms: 5000,
    }
}

fn run(corpus: &Corpus, out: &Path) -> Vec<ReplayRecord> {
    let driver = corpus.path().join("fake-sm");
    write_fake_driver(&driver);
    let opts = options(corpus, &driver, out);
    replay(&opts).expect("replay");
    report::load(out).expect("load records")
}

// ----------------------------------------------------------------- the tests

#[test]
fn the_replay_buckets_every_case_and_grades_the_ones_it_ran() {
    let corpus = build();
    let out = tempfile::tempdir().expect("out");
    let records = run(&corpus, out.path());

    assert_eq!(records.len(), 8, "one record per case, one arm");
    let by_id = |id: &str| {
        records
            .iter()
            .find(|r| r.case_id == id)
            .unwrap_or_else(|| panic!("no record for {id}"))
    };

    let correct = by_id("c1-correct");
    assert_eq!(correct.bucket, Bucket::Gradeable);
    assert_eq!(correct.exit, Some(0));
    assert_eq!(correct.byte_exact, Some(true));
    assert_eq!(correct.ast_equal, Some(true));
    assert_eq!(correct.parsable, Some(true));
    assert_eq!(correct.universal, Some(true));

    let wrong = by_id("c3-wrong");
    assert_eq!(wrong.exit, Some(0));
    assert_eq!(wrong.byte_exact, Some(false));
    assert_eq!(wrong.ast_equal, Some(false));
    assert!(
        wrong.output_file.is_some(),
        "an incorrect resolve is kept for the gallery"
    );

    let declined = by_id("c4-declined");
    assert_eq!(declined.exit, Some(1));
    assert_eq!(declined.ast_equal, Some(false));
    assert_eq!(declined.parsable, None, "only a clean output is scored");

    let errored = by_id("c5-errored");
    assert_eq!(errored.exit, Some(2));
    assert!(
        errored.harness_error.is_none(),
        "exit 2 writes nothing by contract"
    );
    assert_eq!(errored.output_bytes, None);

    assert_eq!(by_id("c6-dirty").bucket, Bucket::Contaminated);

    let rename = by_id("c7-rename");
    assert_eq!(rename.bucket, Bucket::Rename);
    assert!(!rename.ran, "a rename conflict is not a three-way merge");
    assert!(rename.not_run_reason.is_some());

    let clean = by_id("c8-clean");
    assert_eq!(clean.bucket, Bucket::Clean);
    assert_eq!(clean.matches_git_result, Some(true));
    assert_eq!(correct.resolution_kind, "merged");
}

#[test]
fn the_metrics_are_the_arithmetic_the_buckets_imply() {
    let corpus = build();
    let out = tempfile::tempdir().expect("out");
    let records = run(&corpus, out.path());
    let m = report::compute(&records);

    // Gradeable: c1, c2 correct; c3 incorrect; c4 declined; c5 errored.
    assert_eq!(m.gradeable.total, 5);
    assert_eq!(m.gradeable.resolved, 3);
    assert_eq!(m.gradeable.correct_ast, 2);
    assert_eq!(m.gradeable.correct_bytes, 2);
    assert_eq!(m.gradeable.incorrect, 1);
    assert_eq!(m.gradeable.declined, 1);
    assert_eq!(m.gradeable.errored, 1);
    assert!((m.gradeable.resolve_rate() - 60.0).abs() < 1e-9);
    assert!((m.gradeable.incorrect_of_resolved() - 100.0 / 3.0).abs() < 1e-9);

    assert_eq!(
        m.contaminated.total, 1,
        "contaminated cases are counted apart"
    );
    assert_eq!(m.clean.total, 1);
    assert_eq!(m.clean.identical, 1);
    assert_eq!(m.clean.regressions, 0);
    assert_eq!(m.clean.divergences, 0);
    assert_eq!(m.not_run["rename"], 1);

    // Per-repo splits follow the directory layout, not the case ids.
    assert_eq!(m.per_repo["a/one"].total, 4);
    assert_eq!(m.per_repo["b/two"].total, 1);

    // The conflict-reason histogram comes from the driver's record, on every
    // case that ran — seven of the eight; the rename case never started.
    assert_eq!(m.conflict_reasons["update_update@method_declaration"], 7);

    let g = report::gallery(&records, 10);
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].case_id, "c3-wrong");

    // And the rendered report is derived from the same numbers.
    let md = report::render(&m, report::load_run(out.path()).as_ref(), &records);
    assert!(md.contains("| **Resolve rate** | 60.00% |"), "{md}");
    assert!(
        md.contains("`rename`"),
        "the rename bucket must be visible: {md}"
    );
}

/// Everything in a record except the two fields that are measurements of this
/// machine at this moment. Those cannot be deterministic and pretending
/// otherwise would make the test a flake detector rather than an ordering
/// check; everything a metric is computed from is in here.
fn decisions(dir: &Path) -> String {
    report::load(dir)
        .expect("load")
        .into_iter()
        .map(|mut r| {
            r.wall_ms = 0.0;
            r.driver_ms = None;
            serde_json::to_string(&r).expect("serialize")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_log_is_deterministic_across_job_counts_and_resumable() {
    let corpus = build();
    let driver = corpus.path().join("fake-sm");
    write_fake_driver(&driver);

    let a = tempfile::tempdir().expect("out");
    let mut opts = options(&corpus, &driver, a.path());
    opts.jobs = 1;
    replay(&opts).expect("replay");

    let b = tempfile::tempdir().expect("out");
    let mut opts = options(&corpus, &driver, b.path());
    opts.jobs = 4;
    replay(&opts).expect("replay");

    assert_eq!(
        decisions(a.path()),
        decisions(b.path()),
        "neither the order nor any decision may depend on --jobs"
    );
    let four = decisions(b.path());

    // Resuming a complete run adds nothing and changes nothing.
    let mut opts = options(&corpus, &driver, b.path());
    opts.resume = true;
    let info = replay(&opts).expect("replay");
    assert_eq!(decisions(b.path()), four);
    assert_eq!(info.records_written, 8);

    // Resuming a *partial* run finishes it, and lands on the same answers.
    let c = tempfile::tempdir().expect("out");
    std::fs::create_dir_all(c.path()).expect("out dir");
    let partial: String = std::fs::read_to_string(b.path().join("records.jsonl"))
        .expect("read")
        .lines()
        .take(3)
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(c.path().join("records.jsonl"), partial).expect("seed");
    let mut opts = options(&corpus, &driver, c.path());
    opts.resume = true;
    replay(&opts).expect("replay");
    let mut resumed: Vec<String> = decisions(c.path()).lines().map(ToOwned::to_owned).collect();
    let mut whole: Vec<String> = four.lines().map(ToOwned::to_owned).collect();
    resumed.sort();
    whole.sort();
    assert_eq!(
        resumed, whole,
        "a resumed run must cover exactly the same cases"
    );
}

#[test]
fn the_control_arm_is_recorded_separately() {
    let corpus = build();
    let out = tempfile::tempdir().expect("out");
    let driver = corpus.path().join("fake-sm");
    write_fake_driver(&driver);
    let mut opts = options(&corpus, &driver, out.path());
    opts.control = true;
    replay(&opts).expect("replay");

    let records = report::load(out.path()).expect("load");
    assert_eq!(records.len(), 16, "two arms");
    assert_eq!(records.iter().filter(|r| r.arm == Arm::Line).count(), 8);

    let m = report::compute(&records);
    assert_eq!(
        m.gradeable.total, 5,
        "the control arm must not inflate the `sm` numbers"
    );
    assert_eq!(m.control_line_conflicted.total, 5);
}

#[test]
fn a_stratified_subset_is_proportional_and_reproducible() {
    let corpus = Corpus::new();
    for i in 0..40 {
        corpus.case("a/one", &format!("a{i:03}"), "CLEAN");
    }
    for i in 0..10 {
        corpus.case("b/two", &format!("b{i:03}"), "CLEAN");
    }
    let all = sm_eval::replay::enumerate(corpus.path()).expect("enumerate");
    assert_eq!(all.len(), 50);

    let picked = sm_eval::replay::apply_subset(all.clone(), &Subset::Sample { n: 10, seed: 5 });
    assert_eq!(picked.len(), 10);
    assert_eq!(picked.iter().filter(|c| c.case.repo == "a/one").count(), 8);
    assert_eq!(picked.iter().filter(|c| c.case.repo == "b/two").count(), 2);

    let again = sm_eval::replay::apply_subset(all, &Subset::Sample { n: 10, seed: 5 });
    assert_eq!(
        picked
            .iter()
            .map(|c| c.case.case_id.clone())
            .collect::<Vec<_>>(),
        again
            .iter()
            .map(|c| c.case.case_id.clone())
            .collect::<Vec<_>>()
    );
}
