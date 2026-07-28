//! On-disk schema for the corpus: `case.json` and `manifest.json`.
//!
//! Both carry a `schema_version`. M5 reads these files years (well, milestones)
//! after they are written, and a silently changed field name would be a
//! measurement bug rather than a compile error, so the wire format is a
//! deliberate DTO rather than whatever the internal structs happen to look
//! like.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Bumped whenever the meaning of a field changes.
pub const SCHEMA_VERSION: u32 = 1;

/// What sort of case this is: one git conflicted on, or one it merged cleanly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseKind {
    /// `git merge-tree` reported a conflict for this path.
    Conflicted,
    /// Both sides changed the path relative to base and git merged it cleanly.
    Clean,
}

impl CaseKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CaseKind::Conflicted => "conflicted",
            CaseKind::Clean => "clean",
        }
    }
}

/// The shape of the three-way input, recorded rather than filtered.
///
/// SPEC §6.1 only describes the modify/modify case. The others are real and
/// M4/M5 must handle them, so they are kept and labelled instead of dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseShape {
    /// Present in base, ours and theirs.
    ModifyModify,
    /// Absent from base, added on both sides.
    AddAdd,
    /// Present in base and theirs, deleted by ours.
    DeleteModify,
    /// Present in base and ours, deleted by theirs.
    ModifyDelete,
    /// Present in base only — deleted by both sides, still conflicted (rename).
    DeleteDelete,
    /// Present on exactly one of the two sides and not in base.
    AddNone,
}

impl CaseShape {
    pub fn as_str(self) -> &'static str {
        match self {
            CaseShape::ModifyModify => "modify_modify",
            CaseShape::AddAdd => "add_add",
            CaseShape::DeleteModify => "delete_modify",
            CaseShape::ModifyDelete => "modify_delete",
            CaseShape::DeleteDelete => "delete_delete",
            CaseShape::AddNone => "add_none",
        }
    }
}

/// How the human's committed resolution relates to the two sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionKind {
    /// Byte-identical to `ours`.
    TookOurs,
    /// Byte-identical to `theirs`.
    TookTheirs,
    /// Neither — a genuine hand merge (or an edit on top of one side).
    Merged,
    /// The path does not exist in the merge commit at all.
    Absent,
}

impl ResolutionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ResolutionKind::TookOurs => "took_ours",
            ResolutionKind::TookTheirs => "took_theirs",
            ResolutionKind::Merged => "merged",
            ResolutionKind::Absent => "absent",
        }
    }
}

/// One version of the file, as it exists on disk in the case directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    /// File name inside the case directory, e.g. `ours.java`.
    pub file: String,
    /// Git blob oid, so a case can be traced back to the source repository.
    pub blob: String,
    pub bytes: u64,
    pub lines: u64,
}

/// `case.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Case {
    pub schema_version: u32,
    pub case_id: String,
    pub kind: CaseKind,

    /// `owner/repo`, matching the corpus directory name.
    pub repo: String,
    pub repo_url: String,
    /// SPDX identifier as declared in `repos.txt`.
    pub license: String,

    pub merge_commit: String,
    pub parents: [String; 2],
    pub base_commit: String,
    pub path: String,

    pub shape: CaseShape,
    pub resolution_kind: ResolutionKind,

    /// SPEC §6.1's filter. Flagged, never deleted: M5 needs an honest
    /// denominator, and "the human also did something unrelated" is itself a
    /// finding rather than a reason to make the corpus look cleaner.
    pub contaminated: bool,
    /// Lines in the resolution present in none of base/ours/theirs, after
    /// trailing whitespace is trimmed.
    pub contaminating_lines: u64,
    /// The first few of those lines, verbatim, for eyeballing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contaminating_sample: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<Version>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ours: Option<Version>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theirs: Option<Version>,
    /// The human resolution: the path's content in the merge commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<Version>,
    /// What git's own merge produced. Always present for clean cases; absent
    /// for conflicted ones, where the merge result is a conflict-marked file we
    /// deliberately do not treat as an input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_result: Option<Version>,

    /// merge-tree's own words about this path, e.g. `CONFLICT (contents)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflict_messages: Vec<String>,
}

