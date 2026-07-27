//! Trivia attachment over TypeScript.
//!
//! **This file is the point of the whole TypeScript exercise.** The attachment
//! pass in `src/trivia.rs` is written entirely against the [`Language`] trait —
//! it asks `is_comment` and nothing else about the language — so if the
//! abstraction holds, the policy documented in that module's doc comment should
//! produce the *same answers* on TypeScript as it does on Java, with no change to
//! `trivia.rs` at all. Not one byte of `trivia.rs`, `parse.rs`, `arena.rs` or
//! `render.rs` was touched to make these pass.
//!
//! The scenarios below are deliberately the same ones `tests/trivia.rs` uses for
//! Java — trailing on the same line, a leading JSDoc block, a blank line
//! floating a banner, a run of comments attaching as a unit, the
//! trailing-beats-leading tie-break — restated in TypeScript syntax. Where the
//! answer differs from Java's, it is because the *grammar shape* differs, and
//! that is called out in the test.
//!
//! Structure mirrors `tests/trivia.rs`: table-driven over small inline snippets,
//! each asserting the whole list of comments in the snippet and where each one
//! landed, so a policy change shows up as a diff of the policy. Every assertion
//! also re-checks the structural invariants and the attachment back-pointers.

mod support;

use std::collections::HashSet;

use sm_cst::{
    Attachment, Language, NodeId, SourceTree, TriviaConfig, invariants, parse_with_trivia_config,
};

// ---------------------------------------------------------------- helpers --

fn parse_src(src: &str) -> SourceTree {
    parse_src_in(support::typescript(), src)
}

fn parse_src_in(lang: &'static dyn Language, src: &str) -> SourceTree {
    parse_with_trivia_config(src.as_bytes(), lang, &TriviaConfig::DEFAULT)
        .expect("the snippet should parse")
}

fn parse_src_with(src: &str, config: &TriviaConfig) -> SourceTree {
    parse_with_trivia_config(src.as_bytes(), support::typescript(), config)
        .expect("the snippet should parse")
}

fn lang_of(tree: &SourceTree) -> &'static dyn Language {
    sm_cst::languages::by_name(tree.language_name()).expect("the tree's language is registered")
}

/// Every comment in the tree, in source order. Preorder visits children in
/// source order and comments are always leaves, so ascending ID order *is*
/// source order here.
fn comment_ids(tree: &SourceTree) -> Vec<NodeId> {
    let lang = lang_of(tree);
    tree.ids()
        .filter(|&id| lang.is_comment(tree.node(id).kind))
        .collect()
}

/// Collapse a node's text to one whitespace-normalised, truncated line, so a
/// multi-line JSDoc block can appear in a one-line expectation.
fn one_line(tree: &SourceTree, id: NodeId) -> String {
    let text = tree.node_text(id);
    let mut flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 34 {
        flat = flat.chars().take(31).collect::<String>() + "...";
    }
    flat
}

/// `method_definition `load`` — the kind, plus the declared name when there is
/// an obvious one.
///
/// TypeScript needs a wider net than the Java version of this helper: a class's
/// name is a `type_identifier`, a method's is a `property_identifier` and a
/// function's is an `identifier`, where `tree-sitter-java` uses `identifier` for
/// nearly all of them.
fn describe_owner(tree: &SourceTree, id: NodeId) -> String {
    const NAME_KINDS: &[&str] = &[
        "identifier",
        "type_identifier",
        "property_identifier",
        "private_property_identifier",
        "shorthand_property_identifier",
        "shorthand_property_identifier_pattern",
    ];
    let node = tree.node(id);
    if NAME_KINDS.contains(&node.kind) {
        return format!("{} `{}`", node.kind, tree.node_text(id));
    }
    let named = tree
        .children(id)
        .chain(tree.descendants(id))
        .find(|&c| NAME_KINDS.contains(&tree.node(c).kind));
    match named {
        Some(name) => format!("{} `{}`", node.kind, tree.node_text(name)),
        None => node.kind.to_string(),
    }
}

