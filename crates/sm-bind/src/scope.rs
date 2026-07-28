//! The scope tree: [`ScopeTree`], [`Scope`], [`Decl`], [`Reference`].
//!
//! One [`ScopeTree`] describes *one program*. That program is either a parsed
//! revision ([`ScopeTree::build`]) or the candidate merge result
//! ([`ScopeTree::build_merged`]), and the two are the same type on purpose: the
//! whole M6 check is "resolve this name in both and compare".
//!
//! # The virtual merged program
//!
//! A [`sm_merge::MergedTree`] is not a syntax tree — it is a plan for splicing
//! bytes. A [`sm_merge::MergedNode::Splice`] says "take this subtree verbatim
//! from *that* revision" and carries no children of its own. So building scopes
//! over it means walking the merged tree and, at every splice, continuing the
//! walk inside the origin revision's own arena. That is what
//! [`ScopeTree::build_merged`] does, and it is why every node in this module is
//! identified by a [`NodeRef`] — a `(Side, NodeId)` pair — rather than a bare
//! `NodeId`.
//!
//! Doing it this way rather than by emitting the merge and reparsing the bytes
//! buys three things:
//!
//! 1. **It works on a conflicted merge.** Emitted bytes contain `<<<<<<<`
//!    marker lines, which do not parse; the tree you would get back is
//!    error-recovery noise. Here a [`sm_merge::MergedNode::Conflict`] is simply
//!    not walked, so the regions around it are still checked and the conflicted
//!    region contributes nothing.
//! 2. **Provenance is exact.** Every reference already knows which revision it
//!    came from, which is precisely what the differential check needs. Recovering
//!    that from reparsed bytes would need an offset map the emitter does not
//!    keep.
//! 3. **No second parse.** The check costs one pass over the merge plan.
//!
//! The cost is stated honestly in the crate documentation: spans are reported in
//! the coordinates of the revision they came from, not of the merged file.
//!
//! # Ordering, not byte offsets
//!
//! "A local is visible from its declaration to the end of the block" needs a
//! notion of *before*. Byte offsets cannot supply it here, because two adjacent
//! statements in the merged program can come from two different revisions and
//! their offsets are not comparable. So every node visited gets a monotonically
//! increasing **virtual preorder index** ([`Reference::order`]), and visibility
//! is expressed against that. For a single-revision tree the virtual order is
//! the ordinary preorder, so the two builders agree by construction.

use std::collections::BTreeMap;
use std::ops::Range;

use serde::{Deserialize, Serialize};
use sm_cst::{DeclKind, IdentifierRole, Language, MemberVisibility, NodeId, SourceTree};
use sm_merge::{MergedId, MergedNode, MergedTree, Side};

/// Index of a [`Scope`] in a [`ScopeTree`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScopeId(pub u32);

/// Index of a [`Decl`] in a [`ScopeTree`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeclId(pub u32);

/// Index of a [`Reference`] in a [`ScopeTree`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RefId(pub u32);

/// A node, qualified by the revision it lives in.
///
/// The merged program is stitched together from three arenas, so a bare
/// [`NodeId`] is ambiguous there. For a single-revision [`ScopeTree`] every
/// `NodeRef` carries the same [`Side`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct NodeRef {
    /// Which revision the node lives in.
    pub side: Side,
    /// The node within that revision's arena.
    pub node: NodeId,
}

impl NodeRef {
    /// Construct a reference to `node` in `side`.
    #[must_use]
    pub const fn new(side: Side, node: NodeId) -> Self {
        Self { side, node }
    }

    /// A total order over `NodeRef`s, so the index below can be a `BTreeMap`.
    ///
    /// `sm_merge::Side` is deliberately not `Ord` — there is no meaningful
    /// order on the three revisions — and this crate must not iterate a hash
    /// map, because `sm_merge::merge` guarantees determinism and it would be
    /// odd for the check layered on top of it not to.
    const fn key(self) -> (u8, NodeId) {
        let side = match self.side {
            Side::Base => 0,
            Side::Ours => 1,
            Side::Theirs => 2,
        };
        (side, self.node)
    }
}

