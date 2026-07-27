//! Guards on the TypeScript `Language` configuration.
//!
//! The same contract `tests/language_config.rs` holds Java to, with one extra
//! obligation: `tree-sitter-typescript` ships *two* grammars, and every
//! configured kind name has to be checked against the right one. A name that
//! exists only in the TSX grammar (`jsx_element`) must not be configured for
//! `.ts`, and a name that exists only in the plain grammar (`type_assertion`)
//! must not be configured for `.tsx`.
//!
//! Nothing here is configured by guesswork: every kind name below was taken from
//! the grammars' own node inventories, and [`assert_all_present`] is what keeps
//! it that way.

mod support;

use std::collections::BTreeSet;

use sm_cst::{
    ChildListKind, Language,
    languages::{TsDialect, TypeScriptLanguage},
};
use support::{TS_FIXTURES, parse_ts_fixture};

/// Every node kind a compiled-in grammar knows about.
fn grammar_kinds(lang: &dyn Language) -> BTreeSet<&'static str> {
    let ts = lang.ts_language();
    (0..ts.node_kind_count())
        .filter_map(|id| ts.node_kind_for_id(u16::try_from(id).expect("kind id fits in u16")))
        .collect()
}

#[track_caller]
fn assert_all_present(lang: &dyn Language, label: &str, names: &[&str]) {
    let known = grammar_kinds(lang);
    let missing: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| !known.contains(n))
        .collect();
    assert!(
        missing.is_empty(),
        "{label}: these kind names are not in the {} grammar: {missing:?}. \
         Either they are typos, or the grammar renamed them and the configuration needs updating.",
        lang.name()
    );
}

fn both() -> [&'static dyn Language; 2] {
    [support::typescript(), support::tsx()]
}

// ------------------------------------------------- the grammars themselves --

/// The compatibility check the brief asks for, done by parsing rather than by
/// comparing version numbers: `tree-sitter-typescript 0.23.2` ships ABI-14
/// grammars and the `tree-sitter 0.26` runtime loads them.
///
/// `parse` maps a rejected grammar to `ParseError::LanguageInit`, so this fails
/// loudly and specifically if the ABI ever stops lining up.
#[test]
fn both_grammars_load_under_our_tree_sitter_runtime() {
    for lang in both() {
        let tree = sm_cst::parse(b"const x: number = 1;\n", lang)
            .unwrap_or_else(|err| panic!("{}: {err}", lang.name()));
        assert!(
            !tree.has_errors(),
            "{}: trivial source should parse",
            lang.name()
        );
        assert_eq!(tree.root().kind, "program");
        assert!(tree.len() > 1);
        // A grammar that failed to load would give a zero-kind language.
        assert!(grammar_kinds(lang).len() > 100, "{}", lang.name());

        // The real compatibility constraint is the parser ABI, not the crate
        // version (docs/prior-art.md §6.2). Assert it rather than describing it:
        // `tree-sitter-typescript 0.23.2` generates ABI 14, and the 0.26 runtime
        // accepts 13..=15.
        let abi = lang.ts_language().abi_version();
        assert!(
            (tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION..=tree_sitter::LANGUAGE_VERSION)
                .contains(&abi),
            "{}: grammar ABI {abi} is outside the runtime's supported range {}..={}",
            lang.name(),
            tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION,
            tree_sitter::LANGUAGE_VERSION
        );
    }
}

/// The Java grammar and the two TypeScript grammars are at the same ABI, which
/// is the whole reason one runtime can host both crates at unrelated version
/// numbers.
#[test]
fn every_registered_grammar_is_at_a_supported_abi() {
    for lang in sm_cst::languages::all() {
        let abi = lang.ts_language().abi_version();
        assert!(
            (tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION..=tree_sitter::LANGUAGE_VERSION)
                .contains(&abi),
            "{}: ABI {abi}",
            lang.name()
        );
    }
}

