//! Guards on the TypeScript **identifier role** and **declaration**
//! configuration (M6). The Java counterpart is `roles.rs`; see its module docs
//! for why these tests exist.
//!
//! The case this file exists to pin down is the one that motivated
//! [`sm_cst::IdentifierRole`] in the first place: `property_identifier` and
//! `private_property_identifier` are *receiver-typed*. If they were treated as
//! lexical references, `sm-bind` would report a broken reference on every
//! property access in every TypeScript file it saw.

mod support;

use std::collections::BTreeSet;
use std::fmt::Write as _;

use sm_cst::{
    DeclKind, IdentifierRole, Language, SourceTree,
    languages::{TsDialect, TypeScriptLanguage},
};

fn both() -> [&'static dyn Language; 2] {
    [support::typescript(), support::tsx()]
}

fn grammar_kinds(lang: &dyn Language) -> BTreeSet<&'static str> {
    let ts = lang.ts_language();
    (0..ts.node_kind_count())
        .filter_map(|id| ts.node_kind_for_id(u16::try_from(id).expect("kind id fits in u16")))
        .collect()
}

fn assert_all_present(lang: &dyn Language, label: &str, names: &[&str]) {
    let known = grammar_kinds(lang);
    let missing: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| !known.contains(n))
        .collect();
    assert!(
        missing.is_empty(),
        "{label}: not in the {} grammar: {missing:?}",
        lang.name()
    );
}

// ---------------------------------------------------------- inventory guards

#[test]
fn declaration_kinds_exist_in_both_grammars() {
    let names: Vec<&str> = TypeScriptLanguage::DECLARATIONS
        .iter()
        .map(|(k, _)| *k)
        .collect();
    for lang in both() {
        assert_all_present(lang, "DECLARATIONS", &names);
    }
}

#[test]
fn member_ref_context_parents_exist_in_both_grammars() {
    let names: Vec<&str> = TypeScriptLanguage::MEMBER_REF_CONTEXTS
        .iter()
        .map(|(k, _)| *k)
        .collect();
    for lang in both() {
        assert_all_present(lang, "MEMBER_REF_CONTEXTS", &names);
    }
}

#[test]
fn receiver_typed_name_kinds_exist_in_both_grammars() {
    for lang in both() {
        assert_all_present(
            lang,
            "RECEIVER_TYPED_NAMES",
            TypeScriptLanguage::RECEIVER_TYPED_NAMES,
        );
    }
}

#[test]
fn self_declaring_parents_exist_in_both_grammars() {
    let names: Vec<&str> = TypeScriptLanguage::SELF_DECLARING_PARENTS
        .iter()
        .map(|(k, _)| *k)
        .collect();
    for lang in both() {
        assert_all_present(lang, "SELF_DECLARING_PARENTS", &names);
    }
}

#[test]
fn function_scopes_exist_in_both_grammars() {
    for lang in both() {
        assert_all_present(lang, "FUNCTION_SCOPES", TypeScriptLanguage::FUNCTION_SCOPES);
    }
}

/// A function scope is a scope. If one of these were missing from
/// `SCOPE_INTRODUCING`, a `var` would hoist to a scope that does not exist and
/// land at the file scope instead.
#[test]
fn every_function_scope_is_also_scope_introducing() {
    let scopes: BTreeSet<&str> = TypeScriptLanguage::SCOPE_INTRODUCING
        .iter()
        .copied()
        .collect();
    let missing: Vec<&str> = TypeScriptLanguage::FUNCTION_SCOPES
        .iter()
        .copied()
        .filter(|k| !scopes.contains(k))
        .collect();
    assert!(missing.is_empty(), "not scope-introducing: {missing:?}");
}

#[test]
fn no_declaration_kind_is_listed_twice() {
    let unique: BTreeSet<&str> = TypeScriptLanguage::DECLARATIONS
        .iter()
        .map(|(k, _)| *k)
        .collect();
    assert_eq!(unique.len(), TypeScriptLanguage::DECLARATIONS.len());
}

