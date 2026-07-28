//! The per-language configuration trait (SPEC.md §4.2).

use crate::arena::{NodeId, SourceTree};

/// How the merge algorithm (M4) must treat a container node's child list.
///
/// This is the single most consequential piece of per-language configuration in
/// the project: getting it wrong produces *silently wrong merges*, which
/// SPEC.md §0.4 rules out categorically. The bias is therefore always towards
/// [`ChildListKind::Ordered`], which can at worst produce an unnecessary
/// conflict.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChildListKind {
    /// Order is semantic. Merge as a sequence: a diff3-style sequence merge
    /// where elements are identified by node matching rather than by position.
    /// Insertions from both branches at the same anchor conflict — the
    /// interleaving is not ours to guess.
    Ordered,
    /// Order carries no meaning. Merge as a set: disjoint additions from both
    /// sides all apply and identical additions deduplicate. This is what
    /// eliminates the classic "both branches added an import" conflict.
    Unordered,
    /// Ordered overall, but children of certain kinds form order-insensitive
    /// runs within it.
    ///
    /// Java's compilation unit is the motivating case. A `program` node holds
    /// `package_declaration`, then imports, then type declarations. The imports
    /// are freely reorderable among themselves, but the package declaration
    /// cannot move and the type declarations are not interchangeable with
    /// either. Collapsing that to a single [`ChildListKind::Unordered`] would
    /// let a merge hoist a class above the package statement.
    PartiallyUnordered {
        /// Child kinds that may be reordered among themselves. Everything else
        /// in the list is positionally fixed relative to them.
        unordered_kinds: &'static [&'static str],
    },
}

/// What a declaration declares (SPEC.md §7, M6).
///
/// This is the *shared vocabulary* between the per-language configuration and
/// `sm-bind`'s language-agnostic scope builder. Every rule `sm-bind` applies —
/// which namespace a declaration occupies, whether it is visible before its own
/// declaration point, whether it belongs to the enclosing type rather than the
/// enclosing block — is keyed on this enum and on nothing language-specific.
///
/// The variants are deliberately finer than name resolution strictly needs. A
/// semantic-conflict explanation that says "renamed *method* `getUser`" reads
/// very differently from one that says "renamed *declaration* `getUser`", and
/// the distinction is free here.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DeclKind {
    /// A class. In Java a type only; in TypeScript a type *and* a value.
    Class,
    /// An interface. Type namespace only, in both languages.
    Interface,
    /// An enum type.
    Enum,
    /// One constant of an enum (`RED`), or one TypeScript enum member.
    EnumMember,
    /// A Java `record`.
    Record,
    /// A Java `@interface` declaration.
    Annotation,
    /// A TypeScript `type X = …` alias.
    TypeAlias,
    /// A generic type parameter, `<T>`.
    TypeParam,
    /// A free function (TypeScript `function f() {}`). Callable *and* a value:
    /// JavaScript has one namespace, so `f` and `f()` are the same binding.
    Function,
    /// A method of a type.
    Method,
    /// A constructor.
    Constructor,
    /// A field of a type — Java's `int x;`, TypeScript's `x = 1` in a class
    /// body.
    Field,
    /// A property signature of a TypeScript interface or object type.
    Property,
    /// A parameter of a callable, or a `catch` parameter.
    Parameter,
    /// A block-scoped local: Java's locals, TypeScript's `let`/`const`.
    Local,
    /// A function-scoped local: TypeScript's `var`. Hoisted to the nearest
    /// enclosing function scope, which is why it is not just a [`Self::Local`].
    Var,
    /// A name introduced into the file's scope by an import.
    Import,
    /// A module declaration (Java's `module-info`, TypeScript's
    /// `declare module`).
    Module,
    /// A TypeScript `namespace X { … }`.
    Namespace,
}

