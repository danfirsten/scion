//! `--semantic` — running M6's name-binding check inside the merge driver.
//!
//! [`sm_bind`] resolves names across the *candidate merge* and reports
//! references whose binding the merge changed: one branch renames or deletes a
//! declaration, the other branch adds a use of the old name, the two edits touch
//! different lines, and every line-based tool — and every purely syntactic
//! structural one, ours included — calls it clean. Its crate documentation is
//! the authority on what it can and cannot see; the one limitation to keep in
//! mind here is that **a merge driver sees one file**, so this catches the
//! within-file subset of the problem and nothing else.
//!
//! # When it runs
//!
//! Exactly one situation: **the semantic path ran, and its tree merge came out
//! clean.** Both halves are deliberate.
//!
//! - *Clean only.* A file that already carries conflict markers is going to be
//!   read by a human line by line. The dangerous case is the merge that looks
//!   finished — that is the one worth spending a check on, and the only one
//!   where a report is news.
//! - *Semantic path only.* The **fast path skips it**: when `git merge-file`
//!   produced a conflict-free result that parses, the driver ships those bytes
//!   without ever building a tree, and there is no merge plan for
//!   [`sm_bind::check_report`] to walk. That is a real gap — a line-clean merge
//!   is precisely where a broken reference hides — and it is accepted for now
//!   for one reason: a line-clean merge is what *every other tool ships today*,
//!   so skipping the check there leaves the user no worse off than any
//!   alternative, while running it would cost three parses, two matchings and a
//!   merge on the driver's hot path, on every invocation, for the population
//!   that is overwhelmingly fine.
//!
//!   The extension is small and worth doing once M5 has a number for how often
//!   this fires: parse the three inputs and the fast path's own output, build a
//!   merge plan for it (or teach `sm-bind` to check a *file* rather than a
//!   plan), and run the same check. `--no-fast-path` already reaches the check
//!   today, which is how the corpus scan will measure the fast-path population
//!   offline before anyone pays for it online.
//!
//! # `report` versus `conflict`, and the exit-code question
//!
//! [`SemanticMode::Report`] is the default and changes no exit code. A merge
//! driver that starts refusing merges on a new heuristic is a merge driver that
//! gets uninstalled; `sm-bind`'s crate docs say so and they are right.
//!
//! [`SemanticMode::Conflict`] is for teams that want the gate. The interesting
//! question is what "conflict" can even mean when **the text merged fine**:
//!
//! - **Conflict markers are wrong here.** There is no textual disagreement to
//!   bracket. The two sides' edits are in different places and both belong in
//!   the result; markers would have to enclose some arbitrary region and
//!   present a choice that does not exist. It would also destroy the one thing
//!   that makes this finding actionable — the merged file, which is what the
//!   user needs to look at.
//! - **A trailing comment block is worse.** It writes bytes that came from no
//!   input into the user's source (breaking SPEC.md §5), it survives into the
//!   commit if anyone forgets to delete it, and it needs a comment syntax per
//!   language.
//! - **Chosen: write the clean merge to `%A` and exit 1.** Git treats a
//!   non-zero exit from a merge driver as "conflicts remain", so it leaves the
//!   path *unmerged* in the index with all three stages intact. Concretely,
//!   `git merge` prints `CONFLICT (content): Merge conflict in <path>`, `git
//!   status` lists the file under "Unmerged paths" as "both modified", and the
//!   merge does not commit. The working-tree file is the clean merged result
//!   with no markers in it, and the explanations are on stderr where the user
//!   just read git's own output.
//!
//! **The tradeoff, stated plainly:** a user who ignores stderr sees a file git
//! calls conflicted with nothing visibly wrong in it, which is confusing.
//! `git checkout --conflict=merge <path>` will not help them either — there are
//! no markers to regenerate. What they do is read the file, decide, and `git
//! add` it. That is one puzzled moment against a silently broken build, and it
//! is why this is opt-in per invocation rather than the default.
//!
//! `sm install-driver` writes the plain driver line, unchanged. Turning the
//! gate on is a one-word edit to `merge.semantic.driver` in git config; the
//! README's setup section shows it.

