//! Token fusion: the bug, the fix, and the property that now holds.
//!
//! # The bug
//!
//! Found by M4b's corpus dry run — one case in 1154 mined real-world conflicts
//! (`oracle/graal`, `…/model/symbols/instructions/Call.java`), reduced to three
//! lines:
//!
//! ```text
//! base:   interface Call extends Instruction { … }
//! ours:   interface Call { … }                       (dropped `extends Instruction`)
//! theirs: public interface Call extends Instruction { … }   (added `public`)
//!
//! merged: publicinterface Call { … }
//! ```
//!
//! The merge decision was right — our removal and their addition both applied —
//! but the two tokens were written with nothing between them.
//!
//! The cause was in the whitespace-ownership rule (`sm-merge`'s crate docs, §9).
//! Every emitted child carries a **leading gap** copied from some revision.
//! Their `modifiers` node is an insertion at position 0, so it took its gap from
//! their file, where it is also first and its gap is empty. The `interface`
//! keyword after it came from our file, where *it* was first, so its gap was
//! empty too. Two empty gaps in a row, and the tokens fused.
//!
//! The rule was right about which *revision* to copy layout from. What it had
//! no notion of is that a gap is not a property of an item but of a **pair** of
//! items: an inserted item changes what precedes its successor, and a gap that
//! was empty because the item was first is not still valid once it is not.
//!
//! The trigger is therefore **any insertion in front of an item that was first
//! in the child list its gap was copied from** — not only at the top of a file.
//! `int a = 1;` versus `static int a = 1;` hits it too, and *that* one is the
//! dangerous shape: `staticint a = 2;` parses perfectly well, because
//! `tree-sitter-java` reads `staticint` as a type name. Reparsing the output
//! does not catch it.
//!
//! # The fix
//!
//! Two layers, both documented where they live:
//!
//! 1. **`sm-merge` validates the gap it copies** (crate docs, §9). When the
//!    item's predecessor in the merged list is not the one the gap was measured
//!    against, the gap is replaced — first by one a revision wrote for exactly
//!    this pair, and failing that, if the gap is empty, by any whitespace-only
//!    gap the item has elsewhere. Substitutes are whitespace or nothing, never a
//!    gap holding a comment, so the output stays a pure splice: every case below
//!    reports **zero** synthesized bytes.
//! 2. **`sm-emit` keeps a lexical backstop** (crate docs, "Token separation").
//!    If two items still meet with no bytes between them and those bytes would
//!    lex together, one space is written and counted in
//!    `EmitResult::synthesized_separators`. It does not fire on anything in this
//!    file; it exists so the invariant holds by construction rather than by
//!    exhaustion of the cases anyone thought of.
//!
//! `sm merge`'s output self-check (fallback rung 7) still refuses any clean
//! result containing a token that appears in none of the three inputs. It was
//! written because of this bug and it stays as belt-and-braces: a fusion that
//! slipped past both layers costs a resolve, not a corrupted file.
//!
//! # The property
//!
//! > Between two adjacent emitted tokens that would lex differently when
//! > juxtaposed, there is at least one separator byte.
//!
//! `no_clean_output_contains_a_token_from_no_input` in `properties.rs` is the
//! general statement, checked over the whole scenario corpus. This file is the
//! adversarial table: the shapes that reach the rule.

mod support;

use support::{Merged, java, run, typescript};

/// Assert the merge is clean, reparses, splices only real bytes, and fused
/// nothing.
#[track_caller]
fn assert_sound(name: &str, m: &Merged, lang: &'static dyn sm_cst::Language) {
    assert!(m.is_clean(), "{name}: {:?}", m.reasons());
    assert!(
        !sm_cst::parse(&m.result.bytes, lang)
            .expect("reparse")
            .has_errors(),
        "{name}: output does not parse\n{}",
        m.text()
    );
    assert_eq!(
        m.result.synthesized_bytes,
        0,
        "{name}: the gap repair should have found real bytes\n{}",
        m.text()
    );
    let fabricated = no_input_tokens(m, lang);
    assert!(
        fabricated.is_empty(),
        "{name}: {fabricated:?} appear in none of the inputs\n{}",
        m.text()
    );
}