impl DeclKind {
    /// A short stable identifier, for reports and snapshot tests.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Class => "class",
            Self::Interface => "interface",
            Self::Enum => "enum",
            Self::EnumMember => "enum member",
            Self::Record => "record",
            Self::Annotation => "annotation type",
            Self::TypeAlias => "type alias",
            Self::TypeParam => "type parameter",
            Self::Function => "function",
            Self::Method => "method",
            Self::Constructor => "constructor",
            Self::Field => "field",
            Self::Property => "property",
            Self::Parameter => "parameter",
            Self::Local => "local variable",
            Self::Var => "var",
            Self::Import => "import",
            Self::Module => "module",
            Self::Namespace => "namespace",
        }
    }

    /// Whether this declaration is a **member of a type** rather than of a
    /// lexical block.
    ///
    /// Members are visible throughout the type body regardless of declaration
    /// order — but only in a language whose
    /// [`Language::member_visibility`] says so.
    #[must_use]
    pub const fn is_member(self) -> bool {
        matches!(
            self,
            Self::Field | Self::Property | Self::Method | Self::Constructor | Self::EnumMember
        )
    }

    /// Whether the declaration **names itself**, i.e. the name it introduces
    /// belongs to the *enclosing* scope rather than to the scope the
    /// declaration itself opens.
    ///
    /// `class C { … }` opens a scope and puts `C` in the scope around it;
    /// `catch (E e) { … }` also opens a scope but puts `e` *inside* it.
    #[must_use]
    pub const fn is_self_named(self) -> bool {
        matches!(
            self,
            Self::Class
                | Self::Interface
                | Self::Enum
                | Self::Record
                | Self::Annotation
                | Self::TypeAlias
                | Self::Function
                | Self::Method
                | Self::Constructor
                | Self::Module
                | Self::Namespace
        )
    }

    /// Whether the name is visible throughout its scope rather than only from
    /// its declaration point onwards.
    ///
    /// True for everything a type body declares, for type declarations (Java
    /// resolves top-level types in any order; TypeScript classes are routinely
    /// referenced above their definition in type position), for hoisted
    /// `function` and `var`, and for imports. False for block-scoped locals and
    /// for parameters, both of which textually precede every use anyway.
    #[must_use]
    pub const fn is_hoisted(self) -> bool {
        !matches!(self, Self::Local | Self::Parameter)
    }

    /// Whether a `let`/`var`-style *value* reference can bind to this.
    #[must_use]
    pub const fn in_value_namespace(self) -> bool {
        matches!(
            self,
            Self::Class
                | Self::Enum
                | Self::EnumMember
                | Self::Function
                | Self::Field
                | Self::Property
                | Self::Parameter
                | Self::Local
                | Self::Var
                | Self::Import
                | Self::Module
                | Self::Namespace
        )
    }

    /// Whether a *type*-position reference can bind to this.
    #[must_use]
    pub const fn in_type_namespace(self) -> bool {
        matches!(
            self,
            Self::Class
                | Self::Interface
                | Self::Enum
                | Self::Record
                | Self::Annotation
                | Self::TypeAlias
                | Self::TypeParam
                | Self::Import
                | Self::Module
                | Self::Namespace
        )
    }

    /// Whether a *call*-position reference can bind to this.
    ///
    /// Java keeps methods in their own namespace — `foo` and `foo()` can name
    /// different things in the same class — so [`IdentifierRole::CallRef`]
    /// exists and matches only these. TypeScript has one namespace and never
    /// emits `CallRef`.
    #[must_use]
    pub const fn in_callable_namespace(self) -> bool {
        matches!(
            self,
            Self::Method
                | Self::Constructor
                | Self::Function
                | Self::Import
                | Self::Class
                | Self::Record
        )
    }
}