/// Why a candidate never became a case. Counted, never silently dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Exclusion {
    /// The two parents share no common ancestor.
    NoMergeBase,
    /// The merge base *is* one of the parents: a fast-forward dressed up as a
    /// merge commit. There is no three-way decision to learn from.
    MergeBaseIsParent,
    /// `git merge-tree` did not produce usable output for this merge commit.
    MergeTreeFailed,
    /// A conflicted path that is not a `.java` file.
    NonJavaPath,
    /// Some version of the file exceeds the 1 MiB extraction limit.
    FileTooLarge,
    /// Some version of the file looks binary (contains a NUL byte).
    BinaryContent,
    /// The path is a submodule or a symlink in some version.
    NotARegularFile,
    /// Neither side has the file — nothing to merge.
    NoContent,
    /// A blob that git listed could not be read back.
    BlobReadFailed,
    /// `--max-cases-per-repo` was already reached.
    CaseLimitReached,
}

impl Exclusion {
    pub fn as_str(self) -> &'static str {
        match self {
            Exclusion::NoMergeBase => "no_merge_base",
            Exclusion::MergeBaseIsParent => "merge_base_is_parent",
            Exclusion::MergeTreeFailed => "merge_tree_failed",
            Exclusion::NonJavaPath => "non_java_path",
            Exclusion::FileTooLarge => "file_too_large",
            Exclusion::BinaryContent => "binary_content",
            Exclusion::NotARegularFile => "not_a_regular_file",
            Exclusion::NoContent => "no_content",
            Exclusion::BlobReadFailed => "blob_read_failed",
            Exclusion::CaseLimitReached => "case_limit_reached",
        }
    }

    pub const ALL: [Exclusion; 10] = [
        Exclusion::NoMergeBase,
        Exclusion::MergeBaseIsParent,
        Exclusion::MergeTreeFailed,
        Exclusion::NonJavaPath,
        Exclusion::FileTooLarge,
        Exclusion::BinaryContent,
        Exclusion::NotARegularFile,
        Exclusion::NoContent,
        Exclusion::BlobReadFailed,
        Exclusion::CaseLimitReached,
    ];
}

/// Outcome of mining one repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoStatus {
    Ok,
    CloneFailed,
    /// Cloned, but the default branch has no `.java` files.
    NoJava,
    /// Something went wrong mid-walk; whatever was extracted is still on disk.
    Error,
}

/// Per-repository row of `manifest.json`.
///
/// No `Eq`: `duration_secs` is a wall-clock float and comparing two reports for
/// exact equality is not a meaningful thing to want.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoReport {
    /// `owner/repo`, and the corpus directory name.
    pub name: String,
    pub url: String,
    pub license: String,
    pub status: RepoStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Tip of the cloned default branch — what makes a run reproducible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,

    /// Two-parent merge commits found by `git log --merges`.
    pub merges_found: u64,
    /// Of those, the ones that survived the merge-base checks and were replayed.
    pub merges_walked: u64,
    /// Merges whose replay produced at least one conflicted path.
    pub merges_with_conflicts: u64,

    /// Conflicted paths ending in `.java`, before extraction limits.
    pub conflicted_java_files: u64,
    /// Conflicted cases written (or already present) on disk.
    pub conflicted_cases: u64,
    /// Of those, flagged by the contamination filter.
    pub contaminated_cases: u64,

    /// Paths changed on both sides that git merged cleanly.
    pub clean_candidates: u64,
    /// Of those, the deterministic sample that was extracted.
    pub clean_cases: u64,

    #[serde(default)]
    pub exclusions: BTreeMap<String, u64>,

    pub duration_secs: f64,
}

impl RepoReport {
    pub fn failed(name: &str, url: &str, license: &str, status: RepoStatus, error: String) -> Self {
        Self {
            name: name.to_string(),
            url: url.to_string(),
            license: license.to_string(),
            status,
            error: Some(error),
            head: None,
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
        }
    }
}

/// `manifest.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub tool: String,
    pub git_version: String,
    /// Options the run was invoked with, so the numbers can be reproduced.
    pub settings: MineSettings,
    pub repos: Vec<RepoReport>,
}

/// The knobs that change what ends up in the corpus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MineSettings {
    pub max_cases_per_repo: u64,
    pub clean_sample: u64,
    pub max_merges_per_repo: u64,
    pub max_file_bytes: u64,
}

impl Manifest {
    pub fn new(git_version: String, settings: MineSettings) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            tool: concat!("sm-eval ", env!("CARGO_PKG_VERSION")).to_string(),
            git_version,
            settings,
            repos: Vec::new(),
        }
    }

    /// Insert or replace the row for a repository, keeping rows sorted by name
    /// so a re-run produces a byte-stable manifest.
    pub fn upsert(&mut self, report: RepoReport) {
        match self.repos.iter().position(|r| r.name == report.name) {
            Some(i) => self.repos[i] = report,
            None => self.repos.push(report),
        }
        self.repos.sort_by(|a, b| a.name.cmp(&b.name));
    }
}
