//! Java configuration for the [`Language`] trait.
//!
//! Every kind name in this file is checked against the grammar's node-kind
//! inventory by `tests/language_config.rs`, so a typo or a grammar upgrade that
//! renames a node fails the build rather than silently disabling a rule.

use crate::arena::{NodeId, SourceTree};
use crate::language::{ChildListKind, DeclKind, IdentifierRole, Language, MemberVisibility};

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
    ///
    /// The last five entries were added in M6, which is the first milestone
    /// that actually resolves names against this list (PROGRESS.md recorded the
    /// original as "a first draft"):
    ///
    /// - `record_declaration` scopes its type parameters and its components,
    ///   which are otherwise visible throughout the enclosing class.
    /// - `annotation_type_declaration` scopes its elements.
    /// - `compact_constructor_declaration` is a declaration with a body and
    ///   therefore a scope, exactly like `constructor_declaration`.
    /// - `switch_block` is one scope shared by every case: a local declared in
    ///   one arm of an old-style `switch` is in scope in the others. Without
    ///   it, such a local would leak into the enclosing block.
    /// - `try_with_resources_statement` scopes its resources, which are not
    ///   visible after the statement.
    ///
    /// Nothing outside `sm-bind` reads this, so widening it changes no merge
    /// decision.
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
        "record_declaration",
        "annotation_type_declaration",
        "compact_constructor_declaration",
        "switch_block",
        "try_with_resources_statement",
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

    // ------------------------------------------------------------- M6: names

    /// Declaration node kinds, and what each declares (M6).
    ///
    /// Keyed on the *declaration* node, never on its name; the name is
    /// [`Language::declared_name`], which for all of these except
    /// `import_declaration` and `type_parameter` is the grammar's `name` field.
    ///
    /// `variable_declarator` is absent because its answer depends on its
    /// parent — a declarator under `field_declaration` declares a field, under
    /// `local_variable_declaration` a local, and under `spread_parameter` a
    /// parameter. See [`JavaLanguage::declarator_kind`].
    ///
    /// Deliberately absent: `static_initializer` and `block` declare nothing;
    /// `package_declaration` names a package, which is not a lexical binding
    /// this file can resolve against.
    pub const DECLARATIONS: &'static [(&'static str, DeclKind)] = &[
        ("class_declaration", DeclKind::Class),
        ("interface_declaration", DeclKind::Interface),
        ("enum_declaration", DeclKind::Enum),
        ("record_declaration", DeclKind::Record),
        ("annotation_type_declaration", DeclKind::Annotation),
        ("method_declaration", DeclKind::Method),
        ("annotation_type_element_declaration", DeclKind::Method),
        ("constructor_declaration", DeclKind::Constructor),
        ("compact_constructor_declaration", DeclKind::Constructor),
        ("enum_constant", DeclKind::EnumMember),
        ("formal_parameter", DeclKind::Parameter),
        ("catch_formal_parameter", DeclKind::Parameter),
        ("type_parameter", DeclKind::TypeParam),
        ("resource", DeclKind::Local),
        ("import_declaration", DeclKind::Import),
        ("module_declaration", DeclKind::Module),
    ];

    /// `(parent kind, field name)` pairs whose `identifier` child is a
    /// **member reference** — a name resolved against something other than the
    /// lexical scope chain — and which `sm-bind` must therefore never resolve.
    ///
    /// This list is the whole reason [`IdentifierRole`] exists. Without it,
    /// `user.getName()` would look like a lexical reference to `getName`, find
    /// nothing in the file, and be reported as a broken reference — on every
    /// member access in every file.
    ///
    /// - `field_access.field` — `obj.count`; resolved against `obj`'s type.
    /// - `scoped_identifier.name` — the tail of `a.b.c`, whether that is a
    ///   package, a type or a static member.
    /// - `element_value_pair.key` — `@Anno(name = "x")`; an annotation element,
    ///   resolved against the annotation type.
    /// - `annotation.name` / `marker_annotation.name` are **not** here: an
    ///   annotation name is a type name, and a `@Nested` annotation type
    ///   declared in the same file genuinely does resolve. They are classified
    ///   [`IdentifierRole::TypeRef`].
    ///
    /// `method_invocation.name` is handled separately because its answer
    /// depends on whether the invocation has a receiver; see
    /// [`JavaLanguage::identifier_role`].
    pub const MEMBER_REF_CONTEXTS: &'static [(&'static str, &'static str)] = &[
        ("field_access", "field"),
        ("scoped_identifier", "name"),
        ("element_value_pair", "key"),
    ];

    /// Parent kinds whose identifier children are statement **labels**.
    ///
    /// `labeled_statement` declares one, `break`/`continue` use one. The
    /// grammar gives none of them a field name, so they are recognised by
    /// parent kind.
    pub const LABEL_PARENTS: &'static [&'static str] =
        &["labeled_statement", "break_statement", "continue_statement"];

    /// Parent kinds inside which every identifier is part of a **qualified
    /// name**, not a lexical reference: `package com.example;` and
    /// `import java.util.List;`.
    ///
    /// The leading segment of such a name (`com`, `java`) would otherwise look
    /// like an ordinary value reference.
    pub const QUALIFIED_NAME_ROOTS: &'static [&'static str] =
        &["package_declaration", "import_declaration"];

    /// What a `variable_declarator` declares, given the kind of its parent.
    ///
    /// The declarator node itself is identical in all three positions; only the
    /// parent says whether this is a field, a `...` parameter or a local.
    #[must_use]
    pub fn declarator_kind(parent_kind: &str) -> DeclKind {
        match parent_kind {
            "field_declaration" | "constant_declaration" => DeclKind::Field,
            "spread_parameter" => DeclKind::Parameter,
            _ => DeclKind::Local,
        }
    }

    /// The simple name a Java import introduces, or `None` for a wildcard.
    ///
    /// `import java.util.List;` binds `List`; `import static
    /// java.util.Objects.requireNonNull;` binds `requireNonNull`;
    /// `import java.util.*;` binds nothing this analysis can see, which is
    /// exactly why unresolved names are never reported (`sm-bind`'s
    /// single-file honesty rule).
    fn import_simple_name(tree: &SourceTree, id: NodeId) -> Option<NodeId> {
        // A wildcard import has an `asterisk` child directly under the
        // declaration.
        if tree
            .children(id)
            .any(|c| tree.node(c).kind == "asterisk" || tree.node(c).kind == "*")
        {
            return None;
        }
        let path = tree
            .children(id)
            .find(|&c| matches!(tree.node(c).kind, "scoped_identifier" | "identifier"))?;
        match tree.node(path).kind {
            "identifier" => Some(path),
            _ => tree.child_by_field_name(path, "name"),
        }
    }
}