/// The three parsed revisions, so a [`NodeRef`] can be dereferenced.
#[derive(Clone, Copy)]
pub struct Revisions<'a> {
    /// The common ancestor.
    pub base: &'a SourceTree,
    /// The revision being merged into.
    pub ours: &'a SourceTree,
    /// The revision being merged in.
    pub theirs: &'a SourceTree,
}

impl<'a> Revisions<'a> {
    /// The tree for one side.
    #[must_use]
    pub fn get(&self, side: Side) -> &'a SourceTree {
        match side {
            Side::Base => self.base,
            Side::Ours => self.ours,
            Side::Theirs => self.theirs,
        }
    }

    /// The bytes a node spans, in its own revision.
    #[must_use]
    pub fn bytes(&self, at: NodeRef) -> &'a [u8] {
        self.get(at.side).node_bytes(at.node)
    }

    /// A node's byte range, in its own revision.
    #[must_use]
    pub fn span(&self, at: NodeRef) -> Range<u32> {
        self.get(at.side).node(at.node).byte_range.clone()
    }
}

/// One lexical scope.
#[derive(Clone, Debug)]
pub struct Scope {
    /// The kind of the node that introduced this scope, e.g. `method_declaration`.
    pub kind: &'static str,
    /// The node that introduced it.
    pub node: NodeRef,
    /// The enclosing scope; `None` only for the file scope.
    pub parent: Option<ScopeId>,
    /// Declarations owned by this scope, in the order they were seen.
    pub decls: Vec<DeclId>,
    /// Whether this is a function-level scope, i.e. where a hoisted
    /// [`DeclKind::Var`] lands. See [`Language::is_function_scope`].
    pub function_scope: bool,
}

/// One declaration.
#[derive(Clone, Debug)]
pub struct Decl {
    /// The declared name, as raw bytes. Not a `String`: sources are not
    /// required to be UTF-8 (see [`SourceTree`]).
    pub name: Vec<u8>,
    /// What is declared.
    pub kind: DeclKind,
    /// The node holding the name.
    pub name_node: NodeRef,
    /// The declaration node itself — `method_declaration`, `variable_declarator`.
    pub decl_node: NodeRef,
    /// The scope this name is visible in.
    pub scope: ScopeId,
    /// The virtual preorder index from which the name is visible.
    ///
    /// `0` for a hoisted declaration (see [`DeclKind::is_hoisted`]); otherwise
    /// one past the **name node**, so that a local shadows an outer name from
    /// its declaration point and not before.
    ///
    /// The name node rather than the whole declaration, because some
    /// declarations *contain* the scope their name is visible in —
    /// `for (const item of xs) { … }` and `catch (e) { … }` both have their
    /// body inside the declaration node, and closing at the end of the
    /// declaration would make the name invisible in exactly the block it was
    /// declared for.
    pub visible_from: u32,
}

/// One name that must be resolved against the scope chain.
#[derive(Clone, Debug)]
pub struct Reference {
    /// The identifier node.
    pub node: NodeRef,
    /// Its bytes.
    pub name: Vec<u8>,
    /// Why it is being resolved — which namespace it looks in.
    pub role: RefRole,
    /// The innermost scope enclosing it.
    pub scope: ScopeId,
    /// Its virtual preorder index. See the module docs.
    pub order: u32,
    /// For a merged [`ScopeTree`], the nearest enclosing merged node — the
    /// handle a caller with the [`MergedTree`] in hand can use to locate this
    /// reference in the merge plan. `None` for a single-revision tree.
    pub anchor: Option<MergedId>,
}

/// The namespace a reference looks in.
///
/// A narrowing of [`IdentifierRole`] to the three variants that are actually
/// resolved, so that a [`Reference`] cannot hold a role that makes no sense.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefRole {
    /// Value namespace.
    Value,
    /// Type namespace.
    Type,
    /// Callable namespace (Java's unqualified method invocations).
    Callable,
}

