//! The `sm parse` renderer over TypeScript.
//!
//! The snapshot is taken through the same library function the CLI calls, so it
//! is a real regression test on the output the user sees without shelling out to
//! a binary — and, since `render_parse` takes a `&dyn Language` and nothing
//! else, it is also a test that the renderer needed no changes to gain a second
//! language.

mod support;

use sm_cst::{HeaderInfo, RenderOptions, render_parse};
use support::{parse_ts_fixture, ts_lang};

/// The header carries no timing here: wall-clock numbers are not reproducible
/// and have no place in a committed snapshot. The CLI supplies a real duration.
fn render(file_name: &str) -> String {
    let tree = parse_ts_fixture(file_name);
    render_parse(
        &tree,
        ts_lang(file_name),
        &RenderOptions {
            header: Some(HeaderInfo {
                path: file_name,
                parse_time: None,
            }),
            ..RenderOptions::default()
        },
    )
}

#[test]
fn typical_module_renders_as_expected() {
    insta::assert_snapshot!("ts_typical", render("ts_typical.ts"));
}

#[test]
fn the_header_names_the_dialect_the_registry_chose() {
    assert!(
        render("ts_typical.ts").starts_with("ts_typical.ts  language=typescript  nodes="),
        "{}",
        render("ts_typical.ts").lines().next().unwrap_or_default()
    );
    assert!(
        render("ts_react.tsx").starts_with("ts_react.tsx  language=tsx  nodes="),
        "{}",
        render("ts_react.tsx").lines().next().unwrap_or_default()
    );
}

#[test]
fn no_trivia_hides_every_comment() {
    let tree = parse_ts_fixture("ts_typical.ts");
    let lang = ts_lang("ts_typical.ts");
    let with = render_parse(&tree, lang, &RenderOptions::default());
    let without = render_parse(
        &tree,
        lang,
        &RenderOptions {
            show_trivia: false,
            ..RenderOptions::default()
        },
    );

    assert!(with.contains("comment"));
    assert!(!without.contains("comment"));
    assert!(without.lines().count() < with.lines().count());
    assert!(without.contains("class_declaration"));
}

#[test]
fn broken_file_renders_a_warning_and_marks_the_bad_nodes() {
    let rendered = render("ts_broken.ts");
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
fn clean_files_render_no_warning() {
    for name in ["ts_typical.ts", "ts_react.tsx", "ts_unicode.ts"] {
        assert!(!render(name).contains(sm_cst::ERROR_WARNING), "{name}");
    }
}

/// Every node in the tree appears exactly once in the rendered output, for every
/// TypeScript fixture and both dialects.
#[test]
fn every_node_is_rendered_exactly_once() {
    for name in support::TS_FIXTURES {
        let tree = parse_ts_fixture(name);
        let rendered = render_parse(&tree, ts_lang(name), &RenderOptions::default());
        assert_eq!(
            rendered.lines().count(),
            tree.len(),
            "{name}: one line per node"
        );
    }
}

/// Long text is truncated and escaped, so that a comment containing a newline
/// cannot break the one-node-per-line format. A JSDoc block is the obvious case.
#[test]
fn text_is_escaped_and_truncated() {
    let rendered = render("ts_typical.ts");
    for line in rendered.lines() {
        assert!(!line.is_empty(), "the renderer should emit no blank lines");
    }
    assert!(
        rendered.contains("\\n"),
        "multi-line comments should have their newlines escaped"
    );
    assert!(
        rendered.contains("\"..."),
        "long text should be truncated with an ellipsis"
    );
}

/// The exact path `sm parse` takes: detect the language from the path, parse,
/// render. If this works, `sm parse foo.ts` works.
#[test]
fn the_cli_path_works_end_to_end_for_both_dialects() {
    for (file, expected_language, expected_kind) in [
        ("ts_typical.ts", "typescript", "interface_declaration"),
        ("ts_react.tsx", "tsx", "jsx_element"),
    ] {
        let path = support::ts_fixture_path(file);
        let lang = sm_cst::languages::detect(&path).expect("registered");
        assert_eq!(lang.name(), expected_language);

        let source = std::fs::read(&path).expect("fixture is readable");
        let tree = sm_cst::parse(&source, lang).expect("parses");
        assert!(!tree.has_errors(), "{file}");
        assert!(
            tree.nodes().any(|n| n.kind == expected_kind),
            "{file}: expected a {expected_kind}"
        );

        let rendered = render_parse(&tree, lang, &RenderOptions::default());
        assert_eq!(rendered.lines().count(), tree.len());
    }
}
