//! Guards on the Java **identifier role** and **declaration** configuration
//! (M6).
//!
//! Two kinds of test:
//!
//! 1. **Inventory guards**, in the style of `language_config.rs`: every kind
//!    name the role table mentions must exist in the compiled-in grammar, so a
//!    typo or a grammar rename fails the build rather than silently turning a
//!    rule off. A silently-disabled `MEMBER_REF_CONTEXTS` entry would make
//!    `sm-bind` report a semantic conflict on every qualified call in the file,
//!    which is exactly the failure this whole layer exists to prevent.
//! 2. **Classification tests**, which pin the role of every name in a gallery
//!    of real Java and assert the specific rules by hand.

mod support;

use std::collections::BTreeSet;
use std::fmt::Write as _;

use sm_cst::{DeclKind, IdentifierRole, Language, NodeId, SourceTree, languages::JavaLanguage};

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

// ---------------------------------------------------------- inventory guards

#[test]
fn declaration_kinds_exist_in_the_grammar() {
    let names: Vec<&str> = JavaLanguage::DECLARATIONS.iter().map(|(k, _)| *k).collect();
    assert_all_present("DECLARATIONS", &names);
}

#[test]
fn member_ref_context_parents_exist_in_the_grammar() {
    let names: Vec<&str> = JavaLanguage::MEMBER_REF_CONTEXTS
        .iter()
        .map(|(k, _)| *k)
        .collect();
    assert_all_present("MEMBER_REF_CONTEXTS", &names);
}

#[test]
fn label_parents_exist_in_the_grammar() {
    assert_all_present("LABEL_PARENTS", JavaLanguage::LABEL_PARENTS);
}

#[test]
fn qualified_name_roots_exist_in_the_grammar() {
    assert_all_present("QUALIFIED_NAME_ROOTS", JavaLanguage::QUALIFIED_NAME_ROOTS);
}

/// A declaration kind that is never listed twice, so the lookup is unambiguous.
#[test]
fn no_declaration_kind_is_listed_twice() {
    let unique: BTreeSet<&str> = JavaLanguage::DECLARATIONS.iter().map(|(k, _)| *k).collect();
    assert_eq!(unique.len(), JavaLanguage::DECLARATIONS.len());
}

/// The kinds `sm-bind`'s resolver consumes must all be reachable: every
/// declaration kind in the table must be one `declaration_kind` actually
/// returns, and every one must produce a usable name on a real example.
#[test]
fn every_declaration_kind_in_the_table_is_produced_by_a_real_file() {
    let src = "\
package p;

import java.util.List;

module M {}

class C<T> {
    int field;
    C(int p) { int local = p; }
    <R> R m(R r, int... rest) {
        try (AutoCloseable c = null) {} catch (Exception e) {}
        return r;
    }
    enum E { RED }
    record Rec(int x) {}
    interface I { void h(); }
    @interface A { String value(); }
}
";
    let (tree, java) = parse(src);
    let mut seen: BTreeSet<&'static str> = BTreeSet::new();
    for id in tree.ids() {
        if let Some(kind) = java.declaration_kind(&tree, id) {
            seen.insert(kind.tag());
        }
    }
    // `module_declaration` only parses in a `module-info.java`, so it is
    // exercised separately below; everything else must appear here.
    let expected: BTreeSet<&'static str> = JavaLanguage::DECLARATIONS
        .iter()
        .map(|(_, d)| d.tag())
        .chain(std::iter::once(DeclKind::Local.tag()))
        .chain(std::iter::once(DeclKind::Field.tag()))
        .collect();
    let missing: Vec<&str> = expected
        .difference(&seen)
        .copied()
        .filter(|t| *t != DeclKind::Module.tag())
        .collect();
    assert!(missing.is_empty(), "never produced: {missing:?}");
}