/// The two grammars are genuinely different parsers, not the same one twice.
/// If they ever became identical, one of the two registry entries would be
/// pointless and `.tsx` files would silently parse with the wrong grammar.
#[test]
fn the_two_grammars_differ_where_they_are_supposed_to() {
    let ts = grammar_kinds(support::typescript());
    let tsx = grammar_kinds(support::tsx());

    assert!(tsx.contains("jsx_element"), "TSX should know JSX");
    assert!(
        !ts.contains("jsx_element"),
        "the plain TypeScript grammar should not know JSX"
    );
    assert!(
        ts.contains("type_assertion"),
        "the plain TypeScript grammar should know <T>x assertions"
    );
    assert!(
        !tsx.contains("type_assertion"),
        "TSX cannot have angle-bracket type assertions; <T>x is an element there"
    );

    // And the difference shows up in a real parse, not just in the symbol table.
    let jsx = b"const a = <Foo bar=\"baz\">hi</Foo>;\n";
    assert!(
        !sm_cst::parse(jsx, support::tsx()).unwrap().has_errors(),
        "TSX should parse JSX cleanly"
    );
    assert!(
        sm_cst::parse(jsx, support::typescript())
            .unwrap()
            .has_errors(),
        "the plain TypeScript grammar should choke on JSX — if it does not, \
         the two registry entries are parsing with the same grammar"
    );
}

// ------------------------------------------------------- inventory guards --

#[test]
fn unordered_containers_exist_in_both_grammars() {
    for lang in both() {
        assert_all_present(
            lang,
            "UNORDERED_CONTAINERS",
            TypeScriptLanguage::UNORDERED_CONTAINERS,
        );
    }
}

#[test]
fn ordered_containers_exist_in_both_grammars() {
    for lang in both() {
        assert_all_present(
            lang,
            "ORDERED_CONTAINERS",
            TypeScriptLanguage::ORDERED_CONTAINERS,
        );
    }
}

#[test]
fn tsx_only_containers_exist_in_the_tsx_grammar_and_only_there() {
    assert_all_present(
        support::tsx(),
        "TSX_ONLY_ORDERED_CONTAINERS",
        TypeScriptLanguage::TSX_ONLY_ORDERED_CONTAINERS,
    );
    let ts = grammar_kinds(support::typescript());
    let leaked: Vec<&str> = TypeScriptLanguage::TSX_ONLY_ORDERED_CONTAINERS
        .iter()
        .copied()
        .filter(|k| ts.contains(k))
        .collect();
    assert!(
        leaked.is_empty(),
        "these are in the plain TypeScript grammar after all, so they are not TSX-only: {leaked:?}"
    );
}

#[test]
fn scope_introducing_kinds_exist_in_both_grammars() {
    for lang in both() {
        assert_all_present(
            lang,
            "SCOPE_INTRODUCING",
            TypeScriptLanguage::SCOPE_INTRODUCING,
        );
    }
}

#[test]
fn identifier_kinds_exist_in_both_grammars() {
    for lang in both() {
        assert_all_present(lang, "IDENTIFIERS", TypeScriptLanguage::IDENTIFIERS);
    }
}

#[test]
fn comment_kinds_exist_in_both_grammars() {
    for lang in both() {
        assert_all_present(lang, "COMMENTS", TypeScriptLanguage::COMMENTS);
    }
}

#[test]
fn significant_text_kinds_exist_in_both_grammars() {
    for lang in both() {
        assert_all_present(
            lang,
            "SIGNIFICANT_TEXT",
            TypeScriptLanguage::SIGNIFICANT_TEXT,
        );
    }
    assert_all_present(
        support::tsx(),
        "TSX_ONLY_SIGNIFICANT_TEXT",
        TypeScriptLanguage::TSX_ONLY_SIGNIFICANT_TEXT,
    );
}

/// The ordered and unordered lists must not overlap; a kind cannot be both.
#[test]
fn ordered_and_unordered_lists_are_disjoint() {
    let unordered: BTreeSet<&str> = TypeScriptLanguage::UNORDERED_CONTAINERS
        .iter()
        .copied()
        .collect();
    let overlap: Vec<&str> = TypeScriptLanguage::ORDERED_CONTAINERS
        .iter()
        .chain(TypeScriptLanguage::TSX_ONLY_ORDERED_CONTAINERS)
        .copied()
        .filter(|k| unordered.contains(k))
        .collect();
    assert!(overlap.is_empty(), "kinds in both lists: {overlap:?}");
}

