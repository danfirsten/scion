//! SPEC.md §5's property tests, over generated three-way merges.
//!
//! # What this suite is for
//!
//! `sm-emit`'s `tests/properties.rs` states the same invariants over a
//! hand-written table of ~27 scenarios. That table is the *documentation* of
//! what the merge does; this file is the *search* for where it does not.
//! Everything here reuses that table's shape deliberately — the scenarios were
//! written first so the generators start from a passing baseline rather than
//! from a bug hunt.
//!
//! The generator is `mutate/mod.rs`: a pool of realistic base programs, plus
//! thirteen kinds of structured edit (rename, move, wrap, delete, reindent…)
//! applied 1–6 at a time to produce *ours* and *theirs*. Read that module's
//! docs for why the mutations are structural rather than textual.
//!
//! # Determinism
//!
//! Every property runs with an explicitly seeded RNG
//! ([`TestRng::deterministic_rng`]) and `failure_persistence: None`. Two
//! consequences, both wanted:
//!
//! - A failure reproduces from a clean checkout with no `.proptest-regressions`
//!   file to commit, and CI and a laptop explore exactly the same cases.
//! - The suite has a fixed runtime, which is what lets it live in
//!   `cargo test` rather than in a nightly job.
//!
//! Widening the search is a matter of raising `CASES`, and a failure found that
//! way should be *promoted into `sm-emit`'s scenario table* rather than pinned
//! here.
//!
//! # Properties, and what each one is really checking
//!
//! | property | the bug it is looking for |
//! |---|---|
//! | `merge(B,X,B) == X` | any decision that consults the wrong side, or an emitter that reformats an untouched file |
//! | `merge(B,B,Y) == Y` | the same, mirrored — the case where "prefer ours" is wrong |
//! | `merge(B,X,X) == X` | a convergence check that is comparing the wrong things |
//! | `merge(B,B,B) == B` | the identity of the whole pipeline |
//! | conflict-count symmetry | a decision that depends on argument order, which would make the resolve rate an artefact of which branch you happen to be on |
//! | AST equality under mirroring | a *clean* merge that produces different code depending on argument order — much worse than an asymmetric conflict count |
//! | idempotence | a merge that keeps rewriting a file it already merged |
//! | parse stability | the headline correctness signal: clean output that is not a program |
//! | byte preservation | text that came from nowhere, which is how a splicing emitter fails |
//! | provenance totality | a merged node with no input range behind it |

mod mutate;

use std::sync::atomic::{AtomicUsize, Ordering};

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};

use sm_cst::{Language, SourceTree};
use sm_emit::{EmitOptions, EmitResult, emit};
use sm_merge::{Gap, MergeConfig, MergeOutcome, MergedNode, merge};

use mutate::{Mutation, MutationKind};

/// Cases per property.
///
/// Tuned so the whole file runs in well under two minutes on one core: the
/// generated files are ~20 lines, a merge of three of them is ~1 ms, and the
/// heaviest property does six of them per case.
const CASES: u32 = 300;

// ------------------------------------------------------------- the generator

/// A generated three-way merge case.
#[derive(Clone, Debug)]
struct Case {
    base_name: &'static str,
    lang_name: &'static str,
    base: String,
    ours: String,
    theirs: String,
    ours_mutations: Vec<Mutation>,
    theirs_mutations: Vec<Mutation>,
}

impl Case {
    fn language(&self) -> &'static dyn Language {
        sm_cst::languages::by_name(self.lang_name).expect("registered language")
    }

    /// Everything a failure message needs to reproduce the case by hand.
    fn describe(&self) -> String {
        format!(
            "base={} lang={}\nours mutations: {}\ntheirs mutations: {}\n\
             --- BASE ---\n{}--- OURS ---\n{}--- THEIRS ---\n{}",
            self.base_name,
            self.lang_name,
            join(&self.ours_mutations),
            join(&self.theirs_mutations),
            self.base,
            self.ours,
            self.theirs
        )
    }
}

