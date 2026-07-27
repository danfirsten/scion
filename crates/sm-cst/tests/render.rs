//! Snapshot and behaviour tests for the `sm parse` renderer.
//!
//! The snapshot is taken through the same library function the CLI calls, so it
//! is a real regression test on the output the user sees without shelling out to
//! a binary.

mod support;

use sm_cst::{Attachment, HeaderInfo, RenderOptions, render_parse};
use support::{java, parse_fixture};

fn render(name: &str, opts: RenderOptions<'_>) -> String {
    let tree = parse_fixture(name);
    render_parse(&tree, java(), &opts)
}

/// The header carries no timing here: wall-clock numbers are not reproducible
/// and have no place in a committed snapshot. The CLI supplies a real duration.
fn snapshot_options(path: &str) -> RenderOptions<'_> {
    RenderOptions {
        header: Some(HeaderInfo {
            path,
            parse_time: None,
        }),
        ..RenderOptions::default()
    }
}

#[test]
fn typical_class_renders_as_expected() {
    insta::assert_snapshot!(
        "typical_class",
        render("typical", snapshot_options("typical.java"))
    );
}

#[test]
fn no_trivia_hides_every_comment() {
    let with = render("typical", snapshot_options("typical.java"));
    let without = render(
        "typical",
        RenderOptions {
            show_trivia: false,
            ..snapshot_options("typical.java")
        },
    );

    assert!(with.contains("line_comment"));
    assert!(with.contains("block_comment"));
    assert!(!without.contains("line_comment"));
    assert!(!without.contains("block_comment"));
    // Only comment lines disappear; the structure is otherwise identical.
    assert!(without.lines().count() < with.lines().count());
    assert!(without.contains("class_declaration"));
}

#[test]
fn broken_file_renders_a_warning_and_marks_the_bad_nodes() {
    let rendered = render("broken", snapshot_options("broken.java"));
    assert!(
        rendered.contains(sm_cst::ERROR_WARNING),
        "expected the parse-error warning in:\n{rendered}"
    );
    assert!(
        rendered.contains("ERROR") || rendered.contains("MISSING"),
        "expected an ERROR or MISSING marker in:\n{rendered}"
    );
}

#[test]
fn clean_file_renders_no_warning() {
    let rendered = render("typical", snapshot_options("typical.java"));
    assert!(!rendered.contains(sm_cst::ERROR_WARNING));
    assert!(rendered.starts_with("typical.java  language=java  nodes="));
}

#[test]
fn header_is_optional() {
    let rendered = render("typical", RenderOptions::default());
    assert!(rendered.starts_with("program ["));
}

/// Long text is truncated and escaped, so that a comment containing a newline
/// cannot break the one-node-per-line format.
#[test]
fn text_is_escaped_and_truncated() {
    let rendered = render("typical", snapshot_options("typical.java"));
    for line in rendered.lines() {
        assert!(!line.is_empty(), "the renderer should emit no blank lines");
    }
    // The Javadoc block is far longer than the 40-character default.
    assert!(
        rendered.contains("\\n"),
        "multi-line comments should have their newlines escaped"
    );
    assert!(
        rendered.contains("\"..."),
        "long text should be truncated with an ellipsis"
    );
}

/// Every node in the tree appears exactly once in the rendered output.
#[test]
fn every_node_is_rendered_exactly_once() {
    for name in support::FIXTURES {
        let tree = parse_fixture(name);
        let rendered = render_parse(&tree, java(), &RenderOptions::default());
        assert_eq!(
            rendered.lines().count(),
            tree.len(),
            "{name}: one line per node"
        );
    }
}

/// The trivia rendering path — a comment attached to a node is printed under
/// that node and *not* at its structural position, so it still appears exactly
/// once.
///
/// Nothing attaches trivia yet; this drives the seam by hand so that the
/// attachment pass inherits a renderer that is already known to work.
#[test]
fn attached_trivia_renders_under_its_owner_and_only_there() {
    let mut tree = parse_fixture("typical");
    let lang = java();

    let comment = tree
        .ids()
        .find(|&id| lang.is_comment(tree.node(id).kind))
        .expect("the typical fixture has comments");
    let owner = tree
        .ids()
        .find(|&id| tree.node(id).kind == "class_declaration")
        .expect("the typical fixture has a class");

    let before = render_parse(&tree, lang, &RenderOptions::default());
    tree.attach_leading(owner, comment);

    assert_eq!(tree.node(comment).attachment, Attachment::Leading(owner));
    assert_eq!(tree.node(owner).leading_trivia, vec![comment]);

    let after = render_parse(&tree, lang, &RenderOptions::default());
    assert_eq!(
        after.lines().count(),
        before.lines().count(),
        "attaching a comment must not add or drop a line"
    );
    assert_eq!(
        after.matches("leading: line_comment").count(),
        1,
        "the attached comment should be rendered once, under its owner:\n{after}"
    );
    assert!(after.contains("attached=leading("));

    // And attaching to the other side works the same way.
    tree.attach_trailing(owner, comment);
    assert_eq!(tree.node(comment).attachment, Attachment::Trailing(owner));
    assert!(tree.node(owner).leading_trivia.is_empty());
    assert_eq!(tree.node(owner).trailing_trivia, vec![comment]);
    let trailing = render_parse(&tree, lang, &RenderOptions::default());
    assert_eq!(trailing.matches("trailing: line_comment").count(), 1);
    assert_eq!(trailing.lines().count(), before.lines().count());

    // Detaching restores the original rendering exactly.
    tree.detach(comment);
    assert_eq!(tree.node(comment).attachment, Attachment::Floating);
    assert_eq!(render_parse(&tree, lang, &RenderOptions::default()), before);
}

/// `--no-trivia` hides attached comments too, not just floating ones.
#[test]
fn no_trivia_also_hides_attached_comments() {
    let mut tree = parse_fixture("typical");
    let lang = java();
    let comment = tree
        .ids()
        .find(|&id| lang.is_comment(tree.node(id).kind))
        .expect("comments exist");
    let owner = tree
        .ids()
        .find(|&id| tree.node(id).kind == "class_declaration")
        .expect("a class exists");
    tree.attach_leading(owner, comment);

    let rendered = render_parse(
        &tree,
        lang,
        &RenderOptions {
            show_trivia: false,
            ..RenderOptions::default()
        },
    );
    assert!(!rendered.contains("line_comment"));
    assert!(!rendered.contains("leading:"));
}
