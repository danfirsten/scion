//! TypeScript configuration for the [`Language`] trait.
//!
//! Pulled forward from M5 to prove the [`Language`] abstraction actually holds
//! (SPEC.md §3). Every kind name in this file is checked against the relevant
//! grammar's node-kind inventory by `tests/ts_language_config.rs`, so a typo or a
//! grammar upgrade that renames a node fails the build rather than silently
//! disabling a rule.
//!
//! # Two grammars, one configuration
//!
//! `tree-sitter-typescript` ships **two** grammars, `LANGUAGE_TYPESCRIPT` and
//! `LANGUAGE_TSX`. They are not interchangeable: `.tsx` needs the TSX grammar to
//! read `<Foo />` as an element, and `.ts` needs the TypeScript grammar to read
//! `<Foo>x` as a type assertion. The two node inventories differ accordingly —
//! TSX adds `jsx_element` and friends, TypeScript adds `type_assertion` — so a
//! single flat list of configured kind names could not be inventory-checked
//! against both.
//!
//! The answer here is **one parameterised type, two registry entries**
//! ([`TypeScriptLanguage::TYPESCRIPT`] and [`TypeScriptLanguage::TSX`]) rather
//! than two unrelated structs. Every classification below is shared, because the
//! dialects differ only in surface syntax and not at all in what order means;
//! [`TypeScriptLanguage::TSX_ONLY_ORDERED_CONTAINERS`] is the one list that is
//! dialect-specific, and it is documentation rather than behaviour (Ordered is
//! already the default).
//!
//! `.d.ts` needs no special handling: `Path::extension` on `foo.d.ts` is `ts`,
//! and a declaration file is ordinary TypeScript as far as every classification
//! here is concerned.

use crate::arena::{NodeId, SourceTree};
use crate::language::{ChildListKind, DeclKind, IdentifierRole, Language, MemberVisibility};

/// Which of `tree-sitter-typescript`'s two grammars a [`TypeScriptLanguage`]
/// parses with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TsDialect {
    /// `.ts`, `.mts`, `.cts` (and `.d.ts`, which is just a `.ts`). Angle-bracket
    /// type assertions parse; JSX does not.
    TypeScript,
    /// `.tsx`. JSX parses; angle-bracket type assertions do not.
    Tsx,
}

/// TypeScript, via `tree-sitter-typescript`.
///
/// See the module documentation for why this is parameterised over
/// [`TsDialect`] rather than split into two types.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TypeScriptLanguage {
    dialect: TsDialect,
}

impl TypeScriptLanguage {
    /// The `.ts` / `.mts` / `.cts` dialect.
    pub const TYPESCRIPT: Self = Self {
        dialect: TsDialect::TypeScript,
    };

    /// The `.tsx` dialect.
    pub const TSX: Self = Self {
        dialect: TsDialect::Tsx,
    };

    /// Which grammar this instance uses.
    #[must_use]
    pub const fn dialect(&self) -> TsDialect {
        self.dialect
    }

