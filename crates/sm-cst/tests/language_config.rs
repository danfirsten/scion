//! Guards on the Java `Language` configuration.
//!
//! The classification lists in `JavaLanguage` are plain string constants, which
//! means a typo or an upstream grammar rename would silently disable a rule —
//! `class_body` quietly becoming ordered would turn a large class of clean
//! merges into conflicts, and nothing would say so. Every configured kind name
//! is therefore checked against the grammar's own node-kind inventory.

mod support;

use std::collections::BTreeSet;

use sm_cst::{ChildListKind, Language, languages::JavaLanguage};
use support::parse_fixture;

/// Every node kind the compiled-in grammar knows about.
fn grammar_kinds() -> BTreeSet<&'static str> {
    let lang = JavaLanguage.ts_language();
    (0..lang.node_kind_count())
        .filter_map(|id| lang.node_kind_for_id(u16::try_from(id).expect("kind id fits in u16")))
        .collect()
}

fn assert_all_present(label: &str, names: &[&str]) {
    let known = grammar_kinds();
    let missing: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| !known.contains(n))
        .collect();
    assert!(
        missing.is_empty(),
        "{label}: these kind names are not in the tree-sitter-java grammar: {missing:?}. \
         Either they are typos, or the grammar renamed them and the configuration needs updating."
    );
}

#[test]
fn unordered_containers_exist_in_the_grammar() {
    assert_all_present("UNORDERED_CONTAINERS", JavaLanguage::UNORDERED_CONTAINERS);
}

#[test]
fn ordered_containers_exist_in_the_grammar() {
    assert_all_present("ORDERED_CONTAINERS", JavaLanguage::ORDERED_CONTAINERS);
}

#[test]
fn program_unordered_kinds_exist_in_the_grammar() {
    assert_all_present(
        "PROGRAM_UNORDERED_KINDS",
        JavaLanguage::PROGRAM_UNORDERED_KINDS,
    );
}

#[test]
fn scope_introducing_kinds_exist_in_the_grammar() {
    assert_all_present("SCOPE_INTRODUCING", JavaLanguage::SCOPE_INTRODUCING);
}

#[test]
fn identifier_kinds_exist_in_the_grammar() {
    assert_all_present("IDENTIFIERS", JavaLanguage::IDENTIFIERS);
}

#[test]
fn comment_kinds_exist_in_the_grammar() {
    assert_all_present("COMMENTS", JavaLanguage::COMMENTS);
}

#[test]
fn significant_text_kinds_exist_in_the_grammar() {
    assert_all_present("SIGNIFICANT_TEXT", JavaLanguage::SIGNIFICANT_TEXT);
}

/// The ordered and unordered lists must not overlap; a kind cannot be both.
#[test]
fn ordered_and_unordered_lists_are_disjoint() {
    let unordered: BTreeSet<&str> = JavaLanguage::UNORDERED_CONTAINERS.iter().copied().collect();
    let overlap: Vec<&str> = JavaLanguage::ORDERED_CONTAINERS
        .iter()
        .copied()
        .filter(|k| unordered.contains(k))
        .collect();
    assert!(overlap.is_empty(), "kinds in both lists: {overlap:?}");
}

/// The classifications the merge algorithm will actually ask for.
#[test]
fn child_list_kinds_are_what_we_intend() {
    let java = JavaLanguage;

    for kind in JavaLanguage::UNORDERED_CONTAINERS {
        assert_eq!(
            java.child_list_kind(kind),
            ChildListKind::Unordered,
            "{kind} should be unordered"
        );
    }
    for kind in JavaLanguage::ORDERED_CONTAINERS {
        assert_eq!(
            java.child_list_kind(kind),
            ChildListKind::Ordered,
            "{kind} should be ordered"
        );
    }

    // `program` is the mixed case: imports commute, the package declaration and
    // the type declarations do not.
    assert_eq!(
        java.child_list_kind("program"),
        ChildListKind::PartiallyUnordered {
            unordered_kinds: JavaLanguage::PROGRAM_UNORDERED_KINDS
        }
    );

    // The default for anything unrecognised is the conservative one.
    assert_eq!(
        java.child_list_kind("a_kind_no_grammar_has"),
        ChildListKind::Ordered
    );
}