/// Every token of the output that is a token of none of the three inputs.
///
/// The same question `sm merge`'s `fabricated_token` self-check asks, and the
/// exact statement of the property: a splicing emitter can only ever write
/// tokens it was given, so a fused token — or a split one — is one that is in
/// no input. Comments and multi-line leaves are excluded because the reindenter
/// is allowed to rewrite their interior whitespace.
fn no_input_tokens(m: &Merged, lang: &'static dyn sm_cst::Language) -> Vec<String> {
    let output = sm_cst::parse(&m.result.bytes, lang).expect("reparse");
    let mut known: std::collections::HashSet<&[u8]> = std::collections::HashSet::new();
    for tree in [&m.base, &m.ours, &m.theirs] {
        for id in tree.leaves() {
            known.insert(tree.node_bytes(id));
        }
    }
    let mut out = Vec::new();
    for id in output.leaves() {
        let node = output.node(id);
        if node.is_extra || node.kind.contains("comment") {
            continue;
        }
        let text = output.node_bytes(id);
        if text.is_empty()
            || text.contains(&b'\n')
            || text.iter().all(u8::is_ascii_whitespace)
            || known.contains(text)
        {
            continue;
        }
        out.push(String::from_utf8_lossy(text).into_owned());
    }
    out
}

/// **The corpus case**, from `oracle/graal`. Was `publicinterface`.
#[test]
fn an_insertion_before_the_first_item_keeps_its_separator() {
    let m = run(
        "interface Call extends Instruction {\n    void a();\n}\n",
        "interface Call {\n    void a();\n}\n",
        "public interface Call extends Instruction {\n    void a();\n}\n",
        java(),
    );
    assert_sound("graal", &m, java());

    // The merge decision is unchanged: our removal and their addition apply.
    assert!(
        m.text().starts_with("public interface Call {"),
        "{}",
        m.text()
    );
    assert!(!m.text().contains("extends"), "{}", m.text());
}

/// **The dangerous variant: fused output that still parses.** Was `staticint`.
///
/// `int a = 2;` merged against `static int a = 1;` used to produce
/// `staticint a = 2;`, which `tree-sitter-java` accepts as a declaration of `a`
/// with type `staticint`. Reparsing waved it through; before M4b's token check
/// it would have been written to the user's file as a clean merge, exit 0, and
/// code that does not compile.
#[test]
fn a_modifier_added_where_there_was_none_does_not_fuse_with_the_type() {
    let m = run(
        "class C {\n    int a = 1;\n}\n",
        "class C {\n    int a = 2;\n}\n",
        "class C {\n    static int a = 1;\n}\n",
        java(),
    );
    assert_sound("staticint", &m, java());
    assert_eq!(m.text(), "class C {\n    static int a = 2;\n}\n");
}

/// The same triple with the sides swapped. The gap repair prefers the framing
/// side, so the two directions take different branches through it and both have
/// to be pinned.
#[test]
fn the_modifier_case_is_the_same_with_the_sides_swapped() {
    let m = run(
        "class C {\n    int a = 1;\n}\n",
        "class C {\n    static int a = 1;\n}\n",
        "class C {\n    int a = 2;\n}\n",
        java(),
    );
    assert_sound("staticint mirrored", &m, java());
    assert_eq!(m.text(), "class C {\n    static int a = 2;\n}\n");
}

/// **The splitting direction**: the leading item is *removed* rather than
/// inserted, so the item behind it inherits a gap that was measured against
/// something no longer there.
///
/// This one never fused — the inherited gap is a space, not nothing — but it
/// was visibly wrong: the surviving `int` kept the separator that used to
/// follow `static`, and the field came out indented by five spaces instead of
/// four. It is the same rule violation seen from the other side, and step 1 of
/// the repair (take the gap a revision wrote for *this* pair) fixes it.
#[test]
fn removing_the_leading_item_does_not_leave_its_separator_behind() {
    let m = run(
        "class C {\n    static int a = 1;\n}\n",
        "class C {\n    static int a = 2;\n}\n",
        "class C {\n    int a = 1;\n}\n",
        java(),
    );
    assert_sound("modifier removed", &m, java());
    assert_eq!(m.text(), "class C {\n    int a = 2;\n}\n");
}