/// No list may repeat a name; a duplicate is a sign of a merge gone wrong and
/// makes the reasoning above harder to read than it needs to be.
#[test]
fn no_configuration_list_repeats_itself() {
    for (label, names) in [
        (
            "UNORDERED_CONTAINERS",
            TypeScriptLanguage::UNORDERED_CONTAINERS,
        ),
        ("ORDERED_CONTAINERS", TypeScriptLanguage::ORDERED_CONTAINERS),
        (
            "TSX_ONLY_ORDERED_CONTAINERS",
            TypeScriptLanguage::TSX_ONLY_ORDERED_CONTAINERS,
        ),
        ("SCOPE_INTRODUCING", TypeScriptLanguage::SCOPE_INTRODUCING),
        ("IDENTIFIERS", TypeScriptLanguage::IDENTIFIERS),
        ("COMMENTS", TypeScriptLanguage::COMMENTS),
        ("SIGNIFICANT_TEXT", TypeScriptLanguage::SIGNIFICANT_TEXT),
    ] {
        let unique: BTreeSet<&str> = names.iter().copied().collect();
        assert_eq!(unique.len(), names.len(), "{label} has a duplicate entry");
    }
}

// ------------------------------------------------------ the classification --

#[test]
fn child_list_kinds_are_what_we_intend() {
    for lang in both() {
        for kind in TypeScriptLanguage::UNORDERED_CONTAINERS {
            assert_eq!(
                lang.child_list_kind(kind),
                ChildListKind::Unordered,
                "{}: {kind} should be unordered",
                lang.name()
            );
        }
        for kind in TypeScriptLanguage::ORDERED_CONTAINERS
            .iter()
            .chain(TypeScriptLanguage::TSX_ONLY_ORDERED_CONTAINERS)
        {
            assert_eq!(
                lang.child_list_kind(kind),
                ChildListKind::Ordered,
                "{}: {kind} should be ordered",
                lang.name()
            );
        }

        // The default for anything unrecognised is the conservative one.
        assert_eq!(
            lang.child_list_kind("a_kind_no_grammar_has"),
            ChildListKind::Ordered
        );
    }
}

/// **The headline difference from Java.** A Java compilation unit is
/// `PartiallyUnordered` over its imports because a Java import has no runtime
/// effect. An ES import is an edge in the module graph, evaluated in source
/// order, so a TypeScript `program` is flatly Ordered.
#[test]
fn program_is_ordered_unlike_java() {
    use sm_cst::languages::JavaLanguage;

    for lang in both() {
        assert_eq!(
            lang.child_list_kind("program"),
            ChildListKind::Ordered,
            "{}: TypeScript imports are evaluated in order",
            lang.name()
        );
    }
    // Stated as a contrast so that a future change to either side is visible
    // here rather than only in prose.
    assert!(matches!(
        JavaLanguage.child_list_kind("program"),
        ChildListKind::PartiallyUnordered { .. }
    ));
}

/// The optimisation we *can* have safely: the brace list of a named import is a
/// pure set of bindings, and two branches each adding a name to it is the most
/// common TypeScript import conflict there is.
#[test]
fn named_import_and_export_lists_are_unordered() {
    for lang in both() {
        assert_eq!(
            lang.child_list_kind("named_imports"),
            ChildListKind::Unordered
        );
        assert_eq!(
            lang.child_list_kind("export_clause"),
            ChildListKind::Unordered
        );
        // But the statement that wraps them is not.
        assert_eq!(
            lang.child_list_kind("import_statement"),
            ChildListKind::Ordered
        );
    }
}

/// Numeric enum members auto-increment, so `enum_body` is a set-shaped trap in
/// exactly the way Java's is — worse, in fact, since the member's own value
/// changes rather than just its `ordinal()`.
#[test]
fn enum_bodies_are_ordered_but_type_bodies_are_not() {
    for lang in both() {
        assert_eq!(lang.child_list_kind("enum_body"), ChildListKind::Ordered);
        assert_eq!(
            lang.child_list_kind("interface_body"),
            ChildListKind::Unordered
        );
        assert_eq!(
            lang.child_list_kind("object_type"),
            ChildListKind::Unordered
        );
    }
}