impl JavaLanguage {
    /// The role of one identifier node. Split out of the trait impl so the
    /// reasoning can be read top to bottom.
    fn role(&self, tree: &SourceTree, id: NodeId) -> IdentifierRole {
        let node = tree.node(id);
        if !Self::IDENTIFIERS.contains(&node.kind) {
            return IdentifierRole::NotAName;
        }
        // A name with no parent cannot be classified; the catch-all applies.
        let Some(parent) = node.parent else {
            return IdentifierRole::MemberRef;
        };
        let parent_kind = tree.node(parent).kind;
        let field = tree.field_name(id);

        // 1. Is this the name of a declaration? Ask the declaration itself, so
        //    the two APIs cannot drift apart.
        if let Some(kind) = self.declaration_kind(tree, parent)
            && self.declared_name(tree, parent) == Some(id)
        {
            return IdentifierRole::Declaration(kind);
        }

        // 2. Labels.
        if Self::LABEL_PARENTS.contains(&parent_kind) {
            return IdentifierRole::Label;
        }

        // 3. Anything inside a package or import path is a qualified-name
        //    segment, not a lexical reference — except the one segment that is
        //    the simple name the import binds, which sits several levels down
        //    inside a `scoped_identifier` and so was missed by step 1.
        if let Some(root) = tree
            .ancestors(id)
            .find(|&a| Self::QUALIFIED_NAME_ROOTS.contains(&tree.node(a).kind))
        {
            if tree.node(root).kind == "import_declaration"
                && self.declared_name(tree, root) == Some(id)
            {
                return IdentifierRole::Declaration(DeclKind::Import);
            }
            return IdentifierRole::MemberRef;
        }

        // 4. Receiver-typed names.
        if let Some(field) = field
            && Self::MEMBER_REF_CONTEXTS.contains(&(parent_kind, field))
        {
            return IdentifierRole::MemberRef;
        }
        if parent_kind == "method_invocation" && field == Some("name") {
            // `user.getName()` resolves `getName` against `user`'s type;
            // `helper()` resolves against the enclosing class and its
            // supertypes, and the in-file part of that is exactly what a scope
            // tree can answer.
            return if tree.child_by_field_name(parent, "object").is_some() {
                IdentifierRole::MemberRef
            } else {
                IdentifierRole::CallRef
            };
        }
        // `A.B` as a *type*: the grammar gives `scoped_type_identifier` no
        // field names, so the first named child is the qualifier and everything
        // after it is selected from it.
        if parent_kind == "scoped_type_identifier" {
            let first = tree.named_children(parent).next();
            return if first == Some(id) {
                IdentifierRole::TypeRef
            } else {
                IdentifierRole::MemberRef
            };
        }
        // `String::valueOf` — same shape, same rule.
        if parent_kind == "method_reference" {
            let first = tree.named_children(parent).next();
            return if first == Some(id) {
                IdentifierRole::LexicalRef
            } else {
                IdentifierRole::MemberRef
            };
        }

        // 5. Everything else. A `type_identifier` is a type-position name by
        //    construction, and so is an annotation's name — `@Nested` may well
        //    name an annotation type declared in this very file. A bare
        //    `identifier` in an expression is a value.
        let is_type_position = node.kind == "type_identifier"
            || (matches!(parent_kind, "annotation" | "marker_annotation") && field == Some("name"));
        if is_type_position {
            IdentifierRole::TypeRef
        } else {
            IdentifierRole::LexicalRef
        }
    }
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