use sm_bind::{CheckReport, SemanticConflict};

/// What the driver does with [`sm_bind`]'s findings.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum SemanticMode {
    /// Do not run the check at all.
    Off,
    /// Run it, print findings to stderr, leave the exit code alone.
    #[default]
    Report,
    /// Run it, print findings, and exit 1 if there are any.
    Conflict,
}

impl SemanticMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Report => "report",
            Self::Conflict => "conflict",
        }
    }
}

/// One line per finding, plus a summary line, on stderr.
///
/// The `semantic-merge: warning:` prefix is fixed: it names the tool (git's
/// output is all around it), and `warning:` is the level a reader's eye already
/// scans for. In [`SemanticMode::Conflict`] the summary says what the exit code
/// is about to be, because the file the user is about to open has no markers in
/// it and nothing else would explain why git called it conflicted.
pub fn report(report: &CheckReport, path: &str, mode: SemanticMode) {
    if report.conflicts.is_empty() {
        return;
    }
    for c in &report.conflicts {
        eprintln!("semantic-merge: warning: {}: {}", path, c.explanation);
    }
    let n = report.conflicts.len();
    let plural = if n == 1 { "" } else { "s" };
    match mode {
        SemanticMode::Conflict => eprintln!(
            "semantic-merge: warning: {path}: {n} semantic conflict{plural} \
             ({}); the merged text is in place and has no conflict markers, \
             but the file is left unmerged for you to check",
            tags(&report.conflicts)
        ),
        _ => eprintln!(
            "semantic-merge: warning: {path}: {n} semantic conflict{plural} \
             ({}); the merge itself was clean",
            tags(&report.conflicts)
        ),
    }
}

/// `broken_reference x2, captured_reference` — a stable, greppable summary.
fn tags(conflicts: &[SemanticConflict]) -> String {
    let mut counts: Vec<(&'static str, usize)> = Vec::new();
    for c in conflicts {
        let tag = c.kind.tag();
        if let Some(slot) = counts.iter_mut().find(|(t, _)| *t == tag) {
            slot.1 += 1;
        } else {
            counts.push((tag, 1));
        }
    }
    counts
        .iter()
        .map(|(tag, n)| {
            if *n == 1 {
                (*tag).to_owned()
            } else {
                format!("{tag} x{n}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::{SemanticMode, tags};
    use sm_bind::{SemanticConflict, SemanticConflictKind, Span};
    use sm_merge::Side;

    fn conflict(kind: SemanticConflictKind) -> SemanticConflict {
        SemanticConflict {
            kind,
            name: "x".to_owned(),
            reference: Span {
                side: Side::Ours,
                node: sm_cst::NodeId(0),
                range: 0..1,
            },
            origin_declaration: None,
            merged_declaration: None,
            anchor: None,
            explanation: String::new(),
        }
    }

    #[test]
    fn the_mode_names_match_the_flag_values() {
        assert_eq!(SemanticMode::Off.as_str(), "off");
        assert_eq!(SemanticMode::Report.as_str(), "report");
        assert_eq!(SemanticMode::Conflict.as_str(), "conflict");
        assert_eq!(SemanticMode::default(), SemanticMode::Report);
    }

    #[test]
    fn the_summary_counts_each_kind_once() {
        use SemanticConflictKind::{BrokenReference, CapturedReference};
        assert_eq!(tags(&[conflict(BrokenReference)]), "broken_reference");
        assert_eq!(
            tags(&[
                conflict(BrokenReference),
                conflict(CapturedReference),
                conflict(BrokenReference),
            ]),
            "broken_reference x2, captured_reference"
        );
    }
}
