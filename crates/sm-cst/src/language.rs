//! The per-language configuration trait (SPEC.md §4.2).

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
    /// declares or references something. `sm-bind` resolves exactly these.
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
}