/// One `"<comment text> => <where it went>"` line per comment, in source order.
fn summarize(tree: &SourceTree) -> Vec<String> {
    comment_ids(tree)
        .into_iter()
        .map(|id| {
            let where_ = match tree.node(id).attachment {
                Attachment::Leading(owner) => format!("leading {}", describe_owner(tree, owner)),
                Attachment::Trailing(owner) => format!("trailing {}", describe_owner(tree, owner)),
                Attachment::Floating => "floating".to_string(),
            };
            format!("{} => {}", one_line(tree, id), where_)
        })
        .collect()
}

/// The properties attachment must preserve no matter what it decides. Copied in
/// spirit from `tests/trivia.rs`, because a second language is exactly where a
/// pass that quietly restructured the tree would first show up.
fn check_consistency(tree: &SourceTree) {
    assert_eq!(
        invariants::check_all(tree),
        Vec::new(),
        "attachment must not break any structural invariant"
    );

    let lang = lang_of(tree);
    for id in tree.ids() {
        let node = tree.node(id);

        if lang.is_comment(node.kind) {
            let parent = node.parent.expect("a comment is never the root");
            assert!(
                tree.children(parent).any(|c| c == id),
                "comment {id} left its parent's child list"
            );
            assert!(
                node.leading_trivia.is_empty() && node.trailing_trivia.is_empty(),
                "comment {id} was given trivia of its own"
            );
        } else {
            assert_eq!(
                node.attachment,
                Attachment::Floating,
                "non-comment node {id} ({}) must stay floating",
                node.kind
            );
        }

        for &t in &node.leading_trivia {
            assert_eq!(tree.node(t).attachment, Attachment::Leading(id));
        }
        for &t in &node.trailing_trivia {
            assert_eq!(tree.node(t).attachment, Attachment::Trailing(id));
        }
        match node.attachment {
            Attachment::Leading(owner) => {
                assert!(tree.node(owner).leading_trivia.contains(&id));
                assert_eq!(tree.node(owner).parent, node.parent, "owner is a sibling");
            }
            Attachment::Trailing(owner) => {
                assert!(tree.node(owner).trailing_trivia.contains(&id));
                assert_eq!(tree.node(owner).parent, node.parent, "owner is a sibling");
            }
            Attachment::Floating => {}
        }
    }

    let mut seen: HashSet<NodeId> = HashSet::new();
    for id in tree.ids() {
        for list in [
            &tree.node(id).leading_trivia,
            &tree.node(id).trailing_trivia,
        ] {
            assert!(
                list.windows(2).all(|w| w[0] < w[1]),
                "trivia of {id} is not in source order: {list:?}"
            );
            for &t in list {
                assert!(seen.insert(t), "comment {t} is attached twice");
            }
        }
    }
}

#[track_caller]
fn assert_attachments(src: &str, expected: &[&str]) {
    assert_attachments_in(support::typescript(), src, expected);
}

#[track_caller]
fn assert_attachments_in(lang: &'static dyn Language, src: &str, expected: &[&str]) {
    let tree = parse_src_in(lang, src);
    assert!(
        !tree.has_errors(),
        "snippet should parse cleanly under {}:\n{src}",
        lang.name()
    );
    check_consistency(&tree);
    let actual = summarize(&tree);
    let expected: Vec<String> = expected.iter().map(|s| (*s).to_string()).collect();
    assert_eq!(actual, expected, "\n--- source ---\n{src}");
}

#[track_caller]
fn assert_attachments_with(src: &str, config: &TriviaConfig, expected: &[&str]) {
    let tree = parse_src_with(src, config);
    check_consistency(&tree);
    let actual = summarize(&tree);
    let expected: Vec<String> = expected.iter().map(|s| (*s).to_string()).collect();
    assert_eq!(actual, expected, "\n--- source ---\n{src}\n{config:?}");
}

// -------------------------------------------------------- rule 1: trailing --