    /// Containers whose children may be freely reordered without changing the
    /// meaning of the program, and which the merge algorithm may therefore treat
    /// as sets (SPEC.md §4.5).
    ///
    /// # Why each one is here
    ///
    /// - `named_imports` — the `{ a, b, c }` of `import { a, b, c } from "m"`.
    ///   This is a pure list of bindings introduced into the current module's
    ///   scope. It has no evaluation order of its own (the *module* is evaluated
    ///   once, by the enclosing `import_statement`), duplicates are a
    ///   compile-time error rather than a last-one-wins, and nothing observable
    ///   depends on the order. It is also, empirically, where the most common
    ///   TypeScript import conflict lives: two branches each pulling one more
    ///   name out of the same module.
    /// - `export_clause` — the `{ a, b }` of `export { a, b }`. Same argument:
    ///   a set of re-exported bindings with no order semantics.
    /// - `class_body`, `interface_body`, `object_type` — members resolve by
    ///   name, forward references between them are legal, and disjoint member
    ///   additions from two branches are the most common false conflict in real
    ///   history. `object_type` is the type-literal form `{ a: string; b: number }`
    ///   and shares `interface_body`'s child inventory exactly.
    ///
    /// # The caveat we are accepting on `class_body`
    ///
    /// This mirrors Java's decision 10 (see `JavaLanguage::UNORDERED_CONTAINERS`)
    /// and inherits exactly the same caveat: `public_field_definition`
    /// initialisers and `class_static_block`s execute in textual order, so
    /// permuting two members where one initialiser reads the other changes
    /// behaviour. The bet is the same one — that the merge algorithm only ever
    /// *inserts* into these lists, and a set merge of disjoint insertions
    /// preserves the relative order of everything already there. TypeScript adds
    /// one wrinkle Java does not have: with `useDefineForClassFields`, a field
    /// declared *without* an initialiser still emits a `defineProperty` at
    /// construction time, so even initialiser-free fields have an ordered
    /// effect. It is still unobservable unless two members interact, so the bet
    /// stands; the fix, if M5's divergence analysis finds it costing merges, is
    /// to demote `class_body` to [`ChildListKind::PartiallyUnordered`].
    ///
    /// # The caveat on the type bodies
    ///
    /// `interface_body` and `object_type` have no runtime existence at all, so
    /// the initialiser argument does not apply to them — but there is a second,
    /// TypeScript-specific one that does: **overload signatures resolve in
    /// declaration order.**
    ///
    /// ```ts
    /// interface I {
    ///   f(x: unknown): void;
    ///   f(x: string): number;   // unreachable if the two are swapped
    /// }
    /// ```
    ///
    /// The compiler picks the first signature the call site is assignable to, so
    /// permuting two overloads of the same name can change which one is chosen
    /// and therefore the inferred return type. `class_body` inherits this on top
    /// of its initialiser caveat, via `method_signature`.
    ///
    /// This is accepted on the same bet as the initialiser case — insertions,
    /// not permutations — and it is narrower, since it only bites for two
    /// members that *share a name*, which a set merge keyed on node matching
    /// will normally see as one element rather than two. It is recorded here
    /// because it is the kind of thing that is obvious only once someone has
    /// written it down.
    ///
    /// Neither `named_imports` nor `export_clause` carries any caveat: those two
    /// are the unqualified wins in this list.
    pub const UNORDERED_CONTAINERS: &'static [&'static str] = &[
        "named_imports",
        "export_clause",
        "class_body",
        "interface_body",
        "object_type",
    ];

    /// Containers that were considered for the unordered list and deliberately
    /// left ordered.
    ///
    /// [`ChildListKind::Ordered`] is already the default, so this list changes no
    /// behaviour. It exists because "we thought about this one and the answer is
    /// no" is information that must survive, and because the test suite asserts
    /// these names still exist in the grammar — which is what makes the reasoning
    /// below checkable rather than folkloric.
    ///
    /// - `program` — **the headline difference from Java.** A Java `program` is
    ///   `PartiallyUnordered` over `import_declaration`, because a Java import is
    ///   a purely lexical alias with no runtime effect whatsoever. An ES
    ///   `import_statement` is nothing of the sort: it is an edge in the module
    ///   graph, and modules are *evaluated* in the order their imports appear.
    ///   `import "./polyfill"` exists solely for that effect, and even a
    ///   perfectly ordinary `import { a } from "./a"` will run `./a`'s top-level
    ///   code before the next import's. Reordering imports can therefore change
    ///   what the program does. See "Known limitations" in PROGRESS.md: the
    ///   optimisation we would like — commute *only* imports that have an
    ///   `import_clause`, i.e. that are not side-effect-only — is not expressible
    ///   in a kind-keyed API, and the conservative answer is the one that cannot
    ///   produce a wrong merge.
    /// - `statement_block` — statement sequences. Reordering statements is the
    ///   definition of changing a program.
    /// - `switch_body`, `switch_case`, `switch_default` — fallthrough makes case
    ///   order load-bearing even where the cases are disjoint.
    /// - `enum_body` — a numeric TypeScript enum auto-increments from the
    ///   previous member, so inserting or permuting members silently renumbers
    ///   every member after the edit. Those numbers get persisted to databases
    ///   and wire formats. This is the same "looks like a set, is not" trap as
    ///   Java's `enum_body` (decision 11), and it is worse here, because in Java
    ///   only `ordinal()` changes while in TypeScript the member's *own value*
    ///   changes.
    /// - `object` — the object literal. Later keys override earlier ones
    ///   (`{ a: 1, a: 2 }` is `{ a: 2 }`), `spread_element` children interleave
    ///   with explicit keys and the last write wins, and enumeration order is
    ///   observable via `Object.keys`. A set merge here could change a value.
    /// - `object_pattern` — `const { a, b } = o` looks order-insensitive and
    ///   mostly is, but getters on `o` run in destructuring order and
    ///   `rest_pattern` must come last. Conservative.
    /// - `array`, `array_pattern` — element position is the array index.
    /// - `arguments`, `formal_parameters` — positional. This is the trap Java's
    ///   decision 11 singles out: a parameter list *looks* like a set of
    ///   declarations and is nothing of the sort.
    /// - `type_parameters`, `type_arguments`, `tuple_type` — positional.
    /// - `union_type`, `intersection_type` — set-like in the type algebra
    ///   (`A | B` and `B | A` denote the same type) but *not* interchangeable to
    ///   the compiler: overload resolution walks a union in order, excess
    ///   property checking and the "first assignable constituent" rule are
    ///   order-dependent, and error messages name the first constituent.
    ///   Conservative.
    /// - `variable_declaration`, `lexical_declaration` — `const a = 1, b = a;`
    ///   is a sequence; the declarators initialise left to right.
    /// - `sequence_expression` — the comma operator; evaluation order is the
    ///   whole point.
    /// - `class_heritage`, `implements_clause`, `extends_type_clause` — an
    ///   interface's `extends` list order decides which base member wins in some
    ///   declaration-merging cases.
    /// - `import_statement`, `import_clause`, `export_statement` — fixed-shape
    ///   constructs whose children have positional roles, and see `program`
    ///   above for why an import is not a free-floating declaration.
    /// - `template_string` — the literal's own text, in order.
    /// - `for_statement`, `for_in_statement`, `if_statement`, `while_statement`,
    ///   `do_statement`, `try_statement`, `labeled_statement`,
    ///   `with_statement` — fixed-shape statement-bearing constructs, where a
    ///   child's position *is* its role (initialiser / condition / update /
    ///   body).
    /// - `class_static_block`, `ambient_declaration`, `module`,
    ///   `internal_module` — bodies, i.e. statement sequences once unwrapped.
    pub const ORDERED_CONTAINERS: &'static [&'static str] = &[
        "program",
        "statement_block",
        "switch_body",
        "switch_case",
        "switch_default",
        "enum_body",
        "object",
        "object_pattern",
        "array",
        "array_pattern",
        "arguments",
        "formal_parameters",
        "type_parameters",
        "type_arguments",
        "tuple_type",
        "union_type",
        "intersection_type",
        "variable_declaration",
        "lexical_declaration",
        "sequence_expression",
        "class_heritage",
        "implements_clause",
        "extends_type_clause",
        "import_statement",
        "import_clause",
        "export_statement",
        "template_string",
        "for_statement",
        "for_in_statement",
        "if_statement",
        "while_statement",
        "do_statement",
        "try_statement",
        "labeled_statement",
        "with_statement",
        "class_static_block",
        "ambient_declaration",
        "module",
        "internal_module",
    ];

    /// Ordered containers that only the TSX grammar has.
    ///
    /// Split out because the two grammars have different node inventories, and
    /// the inventory guard would (correctly) reject `jsx_element` when checked
    /// against the plain TypeScript grammar.
    ///
    /// - `jsx_element` — children are rendered in order; JSX children are an
    ///   array, and React uses position as the reconciliation key when no `key`
    ///   prop is given, so permuting them remounts components.
    /// - `jsx_opening_element`, `jsx_self_closing_element` — attribute lists.
    ///   These read like unordered property bags and are not: a later duplicate
    ///   attribute wins, and `{...props}` spreads interleave with explicit
    ///   attributes exactly as in an object literal.
    /// - `jsx_expression` — the `{ … }` escape; a fixed-shape wrapper.
    pub const TSX_ONLY_ORDERED_CONTAINERS: &'static [&'static str] = &[
        "jsx_element",
        "jsx_opening_element",
        "jsx_self_closing_element",
        "jsx_expression",
    ];

    /// Kinds that introduce a lexical scope, for the M6 name binder.
    ///
    /// A first draft, as Java's is. Grouped by why:
    ///
    /// - **Module and block scopes.** `program` is the module scope (a file with
    ///   a top-level `import`/`export` is a module, not a script).
    ///   `statement_block` covers function bodies and bare blocks alike.
    ///   `switch_body` is a scope in its own right — a `let` in one case is
    ///   visible, and in its temporal dead zone, in the others, which is a real
    ///   and frequently-hit JavaScript rule.
    /// - **Function scopes.** Every callable form binds parameters and type
    ///   parameters: `function_declaration`, `function_expression`,
    ///   `generator_function`, `generator_function_declaration`, `arrow_function`
    ///   (which notably does *not* bind `this`, so M6 must not treat it as a
    ///   `this` scope), `method_definition`, and `class_static_block`.
    /// - **Signature-only forms.** `function_signature`, `method_signature`,
    ///   `abstract_method_signature`, `call_signature`, `construct_signature`
    ///   and `index_signature` have no body but still bind parameter names and
    ///   type parameters, and those names are visible in the return type
    ///   (`function f<T>(x: T): T`).
    /// - **Type-and-value declaration scopes.** `class_declaration`,
    ///   `abstract_class_declaration` and `class` (the expression form) scope
    ///   their type parameters and their own name; `interface_declaration` and
    ///   `type_alias_declaration` scope type parameters; `enum_declaration`
    ///   scopes its members, which may reference each other unqualified.
    /// - **Namespaces.** `module` (`declare module "x" { … }`) and
    ///   `internal_module` (`namespace X { … }`).
    /// - **Binding statements.** `for_statement` and `for_in_statement` scope a
    ///   `let`/`const` loop variable even when the body is a single statement
    ///   rather than a block — and `for (let i …)` famously creates a *fresh*
    ///   binding per iteration. `catch_clause` scopes its exception parameter the
    ///   same way.
    /// - **Type-level binding.** `mapped_type_clause` binds a type variable in
    ///   `{ [K in keyof T]: … }`. TypeScript has type-level scopes that Java's
    ///   generics never introduce mid-expression; this is the smallest example.
    pub const SCOPE_INTRODUCING: &'static [&'static str] = &[
        "program",
        "statement_block",
        "switch_body",
        "function_declaration",
        "function_expression",
        "generator_function",
        "generator_function_declaration",
        "arrow_function",
        "method_definition",
        "class_static_block",
        "function_signature",
        "method_signature",
        "abstract_method_signature",
        "call_signature",
        "construct_signature",
        "index_signature",
        "class_declaration",
        "abstract_class_declaration",
        "class",
        "interface_declaration",
        "type_alias_declaration",
        "enum_declaration",
        "module",
        "internal_module",
        "for_statement",
        "for_in_statement",
        "catch_clause",
        "mapped_type_clause",
    ];

    /// Identifier kinds — names that either declare or reference something a
    /// *lexical* scope can resolve.
    ///
    /// Java needed exactly two (`identifier`, `type_identifier`) because
    /// `tree-sitter-java` uses `identifier` for field and method names in a
    /// member access too. TypeScript splits that hair, and the split matters:
    ///
    /// - `identifier` — value-position names.
    /// - `type_identifier` — type-position names.
    /// - `shorthand_property_identifier` — the `a` of the object literal
    ///   `{ a }`. It is simultaneously the key and a *reference to the lexical
    ///   variable* `a`, so it belongs here.
    /// - `shorthand_property_identifier_pattern` — the `a` of the destructuring
    ///   `const { a } = o`. It *introduces* a lexical binding.
    /// - `statement_identifier` — labels (`outer: for (…) { break outer; }`).
    ///   A separate namespace from variables, but a lexical one.
    ///
    /// # What is deliberately absent, and why it is a gap
    ///
    /// `property_identifier` (`o.foo`, a class member name, an object-literal
    /// key) and `private_property_identifier` (`#x`) are **not** here. They are
    /// names, and M6 will very much want to reason about them — a renamed method
    /// is exactly the semantic-conflict case SPEC.md §7 exists to catch — but
    /// they do not resolve against a lexical scope. They resolve against the
    /// *type* of the receiver expression, which needs a type checker rather than
    /// a scope tree. Declaring them identifiers would have `sm-bind` walk out to
    /// the module scope, find nothing, and report a semantic conflict on every
    /// property access in the file.
    ///
    /// `is_identifier` is a single boolean, so it cannot say "this is a name but
    /// not a lexically-resolved one". **M6 closed that gap**: every name node is
    /// now classified by [`Language::identifier_role`], which reports these
    /// kinds as [`IdentifierRole::MemberRef`] — a name `sm-bind` never resolves
    /// and therefore never reports — while still recognising them as
    /// [`IdentifierRole::Declaration`] when they are a class member's own name.
    /// This list is what `is_identifier` still answers, and it is unchanged.
    ///
    /// Note that the excluded kinds are still in [`Self::SIGNIFICANT_TEXT`]:
    /// the matcher absolutely must tell `.foo` from `.bar`.
    pub const IDENTIFIERS: &'static [&'static str] = &[
        "identifier",
        "type_identifier",
        "shorthand_property_identifier",
        "shorthand_property_identifier_pattern",
        "statement_identifier",
    ];

    /// Comment kinds.
    ///
    /// Where `tree-sitter-java` splits comments into `line_comment` and
    /// `block_comment`, `tree-sitter-typescript` has a single `comment` node
    /// covering both `//` and `/* */` — JSDoc is a `comment` whose text happens
    /// to start with `/**`. It also has `html_comment`, the legacy
    /// `<!-- … -->` / `--> …` form that Annex B still requires JavaScript hosts
    /// to accept; it is an `extra` like `comment`, it is vanishingly rare in
    /// TypeScript, and leaving it out would mean the trivia pass treated it as
    /// code.
    ///
    /// This is a small but real proof that the abstraction holds: nothing in
    /// `trivia.rs` or `render.rs` knows how many comment kinds a language has.
    pub const COMMENTS: &'static [&'static str] = &["comment", "html_comment"];

    /// Kinds whose source text carries meaning beyond the kind itself.
    ///
    /// Identifiers and literals qualify: two `identifier` nodes reading `foo` and
    /// `bar` denote different things. Keywords and punctuation do not — their
    /// text is a function of their kind, so hashing it is wasted work. `true`,
    /// `false`, `null`, `undefined`, `this` and `super` are deliberately absent
    /// for that reason: each is a distinct *kind*, so the kind comparison already
    /// separates them.
    ///
    /// Three entries are worth calling out because Java has no analogue:
    ///
    /// - `predefined_type` — `string`, `number`, `boolean`, `any`, `unknown`,
    ///   `never`, `void`, `symbol`, `object`. One node kind, nine meanings; the
    ///   text is the only thing that distinguishes them. Java's primitive types
    ///   are separate kinds (`integral_type`, `floating_point_type`, …), so this
    ///   trap does not exist there.
    /// - `accessibility_modifier` — `public` / `private` / `protected` as a
    ///   *named* node rather than three anonymous keywords, which is how
    ///   `tree-sitter-typescript` models it. Two nodes of this kind with
    ///   different text mean different visibility.
    /// - `hash_bang_line` — `#!/usr/bin/env node`. Its text is the interpreter.
    ///
    /// The property-name kinds are here even though they are not in
    /// [`Self::IDENTIFIERS`]: whether a name resolves lexically is a different
    /// question from whether its bytes matter to the structural hash.
    pub const SIGNIFICANT_TEXT: &'static [&'static str] = &[
        "identifier",
        "type_identifier",
        "property_identifier",
        "private_property_identifier",
        "shorthand_property_identifier",
        "shorthand_property_identifier_pattern",
        "statement_identifier",
        "predefined_type",
        "accessibility_modifier",
        "hash_bang_line",
        "number",
        "string_fragment",
        "escape_sequence",
        "regex_pattern",
        "regex_flags",
    ];

    /// Significant-text kinds that only the TSX grammar has.
    ///
    /// `jsx_text` is the literal text between JSX tags. It is not trivia and it
    /// is not whitespace-insignificant — it is rendered.
    pub const TSX_ONLY_SIGNIFICANT_TEXT: &'static [&'static str] = &["jsx_text"];

    // ------------------------------------------------------------- M6: names

    /// Declaration node kinds, and what each declares (M6).
    ///
    /// Keyed on the *declaration* node. The name is [`Language::declared_name`],
    /// which is the grammar's `name` field for all of these except the ones
    /// listed in [`TypeScriptLanguage::declared_name`].
    ///
    /// # What is deliberately absent
    ///
    /// - `class`, `function_expression`, `generator_function` — the *expression*
    ///   forms. `const f = function named() { … }` binds `named` only inside the
    ///   function's own body, and this crate's scope model puts a self-named
    ///   declaration in the *enclosing* scope. Registering them would leak the
    ///   name outward, which could make an unrelated later `named` resolve. Not
    ///   registering them makes such a name unresolved, which is never reported.
    ///   Under-approximate, on purpose.
    /// - `variable_declarator` — its answer depends on its parent
    ///   (`lexical_declaration` ⇒ block-scoped [`DeclKind::Local`],
    ///   `variable_declaration` ⇒ function-hoisted [`DeclKind::Var`]), so it is
    ///   handled in [`TypeScriptLanguage::declaration_kind`].
    /// - Destructuring leaves (`shorthand_property_identifier_pattern`,
    ///   `rest_pattern`, bare identifiers under `array_pattern`) declare
    ///   themselves and are handled the same way.
    pub const DECLARATIONS: &'static [(&'static str, DeclKind)] = &[
        ("class_declaration", DeclKind::Class),
        ("abstract_class_declaration", DeclKind::Class),
        ("interface_declaration", DeclKind::Interface),
        ("type_alias_declaration", DeclKind::TypeAlias),
        ("enum_declaration", DeclKind::Enum),
        ("enum_assignment", DeclKind::EnumMember),
        ("function_declaration", DeclKind::Function),
        ("generator_function_declaration", DeclKind::Function),
        ("method_definition", DeclKind::Method),
        ("method_signature", DeclKind::Method),
        ("abstract_method_signature", DeclKind::Method),
        ("public_field_definition", DeclKind::Field),
        ("property_signature", DeclKind::Property),
        ("required_parameter", DeclKind::Parameter),
        ("optional_parameter", DeclKind::Parameter),
        ("catch_clause", DeclKind::Parameter),
        ("type_parameter", DeclKind::TypeParam),
        ("import_specifier", DeclKind::Import),
        ("namespace_import", DeclKind::Import),
        ("internal_module", DeclKind::Namespace),
        ("module", DeclKind::Module),
        ("pair_pattern", DeclKind::Local),
        ("rest_pattern", DeclKind::Local),
        ("shorthand_property_identifier_pattern", DeclKind::Local),
        ("for_in_statement", DeclKind::Local),
    ];

    /// `(parent kind, field name)` pairs whose identifier child is a
    /// **member reference** — resolved against a receiver's type or a
    /// namespace, never against the lexical scope chain.
    ///
    /// TypeScript needs fewer entries than Java because the grammar already
    /// separates `property_identifier` from `identifier`; those two kinds are
    /// handled wholesale by kind. What is left is the
    /// qualified-name shapes:
    ///
    /// - `nested_type_identifier.name` — the `Type` of `ns.Type`.
    /// - `nested_identifier.property` — the tail of a qualified value name.
    /// - `import_specifier.name` — the *exported* name in
    ///   `import { a as b }`, which lives in the other module, not here. (When
    ///   there is no alias the same node **is** the local binding and is caught
    ///   as a declaration first.)
    pub const MEMBER_REF_CONTEXTS: &'static [(&'static str, &'static str)] = &[
        ("nested_type_identifier", "name"),
        ("nested_identifier", "property"),
        ("import_specifier", "name"),
    ];

    /// Kinds that are a **function-level** scope, i.e. where a `var` lands.
    ///
    /// `statement_block` is emphatically not here: that is the whole point of
    /// `var`. `program` is, so that a top-level `var` is module-wide.
    pub const FUNCTION_SCOPES: &'static [&'static str] = &[
        "program",
        "function_declaration",
        "generator_function_declaration",
        "function_expression",
        "generator_function",
        "arrow_function",
        "method_definition",
        "class_static_block",
        "module",
        "internal_module",
    ];

    /// Name kinds that resolve against a **receiver's type**, never against the
    /// lexical scope chain.
    ///
    /// This is the finding that motivated [`IdentifierRole`]: `o.foo`, a class
    /// member name and an object-literal key are all `property_identifier`, and
    /// `#x` is `private_property_identifier`. Treating them as lexical
    /// references would report a broken reference on every property access in
    /// the file. They are still names — when one of them is a *declaration's*
    /// name it is reported as [`IdentifierRole::Declaration`], which is what
    /// lets a renamed method be described in a conflict explanation.
    pub const RECEIVER_TYPED_NAMES: &'static [&'static str] =
        &["property_identifier", "private_property_identifier"];

    /// Parent kinds under which a bare `identifier` **is itself** a
    /// declaration, with no wrapper node and no field name to key on.
    ///
    /// - `array_pattern` — the `p` of `const [p, ...rest] = arr`.
    /// - `import_clause` — the `def` of `import def from "m"`.
    pub const SELF_DECLARING_PARENTS: &'static [(&'static str, DeclKind)] = &[
        ("array_pattern", DeclKind::Local),
        ("import_clause", DeclKind::Import),
    ];

    /// What a bare `identifier` declares purely by virtue of where it sits, or
    /// `None` if it declares nothing. See
    /// [`TypeScriptLanguage::SELF_DECLARING_PARENTS`].
    fn self_declared(tree: &SourceTree, id: NodeId) -> Option<DeclKind> {
        let node = tree.node(id);
        match node.kind {
            "shorthand_property_identifier_pattern" => Some(DeclKind::Local),
            "identifier" => {
                let parent_kind = tree.node(node.parent?).kind;
                Self::SELF_DECLARING_PARENTS
                    .iter()
                    .find(|(k, _)| *k == parent_kind)
                    .map(|(_, d)| *d)
            }
            _ => None,
        }
    }

    /// The role of one name node. Split out of the trait impl so the reasoning
    /// can be read top to bottom.
    fn role(&self, tree: &SourceTree, id: NodeId) -> IdentifierRole {
        let node = tree.node(id);
        let kind = node.kind;
        let is_name =
            Self::IDENTIFIERS.contains(&kind) || Self::RECEIVER_TYPED_NAMES.contains(&kind);
        if !is_name {
            return IdentifierRole::NotAName;
        }
        let Some(parent) = node.parent else {
            return IdentifierRole::MemberRef;
        };
        let parent_kind = tree.node(parent).kind;
        let field = tree.field_name(id);

        // 1. The name of a declaration — asked of the declaration itself, so
        //    the two APIs cannot drift apart. A destructuring leaf and a
        //    default import declare *themselves*, hence the `id` probe as well
        //    as the `parent` one.
        for candidate in [parent, id] {
            if let Some(decl) = self.declaration_kind(tree, candidate)
                && self.declared_name(tree, candidate) == Some(id)
            {
                return IdentifierRole::Declaration(decl);
            }
        }

        // 2. Labels: `outer: for (…) { break outer; }`.
        if kind == "statement_identifier" {
            return IdentifierRole::Label;
        }

        // 3. Receiver-typed names, in bulk and by context.
        if Self::RECEIVER_TYPED_NAMES.contains(&kind) {
            return IdentifierRole::MemberRef;
        }
        if let Some(field) = field
            && Self::MEMBER_REF_CONTEXTS.contains(&(parent_kind, field))
        {
            return IdentifierRole::MemberRef;
        }

        // 5. Everything else. TypeScript has a single value namespace, so a
        //    call target is an ordinary `LexicalRef` and `CallRef` is never
        //    produced here.
        if kind == "type_identifier" {
            IdentifierRole::TypeRef
        } else {
            IdentifierRole::LexicalRef
        }
    }
}

