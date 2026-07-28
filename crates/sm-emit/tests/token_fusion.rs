//! A known bug, pinned: two adjacent items can be emitted with no separator.
//!
//! # What happens
//!
//! Found by M4b's corpus dry run — one case in 1154 mined real-world conflicts
//! (`oracle/graal`, `…/model/symbols/instructions/Call.java`), reduced to three
//! lines here:
//!
//! ```text
//! base:   interface Call extends Instruction { … }
//! ours:   interface Call { … }                       (dropped `extends Instruction`)
//! theirs: public interface Call extends Instruction { … }   (added `public`)
//!
//! merged: publicinterface Call { … }
//! ```
//!
//! The merge is *clean* — it correctly applies our removal and their addition —
//! but the two tokens are written with nothing between them.
//!
//! # Why
//!
//! `sm-merge`'s whitespace-ownership rule (crate docs §9) gives every emitted
//! child a **leading gap** copied from some revision. Their `modifiers` node is
//! an insertion at position 0, so it takes its gap from their file, where it is
//! also first and its gap is empty. The `interface` keyword after it comes from
//! our file, where *it* was first, so its gap is empty too. Two empty gaps in a
//! row, and the tokens fuse.
//!
//! The rule is right about which *revision* to copy layout from. What it has no
//! notion of is that a gap between two items can be **required** rather than
//! merely cosmetic: an inserted item changes what precedes its successor, and a
//! gap that was empty because the item was first is not still valid once it is
//! not.
//!
//! The trigger is therefore **any insertion in front of an item that was first
//! in the child list its gap was copied from** — not only at the top of a file.
//! `int a = 1;` versus `static int a = 1;` hits it too, because in our revision
//! `int` is the first item of the field declaration's content sequence. Adding
//! a modifier to a declaration the other branch also edited is not an exotic
//! shape, which is why the second test below matters more than the first.
//!
//! # Why it is not fixed here
//!
//! Two candidate fixes, both of which are design changes to `sm-merge` /
//! `sm-emit` rather than local repairs, and both of which belong with M5's
//! divergence analysis and its evidence:
//!
//! 1. **Fix the gap selection**: when an item's predecessor in the merged list
//!    differs from its predecessor in the revision its gap came from, that gap
//!    is not reusable. Precise, but it needs a fallback answer for the case
//!    where no revision has the right gap.
//! 2. **Synthesize a separator in the emitter**: before writing a child whose
//!    lead is empty, if the last byte written and the first byte to be written
//!    would lex as one token, write one space and count it in
//!    `EmitResult::synthesized_bytes`. SPEC.md §5 explicitly allows "an
//!    explicitly synthesized token", so this is within the contract — but it
//!    makes `synthesized_bytes == 0` no longer true of every clean merge, which
//!    is currently asserted in three places including `sm-cli`'s pre-write
//!    self-check.
//!
//! # What protects users today
//!
//! `sm merge`'s self-check (fallback rung 7) refuses to write a clean result
//! that contains a **token appearing in none of the three inputs**, and falls
//! back to `git merge-file` instead. So this bug costs a resolve; it does not
//! reach a file.
//!
//! The token check exists *because of this bug*, and specifically because
//! reparsing is not enough to catch it: `staticint a = 2;` parses perfectly
//! well — `tree-sitter-java` reads `staticint` as a type name. Had the driver
//! only checked that its output parses, this would have been written to a
//! user's file as a clean merge that does not compile, which is the failure
//! SPEC.md §0.4 rules out absolutely. See `fabricated_token` in `sm-cli`'s
//! `merge` module.

mod support;

use support::{java, run};

