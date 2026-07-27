//! Java configuration for the [`Language`] trait.
//!
//! Every kind name in this file is checked against the grammar's node-kind
//! inventory by `tests/language_config.rs`, so a typo or a grammar upgrade that
//! renames a node fails the build rather than silently disabling a rule.

use crate::language::{ChildListKind, Language};

/// Java, via `tree-sitter-java`.
#[derive(Clone, Copy, Debug, Default)]
pub struct JavaLanguage;

impl JavaLanguage {
    /// Containers whose children may be freely reordered without changing the
    /// meaning of the program, and which the merge algorithm may therefore treat
    /// as sets (SPEC.md §4.5).
    ///
    /// # Why each one is here
    ///
    /// - `class_body`, `interface_body`, `annotation_type_body`: Java resolves
    ///   members by name, not by position, and forward references between
    ///   members are legal. Two branches adding different methods to the same
    ///   class is the single most common false conflict in real Java history.
    /// - `enum_body_declarations`: the *member* section of an enum body, i.e.
    ///   everything after the `;` that terminates the constant list. Same
    ///   argument as `class_body`. Note that this is a different node from
    ///   `enum_body`, which holds the constants and is emphatically ordered.
    ///
    /// # The caveat we are accepting
    ///
    /// Class-member order is not *entirely* meaningless: instance field
    /// initialisers and `static_initializer` blocks execute in textual order, so
    /// reordering two fields where one initialiser reads the other changes
    /// behaviour. Treating `class_body` as unordered is a deliberate, documented
    /// bet that this pattern is rare and that the merge algorithm will in
    /// practice only ever *insert* into these lists rather than permute them —
    /// a set merge of two disjoint insertions preserves the relative order of
    /// everything that was already there. M5's divergence analysis is where this
    /// bet gets tested against real merges; if it produces incorrect merges,
    /// the fix is to demote `class_body` to
    /// [`ChildListKind::PartiallyUnordered`] over the member kinds that cannot
    /// have initialiser side effects.
    pub const UNORDERED_CONTAINERS: &'static [&'static str] = &[
        "class_body",
        "interface_body",
        "enum_body_declarations",
        "annotation_type_body",
    ];

    /// Child kinds of `program` that form an order-insensitive run.
    ///
    /// A compilation unit is `package_declaration?` then imports then type
    /// declarations. Only the imports are interchangeable: the package
    /// declaration must stay first, and top-level types are not interchangeable
    /// with imports. See [`ChildListKind::PartiallyUnordered`].
    pub const PROGRAM_UNORDERED_KINDS: &'static [&'static str] = &["import_declaration"];

    /// Containers that were considered for the unordered list and deliberately
    /// left ordered.
    ///
    /// [`ChildListKind::Ordered`] is already the default, so this list changes
    /// no behaviour. It exists because "we thought about this one and the answer
    /// is no" is information that must survive, and because the test suite
    /// asserts these names still exist in the grammar — which is what makes the
    /// reasoning below checkable rather than folkloric.
    ///
    /// - `block`, `constructor_body`: statement sequences. Reordering statements
    ///   is the definition of changing a program.
    /// - `switch_block`, `switch_block_statement_group`: fallthrough makes case
    ///   order load-bearing even where the cases are disjoint.
    /// - `enum_body`: holds `enum_constant`s, whose order defines `ordinal()`
    ///   and the order of `values()`. Enum ordinals get serialised into
    ///   databases and wire formats; permuting them is a data-corruption bug.
    /// - `formal_parameters`: parameter order is the calling convention. This is
    ///   the trap the brief singles out — a formal parameter list *looks* like a
    ///   set of declarations and is nothing of the sort.
    /// - `argument_list`: positional arguments, same reason.
    /// - `type_parameters`, `type_arguments`: positional.
    /// - `array_initializer`: element position is the array index.
    /// - `type_list`: the `implements` list; order decides which interface's
    ///   default method wins in some resolution cases.
    /// - `resource_specification`: try-with-resources closes in reverse
    ///   declaration order.
    /// - `annotation_argument_list`: named element-value pairs are formally
    ///   order-insensitive, but the single-element form `@Anno(x)` is
    ///   positional and the two share a node kind. Conservative wins.
    /// - `modifiers`: reordering is harmless but pointless, and leaving it
    ///   ordered costs nothing since modifier lists are rarely edited from both
    ///   sides.
    /// - `for_statement`, `enhanced_for_statement`, `if_statement`,
    ///   `while_statement`, `do_statement`, `try_statement`,
    ///   `labeled_statement`, `synchronized_statement`: fixed-shape
    ///   statement-bearing constructs, where a child's position *is* its role
    ///   (init / condition / update / body).
    pub const ORDERED_CONTAINERS: &'static [&'static str] = &[
        "block",
        "constructor_body",
        "switch_block",
        "switch_block_statement_group",
        "enum_body",
        "formal_parameters",
        "argument_list",
        "type_parameters",
        "type_arguments",
        "array_initializer",
        "type_list",
        "resource_specification",
        "annotation_argument_list",
        "modifiers",
        "for_statement",
        "enhanced_for_statement",
        "if_statement",
        "while_statement",
        "do_statement",
        "try_statement",
        "labeled_statement",
        "synchronized_statement",
    ];

    /// Kinds that introduce a lexical scope, for the M6 name binder.
    ///
    /// `program` is included because a compilation unit scopes imports and
    /// top-level types. `block` covers method bodies and bare blocks alike.
    /// `for_statement` and `enhanced_for_statement` scope their loop variable
    /// even when the body is a single statement rather than a block, and
    /// `catch_clause` scopes its exception parameter the same way.
    pub const SCOPE_INTRODUCING: &'static [&'static str] = &[
        "program",
        "class_declaration",
        "interface_declaration",
        "enum_declaration",
        "method_declaration",
        "constructor_declaration",
        "block",
        "for_statement",
        "enhanced_for_statement",
        "lambda_expression",
        "catch_clause",
    ];

    /// Identifier kinds. `tree-sitter-java` splits names into `identifier` for
    /// value-position names and `type_identifier` for type-position ones.
    pub const IDENTIFIERS: &'static [&'static str] = &["identifier", "type_identifier"];

    /// Comment kinds. `tree-sitter-java` distinguishes `line_comment` from
    /// `block_comment` (there is no generic `comment` node); Javadoc is a
    /// `block_comment` whose text happens to start with `/**`.
    pub const COMMENTS: &'static [&'static str] = &["line_comment", "block_comment"];

    /// Kinds whose source text carries meaning beyond the kind itself.
    ///
    /// Identifiers and literals qualify: two `identifier` nodes reading `foo`
    /// and `bar` denote different things. Keywords and punctuation do not —
    /// their text is a function of their kind, so hashing it is wasted work.
    /// `true`, `false` and `null_literal` are deliberately absent for that
    /// reason: they are distinct *kinds*, so the kind comparison already
    /// separates them.
    pub const SIGNIFICANT_TEXT: &'static [&'static str] = &[
        "identifier",
        "type_identifier",
        "decimal_integer_literal",
        "hex_integer_literal",
        "octal_integer_literal",
        "binary_integer_literal",
        "decimal_floating_point_literal",
        "hex_floating_point_literal",
        "character_literal",
        "string_literal",
        "string_fragment",
        "multiline_string_fragment",
        "escape_sequence",
    ];
}

impl Language for JavaLanguage {
    fn ts_language(&self) -> tree_sitter::Language {
        tree_sitter_java::LANGUAGE.into()
    }

    fn name(&self) -> &'static str {
        "java"
    }

    fn file_extensions(&self) -> &'static [&'static str] {
        &["java"]
    }

    fn child_list_kind(&self, container_kind: &str) -> ChildListKind {
        if container_kind == "program" {
            ChildListKind::PartiallyUnordered {
                unordered_kinds: Self::PROGRAM_UNORDERED_KINDS,
            }
        } else if Self::UNORDERED_CONTAINERS.contains(&container_kind) {
            ChildListKind::Unordered
        } else {
            // Everything else, listed in ORDERED_CONTAINERS or not, is ordered.
            // An unknown kind arriving here — a construct from a newer Java
            // version, say — gets the conservative answer by construction.
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
    }
}