/// The two halves of the API must agree: if `declaration_kind(d)` says `d`
/// declares something and `declared_name(d)` points at a name node, then
/// `identifier_role` of that node must be `Declaration` of the same kind.
///
/// `sm-bind` relies on exactly this to decide whether a "name" is a real name
/// or a destructuring pattern it must skip.
#[test]
fn declared_names_are_classified_as_declarations() {
    for fixture in support::FIXTURES {
        let tree = support::parse_fixture(fixture);
        let java = support::java();
        for id in tree.ids() {
            let Some(kind) = java.declaration_kind(&tree, id) else {
                continue;
            };
            let Some(name) = java.declared_name(&tree, id) else {
                continue;
            };
            let role = java.identifier_role(&tree, name);
            if role == IdentifierRole::NotAName {
                // A "name" that is not an identifier at all — `module "x"`.
                continue;
            }
            assert_eq!(
                role,
                IdentifierRole::Declaration(kind),
                "in {fixture}, the name of {} `{}` was classified {}",
                tree.node(id).kind,
                tree.node_text(name),
                role.tag(),
            );
        }
    }
}

// ------------------------------------------------------- classification rules

/// **The finding that motivated roles.** In `tree-sitter-java` the kind
/// `identifier` covers all of these, and treating them alike would make
/// `sm-bind` report a broken reference on every qualified call in the file.
#[test]
fn a_qualified_call_name_is_a_member_ref_and_an_unqualified_one_is_not() {
    let src = "class C { void f(User u) { u.getName(); helper(); } void helper() {} }";
    assert_eq!(role_of(src, "getName", 0), IdentifierRole::MemberRef);
    assert_eq!(role_of(src, "helper", 0), IdentifierRole::CallRef);
    // `u` #0 is the parameter's own declaration; #1 is the receiver.
    assert_eq!(
        role_of(src, "u", 0),
        IdentifierRole::Declaration(DeclKind::Parameter)
    );
    assert_eq!(role_of(src, "u", 1), IdentifierRole::LexicalRef);
}

#[test]
fn a_field_access_selects_against_a_receiver() {
    let src = "class C { void f(P p) { int a = p.x; System.out.println(a); } }";
    // `p` is a lexical reference; `x` and `out` and `println` are not.
    assert_eq!(role_of(src, "p", 1), IdentifierRole::LexicalRef);
    assert_eq!(role_of(src, "x", 0), IdentifierRole::MemberRef);
    assert_eq!(role_of(src, "out", 0), IdentifierRole::MemberRef);
    assert_eq!(role_of(src, "println", 0), IdentifierRole::MemberRef);
    assert_eq!(role_of(src, "System", 0), IdentifierRole::LexicalRef);
}

#[test]
fn declarations_are_recognised_by_their_name_field() {
    let src = "class C { int f; void m(int p) { int l = 0; } }";
    assert_eq!(
        role_of(src, "C", 0),
        IdentifierRole::Declaration(DeclKind::Class)
    );
    assert_eq!(
        role_of(src, "f", 0),
        IdentifierRole::Declaration(DeclKind::Field)
    );
    assert_eq!(
        role_of(src, "m", 0),
        IdentifierRole::Declaration(DeclKind::Method)
    );
    assert_eq!(
        role_of(src, "p", 0),
        IdentifierRole::Declaration(DeclKind::Parameter)
    );
    assert_eq!(
        role_of(src, "l", 0),
        IdentifierRole::Declaration(DeclKind::Local)
    );
}

#[test]
fn an_import_declares_its_simple_name_and_a_wildcard_declares_nothing() {
    let src = "import java.util.List;\nimport java.util.*;\nclass C {}\n";
    let (tree, java) = parse(src);
    let imports: Vec<NodeId> = tree
        .ids()
        .filter(|&id| tree.node(id).kind == "import_declaration")
        .collect();
    assert_eq!(imports.len(), 2);
    let simple = java.declared_name(&tree, imports[0]).expect("simple name");
    assert_eq!(tree.node_text(simple), "List");
    assert_eq!(java.declared_name(&tree, imports[1]), None);

    // The path segments are not lexical references.
    assert_eq!(role_of(src, "java", 0), IdentifierRole::MemberRef);
    assert_eq!(role_of(src, "util", 0), IdentifierRole::MemberRef);
    assert_eq!(
        role_of(src, "List", 0),
        IdentifierRole::Declaration(DeclKind::Import)
    );
}