impl RefRole {
    fn from_role(role: IdentifierRole) -> Option<Self> {
        match role {
            IdentifierRole::LexicalRef => Some(Self::Value),
            IdentifierRole::TypeRef => Some(Self::Type),
            IdentifierRole::CallRef => Some(Self::Callable),
            _ => None,
        }
    }

    /// Whether a declaration of `kind` can satisfy a reference in this role.
    #[must_use]
    pub const fn accepts(self, kind: DeclKind) -> bool {
        match self {
            Self::Value => kind.in_value_namespace(),
            Self::Type => kind.in_type_namespace(),
            Self::Callable => kind.in_callable_namespace(),
        }
    }

    /// A short stable identifier, for reports and snapshot tests.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Value => "value",
            Self::Type => "type",
            Self::Callable => "callable",
        }
    }
}

/// What a name resolved to.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    /// It bound to a declaration in this program.
    Decl(DeclId),
    /// Nothing in this program declares it.
    ///
    /// **This is not, on its own, a problem** — it is the normal answer for
    /// every name that lives in another file, in the JDK or in `node_modules`.
    /// See the crate documentation's "single-file honesty rule".
    Unresolved,
}

/// The identity of a declaration, comparable *across* revisions.
///
/// See [`ScopeTree::signature`] for what it does and does not distinguish.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct DeclSignature {
    /// The declared name.
    pub name: Vec<u8>,
    /// What kind of thing it declares.
    pub kind: DeclKind,
    /// The kinds of the scopes enclosing it, outermost first.
    pub scope_path: Vec<&'static str>,
}

/// Scopes, declarations and references for one program.
#[derive(Clone, Debug)]
pub struct ScopeTree {
    scopes: Vec<Scope>,
    decls: Vec<Decl>,
    refs: Vec<Reference>,
    by_node: BTreeMap<(u8, NodeId), RefId>,
    member_visibility: MemberVisibility,
    conflict_regions: usize,
}

impl ScopeTree {
    /// Build the scope tree for one parsed revision.
    ///
    /// `side` labels every [`NodeRef`] the result contains; pass the side this
    /// tree actually is, so that a merged tree's references can be looked up
    /// here by identity.
    #[must_use]
    pub fn build(tree: &SourceTree, side: Side, lang: &dyn Language) -> Self {
        let revs = Revisions {
            base: tree,
            ours: tree,
            theirs: tree,
        };
        let mut builder = Builder::new(revs, lang);
        builder.push_source(side, tree.root_id(), None);
        builder.run();
        builder.finish()
    }

    /// Build the scope tree for the *candidate merge result*.
    ///
    /// Walks the merge plan, descending into each revision's own arena wherever
    /// the plan splices a subtree verbatim. Conflict regions are skipped and
    /// counted in [`ScopeTree::conflict_regions`].
    #[must_use]
    pub fn build_merged(merged: &MergedTree, revs: Revisions<'_>, lang: &dyn Language) -> Self {
        let mut builder = Builder::new(revs, lang);
        builder.push_merged(merged.root());
        builder.run_merged(merged);
        builder.finish()
    }

    /// Every scope, outermost first (a parent always precedes its children).
    #[must_use]
    pub fn scopes(&self) -> &[Scope] {
        &self.scopes
    }

    /// Every declaration.
    #[must_use]
    pub fn decls(&self) -> &[Decl] {
        &self.decls
    }

    /// Every reference that is resolved against the scope chain. Member
    /// references and labels are *not* here — see [`IdentifierRole`].
    #[must_use]
    pub fn references(&self) -> &[Reference] {
        &self.refs
    }

    /// Look up a scope.
    #[must_use]
    pub fn scope(&self, id: ScopeId) -> &Scope {
        &self.scopes[id.0 as usize]
    }

    /// Look up a declaration.
    #[must_use]
    pub fn decl(&self, id: DeclId) -> &Decl {
        &self.decls[id.0 as usize]
    }