/// What an identifier node *is*, given the syntax around it.
///
/// # Why this exists at all
///
/// [`Language::is_identifier`] is a single boolean over a *kind name*, and that
/// is not enough information to resolve anything. In `tree-sitter-java` the kind
/// `identifier` appears as `method_declaration.name` (a declaration), as
/// `method_invocation.object` (a reference to a local), as
/// `method_invocation.name` (a *method* name, which for a qualified call
/// resolves against the receiver's **type**, not against any lexical scope), as
/// `field_access.field`, as `element_value_pair.key` and as a `break` label.
/// TypeScript splits some of that into `property_identifier` and
/// `private_property_identifier`, which are likewise receiver-typed.
///
/// A resolver that treated all of those as lexical references would walk out to
/// the file scope, find nothing, and report a "broken reference" on **every
/// property access and every qualified method call in the file**. Roles are what
/// keep that from happening, and the catch-all is [`Self::MemberRef`], which
/// `sm-bind` never resolves and therefore never reports.
///
/// Determining a role needs the *parent context*, which is why this method takes
/// a tree and a node rather than a kind name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IdentifierRole {
    /// A name resolved against the lexical scope chain, value namespace.
    LexicalRef,
    /// A name resolved against the lexical scope chain, type namespace.
    TypeRef,
    /// A name in call position resolved against the lexical scope chain,
    /// callable namespace — an *unqualified* Java method invocation. A
    /// qualified one (`user.getName()`) is a [`Self::MemberRef`].
    CallRef,
    /// This node is a declaration's own name.
    Declaration(DeclKind),
    /// A statement label. Its own namespace, and `sm-bind` does not resolve it:
    /// labels are always file-local and never cross-file, so nothing here can
    /// be a merge-time surprise that the syntactic merge did not already catch.
    Label,
    /// A name that a scope tree **cannot** answer: a member selected from a
    /// receiver's type, a segment of a qualified/package name, an annotation
    /// element key. Never resolved, therefore never reported.
    ///
    /// This is the conservative catch-all. When a language implementation is
    /// unsure, this is the answer.
    MemberRef,
    /// Not a name at all.
    NotAName,
}

impl IdentifierRole {
    /// Whether `sm-bind` should resolve this node against the scope chain.
    #[must_use]
    pub const fn is_lexical_reference(self) -> bool {
        matches!(self, Self::LexicalRef | Self::TypeRef | Self::CallRef)
    }

    /// Whether a declaration of `kind` can satisfy a reference in this role.
    ///
    /// `false` for every non-reference role.
    #[must_use]
    pub const fn accepts(self, kind: DeclKind) -> bool {
        match self {
            Self::LexicalRef => kind.in_value_namespace(),
            Self::TypeRef => kind.in_type_namespace(),
            Self::CallRef => kind.in_callable_namespace(),
            _ => false,
        }
    }

    /// A short stable identifier, for snapshot tests.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::LexicalRef => "lexical_ref",
            Self::TypeRef => "type_ref",
            Self::CallRef => "call_ref",
            Self::Declaration(_) => "declaration",
            Self::Label => "label",
            Self::MemberRef => "member_ref",
            Self::NotAName => "not_a_name",
        }
    }
}

/// Whether a type's members are visible to *unqualified* names inside it.
///
/// Java: yes — a method body may say `count` and mean `this.count`, and may call
/// `helper()` with no receiver. TypeScript: no — a class field is only reachable
/// as `this.x`, and a bare `x` in a method body means a *lexical* `x` from an
/// enclosing function or module.
///
/// Getting this backwards for TypeScript would make every method parameter that
/// shadows a field look like a capture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemberVisibility {
    /// Members participate in unqualified lexical lookup inside the type.
    LexicalInType,
    /// Members are reachable only through an explicit receiver. The
    /// conservative default: it can only make a name *fail* to resolve, and an
    /// unresolved name is never reported (see `sm-bind`'s "single-file honesty
    /// rule").
    ReceiverOnly,
}

/// Everything the language-agnostic layers need to know about one language.
///
/// Object-safe on purpose: languages are looked up dynamically by file
/// extension (see [`crate::languages::detect`]) and handed around as
/// `&'static dyn Language`. Nothing above `sm-cst` should ever name a concrete
/// language type.
///
/// All the classification methods are keyed on the *kind name* rather than on a
/// node, so they can be answered without a tree in hand — the merge algorithm
/// asks questions like "is `class_body` unordered?" while planning, before it
/// has a specific node.
pub trait Language: Sync {
    /// The tree-sitter grammar. Cheap to clone; tree-sitter's `Language` is a
    /// refcounted handle to static tables.
    fn ts_language(&self) -> tree_sitter::Language;

    /// A short stable identifier, e.g. `"java"`. Appears in output and in the
    /// JSON schema, so treat it as part of the public interface.
    fn name(&self) -> &'static str;