#[test]
fn a_comment_on_the_line_a_sibling_ends_on_trails_it() {
    assert_attachments(
        "const x: number = 1; // like this\n",
        &["// like this => trailing lexical_declaration `x`"],
    );
}

#[test]
fn trailing_works_at_every_nesting_level() {
    assert_attachments(
        "function f() {\n  const a = 1; // on the statement\n}\n",
        &["// on the statement => trailing lexical_declaration `a`"],
    );
    assert_attachments(
        "for (const k in o) {\n  g(k); // inside the loop\n}\n",
        &["// inside the loop => trailing expression_statement `g`"],
    );
}

/// A comment after a whole class attaches to the `class_body`, not to the
/// `class_declaration`.
///
/// Java's answer for `class A {} // c` is `Trailing(class_declaration)`, because
/// there the comment ends up as a sibling of the declaration in `program`.
/// `tree-sitter-typescript` leaves the comment *inside* the
/// `class_declaration`, where the only preceding candidate sibling is the
/// `class_body`. The policy is identical — "the nearest preceding candidate
/// sibling that ends on this line" — and the two answers differ only because the
/// grammars nest the extra differently, which is exactly the situation decision
/// 18 (attachment runs per child list, never across a parent boundary) was
/// written for. Pinned here so it cannot change silently.
#[test]
fn a_comment_after_a_class_attaches_to_the_body_not_the_declaration() {
    assert_attachments(
        "class A {} // on the class\n",
        &["// on the class => trailing class_body"],
    );
}

/// **The one place TypeScript is meaningfully worse served by the policy than
/// Java is**, and it is worth stating precisely because the whole point of this
/// exercise is to find out.
///
/// Decision 21: rule 1 (trailing) requires that *only comments* sit between the
/// owner and the comment — an anonymous token blocks it. In Java a
/// `field_declaration` swallows its own `;`, so `int x = 1; // c` trails the
/// field. In TypeScript the `;` terminating a class member is an anonymous
/// *sibling* inside `class_body`, so the same comment is blocked from rule 1,
/// finds no following candidate before the `}`, and floats.
///
/// The same shape costs the same thing for `,`-separated enum members and
/// object-literal entries below. It is the Java "comments in argument lists
/// attach forwards" imperfection, hitting a much more common construct.
///
/// Not patched around: the fix, if it turns out to matter, belongs in the
/// policy (e.g. "an anonymous token that is a pure separator does not block
/// rule 1"), not in a language-specific special case.
#[test]
fn a_trailing_comment_on_a_class_member_floats_because_the_semicolon_is_a_sibling() {
    assert_attachments(
        "class A {\n  x: number = 1; // like this\n}\n",
        &["// like this => floating"],
    );
    // Whereas the identical comment on a *statement* trails, because a
    // `lexical_declaration` does swallow its `;`.
    assert_attachments(
        "function f() {\n  const x: number = 1; // like this\n}\n",
        &["// like this => trailing lexical_declaration `x`"],
    );
}

/// A block comment that *opens* on the owner's last line trails it however far
/// it runs — the trailing check uses the comment's start offset (decision 25).
#[test]
fn a_multiline_block_comment_trails_if_it_opens_on_the_owners_line() {
    assert_attachments(
        "const a = 1; /* one\n   two */\nconst b = 2;\n",
        &["/* one two */ => trailing lexical_declaration `a`"],
    );
}

/// Turning the trailing rule off sends the same comment forwards instead.
#[test]
fn trailing_can_be_disabled() {
    let src = "const a = 1; // t\nconst b = 2;\n";
    assert_attachments(src, &["// t => trailing lexical_declaration `a`"]);
    assert_attachments_with(
        src,
        &TriviaConfig {
            attach_trailing_same_line: false,
            ..TriviaConfig::DEFAULT
        },
        &["// t => leading lexical_declaration `b`"],
    );
}

// --------------------------------------------------------- rule 2: leading --