/// The adversarial table: every shape that puts a new item in front of one that
/// was first in its own revision's child list.
///
/// Each is asserted three ways — clean, reparses, no token that is in no input —
/// plus the exact bytes, because "it parses" is a much weaker claim than "it is
/// what a human would have written" and this rule is about layout.
#[test]
fn insertions_in_front_of_a_list_head_keep_a_separator() {
    let cases: &[(&str, &str, &str, &str, &str)] = &[
        (
            "an annotation added in front of a method",
            "class C {\n    void f() {}\n}\n",
            "class C {\n    void f() { g(); }\n}\n",
            "class C {\n    @Override\n    void f() {}\n}\n",
            "class C {\n    @Override\n    void f() { g(); }\n}\n",
        ),
        (
            "a modifier added in front of a parameter",
            "class C {\n    void f(int a) {}\n}\n",
            "class C {\n    void f(int a) { g(); }\n}\n",
            "class C {\n    void f(final int a) {}\n}\n",
            "class C {\n    void f(final int a) { g(); }\n}\n",
        ),
        (
            "a modifier in front of a generic type",
            "class C {\n    List<String> a = x;\n}\n",
            "class C {\n    List<String> a = y;\n}\n",
            "class C {\n    static List<String> a = x;\n}\n",
            "class C {\n    static List<String> a = y;\n}\n",
        ),
        (
            // `>>` and `<>` must stay glued: the emitter's separator predicate
            // deliberately excludes those pairs, and this is the case that
            // would break if it did not.
            "a modifier in front of nested type arguments",
            "class C {\n    Map<String,List<String>> a = new HashMap<>();\n}\n",
            "class C {\n    Map<String,List<String>> a = new HashMap<>(1);\n}\n",
            "class C {\n    static Map<String,List<String>> a = new HashMap<>();\n}\n",
            "class C {\n    static Map<String,List<String>> a = new HashMap<>(1);\n}\n",
        ),
        (
            "a package declaration added above the first import",
            "import java.util.List;\n\nclass C {}\n",
            "import java.util.List;\n\nclass C { int x; }\n",
            "package p;\n\nimport java.util.List;\n\nclass C {}\n",
            "package p;\n\nimport java.util.List;\n\nclass C { int x; }\n",
        ),
        (
            "a label in front of the first statement of a loop body",
            "class C {\n    void f() {\n        while (b) {\n            g();\n        }\n    }\n}\n",
            "class C {\n    void f() {\n        while (b) {\n            g2();\n        }\n    }\n}\n",
            "class C {\n    void f() {\n        outer: while (b) {\n            g();\n        }\n    }\n}\n",
            "class C {\n    void f() {\n        outer: while (b) {\n            g2();\n        }\n    }\n}\n",
        ),
    ];
    for (name, base, ours, theirs, expected) in cases {
        let m = run(base, ours, theirs, java());
        assert_sound(name, &m, java());
        assert_eq!(m.text(), *expected, "{name}");
    }
}

/// TypeScript reaches the same rule through the same code. `export`, `static`,
/// `readonly`, `async` and a decorator are all insertions in front of something
/// that was the first item of its list.
#[test]
fn typescript_insertions_in_front_of_a_list_head_keep_a_separator() {
    let cases: &[(&str, &str, &str, &str, &str)] = &[
        (
            "export added in front of a function",
            "function f() {}\n",
            "function f() { g(); }\n",
            "export function f() {}\n",
            "export function f() { g(); }\n",
        ),
        (
            "static added in front of a class field",
            "class C {\n  a = 1;\n}\n",
            "class C {\n  a = 2;\n}\n",
            "class C {\n  static a = 1;\n}\n",
            "class C {\n  static a = 2;\n}\n",
        ),
        (
            "readonly added in front of an interface member",
            "interface I {\n  a: number;\n}\n",
            "interface I {\n  a: string;\n}\n",
            "interface I {\n  readonly a: number;\n}\n",
            "interface I {\n  readonly a: string;\n}\n",
        ),
        (
            "async added in front of a method",
            "class C {\n  m() { return 1; }\n}\n",
            "class C {\n  m() { return 2; }\n}\n",
            "class C {\n  async m() { return 1; }\n}\n",
            "class C {\n  async m() { return 2; }\n}\n",
        ),
        (
            "a type annotation added to the first parameter",
            "function f(a) { return a; }\n",
            "function f(a) { return a + 1; }\n",
            "function f(a: number) { return a; }\n",
            "function f(a: number) { return a + 1; }\n",
        ),
    ];
    for (name, base, ours, theirs, expected) in cases {
        let m = run(base, ours, theirs, typescript());
        assert_sound(name, &m, typescript());
        assert_eq!(m.text(), *expected, "{name}");
    }
}