    /// File extensions, without the dot, that select this language.
    fn file_extensions(&self) -> &'static [&'static str];

    /// How the children of a node of this kind must be merged.
    ///
    /// Defaults to [`ChildListKind::Ordered`], the conservative answer: an
    /// unlisted container is treated as order-sensitive, which can cost a
    /// spurious conflict but can never reorder code behind the user's back.
    fn child_list_kind(&self, _container_kind: &str) -> ChildListKind {
        ChildListKind::Ordered
    }

    /// Whether a node of this kind introduces a lexical scope (SPEC.md §7, M6).
    ///
    /// Consumed by `sm-bind` to build the scope tree that name resolution walks.
    fn is_scope_introducing(&self, kind: &str) -> bool;

    /// Whether a node of this kind is an identifier — a name that either
    /// declares or references something.
    ///
    /// A question about a *kind*, and therefore too coarse to resolve against:
    /// one kind covers declarations, lexical references and receiver-typed
    /// member names alike. [`Language::identifier_role`] is what `sm-bind`
    /// actually resolves against; see [`IdentifierRole`] for why the
    /// distinction is not optional.
    fn is_identifier(&self, kind: &str) -> bool;

    /// Whether a node of this kind is a comment.
    ///
    /// The trivia attachment pass uses this to decide what it is attaching; the
    /// renderer uses it to decide what to show inline.
    fn is_comment(&self, kind: &str) -> bool;

    /// Whether the *text* of a leaf of this kind matters for structural
    /// matching.
    ///
    /// True for identifiers and literals, where two nodes of the same kind mean
    /// different things if they read differently. False for anonymous tokens and
    /// keywords, whose text is fully determined by their kind, so comparing it
    /// is wasted work. The matcher's structural hash (M2) uses this to decide
    /// whether to fold a leaf's bytes into the hash.
    fn significant_text(&self, kind: &str) -> bool;

    // --------------------------------------------------------------- M6: names

    /// What this node is, as a name (SPEC.md §7, M6).
    ///
    /// See [`IdentifierRole`] for why a boolean `is_identifier` is not enough
    /// and why this needs the tree rather than just a kind name.
    ///
    /// The default is the safe one: anything [`Language::is_identifier`] calls
    /// an identifier is reported as [`IdentifierRole::MemberRef`], which
    /// `sm-bind` never resolves — so a language that has not implemented this
    /// yet contributes no declarations, no references and therefore no semantic
    /// conflicts, rather than a flood of false ones.
    fn identifier_role(&self, tree: &SourceTree, id: NodeId) -> IdentifierRole {
        if self.is_identifier(tree.node(id).kind) {
            IdentifierRole::MemberRef
        } else {
            IdentifierRole::NotAName
        }
    }

    /// Whether `id` is a declaration, and of what kind.
    ///
    /// Answered for the *declaration* node — `method_declaration`,
    /// `variable_declarator`, `import_specifier` — not for its name. The name
    /// is [`Language::declared_name`].
    ///
    /// Default: nothing is a declaration.
    fn declaration_kind(&self, _tree: &SourceTree, _id: NodeId) -> Option<DeclKind> {
        None
    }

    /// The node holding the name a declaration introduces.
    ///
    /// `None` when the declaration introduces no single simple name — a Java
    /// wildcard import, or a TypeScript parameter whose "name" is a
    /// destructuring pattern, whose leaves are declarations in their own right
    /// and are reported as such.
    ///
    /// The default reads the grammar's `name` field, which is correct for the
    /// large majority of declaration kinds in both supported grammars.
    fn declared_name(&self, tree: &SourceTree, id: NodeId) -> Option<NodeId> {
        tree.child_by_field_name(id, "name")
    }

    /// Whether a type's members take part in unqualified lexical lookup inside
    /// it. See [`MemberVisibility`]; the default is the conservative answer.
    fn member_visibility(&self) -> MemberVisibility {
        MemberVisibility::ReceiverOnly
    }

    /// Whether a node of this kind is a **function-level** scope — the scope a
    /// hoisted [`DeclKind::Var`] lands in.
    ///
    /// Only TypeScript needs this; Java has no function-hoisted binding form.
    /// The default `false` makes a `Var` behave like a `Local`, an
    /// under-approximation, and under-approximating resolution is always safe
    /// here (see `sm-bind`'s "single-file honesty rule").
    fn is_function_scope(&self, _kind: &str) -> bool {
        false
    }
}