/// The case the project exists to get right: a JSDoc block above a declaration
/// moves with it. Java's equivalent is a Javadoc `block_comment`; TypeScript's
/// is an ordinary `comment` whose text starts with `/**`. The pass does not care,
/// because it never looks at comment content.
#[test]
fn a_jsdoc_block_leads_the_declaration_below_it() {
    assert_attachments(
        "/**\n * Does the thing.\n */\nexport function doTheThing(): void {}\n",
        &["/** * Does the thing. */ => leading export_statement `doTheThing`"],
    );
}

#[test]
fn jsdoc_leads_a_class_member() {
    assert_attachments(
        "class A {\n  /** The count. */\n  count = 0;\n\n  /** Resets it. */\n  reset(): void {}\n}\n",
        &[
            "/** The count. */ => leading public_field_definition `count`",
            "/** Resets it. */ => leading method_definition `reset`",
        ],
    );
}

/// Zero newlines in the gap qualifies too, so an inline block comment leads what
/// follows it on the same line.
#[test]
fn an_inline_block_comment_leads_the_next_node() {
    assert_attachments(
        "class A {\n  /* c */ x = 1;\n}\n",
        &["/* c */ => leading public_field_definition `x`"],
    );
}

#[test]
fn a_run_of_comments_attaches_as_a_unit() {
    assert_attachments(
        "// a\n// b\nfunction f(): void {}\n",
        &[
            "// a => leading function_declaration `f`",
            "// b => leading function_declaration `f`",
        ],
    );
}

/// Each link of the chain is checked separately, so a multi-line block comment
/// in the middle of a run is harmless: its own interior newlines are never
/// mistaken for a gap.
#[test]
fn a_multiline_comment_inside_a_run_does_not_break_the_chain() {
    assert_attachments(
        "// a\n/* b\n   still b */\n// c\ninterface I {}\n",
        &[
            "// a => leading interface_declaration `I`",
            "/* b still b */ => leading interface_declaration `I`",
            "// c => leading interface_declaration `I`",
        ],
    );
}

// -------------------------------------------------------- rule 3: floating --

#[test]
fn a_blank_line_floats_the_comment_above_it() {
    assert_attachments(
        "// a\n\n// b\nfunction f(): void {}\n",
        &[
            "// a => floating",
            "// b => leading function_declaration `f`",
        ],
    );
}

/// Raising the ceiling reattaches it — the one knob that changes this answer.
#[test]
fn the_blank_line_ceiling_is_configurable() {
    assert_attachments_with(
        "// a\n\n// b\nfunction f(): void {}\n",
        &TriviaConfig {
            max_leading_gap_newlines: 2,
            ..TriviaConfig::DEFAULT
        },
        &[
            "// a => leading function_declaration `f`",
            "// b => leading function_declaration `f`",
        ],
    );
}

/// Nothing follows a comment at the end of a body but the anonymous `}`, and a
/// statement on an earlier line cannot claim it.
#[test]
fn a_comment_at_the_end_of_a_block_floats() {
    assert_attachments(
        "function f(): void {\n  const a = 1;\n  // trailing note\n}\n",
        &["// trailing note => floating"],
    );
}

#[test]
fn a_file_of_only_comments_floats_everything() {
    assert_attachments(
        "// one\n\n/* two */\n",
        &["// one => floating", "/* two */ => floating"],
    );
}

// ------------------------------------------------------------- tie-breaks --

/// Trailing beats leading (decision 20): a comment that could be read either way
/// goes to the *preceding* node, so it does not get dragged along when the next
/// declaration moves.
#[test]
fn trailing_beats_leading() {
    assert_attachments(
        "const a = 1; // about a\nfunction f(): void {}\n",
        &["// about a => trailing lexical_declaration `a`"],
    );
}