/// The reduced case. Pins the current, wrong output so a fix shows up as a
/// deliberate change to this file rather than as a silent one.
#[test]
fn an_insertion_before_the_first_item_can_fuse_with_it() {
    let m = run(
        "interface Call extends Instruction {\n    void a();\n}\n",
        "interface Call {\n    void a();\n}\n",
        "public interface Call extends Instruction {\n    void a();\n}\n",
        java(),
    );

    // The merge decision is right: our removal and their addition both applied.
    assert!(m.is_clean(), "{:?}", m.reasons());
    assert!(m.text().contains("public"), "{}", m.text());
    assert!(!m.text().contains("extends"), "{}", m.text());

    // The rendering is wrong, and this is the pin. When the bug is fixed, this
    // assertion fails and the two below it start passing; swap them over and
    // delete this one.
    assert!(
        m.text().starts_with("publicinterface"),
        "the token-fusion bug appears to be fixed — good. Replace this test's \
         body with the two assertions below and rewrite the module docs.\n{}",
        m.text()
    );

    // The behaviour this *should* have:
    //   assert!(m.text().starts_with("public interface"));
    //   assert!(!m.reparse().has_errors());
    assert!(
        sm_cst::parse(&m.result.bytes, java())
            .expect("parse")
            .has_errors(),
        "output parses, so the pin above is stale"
    );
}

/// **The dangerous variant: fused output that still parses.**
///
/// `int a = 2;` merged against `static int a = 1;` produces
/// `staticint a = 2;`, and `tree-sitter-java` accepts it — `staticint` is a
/// perfectly good type name. So this one is *not* caught by reparsing, and
/// before M4b it would have been written to the user's file: a clean merge,
/// exit 0, and code that does not compile.
///
/// It is caught by `sm merge`'s token check (`fabricated_token` in
/// `sm-cli`'s `merge` module), which asks a stronger question — is every token
/// in the output a token of some input? — and answers no. That check exists
/// because of this case.
#[test]
fn a_fused_token_can_still_parse_which_is_the_dangerous_case() {
    let m = run(
        "class C {\n    int a = 1;\n}\n",
        "class C {\n    int a = 2;\n}\n",
        "class C {\n    static int a = 1;\n}\n",
        java(),
    );
    assert!(m.is_clean(), "{:?}", m.reasons());
    assert!(
        m.text().contains("staticint"),
        "the fusion bug appears to be fixed; update this test and the module docs\n{}",
        m.text()
    );
    assert!(
        !sm_cst::parse(&m.result.bytes, java())
            .expect("parse")
            .has_errors(),
        "this case is only interesting because the broken output parses\n{}",
        m.text()
    );
}

/// Where the bug does **not** reach, so the diagnosis above is checkable rather
/// than merely plausible.
///
/// An insertion whose successor was not first in its own revision's child list
/// keeps a real gap, because that gap was copied from a position that had
/// something before it. Statement insertions into a block and member insertions
/// into a class body are both this shape — which is why the common cases are
/// fine and this bug took a corpus run to find.
#[test]
fn insertions_that_are_not_at_a_list_head_keep_their_separator() {
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "a statement inserted into a block",
            "class C {\n    void f() {\n        a();\n    }\n}\n",
            "class C {\n    void f() {\n        a2();\n    }\n}\n",
            "class C {\n    void f() {\n        pre();\n        a();\n    }\n}\n",
        ),
        (
            "a member inserted before the first member",
            "class C {\n    int a = 1;\n}\n",
            "class C {\n    int a = 2;\n}\n",
            "class C {\n    int z = 0;\n    int a = 1;\n}\n",
        ),
        (
            "a modifier added where one already existed",
            "class C {\n    public int a = 1;\n}\n",
            "class C {\n    public int a = 2;\n}\n",
            "class C {\n    public static int a = 1;\n}\n",
        ),
    ];
    for (name, base, ours, theirs) in cases {
        let m = run(base, ours, theirs, java());
        assert!(m.is_clean(), "{name}: {:?}", m.reasons());
        assert!(
            !sm_cst::parse(&m.result.bytes, java())
                .expect("parse")
                .has_errors(),
            "{name}:\n{}",
            m.text()
        );
        // No two identifier-ish characters ran together across an item join.
        for fused in ["staticint", "publicint", "pre()a", "intz"] {
            assert!(!m.text().contains(fused), "{name}: {fused}\n{}", m.text());
        }
    }
}