    /// Look up a reference.
    #[must_use]
    pub fn reference(&self, id: RefId) -> &Reference {
        &self.refs[id.0 as usize]
    }

    /// Find the reference occupying a given node, if that node is one.
    ///
    /// This is how a reference in the merged program is matched with the same
    /// reference in the revision it came from: the merge plan's provenance
    /// *is* the key.
    #[must_use]
    pub fn reference_at(&self, at: NodeRef) -> Option<RefId> {
        self.by_node.get(&at.key()).copied()
    }

    /// How many conflict regions the walk skipped. Always `0` for a
    /// single-revision tree.
    #[must_use]
    pub fn conflict_regions(&self) -> usize {
        self.conflict_regions
    }

    /// Resolve one reference against the scope chain.
    ///
    /// Innermost scope first; within a scope, the latest declaration visible at
    /// the reference's position wins, so shadowing works and a redeclaration
    /// takes over from its own point.
    #[must_use]
    pub fn resolve(&self, reference: &Reference) -> Resolution {
        let mut scope = Some(reference.scope);
        while let Some(id) = scope {
            let s = &self.scopes[id.0 as usize];
            let mut best: Option<(u32, DeclId)> = None;
            for &d in &s.decls {
                let decl = &self.decls[d.0 as usize];
                if decl.name != reference.name || !reference.role.accepts(decl.kind) {
                    continue;
                }
                // A type's members only take part in unqualified lookup where
                // the language says they do (Java yes, TypeScript no).
                if decl.kind.is_member() && self.member_visibility == MemberVisibility::ReceiverOnly
                {
                    continue;
                }
                if !decl.kind.is_hoisted() && decl.visible_from > reference.order {
                    continue;
                }
                if best.is_none_or(|(seen, _)| decl.visible_from >= seen) {
                    best = Some((decl.visible_from, d));
                }
            }
            if let Some((_, d)) = best {
                return Resolution::Decl(d);
            }
            scope = s.parent;
        }
        Resolution::Unresolved
    }

    /// Resolve every reference in this tree, in order.
    pub fn resolve_all(&self) -> impl Iterator<Item = (RefId, Resolution)> + '_ {
        self.refs
            .iter()
            .enumerate()
            .map(|(i, r)| (RefId(i as u32), self.resolve(r)))
    }

    /// A declaration's identity, in a form comparable across revisions.
    ///
    /// # What it distinguishes, and what it deliberately does not
    ///
    /// The signature is `(name, kind, enclosing scope **kinds**)`. It notably
    /// does **not** include the names of the enclosing declarations, and that
    /// omission is load-bearing: if one branch renames the method `m` to `n`,
    /// the local `x` inside it is still the same local, and a signature that
    /// embedded `m` would call it a different declaration and report a
    /// spurious capture.
    ///
    /// The price is that two locals called `x` in two sibling blocks of the
    /// same method have the same signature and are treated as the same
    /// declaration. That direction is safe: it can only *suppress* a report,
    /// never invent one (SPEC.md §0.4).
    #[must_use]
    pub fn signature(&self, id: DeclId) -> DeclSignature {
        let decl = &self.decls[id.0 as usize];
        let mut path = Vec::new();
        let mut scope = Some(decl.scope);
        while let Some(s) = scope {
            path.push(self.scopes[s.0 as usize].kind);
            scope = self.scopes[s.0 as usize].parent;
        }
        path.reverse();
        DeclSignature {
            name: decl.name.clone(),
            kind: decl.kind,
            scope_path: path,
        }
    }
}

// ---------------------------------------------------------------- the builder

enum Step {
    /// Visit a node of the merge plan.
    Merged(MergedId),
    /// Visit a node of one revision's arena, under a merged anchor.
    Source(Side, NodeId, Option<MergedId>),
    /// Leave a node: pop its scope and/or close a declaration's visibility.
    Close {
        pop_scope: bool,
        end_decl: Option<DeclId>,
    },
}