/// **Both sides add a modifier to the same declaration.**
///
/// Two insertions at one anchor, so this is an `OrderedInsertCollision` and the
/// answer is a conflict, not a merge. It is here because it is the obvious next
/// thing to try after the cases above, and because a rule that "fixed" it by
/// concatenating the two modifiers would be exactly the silently wrong merge
/// SPEC.md §0.4 rules out.
#[test]
fn both_sides_adding_a_modifier_conflicts_rather_than_concatenating() {
    let m = run(
        "class C {\n    int a = 1;\n}\n",
        "class C {\n    public int a = 1;\n}\n",
        "class C {\n    static int a = 1;\n}\n",
        java(),
    );
    assert!(!m.is_clean(), "{}", m.text());
    assert_eq!(m.reasons(), ["ordered_insert_collision@field_declaration"]);
    assert!(!m.text().contains("publicstatic"), "{}", m.text());
    assert!(!m.text().contains("public static"), "{}", m.text());
}

/// Where the bug never reached, kept so the diagnosis stays checkable rather
/// than merely plausible.
///
/// An insertion whose successor was not first in its own revision's child list
/// keeps a real gap, because that gap was copied from a position that had
/// something before it. Statement insertions into a block and member insertions
/// into a class body are both this shape — which is why the common cases were
/// always fine and this bug took a corpus run to find.
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
        assert_sound(name, &m, java());
        for fused in ["staticint", "publicint", "pre()a", "intz"] {
            assert!(!m.text().contains(fused), "{name}: {fused}\n{}", m.text());
        }
    }
}

/// The emitter's backstop, exercised directly.
///
/// `sm_emit::fuses` is the predicate the backstop consults, and it is the one
/// place where a wrong answer is silent in both directions: a missed pair ships
/// fused source (caught later by the driver, at the cost of a resolve), and an
/// invented pair corrupts correct output. The excluded pairs matter as much as
/// the included ones.
#[test]
fn the_separator_predicate_covers_the_fusing_pairs_and_no_others() {
    for (a, b, why) in [
        (b'c', b't', "static|int"),
        (b'0', b'x', "digit then letter"),
        (b'_', b'a', "underscore is an identifier byte"),
        (b'a', b'$', "dollar is an identifier byte in Java"),
        (0xC3, 0xA9, "a UTF-8 pair is never a delimiter"),
        (b'/', b'/', "would open a line comment"),
        (b'/', b'*', "would open a block comment"),
        (b'*', b'/', "would close a block comment"),
        (b'+', b'+', "increment"),
        (b'-', b'-', "decrement"),
        (b'=', b'=', "equality"),
        (b'<', b'=', "comparison"),
        (b'+', b'=', "compound assignment"),
        (b'&', b'&', "logical and"),
        (b'|', b'|', "logical or"),
        (b':', b':', "method reference"),
        (b'.', b'.', "spread"),
        (b'-', b'>', "lambda arrow"),
        (b'=', b'>', "typescript arrow"),
    ] {
        assert!(
            sm_emit::fuses(a, b),
            "{}{} should fuse ({why})",
            a as char,
            b as char
        );
    }
    for (a, b, why) in [
        (b'<', b'>', "new ArrayList<>()"),
        (b'>', b'>', "Map<String, List<X>>"),
        (
            b'<',
            b'<',
            "would be a shift, but generics open this way too",
        ),
        (b'(', b')', "empty argument list"),
        (b',', b'a', "a separator is already there"),
        (b'a', b' ', "whitespace is not a token byte"),
        (b';', b'}', "ordinary punctuation"),
        (b')', b'{', "ordinary punctuation"),
    ] {
        assert!(
            !sm_emit::fuses(a, b),
            "{}{} must not be separated ({why})",
            a as char,
            b as char
        );
    }
}