/// The two halves of the API must agree — see the Java counterpart. In
/// TypeScript this also covers the case `sm-bind` depends on: a parameter whose
/// "name" is a destructuring pattern returns a *non-name* node, which the
/// binder must skip rather than register.
#[test]
fn declared_names_are_classified_as_declarations_or_patterns() {
    let src = "\
import def, * as ns from \"./m\";
import { a, b as renamed } from \"./n\";

export function f({ x, y: z }: any, [p, ...rest]: number[], plain = 1) {
  const { q } = plain;
  for (const item of []) {}
  try {} catch (err) {}
  return [x, z, p, rest, q, item, err];
}

export class C {
  field = 1;
  #secret = 2;
  method<T>(t: T) { return t; }
}

interface I { p: string; m(): void; }
type Alias = string;
enum E { RED = 1 }
namespace NS {}
";
    let (tree, ts) = parse(src);
    for id in tree.ids() {
        let Some(kind) = ts.declaration_kind(&tree, id) else {
            continue;
        };
        let Some(name) = ts.declared_name(&tree, id) else {
            continue;
        };
        let role = ts.identifier_role(&tree, name);
        if role == IdentifierRole::NotAName {
            // A destructuring pattern or a string module name. `sm-bind` skips
            // these, and their leaves declare themselves.
            continue;
        }
        assert_eq!(
            role,
            IdentifierRole::Declaration(kind),
            "the name of {} `{}` was classified {}",
            tree.node(id).kind,
            tree.node_text(name),
            role.tag(),
        );
    }
}

// ------------------------------------------------------- classification rules

/// **The finding.** Every property name here is receiver-typed and must never
/// be resolved lexically.
#[test]
fn property_names_are_never_lexical_references() {
    let src = "\
export class C {
  private log = 1;
  #secret = 2;
  f(o: any) {
    console.log(o.name, o.a.b, this.log, this.#secret);
    return { key: 1 };
  }
}
";
    // Member selections.
    assert_eq!(role_of(src, "name", 0), IdentifierRole::MemberRef);
    assert_eq!(role_of(src, "b", 0), IdentifierRole::MemberRef);
    assert_eq!(role_of(src, "key", 0), IdentifierRole::MemberRef);
    // Class members are declarations, but of a *member* kind, which the binder
    // keeps out of unqualified lookup for TypeScript.
    assert_eq!(
        role_of(src, "log", 0),
        IdentifierRole::Declaration(DeclKind::Field)
    );
    assert_eq!(
        role_of(src, "#secret", 0),
        IdentifierRole::Declaration(DeclKind::Field)
    );
    // The uses of those members, through `this`, are member refs.
    assert_eq!(role_of(src, "log", 2), IdentifierRole::MemberRef);
    assert_eq!(role_of(src, "#secret", 1), IdentifierRole::MemberRef);
    // The receiver itself is lexical.
    assert_eq!(role_of(src, "console", 0), IdentifierRole::LexicalRef);
}

/// TypeScript has one value namespace, so a call target is an ordinary lexical
/// reference and `CallRef` is never produced.
#[test]
fn a_call_target_is_an_ordinary_lexical_reference() {
    let src = "export function f() { return helper(); }\nfunction helper() { return 1; }\n";
    assert_eq!(role_of(src, "helper", 0), IdentifierRole::LexicalRef);
    assert_eq!(
        role_of(src, "helper", 1),
        IdentifierRole::Declaration(DeclKind::Function)
    );
}

#[test]
fn let_const_and_var_are_told_apart() {
    let src = "const a = 1;\nlet b = 2;\nvar c = 3;\n";
    assert_eq!(
        role_of(src, "a", 0),
        IdentifierRole::Declaration(DeclKind::Local)
    );
    assert_eq!(
        role_of(src, "b", 0),
        IdentifierRole::Declaration(DeclKind::Local)
    );
    assert_eq!(
        role_of(src, "c", 0),
        IdentifierRole::Declaration(DeclKind::Var)
    );
}

#[test]
fn imports_declare_the_local_binding_and_not_the_exported_name() {
    let src = "import def, * as ns from \"./m\";\nimport { a, b as renamed } from \"./n\";\n";
    assert_eq!(
        role_of(src, "def", 0),
        IdentifierRole::Declaration(DeclKind::Import)
    );
    assert_eq!(
        role_of(src, "ns", 0),
        IdentifierRole::Declaration(DeclKind::Import)
    );
    assert_eq!(
        role_of(src, "a", 0),
        IdentifierRole::Declaration(DeclKind::Import)
    );
    // `b` is the *exported* name over in `./n`; only `renamed` is bound here.
    assert_eq!(role_of(src, "b", 0), IdentifierRole::MemberRef);
    assert_eq!(
        role_of(src, "renamed", 0),
        IdentifierRole::Declaration(DeclKind::Import)
    );
}

