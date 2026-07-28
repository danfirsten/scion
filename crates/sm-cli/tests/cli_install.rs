//! Tests for `sm install-driver`.
//!
//! These run against **real `git config`** in a throwaway repository, because
//! the thing being tested is an interaction with git's configuration parser,
//! and a test that asserted on a string we generated would only be testing our
//! own `format!`. Every assertion here reads the value back out with
//! `git config --get`.
//!
//! `--global` is never exercised: it writes to the machine's `~/.gitconfig`,
//! and a test suite that edits the developer's home directory is not a test
//! suite. The scope flag is one argument to the same `git config` call the
//! local tests already cover.

use std::path::Path;
use std::process::{Command, Output};

fn sm(repo: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sm"));
    cmd.current_dir(repo);
    cmd
}

/// A fresh repository with a committed file, in a scratch directory.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.email", "t@example.invalid"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    dir
}

fn git(repo: &Path, args: &[&str]) -> Output {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("running git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn config(repo: &Path, key: &str) -> Option<String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["config", "--get", key])
        .output()
        .expect("running git config");
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

#[test]
fn installing_writes_a_driver_line_git_can_read_back() {
    let dir = repo();
    let out = sm(dir.path())
        .args(["install-driver", "--local"])
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let driver = config(dir.path(), "merge.semantic.driver").expect("driver key");
    // The placeholder list is the contract with `sm merge`'s positional
    // arguments. Getting this wrong is how the result lands in the wrong file.
    assert!(
        driver.ends_with("merge %O %A %B %L %P %S %X %Y"),
        "driver line was {driver:?}"
    );
    // And it points at a real executable, not a bare name that may not be on
    // the PATH of whatever process runs the merge.
    let program = driver
        .split(" merge %O")
        .next()
        .expect("split")
        .trim_matches('"');
    assert!(
        Path::new(program).is_absolute(),
        "driver command {program:?} is not an absolute path"
    );

    assert_eq!(
        config(dir.path(), "merge.semantic.name").as_deref(),
        Some("semantic-merge: AST-aware three-way merge")
    );
    assert_eq!(
        config(dir.path(), "merge.semantic.recursive").as_deref(),
        Some("text")
    );
}

#[test]
fn the_recursive_setting_is_selectable() {
    for (flag, expected) in [
        ("text", "text"),
        ("binary", "binary"),
        ("semantic", "semantic"),
    ] {
        let dir = repo();
        let out = sm(dir.path())
            .args(["install-driver", "--local", "--recursive", flag])
            .output()
            .expect("run");
        assert!(out.status.success());
        assert_eq!(
            config(dir.path(), "merge.semantic.recursive").as_deref(),
            Some(expected)
        );
    }
}

#[test]
fn the_printed_gitattributes_lines_cover_the_requested_languages() {
    let dir = repo();
    let out = sm(dir.path())
        .args(["install-driver", "--local", "--langs", "java,tsx"])
        .output()
        .expect("run");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("*.java merge=semantic"), "{text}");
    assert!(text.contains("*.tsx merge=semantic"), "{text}");
    assert!(!text.contains("*.ts merge=semantic"), "{text}");
}

#[test]
fn write_attributes_appends_once_and_is_idempotent() {
    let dir = repo();
    let attrs = dir.path().join(".gitattributes");
    std::fs::write(&attrs, "*.png binary\n").expect("seed");

    for _ in 0..3 {
        let out = sm(dir.path())
            .args([
                "install-driver",
                "--local",
                "--langs",
                "java",
                "--write-attributes",
                attrs.to_str().expect("utf-8"),
            ])
            .output()
            .expect("run");
        assert!(out.status.success());
    }

    let text = std::fs::read_to_string(&attrs).expect("read");
    assert_eq!(
        text.matches("*.java merge=semantic").count(),
        1,
        "re-installing duplicated the rule:\n{text}"
    );
    assert!(
        text.contains("*.png binary"),
        "existing rules were lost:\n{text}"
    );
}

#[test]
fn write_attributes_creates_the_file_and_fixes_a_missing_final_newline() {
    let dir = repo();
    let attrs = dir.path().join(".gitattributes");
    std::fs::write(&attrs, "*.png binary").expect("seed without a final newline");
    let out = sm(dir.path())
        .args([
            "install-driver",
            "--local",
            "--langs",
            "java",
            "--write-attributes",
            attrs.to_str().expect("utf-8"),
        ])
        .output()
        .expect("run");
    assert!(out.status.success());
    let text = std::fs::read_to_string(&attrs).expect("read");
    assert_eq!(text, "*.png binary\n*.java merge=semantic\n");
}

#[test]
fn a_dry_run_changes_nothing() {
    let dir = repo();
    let out = sm(dir.path())
        .args(["install-driver", "--local", "--dry-run"])
        .output()
        .expect("run");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("git config --local merge.semantic.driver"),
        "{text}"
    );
    assert_eq!(config(dir.path(), "merge.semantic.driver"), None);
}

#[test]
fn uninstalling_removes_the_section_and_is_safe_to_repeat() {
    let dir = repo();
    sm(dir.path())
        .args(["install-driver", "--local"])
        .output()
        .expect("run");
    assert!(config(dir.path(), "merge.semantic.driver").is_some());

    for _ in 0..2 {
        let out = sm(dir.path())
            .args(["install-driver", "--local", "--uninstall"])
            .output()
            .expect("run");
        assert!(
            out.status.success(),
            "uninstall must be idempotent: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_eq!(config(dir.path(), "merge.semantic.driver"), None);
    assert_eq!(config(dir.path(), "merge.semantic.name"), None);
}

/// A path with a space in it has to survive git's shell-like splitting of the
/// driver value, or the driver silently never runs.
#[test]
fn a_driver_path_with_spaces_is_quoted_so_git_reads_it_back_whole() {
    let dir = repo();
    let out = sm(dir.path())
        .args([
            "install-driver",
            "--local",
            "--driver-path",
            "/opt/my tools/sm",
        ])
        .output()
        .expect("run");
    assert!(out.status.success());
    let driver = config(dir.path(), "merge.semantic.driver").expect("driver");
    assert_eq!(driver, "\"/opt/my tools/sm\" merge %O %A %B %L %P %S %X %Y");
}