struct Builder<'a> {
    revs: Revisions<'a>,
    lang: &'a dyn Language,
    out: ScopeTree,
    steps: Vec<Step>,
    open: Vec<ScopeId>,
    order: u32,
    /// Declarations whose visibility starts once their name node has been
    /// visited, keyed by that name node. See [`Decl::visible_from`].
    pending: BTreeMap<(u8, NodeId), DeclId>,
}

impl<'a> Builder<'a> {
    fn new(revs: Revisions<'a>, lang: &'a dyn Language) -> Self {
        Self {
            revs,
            lang,
            out: ScopeTree {
                scopes: Vec::new(),
                decls: Vec::new(),
                refs: Vec::new(),
                by_node: BTreeMap::new(),
                member_visibility: lang.member_visibility(),
                conflict_regions: 0,
            },
            steps: Vec::new(),
            open: Vec::new(),
            order: 0,
            pending: BTreeMap::new(),
        }
    }

    fn push_source(&mut self, side: Side, node: NodeId, anchor: Option<MergedId>) {
        self.steps.push(Step::Source(side, node, anchor));
    }

    fn push_merged(&mut self, id: MergedId) {
        self.steps.push(Step::Merged(id));
    }

    /// Drive the walk for a single revision (no merge plan involved).
    fn run(&mut self) {
        while let Some(step) = self.steps.pop() {
            match step {
                Step::Source(side, node, anchor) => self.visit_source(side, node, anchor),
                Step::Close {
                    pop_scope,
                    end_decl,
                } => self.close(pop_scope, end_decl),
                Step::Merged(_) => unreachable!("no merge plan in a single-revision walk"),
            }
        }
    }

    /// Drive the walk over a merge plan.
    fn run_merged(&mut self, merged: &MergedTree) {
        while let Some(step) = self.steps.pop() {
            match step {
                Step::Merged(id) => self.visit_merged(merged, id),
                Step::Source(side, node, anchor) => self.visit_source(side, node, anchor),
                Step::Close {
                    pop_scope,
                    end_decl,
                } => self.close(pop_scope, end_decl),
            }
        }
    }

    fn visit_merged(&mut self, merged: &MergedTree, id: MergedId) {
        match merged.node(id) {
            // A splice is a promise that these bytes are that revision's
            // subtree verbatim, so the walk continues in that revision's arena.
            MergedNode::Splice { side, node } => self.push_source(*side, *node, Some(id)),
            MergedNode::Rebuilt {
                side,
                node,
                children,
            } => {
                let close = self.enter(*side, *node, Some(id));
                self.steps.push(close);
                for &child in children.iter().rev() {
                    self.push_merged(child);
                }
            }
            // Unresolved regions contribute no names. Skipping them is what
            // makes `check` safe to run on a conflicted merge.
            MergedNode::Conflict(_) => self.out.conflict_regions += 1,
        }
    }

    fn visit_source(&mut self, side: Side, node: NodeId, anchor: Option<MergedId>) {
        let close = self.enter(side, node, anchor);
        self.steps.push(close);
        let tree = self.revs.get(side);
        let children: Vec<NodeId> = tree.children(node).collect();
        for child in children.into_iter().rev() {
            self.push_source(side, child, anchor);
        }
    }

    /// Record everything one node contributes, and return the matching
    /// [`Step::Close`].
    fn enter(&mut self, side: Side, node: NodeId, anchor: Option<MergedId>) -> Step {
        let tree = self.revs.get(side);
        let at = NodeRef::new(side, node);
        self.order += 1;
        let order = self.order;

        let mut pop_scope = false;
        if self.lang.is_scope_introducing(tree.node(node).kind) {
            let parent = self.open.last().copied();
            let id = ScopeId(self.out.scopes.len() as u32);
            self.out.scopes.push(Scope {
                kind: tree.node(node).kind,
                node: at,
                parent,
                decls: Vec::new(),
                function_scope: self.lang.is_function_scope(tree.node(node).kind),
            });
            self.open.push(id);
            pop_scope = true;
        }

        self.record_declaration(side, node);
        // A declaration whose *name* this node is becomes visible from here on.
        let end_decl = self.pending.remove(&at.key());
        self.record_reference(side, node, order, anchor);

        Step::Close {
            pop_scope,
            end_decl,
        }
    }