#[test]
fn destructuring_leaves_declare_themselves() {
    let src = "const { x, y: z } = o;\nconst [p, ...rest] = xs;\n";
    assert_eq!(
        role_of(src, "x", 0),
        IdentifierRole::Declaration(DeclKind::Local)
    );
    // `y` is the *key* being read off `o`, not a binding.
    assert_eq!(role_of(src, "y", 0), IdentifierRole::MemberRef);
    assert_eq!(
        role_of(src, "z", 0),
        IdentifierRole::Declaration(DeclKind::Local)
    );
    assert_eq!(
        role_of(src, "p", 0),
        IdentifierRole::Declaration(DeclKind::Local)
    );
    assert_eq!(
        role_of(src, "rest", 0),
        IdentifierRole::Declaration(DeclKind::Local)
    );
}

#[test]
fn a_qualified_type_selects_after_its_first_segment() {
    let src = "let x: ns.Type = 1;\n";
    assert_eq!(role_of(src, "ns", 0), IdentifierRole::LexicalRef);
    assert_eq!(role_of(src, "Type", 0), IdentifierRole::MemberRef);
}

#[test]
fn labels_are_their_own_role() {
    let src = "export function f() { outer: while (true) { break outer; } }\n";
    assert_eq!(role_of(src, "outer", 0), IdentifierRole::Label);
    assert_eq!(role_of(src, "outer", 1), IdentifierRole::Label);
}

/// The whole role table, over a gallery, pinned.
#[test]
fn typescript_role_gallery() {
    let src = "\
import { Db, type Cfg } from \"./db\";

const LIMIT = 10;
type Id = string;

interface Row {
  id: Id;
  load(): void;
}

enum Color { RED = 1, GREEN }

export function load(id: Id, { trace }: Cfg): Row {
  const row = Db.find(id);
  for (const key of Object.keys(row)) {
    console.log(key, LIMIT);
  }
  try {
    row.load();
  } catch (err) {
    console.error(err);
  }
  return row;
}

export class Repo extends Base {
  private cache = new Map<string, Row>();
  #hidden = 1;

  constructor(private readonly cfg: Cfg) {
    super();
  }

  get<T>(id: Id): Row | undefined {
    return this.cache.get(id) ?? load(id, this.cfg);
  }
}

namespace NS {
  export const inner = 1;
}
";
    insta::assert_snapshot!(role_gallery(src));
}

/// Both dialects must classify identically on code that parses in both; the
/// role table is deliberately dialect-independent.
#[test]
fn the_two_dialects_agree_on_shared_syntax() {
    let src = "\
const a = 1;
export function f(o: any) {
  return o.prop + a;
}
";
    let ts = role_gallery_with(support::typescript(), src);
    let tsx = role_gallery_with(support::tsx(), src);
    assert_eq!(ts, tsx);
}

// ------------------------------------------------------------------ helpers

fn parse(src: &str) -> (SourceTree, &'static dyn Language) {
    let ts = support::typescript();
    assert_eq!(
        TypeScriptLanguage::TYPESCRIPT.dialect(),
        TsDialect::TypeScript
    );
    (sm_cst::parse(src.as_bytes(), ts).expect("parses"), ts)
}

fn role_of(src: &str, name: &str, occurrence: usize) -> IdentifierRole {
    let (tree, ts) = parse(src);
    tree.ids()
        .filter(|&id| {
            tree.node(id).is_leaf()
                && tree.node_text(id) == name
                && ts.identifier_role(&tree, id) != IdentifierRole::NotAName
        })
        .nth(occurrence)
        .map(|id| ts.identifier_role(&tree, id))
        .unwrap_or_else(|| panic!("no name node `{name}` #{occurrence} in:\n{src}"))
}

fn role_gallery(src: &str) -> String {
    role_gallery_with(support::typescript(), src)
}

fn role_gallery_with(lang: &'static dyn Language, src: &str) -> String {
    let tree = sm_cst::parse(src.as_bytes(), lang).expect("parses");
    let mut out = String::new();
    for id in tree.ids() {
        let role = lang.identifier_role(&tree, id);
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
            "{:<22} {:<28} {:<28} {}",
            format!("`{}`", tree.node_text(id)),
            label,
            parent,
            tree.field_name(id).unwrap_or("-"),
        )
        .unwrap();
    }
    out
}