/// A comment that loses rule 1 to a trailing attachment is still transparent for
/// the rule-2 chain.
#[test]
fn a_trailing_comment_does_not_block_the_next_comments_chain() {
    assert_attachments(
        "const a = 1; // t\n// b\nfunction f(): void {}\n",
        &[
            "// t => trailing lexical_declaration `a`",
            "// b => leading function_declaration `f`",
        ],
    );
}

// ------------------------------------------ where the grammar shape shows --

/// The import run. Each comment lands on its own import, and the side-effect
/// import is no different from the others to the attachment pass — it is the
/// *classification* that treats it specially, not the trivia policy.
#[test]
fn comments_in_an_import_run() {
    assert_attachments(
        concat!(
            "// polyfills first\n",
            "import \"./polyfill\";\n",
            "import { a } from \"./a\"; // just a\n",
            "\n",
            "// unrelated banner\n",
            "\n",
            "import { b } from \"./b\";\n",
        ),
        &[
            "// polyfills first => leading import_statement",
            "// just a => trailing import_statement `a`",
            "// unrelated banner => floating",
        ],
    );
}

/// A comment inside the braces of a named import attaches to a *specifier*, not
/// to the statement — the pass runs per child list, so the candidates are the
/// specifiers themselves. Java's equivalent is a comment inside an argument
/// list.
#[test]
fn a_comment_inside_named_imports_attaches_within_the_brace_list() {
    assert_attachments(
        "import {\n  a, // keep\n  b,\n} from \"./m\";\n",
        &["// keep => leading import_specifier `b`"],
    );
}

/// The documented imperfection carried over verbatim from Java: a `,` is an
/// anonymous token, and rule 1 requires only comments between the owner and the
/// comment, so `// keep` above leads `b` rather than trailing `a`. Stated as its
/// own test so the answer cannot change silently.
#[test]
fn comments_in_argument_lists_attach_forwards_here_too() {
    assert_attachments(
        "f(\n  a, // about a\n  b,\n);\n",
        &["// about a => leading identifier `b`"],
    );
}

/// A comment between a decorator and the declaration it decorates. Java's
/// annotation equivalent floats because the annotations live inside a
/// `modifiers` node; TypeScript puts the `decorator` directly in the class body,
/// so here the comment leads the member. Different answer, same policy — the
/// difference is the grammar's, not the pass's.
#[test]
fn a_comment_after_a_decorator_leads_the_member() {
    assert_attachments(
        "class A {\n  @dec()\n  // why\n  m(): void {}\n}\n",
        &["// why => leading method_definition `m`"],
    );
}

/// An object literal. The leading rule works as it does everywhere; the
/// trailing one is blocked by the `,` separator, the same way it is for a class
/// member above and for a Java argument list.
#[test]
fn comments_in_an_object_literal() {
    assert_attachments(
        "const o = {\n  // the first key\n  a: 1,\n  b: 2, // the second\n};\n",
        &[
            "// the first key => leading pair `a`",
            "// the second => floating",
        ],
    );
}

/// Enum members, the ordered container the whole enum discussion turns on. Note
/// that the leading comment lands on the bare `property_identifier` `A` — a
/// member with no initialiser is not wrapped in an `enum_assignment`, so the
/// identifier itself is the candidate owner.
#[test]
fn comments_on_enum_members() {
    assert_attachments(
        "enum E {\n  /** first */\n  A,\n  B = 3, // explicit\n}\n",
        &[
            "/** first */ => leading property_identifier `A`",
            "// explicit => floating",
        ],
    );
}

// ------------------------------------------------------------------- TSX --

/// The TSX dialect goes through the identical pass with the identical policy.
#[test]
fn attachment_works_the_same_in_tsx() {
    assert_attachments_in(
        support::tsx(),
        concat!(
            "/** The list. */\n",
            "export function List() {\n",
            "  const n = 1; // count\n",
            "  return <ul />;\n",
            "}\n",
        ),
        &[
            "/** The list. */ => leading export_statement `List`",
            "// count => trailing lexical_declaration `n`",
        ],
    );
}