#[test]
fn labels_are_their_own_role() {
    let src = "class C { void f() { outer: while (true) { break outer; } } }";
    assert_eq!(role_of(src, "outer", 0), IdentifierRole::Label);
    assert_eq!(role_of(src, "outer", 1), IdentifierRole::Label);
}

#[test]
fn an_annotation_name_is_a_type_reference_but_its_element_keys_are_not() {
    let src = "class C { @Anno(name = \"x\") int f; }";
    assert_eq!(role_of(src, "Anno", 0), IdentifierRole::TypeRef);
    assert_eq!(role_of(src, "name", 0), IdentifierRole::MemberRef);
}

#[test]
fn a_qualified_type_selects_after_its_first_segment() {
    let src = "class C { Outer.Inner x; }";
    assert_eq!(role_of(src, "Outer", 0), IdentifierRole::TypeRef);
    assert_eq!(role_of(src, "Inner", 0), IdentifierRole::MemberRef);
}

#[test]
fn a_method_reference_qualifier_is_lexical_and_its_tail_is_not() {
    let src = "class C { Object r = String::valueOf; }";
    assert_eq!(role_of(src, "String", 0), IdentifierRole::LexicalRef);
    assert_eq!(role_of(src, "valueOf", 0), IdentifierRole::MemberRef);
}

/// The whole role table, over a gallery, pinned. A change anywhere in the
/// classification shows up here even if nobody wrote the assertion for it.
#[test]
fn java_role_gallery() {
    let src = "\
package com.example;

import java.util.List;
import static java.util.Objects.requireNonNull;

@Deprecated
class Repo<T extends Number> implements Iface {
    private final Store store;
    static final int LIMIT = 10;

    Repo(Store store) {
        this.store = store;
    }

    <R> R pick(R a, String id) throws IOException {
        User u = store.find(id);
        int n = LIMIT;
        for (int i = 0; i < n; i++) {
            log(i);
        }
        try (AutoCloseable c = open()) {
            u.close();
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
        outer:
        while (true) {
            break outer;
        }
        return a;
    }

    void log(int i) {}

    enum Color { RED, GREEN }
}
";
    insta::assert_snapshot!(role_gallery(src));
}

// ------------------------------------------------------------------ helpers

fn parse(src: &str) -> (SourceTree, &'static dyn Language) {
    let java = support::java();
    (sm_cst::parse(src.as_bytes(), java).expect("parses"), java)
}

/// The role of the `occurrence`-th name node whose text is `name`.
fn role_of(src: &str, name: &str, occurrence: usize) -> IdentifierRole {
    let (tree, java) = parse(src);
    tree.ids()
        .filter(|&id| {
            tree.node(id).is_leaf()
                && tree.node_text(id) == name
                && java.identifier_role(&tree, id) != IdentifierRole::NotAName
        })
        .nth(occurrence)
        .map(|id| java.identifier_role(&tree, id))
        .unwrap_or_else(|| panic!("no name node `{name}` #{occurrence} in:\n{src}"))
}

fn role_gallery(src: &str) -> String {
    let (tree, java) = parse(src);
    let mut out = String::new();
    for id in tree.ids() {
        let role = java.identifier_role(&tree, id);
        if role == IdentifierRole::NotAName {
            continue;
        }
        let label = match role {
            IdentifierRole::Declaration(kind) => format!("declaration({})", kind.tag()),
            other => other.tag().to_owned(),
        };
        let parent = tree.node(id).parent.map_or("-", |p| tree.node(p).kind);
        writeln!(
            out,
            "{:<24} {:<20} {:<28} {}",
            format!("`{}`", tree.node_text(id)),
            label,
            parent,
            tree.field_name(id).unwrap_or("-"),
        )
        .unwrap();
    }
    out
}
