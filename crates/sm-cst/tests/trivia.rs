//! The trivia attachment test suite (SPEC.md §4.2 requires trivia attachment to
//! have one of its own).
//!
//! Most tests are table-driven over small inline snippets: each asserts the
//! whole list of comments in the snippet and where each one landed, so a rule
//! change shows up as a diff of the policy rather than as a single flipped
//! boolean. [`assert_attachments`] additionally re-checks the structural
//! invariants and the consistency of the attachment back-pointers on every
//! snippet it is given, because the one thing attachment must never do is
//! restructure the tree.

mod support;

use std::collections::HashSet;

use sm_cst::{
    Attachment, NodeId, RenderOptions, SourceTree, TriviaConfig, attach_trivia, invariants,
    parse_with_trivia_config, render_parse,
};
use support::{java, parse_fixture};

// ---------------------------------------------------------------- helpers --

fn parse_src(src: &str) -> SourceTree {
    parse_with_trivia_config(src.as_bytes(), java(), &TriviaConfig::DEFAULT)
        .expect("the snippet should parse")
}

fn parse_src_with(src: &str, config: &TriviaConfig) -> SourceTree {
    parse_with_trivia_config(src.as_bytes(), java(), config).expect("the snippet should parse")
}

/// Every comment in the tree, in source order.
///
/// Preorder visits children in source order and comments are always leaves, so
/// ascending ID order *is* source order here.
fn comment_ids(tree: &SourceTree) -> Vec<NodeId> {
    tree.ids()
        .filter(|&id| java().is_comment(tree.node(id).kind))
        .collect()
}

fn first_of_kind(tree: &SourceTree, kind: &str) -> NodeId {
    tree.ids()
        .find(|&id| tree.node(id).kind == kind)
        .unwrap_or_else(|| panic!("no {kind} in this tree"))
}

/// Collapse a node's text to one whitespace-normalised, truncated line, so a
/// multi-line block comment can appear in a one-line expectation.
fn one_line(tree: &SourceTree, id: NodeId) -> String {
    let text = tree.node_text(id);
    let mut flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 34 {
        flat = flat.chars().take(31).collect::<String>() + "...";
    }
    flat
}