fn join(ms: &[Mutation]) -> String {
    if ms.is_empty() {
        return "(none applied)".to_owned();
    }
    ms.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// How many generated mutations were dropped for breaking the parse, and how
/// many were offered. Asserted on at the end of `the_generator_is_healthy`.
static OFFERED: AtomicUsize = AtomicUsize::new(0);
static APPLIED: AtomicUsize = AtomicUsize::new(0);

fn mutation() -> impl Strategy<Value = Mutation> {
    (0..MutationKind::ALL.len(), any::<u16>(), any::<u16>()).prop_map(|(k, site, variant)| {
        Mutation {
            kind: MutationKind::ALL[k],
            site,
            variant,
        }
    })
}

/// One case: a base from the pool, and 1–6 mutations down each side.
fn case() -> impl Strategy<Value = Case> {
    let bases = mutate::bases();
    (
        0..bases.len(),
        prop::collection::vec(mutation(), 1..=6),
        prop::collection::vec(mutation(), 1..=6),
    )
        .prop_map(move |(i, ours_ms, theirs_ms)| {
            let base = &mutate::bases()[i];
            let lang = base.language();
            let (ours, ours_applied) = mutate::apply_all(base.source, lang, &ours_ms);
            let (theirs, theirs_applied) = mutate::apply_all(base.source, lang, &theirs_ms);
            OFFERED.fetch_add(ours_ms.len() + theirs_ms.len(), Ordering::Relaxed);
            APPLIED.fetch_add(ours_applied.len() + theirs_applied.len(), Ordering::Relaxed);
            Case {
                base_name: base.name,
                lang_name: base.lang,
                base: base.source.to_owned(),
                ours,
                theirs,
                ours_mutations: ours_applied,
                theirs_mutations: theirs_applied,
            }
        })
}

/// A runner seeded identically on every machine and every run.
fn runner() -> TestRunner {
    TestRunner::new_with_rng(
        Config {
            cases: CASES,
            failure_persistence: None,
            ..Config::default()
        },
        TestRng::deterministic_rng(RngAlgorithm::ChaCha),
    )
}

/// Run `body` over `CASES` generated cases.
fn for_each_case(name: &str, body: impl Fn(&Case) -> Result<(), TestCaseError>) {
    let result = runner().run(&case(), |c| body(&c));
    if let Err(err) = result {
        panic!("{name}: {err}");
    }
}

// --------------------------------------------------------------- the harness

struct Merged {
    outcome: MergeOutcome,
    result: EmitResult,
}

impl Merged {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.result.bytes).into_owned()
    }

    fn reasons(&self) -> Vec<String> {
        self.outcome
            .conflicts
            .iter()
            .map(|c| format!("{}@{}", c.reason.tag(), c.kind))
            .collect()
    }
}

fn run(base: &str, ours: &str, theirs: &str, lang: &'static dyn Language) -> Merged {
    let base = parse(base, lang);
    let ours = parse(ours, lang);
    let theirs = parse(theirs, lang);
    let outcome = merge(
        &base,
        &ours,
        &theirs,
        lang,
        &MergeConfig::for_language(lang),
    );
    let result = emit(
        &outcome.tree,
        &base,
        &ours,
        &theirs,
        lang,
        &EmitOptions::default(),
    );
    Merged { outcome, result }
}

fn parse(src: &str, lang: &'static dyn Language) -> SourceTree {
    sm_cst::parse(src.as_bytes(), lang).expect("generated source parses")
}