    /// If this node is a declaration with a usable simple name, register it.
    fn record_declaration(&mut self, side: Side, node: NodeId) -> Option<()> {
        let tree = self.revs.get(side);
        let kind = self.lang.declaration_kind(tree, node)?;
        let name_node = self.lang.declared_name(tree, node)?;
        // A "name" that is a destructuring pattern rather than an identifier is
        // not a declaration this layer can use; its leaves register themselves.
        if !matches!(
            self.lang.identifier_role(tree, name_node),
            IdentifierRole::Declaration(_)
        ) {
            return None;
        }
        // The file scope always exists (`program` is scope-introducing in both
        // languages); a grammar that did not open one would have nowhere to put
        // this, and dropping it is the conservative answer.
        let innermost = *self.open.last()?;
        let scope = self.declaration_scope(kind, innermost, side, node);

        let id = DeclId(self.out.decls.len() as u32);
        self.out.decls.push(Decl {
            name: tree.node_bytes(name_node).to_vec(),
            kind,
            name_node: NodeRef::new(side, name_node),
            decl_node: NodeRef::new(side, node),
            scope,
            // `u32::MAX` until the name node is reached. If it somehow never
            // is, the name simply never resolves, which is the safe direction.
            visible_from: if kind.is_hoisted() { 0 } else { u32::MAX },
        });
        self.out.scopes[scope.0 as usize].decls.push(id);
        if !kind.is_hoisted() {
            self.pending.insert(NodeRef::new(side, name_node).key(), id);
        }
        Some(())
    }

    /// Which scope a declaration's name lands in.
    fn declaration_scope(
        &self,
        kind: DeclKind,
        innermost: ScopeId,
        side: Side,
        node: NodeId,
    ) -> ScopeId {
        let scopes = &self.out.scopes;
        // A self-named declaration that opened its own scope (`class C { … }`,
        // `void m() { … }`) puts `C` and `m` in the scope *around* it. A
        // declaration that opened a scope but names something inside it
        // (`catch (E e)`) does not.
        let base = if kind.is_self_named()
            && scopes[innermost.0 as usize].node.node == node
            && scopes[innermost.0 as usize].node.side == side
        {
            scopes[innermost.0 as usize].parent.unwrap_or(innermost)
        } else {
            innermost
        };
        if kind != DeclKind::Var {
            return base;
        }
        // `var` hoists past every block to the nearest function scope.
        let mut cur = base;
        loop {
            if scopes[cur.0 as usize].function_scope {
                return cur;
            }
            match scopes[cur.0 as usize].parent {
                Some(p) => cur = p,
                None => return cur,
            }
        }
    }

    /// If this node is a lexically-resolved reference, register it.
    fn record_reference(&mut self, side: Side, node: NodeId, order: u32, anchor: Option<MergedId>) {
        let tree = self.revs.get(side);
        let Some(role) = RefRole::from_role(self.lang.identifier_role(tree, node)) else {
            return;
        };
        let Some(&scope) = self.open.last() else {
            return;
        };
        let at = NodeRef::new(side, node);
        let id = RefId(self.out.refs.len() as u32);
        self.out.refs.push(Reference {
            node: at,
            name: tree.node_bytes(node).to_vec(),
            role,
            scope,
            order,
            anchor,
        });
        self.out.by_node.insert(at.key(), id);
    }

    fn close(&mut self, pop_scope: bool, end_decl: Option<DeclId>) {
        if let Some(d) = end_decl {
            // The walk has just finished the name node; the binding is live
            // from the next position onwards.
            self.out.decls[d.0 as usize].visible_from = self.order + 1;
        }
        if pop_scope {
            self.open.pop();
        }
    }

    fn finish(self) -> ScopeTree {
        self.out
    }
}