/// An object literal looks like the most set-shaped thing in the language and is
/// not one: a later key overrides an earlier one, spreads interleave, and
/// enumeration order is observable.
#[test]
fn object_literals_and_patterns_are_ordered() {
    for lang in both() {
        assert_eq!(lang.child_list_kind("object"), ChildListKind::Ordered);
        assert_eq!(
            lang.child_list_kind("object_pattern"),
            ChildListKind::Ordered
        );
        assert_eq!(lang.child_list_kind("array"), ChildListKind::Ordered);
    }
}

/// Parameter and argument lists are strictly positional; union and intersection
/// types read like sets but drive order-sensitive overload resolution.
#[test]
fn positional_and_type_lists_are_ordered() {
    for lang in both() {
        for kind in [
            "formal_parameters",
            "arguments",
            "type_parameters",
            "type_arguments",
            "tuple_type",
            "union_type",
            "intersection_type",
        ] {
            assert_eq!(lang.child_list_kind(kind), ChildListKind::Ordered, "{kind}");
        }
    }
}

#[test]
fn scope_identifier_and_comment_predicates_agree_with_their_lists() {
    for lang in both() {
        for kind in TypeScriptLanguage::SCOPE_INTRODUCING {
            assert!(lang.is_scope_introducing(kind), "{kind}");
        }
        for kind in TypeScriptLanguage::IDENTIFIERS {
            assert!(lang.is_identifier(kind), "{kind}");
        }
        for kind in TypeScriptLanguage::COMMENTS {
            assert!(lang.is_comment(kind), "{kind}");
        }
        for kind in TypeScriptLanguage::SIGNIFICANT_TEXT {
            assert!(lang.significant_text(kind), "{kind}");
        }

        assert!(!lang.is_scope_introducing("public_field_definition"));
        assert!(!lang.is_comment("statement_block"));
        // Distinct kinds already, so their text adds nothing to the hash.
        assert!(!lang.significant_text("true"));
        assert!(!lang.significant_text("null"));
        assert!(!lang.significant_text("undefined"));
        assert!(!lang.significant_text("this"));
        assert!(!lang.significant_text(";"));
        assert!(!lang.significant_text("const"));
    }
}

/// `property_identifier` is a name but not a lexically-resolvable one: it
/// resolves against the type of a receiver, which needs a type checker rather
/// than a scope tree. It is excluded from `IDENTIFIERS` on purpose, and included
/// in `SIGNIFICANT_TEXT` on purpose — the matcher must still tell `.foo` from
/// `.bar`. See the gap note in `languages/typescript.rs`.
#[test]
fn member_names_are_significant_but_not_lexical_identifiers() {
    for lang in both() {
        for kind in ["property_identifier", "private_property_identifier"] {
            assert!(!lang.is_identifier(kind), "{kind} is not lexical");
            assert!(lang.significant_text(kind), "{kind} text still matters");
        }
        // Whereas the shorthand forms *are* lexical references/bindings.
        assert!(lang.is_identifier("shorthand_property_identifier"));
        assert!(lang.is_identifier("shorthand_property_identifier_pattern"));
    }
}

/// `predefined_type` is one kind with nine meanings — the only thing separating
/// `string` from `never` is the text. Java has no analogue: its primitive types
/// are distinct node kinds.
#[test]
fn predefined_type_text_is_significant() {
    for lang in both() {
        assert!(lang.significant_text("predefined_type"));
        assert!(lang.significant_text("accessibility_modifier"));
    }
}

/// `jsx_text` is dialect-scoped: it is rendered content in TSX, and the plain
/// dialect has no business claiming it.
#[test]
fn jsx_text_is_significant_only_in_the_tsx_dialect() {
    assert!(support::tsx().significant_text("jsx_text"));
    assert!(!support::typescript().significant_text("jsx_text"));
}