/// A canonical hash of a file: **the same program, modulo layout, comments and
/// documented commutation.**
///
/// The obvious implementation — `TreeMetrics::hash` of the root — is wrong for
/// the symmetry property, and finding out why was worth the detour. Merge two
/// branches that each add an import and you get `List, Mine, Theirs`; mirror the
/// arguments and you get `List, Theirs, Mine`. Those are different trees and
/// `TreeMetrics::hash` says so, but they are the *same program*, and the
/// difference is required by `sm-merge`'s documented side preference (crate docs
/// §3: the union takes ours first). Asserting raw hash equality would therefore
/// be asserting that a documented, deliberate behaviour is a bug.
///
/// So the hash canonicalises exactly what the language says is commutative and
/// nothing else, by asking [`sm_cst::Language::child_list_kind`] — the same
/// question the merge asks:
///
/// - [`ChildListKind::Ordered`]: children hashed in order.
/// - [`ChildListKind::Unordered`]: children's hashes sorted before folding.
/// - [`ChildListKind::PartiallyUnordered`]: **maximal runs** of commutable kinds
///   are sorted within themselves and stay where they are. That is what keeps
///   this from equating a program whose package declaration moved below a class
///   with one where it did not — the failure mode a flat sort would introduce.
///
/// Comments are excluded. Where a *floating* comment lands is a layout question
/// (`sm-merge` crate docs §8), and the property below is about code.
fn ast_hash(src: &str, lang: &'static dyn Language) -> Option<u64> {
    let tree = sm_cst::parse(src.as_bytes(), lang).ok()?;
    if tree.has_errors() {
        return None;
    }
    Some(canonical_hash(&tree, lang, tree.root_id()))
}

fn canonical_hash(tree: &SourceTree, lang: &dyn Language, id: sm_cst::NodeId) -> u64 {
    use std::hash::{Hash as _, Hasher as _};

    let node = tree.node(id);
    let kids: Vec<sm_cst::NodeId> = node
        .children
        .iter()
        .copied()
        .filter(|c| !is_comment(tree, *c))
        .collect();

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    node.kind.hash(&mut hasher);
    if kids.is_empty() {
        tree.node_bytes(id).hash(&mut hasher);
        return hasher.finish();
    }

    let mut hashes: Vec<u64> = kids
        .iter()
        .map(|c| canonical_hash(tree, lang, *c))
        .collect();

    match lang.child_list_kind(node.kind) {
        sm_cst::ChildListKind::Ordered => {}
        sm_cst::ChildListKind::Unordered => hashes.sort_unstable(),
        sm_cst::ChildListKind::PartiallyUnordered { unordered_kinds } => {
            let mut i = 0;
            while i < kids.len() {
                if unordered_kinds.contains(&tree.node(kids[i]).kind) {
                    let mut j = i;
                    while j < kids.len() && unordered_kinds.contains(&tree.node(kids[j]).kind) {
                        j += 1;
                    }
                    hashes[i..j].sort_unstable();
                    i = j;
                } else {
                    i += 1;
                }
            }
        }
    }
    hashes.hash(&mut hasher);
    hasher.finish()
}

fn is_comment(tree: &SourceTree, id: sm_cst::NodeId) -> bool {
    let node = tree.node(id);
    node.is_extra || node.kind.contains("comment")
}

// ------------------------------------------------------------ the generator