    fn identifier_role(&self, tree: &SourceTree, id: NodeId) -> IdentifierRole {
        self.role(tree, id)
    }

    fn declaration_kind(&self, tree: &SourceTree, id: NodeId) -> Option<DeclKind> {
        let kind = tree.node(id).kind;
        if kind == "variable_declarator" {
            let parent_kind = tree.node(id).parent.map_or("", |p| tree.node(p).kind);
            return Some(Self::declarator_kind(parent_kind));
        }
        Self::DECLARATIONS
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, d)| *d)
    }

    fn declared_name(&self, tree: &SourceTree, id: NodeId) -> Option<NodeId> {
        match tree.node(id).kind {
            // `import java.util.List;` binds the *simple* name.
            "import_declaration" => Self::import_simple_name(tree, id),
            // `<T extends Number>` — the grammar gives the bound variable no
            // field name, so it is the first `type_identifier` child.
            "type_parameter" => tree
                .children(id)
                .find(|&c| tree.node(c).kind == "type_identifier"),
            _ => tree.child_by_field_name(id, "name"),
        }
    }

    /// Java resolves an unqualified `count` or `helper()` inside a class
    /// against that class's members, and against the members of every enclosing
    /// class. That is the property the whole rename-detection case rests on.
    fn member_visibility(&self) -> MemberVisibility {
        MemberVisibility::LexicalInType
    }
}