/// A comment written inside JSX has to be wrapped in `{/* … */}`, which makes it
/// a `comment` inside a `jsx_expression`. It is the only candidate-free child
/// list in the snippet, so it floats — the same answer the policy gives for a
/// comment alone in any other container.
#[test]
fn a_jsx_comment_floats_inside_its_expression_wrapper() {
    let tree = parse_src_in(
        support::tsx(),
        "const a = (\n  <div>\n    {/* note */}\n    <span />\n  </div>\n);\n",
    );
    assert!(!tree.has_errors());
    check_consistency(&tree);
    assert_eq!(summarize(&tree), vec!["/* note */ => floating".to_string()]);
    let comment = comment_ids(&tree)[0];
    let parent = tree.node(comment).parent.expect("a comment has a parent");
    assert_eq!(tree.node(parent).kind, "jsx_expression");
}

// ------------------------------------------------------------ the fixture --

/// End-to-end over the real fixture: `parse()` runs the pass, so every tree in
/// the system arrives attached, and the fixture's JSDoc blocks land on their
/// declarations.
#[test]
fn the_typical_fixture_arrives_attached() {
    let tree = support::parse_ts_fixture("ts_typical.ts");
    check_consistency(&tree);

    let attached = comment_ids(&tree)
        .into_iter()
        .filter(|&id| tree.node(id).attachment != Attachment::Floating)
        .count();
    assert!(
        attached >= 8,
        "expected most of the fixture's comments to attach, got {attached}"
    );

    // The class's JSDoc leads the class (via the `export_statement` that wraps
    // it, which is the node that would actually move).
    let store = tree
        .ids()
        .find(|&id| {
            tree.node(id).kind == "export_statement"
                && tree
                    .named_children(id)
                    .any(|c| tree.node(c).kind == "class_declaration")
        })
        .expect("the fixture exports a class");
    let leading: Vec<String> = tree
        .node(store)
        .leading_trivia
        .iter()
        .map(|&id| one_line(&tree, id))
        .collect();
    assert!(
        leading
            .iter()
            .any(|c| c.contains("A store of user records")),
        "expected the class JSDoc to lead its export statement, got {leading:?}"
    );

    // And the end-of-line comment on `count` trails the field.
    let count_field = tree
        .ids()
        .find(|&id| {
            tree.node(id).kind == "public_field_definition"
                && tree.node_text(id).starts_with("private count")
        })
        .expect("the fixture has a `count` field");
    let trailing: Vec<String> = tree
        .node(count_field)
        .trailing_trivia
        .iter()
        .map(|&id| one_line(&tree, id))
        .collect();
    assert!(
        trailing.is_empty(),
        "the `// mutable` comment is written *above* count, so it leads: {trailing:?}"
    );
    let leading: Vec<String> = tree
        .node(count_field)
        .leading_trivia
        .iter()
        .map(|&id| one_line(&tree, id))
        .collect();
    assert_eq!(leading, vec!["// mutable, set by clear()".to_string()]);
}

/// Re-running the pass under a different config fully replaces the previous
/// answer — the property `parse_with_trivia_config` and the M5 constant sweep
/// both depend on, restated for a second language.
#[test]
fn the_pass_is_idempotent_and_re_runnable() {
    let src = "// a\nfunction f(): void {}\nconst b = 1; // t\n";
    let base = parse_src(src);
    let again = {
        let mut t = parse_src(src);
        sm_cst::attach_trivia(&mut t, support::typescript(), &TriviaConfig::DEFAULT);
        t
    };
    assert_eq!(summarize(&base), summarize(&again));

    let mut swept = parse_src(src);
    sm_cst::attach_trivia(
        &mut swept,
        support::typescript(),
        &TriviaConfig {
            attach_trailing_same_line: false,
            ..TriviaConfig::DEFAULT
        },
    );
    check_consistency(&swept);
    assert_eq!(
        summarize(&swept),
        vec![
            "// a => leading function_declaration `f`".to_string(),
            "// t => floating".to_string(),
        ]
    );
}