/// Before trusting any property: check the generator actually generates.
///
/// A generator that silently stopped producing moves, or that produced files
/// that do not parse, would make every property below pass vacuously. This is
/// the test that fails when that happens.
#[test]
fn the_generator_is_healthy() {
    let differed = AtomicUsize::new(0);
    let kinds: std::sync::Mutex<std::collections::BTreeSet<&'static str>> =
        std::sync::Mutex::new(std::collections::BTreeSet::new());
    let both_sides_changed = AtomicUsize::new(0);

    for_each_case("generator", |c| {
        let lang = c.language();
        for (label, text) in [("base", &c.base), ("ours", &c.ours), ("theirs", &c.theirs)] {
            let tree = sm_cst::parse(text.as_bytes(), lang).expect("parse");
            prop_assert!(
                !tree.has_errors(),
                "generated {label} does not parse:\n{}",
                c.describe()
            );
        }
        Ok(())
    });

    // A second, un-asserted pass to collect coverage statistics. Cheap, and it
    // is the only way to know the taxonomy is being exercised.
    let mut r = runner();
    r.run(&case(), |c| {
        {
            let mut seen = kinds.lock().expect("uncontended");
            for m in c.ours_mutations.iter().chain(&c.theirs_mutations) {
                seen.insert(m.kind.tag());
            }
        }
        if c.ours != c.base && c.theirs != c.base {
            both_sides_changed.fetch_add(1, Ordering::Relaxed);
        }
        if c.ours != c.theirs {
            differed.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    })
    .expect("collection pass");

    let offered = OFFERED.load(Ordering::Relaxed);
    let applied = APPLIED.load(Ordering::Relaxed);
    let differed = differed.load(Ordering::Relaxed);
    let both_sides_changed = both_sides_changed.load(Ordering::Relaxed);
    let kinds = kinds.into_inner().expect("uncontended");
    assert!(
        applied * 2 >= offered,
        "only {applied} of {offered} mutations applied; the generator is mostly no-ops"
    );
    assert!(
        both_sides_changed * 2 >= CASES as usize,
        "only {both_sides_changed}/{CASES} cases changed both sides"
    );
    assert!(
        differed * 2 >= CASES as usize,
        "only {differed}/{CASES} cases had ours != theirs"
    );
    assert_eq!(
        kinds.len(),
        MutationKind::ALL.len(),
        "these mutation kinds never applied: {:?}",
        MutationKind::ALL
            .iter()
            .map(|k| k.tag())
            .filter(|t| !kinds.contains(t))
            .collect::<Vec<_>>()
    );
}

// ------------------------------------------------------------ identity laws

/// `merge(B, X, B) == X` and `merge(B, B, Y) == Y`, byte for byte.
///
/// The strongest statement of SPEC.md §4.6's invariant: if one side did
/// nothing, the output *is* the other side's file — not an equivalent file, the
/// same bytes. Any reformatting, any dropped comment, any normalised newline
/// shows up here immediately.
#[test]
fn one_sided_edits_come_through_untouched() {
    for_each_case("identity", |c| {
        let lang = c.language();

        let m = run(&c.base, &c.ours, &c.base, lang);
        prop_assert!(
            m.outcome.is_clean(),
            "merge(B, X, B) conflicted: {:?}\n{}",
            m.reasons(),
            c.describe()
        );
        prop_assert_eq!(&m.text(), &c.ours, "merge(B, X, B) != X\n{}", c.describe());

        let m = run(&c.base, &c.base, &c.theirs, lang);
        prop_assert!(
            m.outcome.is_clean(),
            "merge(B, B, Y) conflicted: {:?}\n{}",
            m.reasons(),
            c.describe()
        );
        prop_assert_eq!(
            &m.text(),
            &c.theirs,
            "merge(B, B, Y) != Y\n{}",
            c.describe()
        );
        Ok(())
    });
}

/// `merge(B, X, X) == X` — convergent edits, and `merge(B, B, B) == B`.
#[test]
fn convergent_and_null_edits_are_the_identity() {
    for_each_case("convergence", |c| {
        let lang = c.language();
        for text in [&c.ours, &c.theirs, &c.base] {
            let m = run(&c.base, text, text, lang);
            prop_assert!(
                m.outcome.is_clean(),
                "merge(B, X, X) conflicted: {:?}\n{}",
                m.reasons(),
                c.describe()
            );
            prop_assert_eq!(&m.text(), text, "merge(B, X, X) != X\n{}", c.describe());
        }
        Ok(())
    });
}

/// An untouched side reduces the merged tree to a single splice of the other.
///
/// The structural half of the invariant above. Byte equality could in principle
/// be reached by a reconstruction that happened to produce the same text; this
/// asserts the merge actually *recognised* that one side did nothing.
#[test]
fn an_untouched_side_reduces_to_one_splice() {
    for_each_case("splice", |c| {
        let lang = c.language();
        for (ours, theirs, expect) in [
            (&c.ours, &c.base, sm_merge::Side::Ours),
            (&c.base, &c.theirs, sm_merge::Side::Theirs),
        ] {
            let m = run(&c.base, ours, theirs, lang);
            let root = m.outcome.tree.root();
            prop_assert!(
                matches!(m.outcome.tree.node(root), MergedNode::Splice { .. }),
                "expected one splice, got {:?}\n{}",
                m.outcome.tree.node(root),
                c.describe()
            );
            // Which side the splice names is only determined when exactly one
            // side moved. If the mutations all bounced (see `apply_all`), this
            // is `merge(B, B, B)` and either answer is right — asserting `Ours`
            // there would be asserting a tie-break, not the invariant.
            if ours != &c.base || theirs != &c.base {
                prop_assert_eq!(
                    m.outcome.tree.provenance(root).map(|(s, _)| s),
                    Some(expect),
                    "{}",
                    c.describe()
                );
            }
        }
        Ok(())
    });
}

// ----------------------------------------------------------------- symmetry

/// SPEC.md §5: the *decision* must not depend on which branch is "ours".
///
/// Two claims, and the second is the one with teeth:
///
/// 1. `conflict_count(B,X,Y) == conflict_count(B,Y,X)`.
/// 2. When both are clean, the two results are **the same program** — equal
///    structural hashes. They may differ in bytes, and legitimately do: the
///    side preference documented in `sm-merge`'s crate docs §3 splices *our*
///    revision when both sides converge, so mirroring can pick up the other
///    side's indentation. What it must never do is pick up different code.
#[test]
fn the_decision_is_symmetric() {
    for_each_case("symmetry", |c| {
        let lang = c.language();
        let forward = run(&c.base, &c.ours, &c.theirs, lang);
        let mirrored = run(&c.base, &c.theirs, &c.ours, lang);

        prop_assert_eq!(
            forward.outcome.conflicts.len(),
            mirrored.outcome.conflicts.len(),
            "conflict counts differ: {:?} vs {:?}\n{}",
            forward.reasons(),
            mirrored.reasons(),
            c.describe()
        );

        if forward.outcome.is_clean() {
            let a = ast_hash(&forward.text(), lang);
            let b = ast_hash(&mirrored.text(), lang);
            prop_assert!(
                a.is_some(),
                "forward output does not parse\n{}",
                c.describe()
            );
            prop_assert_eq!(
                a,
                b,
                "clean merges disagree on the program:\n--- forward ---\n{}--- mirrored ---\n{}{}",
                forward.text(),
                mirrored.text(),
                c.describe()
            );
        }
        Ok(())
    });
}

/// How often the *reasons* mirror, not just the count.
///
/// # What this found, and why it is a rate and not an assertion
///
/// `sm-emit`'s `conflict_reasons_mirror_exactly` asserts that mirroring a
/// scenario mirrors every conflict reason, and it holds over the whole
/// hand-written corpus. It does **not** hold in general, and this suite found
/// the counterexample within 300 cases:
///
/// ```text
/// base:   class Widget { … reset() { this.count = 0; } }  + function log(n) { console.log(n); }
/// ours:   wrapped the body of reset() *and* of log() in `if (wrapped) { … }`
/// theirs: moved `this.count = 0;` out of reset() and into log()
///
/// forward:  [ordered_insert_collision, delete_move]
/// mirrored: [ordered_insert_collision, ordered_insert_collision]
/// ```
///
/// Both directions conflict, and both conflict **twice** — so SPEC.md §5's
/// property ("`merge(B,X,Y)` conflicts ⟺ `merge(B,Y,X)` conflicts") holds, and
/// so does the stronger count property asserted above. What differs is only how
/// the second region is *labelled*: reached from one direction the engine sees
/// a statement moved into a subtree the other side deleted-by-wrapping, and
/// from the other it sees two insertions colliding at one anchor. Both
/// descriptions are true of the same situation.
///
/// Asserting exact mirroring here would therefore be asserting something false,
/// and silently dropping the check would lose a genuine tripwire — a *decision*
/// asymmetry would show up as a collapse in this rate. So it is measured, with
/// a floor well above the observed value, and the divergences are printed.
/// M5's divergence analysis is where the labelling gets tightened, if it is
/// worth tightening.
#[test]
fn conflict_reasons_usually_mirror() {
    let conflicting = AtomicUsize::new(0);
    let mirrored_exactly = AtomicUsize::new(0);
    let examples = std::sync::Mutex::new(Vec::<String>::new());

    for_each_case("reason mirroring", |c| {
        let lang = c.language();
        let forward = run(&c.base, &c.ours, &c.theirs, lang);
        if forward.outcome.is_clean() {
            return Ok(());
        }
        let back = run(&c.base, &c.theirs, &c.ours, lang);
        conflicting.fetch_add(1, Ordering::Relaxed);

        let expected: Vec<String> = forward
            .outcome
            .conflicts
            .iter()
            .map(|x| x.reason.mirrored().tag().to_owned())
            .collect();
        let actual: Vec<String> = back
            .outcome
            .conflicts
            .iter()
            .map(|x| x.reason.tag().to_owned())
            .collect();
        if expected == actual {
            mirrored_exactly.fetch_add(1, Ordering::Relaxed);
        } else if let Ok(mut ex) = examples.lock()
            && ex.len() < 3
        {
            ex.push(format!("{expected:?} vs {actual:?}\n{}", c.describe()));
        }
        Ok(())
    });

    let total = conflicting.load(Ordering::Relaxed);
    let exact = mirrored_exactly.load(Ordering::Relaxed);
    assert!(
        total > 20,
        "only {total} conflicting cases; the rate is not meaningful"
    );
    let rate = exact as f64 / total as f64;
    assert!(
        rate >= 0.90,
        "conflict reasons mirrored in only {exact}/{total} ({rate:.2}) of conflicting cases — \
         that is a decision asymmetry, not a labelling one. Examples:\n{}",
        examples.lock().expect("uncontended").join("\n\n"),
    );
}

// -------------------------------------------------------------- idempotence

/// A clean result `R` satisfies `merge(R, R, R) == R`, and re-merging it
/// against the inputs it came from does not conflict.
///
/// This is what makes the driver safe to run twice — which git does, on a
/// rebase that replays the same commit, and which a user does by re-running a
/// failed merge.
#[test]
fn a_clean_result_is_a_fixed_point() {
    for_each_case("idempotence", |c| {
        let lang = c.language();
        let first = run(&c.base, &c.ours, &c.theirs, lang);
        if !first.outcome.is_clean() {
            return Ok(());
        }
        let text = first.text();
        // A clean result that does not parse is caught by its own property
        // below; here it would only produce a confusing second failure.
        if ast_hash(&text, lang).is_none() {
            return Ok(());
        }
        let again = run(&text, &text, &text, lang);
        prop_assert!(
            again.outcome.is_clean(),
            "merge(R,R,R) conflicted: {:?}\n{}",
            again.reasons(),
            c.describe()
        );
        prop_assert_eq!(
            &again.text(),
            &text,
            "merge is not idempotent\n{}",
            c.describe()
        );
        Ok(())
    });
}

// ----------------------------------------------------------- parse stability

/// **Every conflict-free output reparses without errors.**
///
/// SPEC.md §5's parse-stability property, and the one correctness signal that
/// needs no ground truth — which is exactly why docs/prior-art.md §8.3.3 makes
/// it M5's headline automated metric. A clean merge that is not a program is a
/// wrong merge by any definition.
#[test]
fn every_clean_output_reparses() {
    let checked = AtomicUsize::new(0);
    for_each_case("parse stability", |c| {
        let lang = c.language();
        for (ours, theirs) in [(&c.ours, &c.theirs), (&c.theirs, &c.ours)] {
            let m = run(&c.base, ours, theirs, lang);
            if !m.outcome.is_clean() {
                continue;
            }
            let reparsed = sm_cst::parse(&m.result.bytes, lang).expect("reparse");
            prop_assert!(
                !reparsed.has_errors(),
                "clean output does not parse:\n--- OUTPUT ---\n{}{}",
                m.text(),
                c.describe()
            );
            checked.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    });
    assert!(
        checked.load(Ordering::Relaxed) > CASES as usize / 4,
        "only {} clean outputs were produced out of {} cases — the property is nearly vacuous",
        checked.load(Ordering::Relaxed),
        CASES * 2
    );
}

/// Conflicted output is not expected to parse, but its markers must be
/// well-formed: balanced, one per reported region, each at a line start.
#[test]
fn conflicted_output_is_well_formed() {
    for_each_case("markers", |c| {
        let lang = c.language();
        let m = run(&c.base, &c.ours, &c.theirs, lang);
        if m.outcome.is_clean() {
            return Ok(());
        }
        let text = m.text();
        let opens = text.lines().filter(|l| l.starts_with("<<<<<<<")).count();
        let seps = text.lines().filter(|l| *l == "=======").count();
        let closes = text.lines().filter(|l| l.starts_with(">>>>>>>")).count();
        prop_assert_eq!(opens, m.result.conflict_count, "{}", c.describe());
        prop_assert_eq!(opens, seps, "unbalanced markers\n{}", text);
        prop_assert_eq!(opens, closes, "unbalanced markers\n{}", text);
        prop_assert!(text.ends_with('\n'), "no final newline\n{}", text);
        Ok(())
    });
}

// -------------------------------------------------------- byte preservation

/// A clean merge invents nothing: zero synthesized bytes, every gap copied,
/// every node traceable to an input range.
#[test]
fn a_clean_merge_is_a_pure_splice() {
    for_each_case("byte preservation", |c| {
        let lang = c.language();
        let m = run(&c.base, &c.ours, &c.theirs, lang);
        if m.outcome.is_clean() {
            prop_assert_eq!(
                m.result.synthesized_bytes,
                0,
                "a clean merge synthesized bytes\n{}",
                c.describe()
            );
            for id in m.outcome.tree.walk() {
                prop_assert!(
                    matches!(m.outcome.tree.lead(id), Gap::Copied { .. }),
                    "a gap was synthesized\n{}",
                    c.describe()
                );
            }
        }
        // Provenance is total whether or not the merge was clean.
        for id in m.outcome.tree.walk() {
            match m.outcome.tree.node(id) {
                MergedNode::Conflict(_) => {}
                _ => prop_assert!(
                    m.outcome.tree.provenance(id).is_some(),
                    "node {} has no provenance\n{}",
                    id,
                    c.describe()
                ),
            }
        }
        Ok(())
    });
}

// --------------------------------------------------------------- determinism

/// Two runs over the same inputs agree byte for byte.
///
/// Cheap, and it is the property that catches a `HashMap` iteration creeping
/// into the decision path — which would make every other property here
/// intermittently true.
#[test]
fn merging_is_deterministic() {
    for_each_case("determinism", |c| {
        let lang = c.language();
        let a = run(&c.base, &c.ours, &c.theirs, lang);
        let b = run(&c.base, &c.ours, &c.theirs, lang);
        prop_assert_eq!(&a.result.bytes, &b.result.bytes, "{}", c.describe());
        prop_assert_eq!(
            &a.outcome.conflicts,
            &b.outcome.conflicts,
            "{}",
            c.describe()
        );
        prop_assert_eq!(&a.outcome.stats, &b.outcome.stats, "{}", c.describe());
        Ok(())
    });
}