/// Parameter and argument lists are the trap: they look like declaration sets
/// and are strictly positional. Reordering either changes what the program does.
#[test]
fn parameter_and_argument_lists_are_ordered() {
    let java = JavaLanguage;
    assert_eq!(
        java.child_list_kind("formal_parameters"),
        ChildListKind::Ordered
    );
    assert_eq!(
        java.child_list_kind("argument_list"),
        ChildListKind::Ordered
    );
}

/// Enum constants define `ordinal()`, so `enum_body` is ordered even though the
/// member section after the `;` — a separate `enum_body_declarations` node — is
/// not.
#[test]
fn enum_constants_are_ordered_but_enum_members_are_not() {
    let java = JavaLanguage;
    assert_eq!(java.child_list_kind("enum_body"), ChildListKind::Ordered);
    assert_eq!(
        java.child_list_kind("enum_body_declarations"),
        ChildListKind::Unordered
    );
}

/// The configured names must not merely exist in the grammar — they must be the
/// names that actually show up when parsing Java. A kind that exists but is
/// never produced would be configuration that never fires.
#[test]
fn configured_kinds_actually_appear_in_the_fixtures() {
    let mut seen: BTreeSet<&'static str> = BTreeSet::new();
    for name in support::FIXTURES {
        let tree = parse_fixture(name);
        seen.extend(tree.nodes().map(|n| n.kind));
    }

    // Deliberately not every configured kind: `annotation_type_body`,
    // `resource_specification` and friends are covered by some fixtures and not
    // others, and the point here is to prove the *load-bearing* ones fire.
    let must_appear = [
        "program",
        "class_body",
        "interface_body",
        "enum_body",
        "enum_body_declarations",
        "annotation_type_body",
        "block",
        "argument_list",
        "formal_parameters",
        "import_declaration",
        "class_declaration",
        "interface_declaration",
        "enum_declaration",
        "method_declaration",
        "constructor_declaration",
        "for_statement",
        "enhanced_for_statement",
        "lambda_expression",
        "identifier",
        "type_identifier",
        "line_comment",
        "block_comment",
        "string_literal",
        "decimal_integer_literal",
    ];
    let absent: Vec<&str> = must_appear
        .iter()
        .copied()
        .filter(|k| !seen.contains(k))
        .collect();
    assert!(
        absent.is_empty(),
        "these kinds never appeared while parsing the fixtures: {absent:?}"
    );
}

#[test]
fn scope_identifier_and_comment_predicates_agree_with_their_lists() {
    let java = JavaLanguage;
    for kind in JavaLanguage::SCOPE_INTRODUCING {
        assert!(java.is_scope_introducing(kind), "{kind}");
    }
    for kind in JavaLanguage::IDENTIFIERS {
        assert!(java.is_identifier(kind), "{kind}");
    }
    for kind in JavaLanguage::COMMENTS {
        assert!(java.is_comment(kind), "{kind}");
    }
    for kind in JavaLanguage::SIGNIFICANT_TEXT {
        assert!(java.significant_text(kind), "{kind}");
    }

    assert!(!java.is_scope_introducing("field_declaration"));
    assert!(!java.is_identifier("string_literal"));
    assert!(!java.is_comment("block"));
    // Keywords and punctuation carry no text beyond their kind.
    assert!(!java.significant_text("public"));
    assert!(!java.significant_text(";"));
    // `true`/`false`/`null_literal` are distinct kinds, so their text adds
    // nothing that the kind comparison does not already give the matcher.
    assert!(!java.significant_text("true"));
    assert!(!java.significant_text("null_literal"));
}

#[test]
fn language_detection_is_by_extension_and_case_insensitive() {
    use std::path::Path;

    assert_eq!(
        languages_detect_name(Path::new("/tmp/Foo.java")),
        Some("java")
    );
    assert_eq!(
        languages_detect_name(Path::new("/tmp/Foo.JAVA")),
        Some("java")
    );
    assert_eq!(languages_detect_name(Path::new("/tmp/Foo.kt")), None);
    assert_eq!(languages_detect_name(Path::new("/tmp/Makefile")), None);

    assert_eq!(
        sm_cst::languages::by_name("java").map(Language::name),
        Some("java")
    );
    assert!(sm_cst::languages::by_name("cobol").is_none());
    // Java, plus TypeScript's two dialects. The TypeScript side of the registry
    // is asserted in `tests/ts_language_config.rs`.
    assert_eq!(sm_cst::languages::all().len(), 3);
}

fn languages_detect_name(path: &std::path::Path) -> Option<&'static str> {
    sm_cst::languages::detect(path).map(Language::name)
}
