//! Name binding and merge-time semantic-conflict detection (SPEC.md §7, M6).
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use sm_merge::{MergeConfig, merge};
//!
//! let lang = sm_cst::languages::detect("Foo.java".as_ref()).expect("java");
//! let base = sm_cst::parse(&std::fs::read("base.java")?, lang)?;
//! let ours = sm_cst::parse(&std::fs::read("ours.java")?, lang)?;
//! let theirs = sm_cst::parse(&std::fs::read("theirs.java")?, lang)?;
//!
//! let outcome = merge(&base, &ours, &theirs, lang, &MergeConfig::for_language(lang));
//! for c in sm_bind::check(&outcome, &base, &ours, &theirs, lang) {
//!     eprintln!("{}", c.explanation);
//! }
//! # Ok(()) }
//! ```
//!
//! # What this is, stated honestly
//!
//! Git merges lines. It cannot see that one branch renamed `getUser` while the
//! other branch added a call to `getUser`: the two edits touch different lines,
//! the merge is clean, and the build breaks. Neither can any purely syntactic
//! structural merge, ours included up to M5.
//!
//! This crate resolves names. It builds a scope tree for each revision *and for
//! the candidate merge result*, re-resolves every reference in the merged
//! program, and reports the ones whose binding the merge changed.
//!
//! ## What is novel, and what is not
//!
//! `docs/prior-art.md` §9 is the authority; the short version:
//!
//! - **Detecting this class of conflict is not new.** It is a decade-old
//!   research subfield, under the name *build conflicts* or *semantic
//!   conflicts*. **Bucond** (Towqir, Shen et al., ASE 2022) detects cross-branch
//!   def-use violations in Java with reported 100% precision and 88–100%
//!   recall; its own motivating example is "one branch adds a reference to field
//!   F while the other removes F". **IntelliMerge** (Shen et al., OOPSLA 2019)
//!   builds Program Element Graphs and detects refactorings.
//!   **static-semantic-merge** runs a SOOT-based analysis after a textually
//!   clean merge. Do not claim otherwise.
//! - **What is new is the position in the pipeline and the cost.** Every tool
//!   above is a separate, Java-specific, post-hoc analysis that needs a
//!   compilable project — typically a build and a classpath. No production merge
//!   driver does it. This crate does it *inside the merge driver*, on the
//!   candidate merged tree, before the result is written, in-process, with no
//!   build, no classpath, no solver and no LLM (SPEC.md §9) — over the same
//!   language-agnostic tree-sitter substrate that produced the merge. Adding a
//!   language is a `Language` impl, not a new analyser.
//! - **The measurement is the deliverable.** How often merge-time reference
//!   breakage actually occurs in real merges is not a published number for any
//!   tool of this class. Producing it is what M6 is for.
//!
//! ## The limitation that must be stated first
//!
//! **A merge driver sees one file.** Git invokes it per path, with three blobs
//! and no project. So this crate can only see renames, deletions and captures
//! *within a single file*. The canonical rename-in-A / call-in-B case is
//! frequently **cross-file**, and cross-file is exactly where Bucond and
//! IntelliMerge operate. Within-file detection is real and worth measuring, but
//! it is a subset of the problem and any report must say so.
//!
//! Concretely: if `ours` renames `UserService.getUser` and `theirs` adds a call
//! to it from `OrderController.java`, nothing here will notice — the driver is
//! never shown both files at once.
//!
//! # The single-file honesty rule
//!
//! This is the design decision the whole crate rests on.
//!
//! > **A reference that does not resolve within the file is
//! > [`Resolution::Unresolved`], and `Unresolved` is never, by itself,
//! > reportable.**
//!
//! In one file, most names resolve to nothing: `System`, `Objects`, `List`,
//! every inherited member, every import target, every name a wildcard import
//! supplies. A checker that reported unresolved names would produce thousands of
//! false positives per repository and would be switched off within a minute.
//!
//! So every signal here is **differential** — it compares two resolutions of the
//! same reference, and both come from this same partial analysis:
//!
//! | origin revision | merged result | verdict |
//! |---|---|---|
//! | `Unresolved` | anything | **silent** — a pre-existing cross-file or library name |
//! | `Decl(d)` | `Unresolved` | [`SemanticConflictKind::BrokenReference`] |
//! | `Decl(d)` | `Decl(d)` | silent |
//! | `Decl(d)` | `Decl(e)`, `e` ≠ `d` | [`SemanticConflictKind::CapturedReference`] |
//!
//! "The origin revision" is the branch the reference's bytes came from,
//! recovered from [`sm_merge::MergedTree::provenance`]. A reference spliced from
//! `theirs` is resolved against `theirs`'s own scope tree; one that survived
//! unchanged from the ancestor is resolved against the ancestor's. That is what
//! makes the comparison meaningful: it asks *did the merge change what this name
//! means*, not *is this name resolvable*.
//!
//! Because the analysis is identical on both sides of the comparison, every
//! *systematic* weakness in it cancels out. An inherited method is unresolved in
//! both, so it is silent. A wildcard import is unresolved in both, so it is
//! silent. That is why the false-positive rate is near zero despite the resolver
//! being deliberately simple.
//!
//! # Identifier roles — why `is_identifier` was not enough
//!
//! `sm_cst::Language::is_identifier` answers a question about a *kind name*, and
//! that is not enough to resolve anything. `property_identifier` in TypeScript
//! and the `name` of a qualified Java `method_invocation` are names that resolve
//! against a **receiver's type**, not against any lexical scope. A resolver that
//! walked those out to the file scope would find nothing and report a broken
//! reference on *every property access and every qualified call in the file*.
//!
//! [`sm_cst::IdentifierRole`] is the fix: each name node is classified, using the
//! tree-sitter field name it occupies in its parent, as a declaration, a
//! lexically-resolved reference (value / type / callable namespace), a label, or
//! a [`sm_cst::IdentifierRole::MemberRef`] — the conservative catch-all, which is
//! never resolved and therefore never reported.
//!
//! # How the merged program is analysed
//!
//! Not by emitting bytes and reparsing. [`sm_merge::MergedTree`] is a plan, and
//! [`ScopeTree::build_merged`] walks the plan directly, descending into a
//! revision's own arena wherever the plan splices a subtree verbatim. Three
//! reasons are in [`scope`]'s module docs; the load-bearing one is that a merge
//! with conflicts in it emits `<<<<<<<` marker lines that do not parse, and this
//! way a [`sm_merge::MergedNode::Conflict`] is simply not walked — the regions
//! around it are still checked, and nothing inside it is ever reported.
//!
//! The price is that a [`Span`] is in the coordinates of the revision it came
//! from rather than of the merged file. See [`Span`] for why that costs nothing
//! for the *bytes*, and what the next wave would add to `sm-emit` to also get
//! the offset.
//!
//! # Every simplification, and which way it fails
//!
//! Under-resolving is safe here — it produces `Unresolved`, which is never
//! reported — so almost everything below is a **false negative** (a missed
//! conflict), which SPEC.md §0.4 explicitly prefers to a wrong answer.
//!
//! | simplification | consequence |
//! |---|---|
//! | **No inheritance.** A name inherited from a superclass in another file resolves to nothing. | False negative. It is also why a member-like name that only exists via a supertype must stay excluded from "resolves to nothing ⇒ conflict". |
//! | **No types, so no member resolution.** `user.getName()`, `o.foo`, `#x` are `MemberRef` and are never resolved. | False negative — a renamed method called *through a receiver* is not caught. This is the deliberate fix for the finding above. |
//! | **`this.x` is a member reference.** Java's `this.log` is not resolved even though in principle it could be. | False negative. Unqualified `log` *is* resolved, and that is the common form. |
//! | **Wildcard and on-demand imports declare nothing.** `import java.util.*;` | False negative for every name they supply. |
//! | **Java's nested-class rules are approximated.** A nested class sees every enclosing class's members, including instance members from a `static` nested class. | Over-resolves in a narrow case; both sides over-resolve identically, so it cancels. |
//! | **Overloads are not distinguished.** Two methods with one name are one declaration for signature purposes. | False negative on an overload-only change. |
//! | **Two same-named locals in sibling blocks share a signature** (see [`ScopeTree::signature`]). | False negative. Chosen over the alternative, which would report a spurious capture whenever a branch renamed an enclosing method. |
//! | **TypeScript's TDZ is approximated**: `let`/`const` become visible at the end of their own declaration, not at the top of the block. | False negative for a use-before-declare, which does not compile anyway. |
//! | **Named function and class *expressions* declare nothing** (`const f = function g(){}`). | False negative. Registering them would leak `g` into the enclosing scope, which could over-resolve. |
//! | **Labels are not resolved at all.** | False negative, and a small one: a label never crosses a file. |
//! | **A capture contained entirely in one branch is not reported.** If `theirs` adds a shadowing local *and* `theirs` also contains the statement it captures, the merged binding equals `theirs`'s binding. | Correct, not a limitation: that branch compiled and shipped that way on its own, so the merge introduced nothing. Only *cross-branch* captures — the reference from one side, the shadowing declaration from the other — are merge-induced, and those are reported. |
//! | **A file that does not parse is analysed anyway.** tree-sitter recovers; the scopes are whatever the recovered tree says. | Both sides see the same recovered tree, so it cancels. The driver should fall back to a line merge on a parse error regardless (SPEC.md §4.7). |
//!
//! The one direction that can produce a **false positive** is the signature
//! comparison behind `CapturedReference`: if one branch moves a declaration to a
//! structurally different scope path while the other branch references it, the
//! two signatures differ and the reference looks captured. The signature
//! deliberately excludes enclosing *names* for exactly this reason — renaming a
//! method must not make its locals look like different declarations — which
//! removes the common case. What is left is genuine restructuring, which is
//! arguably worth a report.
//!
//! # Suggested integration (next wave)
//!
//! [`check()`] is report-only and has no opinion about what the driver does with
//! the result. The recommendation, given SPEC.md §0.4:
//!
//! - `--semantic=off` — do not run it.
//! - `--semantic=report` (suggested default) — run it, print each
//!   [`SemanticConflict::explanation`] to stderr, and **do not** change the exit
//!   code. A merge driver that starts refusing merges on a new heuristic is a
//!   merge driver that gets uninstalled.
//! - `--semantic=conflict` — additionally treat a non-empty result as
//!   conflicting (exit non-zero), for teams that want the guarantee.
//!
//! If the goal is to catch the *dangerous* case, run it when the tree merge is
//! otherwise clean: a file that already carries conflict markers is going to be
//! read by a human anyway.
//!
//! # Cost
//!
//! Four scope-tree builds — the three revisions plus the merged program — and
//! one scope-chain walk per reference. No parsing, no matching, no I/O. Measured
//! on a synthetic 167 KB / 84k-node Java file with 8,002 lexical references:
//! **125 ms**, against 280 ms to parse the same three files and 168 ms to merge
//! them. Real Java files are two orders of magnitude smaller, so the check is
//! comfortably inside SPEC.md §6.2's p99 < 1 s budget and is never the dominant
//! cost.

pub mod check;
pub mod scope;

pub use check::{
    CheckReport, CheckStats, DeclInfo, SemanticConflict, SemanticConflictKind, Span, check,
    check_report,
};
pub use scope::{
    Decl, DeclId, DeclSignature, NodeRef, RefId, RefRole, Reference, Resolution, Revisions, Scope,
    ScopeId, ScopeTree,
};