impl Language for TypeScriptLanguage {
    fn ts_language(&self) -> tree_sitter::Language {
        match self.dialect {
            TsDialect::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            TsDialect::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }

    fn name(&self) -> &'static str {
        match self.dialect {
            TsDialect::TypeScript => "typescript",
            TsDialect::Tsx => "tsx",
        }
    }

    fn file_extensions(&self) -> &'static [&'static str] {
        match self.dialect {
            // `mts` and `cts` are the ESM and CommonJS variants Node introduced;
            // they are plain TypeScript to everything in this crate. `d.ts` needs
            // no entry — its extension *is* `ts`.
            TsDialect::TypeScript => &["ts", "mts", "cts"],
            TsDialect::Tsx => &["tsx"],
        }
    }

    fn child_list_kind(&self, container_kind: &str) -> ChildListKind {
        if Self::UNORDERED_CONTAINERS.contains(&container_kind) {
            ChildListKind::Unordered
        } else {
            // Everything else, listed in ORDERED_CONTAINERS or not, is ordered.
            // Note the absence of a `program` arm: unlike Java, a TypeScript
            // compilation unit has no order-insensitive run. See
            // `ORDERED_CONTAINERS`.
            ChildListKind::Ordered
        }
    }

    fn is_scope_introducing(&self, kind: &str) -> bool {
        Self::SCOPE_INTRODUCING.contains(&kind)
    }

    fn is_identifier(&self, kind: &str) -> bool {
        Self::IDENTIFIERS.contains(&kind)
    }

    fn is_comment(&self, kind: &str) -> bool {
        Self::COMMENTS.contains(&kind)
    }

    fn significant_text(&self, kind: &str) -> bool {
        Self::SIGNIFICANT_TEXT.contains(&kind)
            || (self.dialect == TsDialect::Tsx && Self::TSX_ONLY_SIGNIFICANT_TEXT.contains(&kind))
    }

    fn identifier_role(&self, tree: &SourceTree, id: NodeId) -> IdentifierRole {
        self.role(tree, id)
    }

    fn declaration_kind(&self, tree: &SourceTree, id: NodeId) -> Option<DeclKind> {
        if let Some(kind) = Self::self_declared(tree, id) {
            return Some(kind);
        }
        let kind = tree.node(id).kind;
        match kind {
            // `let`/`const` are block-scoped; `var` is function-hoisted.
            "variable_declarator" => {
                let parent_kind = tree.node(id).parent.map_or("", |p| tree.node(p).kind);
                Some(if parent_kind == "variable_declaration" {
                    DeclKind::Var
                } else {
                    DeclKind::Local
                })
            }
            // `for (const x of xs)` binds; `for (x of xs)` assigns. The `kind`
            // field is what tells them apart.
            "for_in_statement" => tree
                .child_by_field_name(id, "kind")
                .map(|_| DeclKind::Local),
            _ => Self::DECLARATIONS
                .iter()
                .find(|(k, _)| *k == kind)
                .map(|(_, d)| *d),
        }
    }

    fn declared_name(&self, tree: &SourceTree, id: NodeId) -> Option<NodeId> {
        if Self::self_declared(tree, id).is_some() {
            return Some(id);
        }
        match tree.node(id).kind {
            // `import { a as b }` binds `b`; `import { a }` binds `a`.
            "import_specifier" => tree
                .child_by_field_name(id, "alias")
                .or_else(|| tree.child_by_field_name(id, "name")),
            // `* as ns` and `...rest` leave their binding unlabelled.
            "namespace_import" | "rest_pattern" => tree
                .children(id)
                .find(|&c| tree.node(c).kind == "identifier"),
            // `catch (e)`, `const { c: renamed } = o`, `for (const x of xs)`.
            "catch_clause" => tree.child_by_field_name(id, "parameter"),
            "pair_pattern" => tree.child_by_field_name(id, "value"),
            "for_in_statement" => tree.child_by_field_name(id, "left"),
            // A parameter's "name" is its `pattern`, which may itself be a
            // destructuring pattern — in which case this returns a non-name
            // node and `sm-bind` skips it, leaving the pattern's own leaves to
            // register themselves.
            "required_parameter" | "optional_parameter" => tree.child_by_field_name(id, "pattern"),
            _ => tree.child_by_field_name(id, "name"),
        }
    }

    /// A TypeScript class member is reachable only through a receiver —
    /// `this.x`, never a bare `x`. Saying otherwise would make every method
    /// parameter that shares a name with a field look like a capture.
    fn member_visibility(&self) -> MemberVisibility {
        MemberVisibility::ReceiverOnly
    }

    fn is_function_scope(&self, kind: &str) -> bool {
        Self::FUNCTION_SCOPES.contains(&kind)
    }
}