/// `method_declaration `f`` — the kind, plus the declared name when there is an
/// obvious one, so that expectations distinguish two methods of the same kind.
fn describe_owner(tree: &SourceTree, id: NodeId) -> String {
    let node = tree.node(id);
    if matches!(node.kind, "identifier" | "type_identifier") {
        return format!("{} `{}`", node.kind, tree.node_text(id));
    }
    // A direct `identifier` child first: for a `method_declaration` that is the
    // method name, whereas the first identifier *descendant* would be an
    // annotation's name. Fall back to the first descendant for wrappers like
    // `field_declaration`, whose name lives in a `variable_declarator`.
    let named = tree
        .children(id)
        .chain(tree.descendants(id))
        .find(|&c| tree.node(c).kind == "identifier");
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

/// The properties attachment must preserve no matter what it decides.
fn check_consistency(tree: &SourceTree) {
    assert_eq!(
        invariants::check_all(tree),
        Vec::new(),
        "attachment must not break any structural invariant"
    );

    let lang = java();
    for id in tree.ids() {
        let node = tree.node(id);

        // A comment stays in its parent's child list; attachment annotates.
        if lang.is_comment(node.kind) {
            let parent = node.parent.expect("a comment is never the root");
            assert!(
                tree.children(parent).any(|c| c == id),
                "comment {id} left its parent's child list"
            );
            // A comment can never own a comment.
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

        // Forward index and back-pointer agree, both ways.
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

    // Trivia lists are in source order and hold each comment exactly once.
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
    let tree = parse_src(src);
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

// ------------------------------------------------------------ rule 1: trailing --

#[test]
fn a_comment_on_the_line_a_sibling_ends_on_trails_it() {
    assert_attachments(
        "class A {\n    int x = 1; // like this\n}\n",
        &["// like this => trailing field_declaration `x`"],
    );
}

#[test]
fn trailing_works_at_every_nesting_level() {
    assert_attachments(
        "class A {} // on the class\n",
        &["// on the class => trailing class_declaration `A`"],
    );
    assert_attachments(
        "class A {\n    void f() {\n        g(); // on the call\n    }\n}\n",
        &["// on the call => trailing expression_statement `g`"],
    );
}

/// Several comments may share one line, and all of them trail the same owner —
/// "only whitespace or other comments between" is satisfied for each.
#[test]
fn several_comments_can_trail_the_same_sibling() {
    assert_attachments(
        "class A {\n    int x = 1; /* a */ /* b */\n}\n",
        &[
            "/* a */ => trailing field_declaration `x`",
            "/* b */ => trailing field_declaration `x`",
        ],
    );
}

/// The trailing rule looks at the comment's **start** line, so a block comment
/// that opens on the owner's last line still trails it however far it runs.
#[test]
fn a_multiline_block_comment_trails_by_its_start_line() {
    assert_attachments(
        "class A {\n    int x = 1; /* opens here\n       and keeps going */\n    int y = 2;\n}\n",
        &["/* opens here and keeps going */ => trailing field_declaration `x`"],
    );
}

/// ...and if it opens on a *later* line it does not trail, even though it ends
/// on the same line as the next declaration starts.
#[test]
fn a_block_comment_opening_on_a_later_line_does_not_trail() {
    assert_attachments(
        "class A {\n    int x = 1;\n    /* opens here\n       ends here */ int y = 2;\n}\n",
        &["/* opens here ends here */ => leading field_declaration `y`"],
    );
}

// ------------------------------------------------------------- rule 2: leading --

#[test]
fn a_comment_on_the_same_line_leads_what_follows_it() {
    assert_attachments(
        "class A {\n    /* c */ int x = 1;\n}\n",
        &["/* c */ => leading field_declaration `x`"],
    );
}

#[test]
fn a_comment_one_newline_above_leads_what_follows_it() {
    assert_attachments(
        "class A {\n    // c\n    void f() {}\n}\n",
        &["// c => leading method_declaration `f`"],
    );
}

#[test]
fn a_blank_line_breaks_leading_attachment() {
    assert_attachments(
        "class A {\n    // c\n\n    void f() {}\n}\n",
        &["// c => floating"],
    );
}

#[test]
fn a_run_of_comments_chains_to_one_owner_in_order() {
    let src = "class A {\n    // a\n    // b\n    /** c */\n    void f() {}\n}\n";
    assert_attachments(
        src,
        &[
            "// a => leading method_declaration `f`",
            "// b => leading method_declaration `f`",
            "/** c */ => leading method_declaration `f`",
        ],
    );
    // ...and the owner records them in source order.
    let tree = parse_src(src);
    assert_eq!(
        tree.node(first_of_kind(&tree, "method_declaration"))
            .leading_trivia,
        comment_ids(&tree)
    );
}

/// Each link of the chain is measured on its own, so the interior newlines of a
/// block comment in the middle of a run are never counted as a gap.
#[test]
fn a_multiline_comment_inside_a_run_does_not_break_it() {
    assert_attachments(
        "class A {\n    // a\n    /* b\n       still b */\n    // c\n    void f() {}\n}\n",
        &[
            "// a => leading method_declaration `f`",
            "/* b still b */ => leading method_declaration `f`",
            "// c => leading method_declaration `f`",
        ],
    );
}

#[test]
fn a_blank_line_splits_a_run() {
    assert_attachments(
        "class A {\n    // a\n\n    // b\n    void f() {}\n}\n",
        &["// a => floating", "// b => leading method_declaration `f`"],
    );
}

// ------------------------------------------------------------- the tie-break --

#[test]
fn trailing_beats_leading() {
    assert_attachments(
        "class A {\n    int x = 1; // trailing\n    void f() {}\n}\n",
        &["// trailing => trailing field_declaration `x`"],
    );
}

/// A comment that lost rule 1 to a trailing attachment is still transparent for
/// the run that follows it.
#[test]
fn a_trailing_comment_does_not_break_the_run_behind_it() {
    assert_attachments(
        "class A {\n    int x = 1; // t\n    // b\n    void f() {}\n}\n",
        &[
            "// t => trailing field_declaration `x`",
            "// b => leading method_declaration `f`",
        ],
    );
}

/// The exact [`Attachment`] values and both directions of the index, spelled out
/// once on the case where the two rules compete.
#[test]
fn the_tie_break_records_exact_owner_ids() {
    let tree = parse_src("class A {\n    int x = 1; // t\n    void f() {}\n}\n");
    let comment = comment_ids(&tree)[0];
    let field = first_of_kind(&tree, "field_declaration");
    let method = first_of_kind(&tree, "method_declaration");

    assert_eq!(tree.node(comment).attachment, Attachment::Trailing(field));
    assert_eq!(tree.node(field).trailing_trivia, vec![comment]);
    assert!(tree.node(field).leading_trivia.is_empty());
    assert!(tree.node(method).leading_trivia.is_empty());
    assert!(tree.node(method).trailing_trivia.is_empty());
}

// ------------------------------------------------------------------- floating --

#[test]
fn a_comment_at_the_end_of_a_block_floats() {
    assert_attachments(
        "class A {\n    void f() {\n        g();\n        // done\n    }\n    void h() {}\n}\n",
        &["// done => floating"],
    );
}

#[test]
fn a_comment_at_the_end_of_a_class_body_floats() {
    assert_attachments(
        "class A {\n    void f() {}\n    // that is all\n}\n",
        &["// that is all => floating"],
    );
}

#[test]
fn a_comment_alone_between_blank_lines_floats() {
    assert_attachments(
        "class A {\n    int x = 1;\n\n    // banner\n\n    int y = 2;\n}\n",
        &["// banner => floating"],
    );
}

#[test]
fn a_file_of_only_comments_floats_entirely() {
    let tree = parse_fixture("only_comment");
    check_consistency(&tree);
    for id in comment_ids(&tree) {
        assert_eq!(
            tree.node(id).attachment,
            Attachment::Floating,
            "nothing in a comment-only file has an owner"
        );
    }
    assert_eq!(comment_ids(&tree).len(), 3);
}

#[test]
fn an_empty_file_survives_the_pass() {
    let tree = parse_fixture("empty");
    check_consistency(&tree);
    assert!(comment_ids(&tree).is_empty());
    assert_eq!(tree.len(), 1);
}

#[test]
fn a_file_that_did_not_parse_survives_the_pass() {
    let tree = parse_fixture("broken");
    assert!(tree.has_errors());
    check_consistency(&tree);
}

// ------------------------------------------------------ real-world shapes --

#[test]
fn a_copyright_header_floats_when_a_blank_line_follows() {
    assert_attachments(
        "// Copyright\n// SPDX-License-Identifier: MIT\n\npackage p;\n\nclass A {}\n",
        &[
            "// Copyright => floating",
            "// SPDX-License-Identifier: MIT => floating",
        ],
    );
}

#[test]
fn a_header_glued_to_the_package_declaration_attaches_to_it() {
    assert_attachments(
        "// Copyright\n// SPDX-License-Identifier: MIT\npackage p;\n\nclass A {}\n",
        &[
            "// Copyright => leading package_declaration `p`",
            "// SPDX-License-Identifier: MIT => leading package_declaration `p`",
        ],
    );
}

#[test]
fn javadoc_leads_the_method_it_documents() {
    assert_attachments(
        "class A {\n    /**\n     * Does a thing.\n     */\n    public void f() {}\n}\n",
        &["/** * Does a thing. */ => leading method_declaration `f`"],
    );
}

/// Annotations live inside the method's own `modifiers` node, so they never come
/// between a Javadoc block and the `method_declaration` it is a sibling of.
#[test]
fn javadoc_leads_an_annotated_method_too() {
    assert_attachments(
        "class A {\n    /** Doc. */\n    @Override\n    public void f() {}\n}\n",
        &["/** Doc. */ => leading method_declaration `f`"],
    );
}

/// The documented imperfection: a comment written *between* the annotations and
/// the rest of the signature is a child of `modifiers`, where the keywords are
/// anonymous tokens and cannot own anything.
#[test]
fn a_comment_between_an_annotation_and_the_signature_floats() {
    let src = "class A {\n    @Override\n    // why\n    public void f() {}\n}\n";
    assert_attachments(src, &["// why => floating"]);
    let tree = parse_src(src);
    assert_eq!(
        tree.node(tree.node(comment_ids(&tree)[0]).parent.unwrap())
            .kind,
        "modifiers",
        "the comment should sit inside `modifiers`"
    );
}

/// The other documented imperfection: an anonymous token between a comment and
/// the sibling before it blocks the trailing rule, so argument comments attach
/// forwards.
#[test]
fn a_comment_after_a_comma_leads_the_next_argument() {
    assert_attachments(
        "class A {\n    void f() {\n        g(\n            a, // about a\n            b);\n    }\n}\n",
        &["// about a => leading identifier `b`"],
    );
}

#[test]
fn a_comment_before_a_braceless_loop_body_leads_it() {
    assert_attachments(
        "class A {\n    void f() {\n        for (int i = 0; i < 3; i++) // go\n            g(i);\n    }\n}\n",
        &["// go => leading expression_statement `g`"],
    );
}

// ------------------------------------------------------------------- scoping --

#[test]
fn a_comment_never_attaches_across_a_parent_boundary() {
    let src = concat!(
        "class A {\n",
        "    void f() {\n",
        "        int inner = 0;\n",
        "        // last in the block\n",
        "    }\n",
        "    void g() {}\n",
        "}\n"
    );
    // If the pass looked past the block's `}` it would find `void g()` one
    // newline away and lead it. It must not.
    assert_attachments(src, &["// last in the block => floating"]);

    let tree = parse_src(src);
    let comment = comment_ids(&tree)[0];
    assert_eq!(tree.node(tree.node(comment).parent.unwrap()).kind, "block");
    for id in tree.ids() {
        assert!(
            tree.node(id).leading_trivia.is_empty() && tree.node(id).trailing_trivia.is_empty(),
            "nothing should have claimed the comment"
        );
    }
}

#[test]
fn nested_blocks_keep_their_own_comments() {
    assert_attachments(
        concat!(
            "class A {\n",
            "    void f() {\n",
            "        {\n",
            "            int inner = 0; // inner\n",
            "        }\n",
            "        int outer = 1; // outer\n",
            "    }\n",
            "}\n"
        ),
        &[
            "// inner => trailing local_variable_declaration `inner`",
            "// outer => trailing local_variable_declaration `outer`",
        ],
    );
}

// --------------------------------------------------------------- line index --

/// Multi-byte characters must not shift line numbers: the index counts `\n`
/// bytes, and every offset it is asked about is a byte offset.
#[test]
fn unicode_does_not_shift_the_line_index() {
    assert_attachments(
        concat!(
            "class Ünïcödé {\n",
            "    String s = \"日本語 — ☕\"; // trailing, after 20-odd bytes of prose\n",
            "    // leading, after more\n",
            "    String t = \"naïve café\";\n",
            "}\n"
        ),
        &[
            "// trailing, after 20-odd bytes... => trailing field_declaration `s`",
            "// leading, after more => leading field_declaration `t`",
        ],
    );
}

#[test]
fn the_unicode_fixture_attaches_sanely() {
    let tree = parse_fixture("unicode");
    check_consistency(&tree);
    // The fixture's comments all sit one newline above a declaration.
    assert!(
        comment_ids(&tree)
            .iter()
            .all(|&id| matches!(tree.node(id).attachment, Attachment::Leading(_))),
        "{:#?}",
        summarize(&tree)
    );
}

/// `\r\n` is one line break, not two: a comment above a declaration in a CRLF
/// file still leads it.
#[test]
fn crlf_counts_as_one_newline() {
    let crlf = "class A {\r\n    // c\r\n    void f() {}\r\n}\r\n";
    assert_attachments(crlf, &["// c => leading method_declaration `f`"]);

    let blank = "class A {\r\n    // c\r\n\r\n    void f() {}\r\n}\r\n";
    assert_attachments(blank, &["// c => floating"]);
}

// ------------------------------------------------------------ configurability --

#[test]
fn zero_gap_restricts_leading_to_the_same_line() {
    let config = TriviaConfig {
        max_leading_gap_newlines: 0,
        ..TriviaConfig::DEFAULT
    };
    assert_attachments_with(
        "class A {\n    /* c */ int x = 1;\n}\n",
        &config,
        &["/* c */ => leading field_declaration `x`"],
    );
    assert_attachments_with(
        "class A {\n    // c\n    void f() {}\n}\n",
        &config,
        &["// c => floating"],
    );
}

#[test]
fn a_larger_gap_reaches_across_a_blank_line() {
    let config = TriviaConfig {
        max_leading_gap_newlines: 2,
        ..TriviaConfig::DEFAULT
    };
    assert_attachments_with(
        "class A {\n    // banner\n\n    void f() {}\n}\n",
        &config,
        &["// banner => leading method_declaration `f`"],
    );
    // Two blank lines is still three newlines, so the ceiling still bites.
    assert_attachments_with(
        "class A {\n    // banner\n\n\n    void f() {}\n}\n",
        &config,
        &["// banner => floating"],
    );
}

#[test]
fn trailing_can_be_switched_off() {
    let config = TriviaConfig {
        attach_trailing_same_line: false,
        ..TriviaConfig::DEFAULT
    };
    // What was trailing the field now leads the method one line below.
    assert_attachments_with(
        "class A {\n    int x = 1; // c\n    void f() {}\n}\n",
        &config,
        &["// c => leading method_declaration `f`"],
    );
    // With nothing below it to lead, it floats rather than trailing.
    assert_attachments_with(
        "class A {\n    int x = 1; // c\n}\n",
        &config,
        &["// c => floating"],
    );
    assert!(
        !summarize(&parse_src_with(
            "class A {\n    int x = 1; // c\n    void f() {}\n}\n",
            &config
        ))
        .iter()
        .any(|line| line.contains("trailing")),
        "no comment should trail with the rule switched off"
    );
}

#[test]
fn the_default_config_is_what_parse_uses() {
    assert_eq!(TriviaConfig::default(), TriviaConfig::DEFAULT);
    assert_eq!(
        TriviaConfig::DEFAULT,
        TriviaConfig {
            max_leading_gap_newlines: 1,
            attach_trailing_same_line: true,
        }
    );

    let src = "class A {\n    // c\n    int x = 1; // t\n}\n";
    let default = summarize(&parse_src(src));
    let explicit = summarize(&parse_src_with(src, &TriviaConfig::DEFAULT));
    assert_eq!(default, explicit);
}

/// Re-running the pass must not double-attach, and re-running it with a
/// different config must fully replace the previous answer.
#[test]
fn the_pass_is_idempotent_and_reversible() {
    let src = "class A {\n    // c\n    int x = 1; // t\n    void f() {}\n}\n";
    let mut tree = parse_src(src);
    let once = summarize(&tree);

    attach_trivia(&mut tree, java(), &TriviaConfig::DEFAULT);
    check_consistency(&tree);
    assert_eq!(summarize(&tree), once, "the pass must be idempotent");

    let no_trailing = TriviaConfig {
        attach_trailing_same_line: false,
        ..TriviaConfig::DEFAULT
    };
    attach_trivia(&mut tree, java(), &no_trailing);
    check_consistency(&tree);
    assert_eq!(
        summarize(&tree),
        summarize(&parse_src_with(src, &no_trailing))
    );

    attach_trivia(&mut tree, java(), &TriviaConfig::DEFAULT);
    check_consistency(&tree);
    assert_eq!(summarize(&tree), once, "and it must be reversible");
}

// ------------------------------------------------------------ over the corpus --

/// Every fixture, including the ones written for other purposes, must come out
/// of the pass structurally sound.
#[test]
fn every_fixture_attaches_consistently() {
    for name in support::FIXTURES {
        let tree = parse_fixture(name);
        check_consistency(&tree);
    }
}

/// The gallery is the readable specification: one fixture exercising every rule,
/// snapshotted through the renderer the user actually sees.
#[test]
fn trivia_gallery_renders() {
    let tree = parse_fixture("trivia_gallery");
    check_consistency(&tree);
    assert!(!tree.has_errors(), "the gallery should be valid Java");
    insta::assert_snapshot!(
        "trivia_gallery",
        render_parse(&tree, java(), &RenderOptions::default())
    );
}

/// ...and its attachments, stated as a table, so a policy change shows up here
/// in prose as well as in the tree snapshot.
#[test]
fn trivia_gallery_attachments() {
    let tree = parse_fixture("trivia_gallery");
    insta::assert_debug_snapshot!("trivia_gallery_attachments", summarize(&tree));
}