// ------------------------------------------------------------- the registry --

#[test]
fn the_registry_wires_up_both_dialects() {
    use std::path::Path;

    let name = |p: &str| sm_cst::languages::detect(Path::new(p)).map(Language::name);

    assert_eq!(name("/tmp/foo.ts"), Some("typescript"));
    assert_eq!(name("/tmp/foo.TS"), Some("typescript"));
    assert_eq!(name("/tmp/foo.mts"), Some("typescript"));
    assert_eq!(name("/tmp/foo.cts"), Some("typescript"));
    // A declaration file needs no special case: its extension *is* `ts`.
    assert_eq!(name("/tmp/globals.d.ts"), Some("typescript"));
    assert_eq!(name("/tmp/App.tsx"), Some("tsx"));
    assert_eq!(name("/tmp/App.TSX"), Some("tsx"));
    // Not ours.
    assert_eq!(name("/tmp/app.js"), None);
    assert_eq!(name("/tmp/app.jsx"), None);
    assert_eq!(name("/tmp/Foo.java"), Some("java"));

    assert_eq!(
        sm_cst::languages::by_name("typescript").map(Language::name),
        Some("typescript")
    );
    assert_eq!(
        sm_cst::languages::by_name("tsx").map(Language::name),
        Some("tsx")
    );
    assert_eq!(
        TypeScriptLanguage::TYPESCRIPT.dialect(),
        TsDialect::TypeScript
    );
    assert_eq!(TypeScriptLanguage::TSX.dialect(), TsDialect::Tsx);
}

/// Two languages must never claim the same extension; `detect` takes the first
/// match, so an overlap would make the registry order load-bearing.
#[test]
fn no_two_languages_claim_the_same_extension() {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for lang in sm_cst::languages::all() {
        for ext in lang.file_extensions() {
            assert!(
                seen.insert(ext),
                "extension {ext:?} is claimed by more than one language"
            );
            assert_eq!(
                *ext,
                ext.to_ascii_lowercase(),
                "extensions are matched lowercased, so they must be stored lowercased"
            );
        }
    }
}

/// The configured names must not merely exist in the grammar — they must be the
/// names that actually show up when parsing TypeScript. A kind that exists but
/// is never produced would be configuration that never fires.
#[test]
fn configured_kinds_actually_appear_in_the_fixtures() {
    let mut seen: BTreeSet<&'static str> = BTreeSet::new();
    for name in TS_FIXTURES {
        let tree = parse_ts_fixture(name);
        seen.extend(tree.nodes().map(|n| n.kind));
    }

    let must_appear = [
        // Containers whose classification is load-bearing.
        "program",
        "import_statement",
        "import_clause",
        "named_imports",
        "export_statement",
        "export_clause",
        "class_body",
        "interface_body",
        "object_type",
        "enum_body",
        "object",
        "object_pattern",
        "array",
        "arguments",
        "formal_parameters",
        "type_parameters",
        "type_arguments",
        "union_type",
        "statement_block",
        "lexical_declaration",
        // Scope-introducing kinds.
        "class_declaration",
        "interface_declaration",
        "type_alias_declaration",
        "enum_declaration",
        "function_declaration",
        "arrow_function",
        "method_definition",
        "class_static_block",
        "internal_module",
        "for_in_statement",
        "catch_clause",
        // Names and text.
        "identifier",
        "type_identifier",
        "property_identifier",
        "shorthand_property_identifier",
        "shorthand_property_identifier_pattern",
        "statement_identifier",
        "predefined_type",
        "accessibility_modifier",
        "number",
        "string_fragment",
        "template_string",
        "comment",
        // TSX.
        "jsx_element",
        "jsx_self_closing_element",
        "jsx_opening_element",
        "jsx_expression",
        "jsx_text",
    ];
    let absent: Vec<&str> = must_appear
        .iter()
        .copied()
        .filter(|k| !seen.contains(k))
        .collect();
    assert!(
        absent.is_empty(),
        "these kinds never appeared while parsing the TypeScript fixtures: {absent:?}"
    );
}
