//! Per-node side tables the matcher needs: participation, the projected
//! "matchable tree", structural hashes, heights and descendant counts.
//!
//! Everything here is a `Vec` indexed by [`NodeId`], which is the pattern
//! `sm-cst` is designed for: IDs are dense preorder `u32`s, `parent < child`,
//! and a node's descendants occupy a contiguous ID range (PROGRESS.md decision
//! 6). Nothing in this module mutates the tree.
//!
//! # What participates in matching
//!
//! A node *participates* iff it is **named**, **not a comment** and **not
//! `MISSING`**. Three separate decisions, each with a reason:
//!
//! - **Anonymous tokens do not participate.** `{`, `;`, `public`, `,` carry no
//!   identity a structural match could follow: their text is entirely
//!   determined by the parent's kind and their position in it. Matching them
//!   would add `O(n)` pairs that say nothing, and it would let a spurious
//!   `{`↔`{` pair contribute to a dice score. `sm-cst` already made exactly
//!   this argument for trivia ownership (PROGRESS.md decision 19). The emitter
//!   recovers punctuation from the matched *parent*, not from a token pair.
//!   **They are still part of a node's structural identity**, though — see
//!   [`ContentItem`], which is where the `a + b` versus `a - b` problem is
//!   solved. "Not matchable" and "not significant" are different claims and
//!   only the first one is being made here.
//! - **Comments do not participate.** A comment's owner is decided by the
//!   trivia attachment pass, and it travels with that owner. If comments were
//!   hashed into their parent, adding a line comment inside a method body would
//!   destroy the subtree isomorphism of a method that is otherwise byte
//!   identical — turning "one comment added" into "the whole method is
//!   unmatched", which is precisely the false conflict this project exists to
//!   remove. Excluding them costs nothing, because
//!   [`sm_cst::Node::leading_trivia`] / [`sm_cst::Node::trailing_trivia`]
//!   already tell M3/M4 which comments ride along with a matched node.
//!   "Comment" means `is_extra || Language::is_comment(kind)` — the same
//!   predicate the trivia pass uses, so the two layers cannot disagree.
//! - **`MISSING` nodes do not participate.** They are zero-width repairs
//!   synthesised by error recovery; there are no bytes for the emitter to
//!   splice and no text for a hash to fold in.
//!
//! `ERROR` nodes *do* participate. They are named, they span real bytes, and an
//! `ERROR`↔`ERROR` match is the honest description of two files that failed to
//! parse the same way. (M4 falls back to a line merge on
//! [`sm_cst::SourceTree::has_errors`] anyway, so this is belt and braces.)
//!
//! Non-participating nodes are **transparent**, not opaque: the projected tree
//! splices their participating children into their grandparent's child list.
//! In Java every non-participating node is a leaf, so the projection is a plain
//! filter; the recursion exists so the model stays correct for a grammar where
//! it is not.

use sm_cst::{Language, NodeId, SourceTree};

/// FNV-1a offset basis, for hashing leaf text.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
/// Golden-ratio constant, used to keep the "no text" and "empty child list"
/// cases from hashing to the same value as a genuine zero.
const GOLDEN: u64 = 0x9e37_79b9_7f4a_7c15;
/// Separates an anonymous token's contribution to a hash from a child node's,
/// so that a token can never collide with a subtree by arithmetic accident.
const TOKEN_SALT: u64 = 0xa076_1d64_78bd_642f;

/// One element of a node's **content sequence** — what its structural identity
/// is made of.
///
/// This is the crate's answer to a question GumTree does not have to ask,
/// because its tree generators drop anonymous tokens before matching begins.
/// We cannot: `a + b` and `a - b` are both
/// `binary_expression(identifier, identifier)`, and the operator that tells
/// them apart is an *anonymous token*. So are `x++` versus `x--`, `=` versus
/// `+=`, and `public` versus `public final` in a `modifiers` node. A matcher
/// that called those pairs isomorphic would hand M4 a silently wrong merge —
/// the one outcome SPEC.md §0.4 rules out categorically.
///
/// So a node's content is the ordered sequence of its non-comment children,
/// where a participating child contributes its whole subtree and an anonymous
/// token contributes **only its kind**. Kind, not text: an anonymous token's
/// text is a function of its kind, so this loses nothing — and it keeps the
/// *whitespace between* tokens out of the hash, which matters because
/// reindentation must never break a match (SPEC.md §1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ContentItem {
    /// A participating child; its structural hash carries its whole subtree.
    Node(NodeId),
    /// An anonymous token, identified by its grammar kind.
    Token(u16),
}

impl ContentItem {
    /// The node ID, for a [`ContentItem::Node`].
    #[must_use]
    pub fn node(&self) -> Option<NodeId> {
        match *self {
            Self::Node(id) => Some(id),
            Self::Token(_) => None,
        }
    }
}

/// The finalizer from `splitmix64`.
///
/// Chosen over `std::hash::DefaultHasher` deliberately: `DefaultHasher`'s
/// output is explicitly not guaranteed stable across Rust releases, and a
/// structural hash that changes under a toolchain upgrade would silently change
/// which subtrees the matcher considers isomorphic.
#[inline]
#[must_use]
const fn mix(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    x
}

/// FNV-1a over a byte slice.
#[inline]
#[must_use]
fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = FNV_OFFSET;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// The side tables for one tree.
///
/// Build once per tree per matching run with [`TreeMetrics::compute`]. Nothing
/// here depends on the *other* tree, so a caller matching one base against two
/// sides (the merge case) can build the base's metrics once and reuse them.
#[derive(Clone, Debug)]
pub struct TreeMetrics {
    participates: Vec<bool>,
    hash: Vec<u64>,
    height: Vec<u32>,
    /// Number of *participating* strict descendants.
    pdesc: Vec<u32>,
    /// One past the largest node ID in this node's subtree, counting every node
    /// (participating or not). Preorder numbering makes the subtree a
    /// contiguous ID range, so `a` is a strict descendant of `b` iff
    /// `b < a < subtree_end[b]`.
    subtree_end: Vec<u32>,
    /// CSR offsets into `pchild_flat`; length `len + 1`.
    pchild_start: Vec<u32>,
    pchild_flat: Vec<NodeId>,
    /// CSR offsets into `content_flat`; length `len + 1`.
    content_start: Vec<u32>,
    content_flat: Vec<ContentItem>,
    /// Nearest participating strict ancestor.
    pparent: Vec<Option<NodeId>>,
    /// Index of this node in its participating parent's projected child list.
    index_in_pparent: Vec<u32>,
    /// Participating nodes in post-order (children before parents, siblings
    /// left to right) — the order GumTree's bottom-up phase requires.
    post_order: Vec<NodeId>,
    /// The topmost participating node, i.e. the root of the projected tree.
    root: Option<NodeId>,
}

impl TreeMetrics {
    /// Compute every side table in a handful of linear scans.
    #[must_use]
    pub fn compute(tree: &SourceTree, lang: &dyn Language) -> Self {
        let n = tree.len();

        // 1. Participation. Independent per node.
        let participates: Vec<bool> = tree
            .nodes()
            .map(|node| {
                node.is_named && !node.is_missing && !node.is_extra && !lang.is_comment(node.kind)
            })
            .collect();

        // 2. Subtree extent, over *all* nodes. Reverse scan: children have
        //    larger IDs than their parent, so they are already done.
        let mut subtree_end = vec![0u32; n];
        for i in (0..n).rev() {
            let mut end = i as u32 + 1;
            for &c in &tree.node(NodeId(i as u32)).children {
                end = end.max(subtree_end[c.index()]);
            }
            subtree_end[i] = end;
        }

        // 3. The content sequences, and the projected child lists derived
        //    from them. Reverse scan, because a transparent child contributes
        //    *its* content here.
        let mut content: Vec<Vec<ContentItem>> = vec![Vec::new(); n];
        for i in (0..n).rev() {
            let mut list: Vec<ContentItem> = Vec::new();
            for &c in &tree.node(NodeId(i as u32)).children {
                let child = tree.node(c);
                if child.is_missing || child.is_extra || lang.is_comment(child.kind) {
                    continue;
                }
                if participates[c.index()] {
                    list.push(ContentItem::Node(c));
                } else if child.children.is_empty() {
                    list.push(ContentItem::Token(child.kind_id));
                } else {
                    // A transparent container. tree-sitter never produces one
                    // in practice (anonymous nodes are always leaves), but
                    // splicing keeps the model total.
                    let inherited = std::mem::take(&mut content[c.index()]);
                    list.extend_from_slice(&inherited);
                    content[c.index()] = inherited;
                }
            }
            content[i] = list;
        }

        let mut content_start = Vec::with_capacity(n + 1);
        let mut content_flat: Vec<ContentItem> = Vec::new();
        let mut pchild_start = Vec::with_capacity(n + 1);
        let mut pchild_flat: Vec<NodeId> = Vec::new();
        for list in &content {
            content_start.push(content_flat.len() as u32);
            pchild_start.push(pchild_flat.len() as u32);
            content_flat.extend_from_slice(list);
            pchild_flat.extend(list.iter().filter_map(ContentItem::node));
        }
        content_start.push(content_flat.len() as u32);
        pchild_start.push(pchild_flat.len() as u32);
        drop(content);

        let span = |start: &[u32], i: usize| -> std::ops::Range<usize> {
            start[i] as usize..start[i + 1] as usize
        };

        // 4. Height, participating-descendant count and structural hash. One
        //    reverse scan; each depends only on what is already computed for
        //    the children.
        let mut height = vec![1u32; n];
        let mut pdesc = vec![0u32; n];
        let mut hash = vec![0u64; n];
        for i in (0..n).rev() {
            let kids = &pchild_flat[span(&pchild_start, i)];
            let items = &content_flat[span(&content_start, i)];
            let node = tree.node(NodeId(i as u32));

            let mut max_child_height = 0;
            let mut descendants = 0u32;
            for &c in kids {
                max_child_height = max_child_height.max(height[c.index()]);
                descendants = descendants
                    .saturating_add(1)
                    .saturating_add(pdesc[c.index()]);
            }
            height[i] = max_child_height + 1;
            pdesc[i] = descendants;

            // kind, then leaf text, then arity, then the content in order.
            let mut h = mix(u64::from(node.kind_id) ^ GOLDEN);
            if kids.is_empty() && lang.significant_text(node.kind) {
                h = mix(h.rotate_left(11) ^ hash_bytes(tree.node_bytes(NodeId(i as u32))));
            }
            h = mix(h.rotate_left(5) ^ (items.len() as u64).wrapping_add(GOLDEN));
            for item in items {
                let contribution = match *item {
                    ContentItem::Node(c) => hash[c.index()],
                    ContentItem::Token(kind_id) => mix(u64::from(kind_id) ^ TOKEN_SALT),
                };
                h = mix(h.rotate_left(7) ^ contribution);
            }
            hash[i] = h;
        }

        // 5. Nearest participating ancestor. Forward scan: `parent < child`.
        let mut pparent: Vec<Option<NodeId>> = vec![None; n];
        for i in 0..n {
            let id = NodeId(i as u32);
            let inherited = if participates[i] {
                Some(id)
            } else {
                pparent[i]
            };
            for &c in &tree.node(id).children {
                pparent[c.index()] = inherited;
            }
        }

        // 6. Index within the projected parent.
        let mut index_in_pparent = vec![0u32; n];
        for (i, _) in participates.iter().enumerate().filter(|&(_, &p)| p) {
            let range = span(&pchild_start, i);
            for (k, &c) in pchild_flat[range].iter().enumerate() {
                index_in_pparent[c.index()] = k as u32;
            }
        }

        // 7. Post-order over the projected tree.
        let root = (n > 0 && participates[0]).then_some(NodeId::ROOT);
        let mut post_order = Vec::with_capacity(n);
        if let Some(r) = root {
            // `(node, next child to visit)`.
            let mut stack: Vec<(NodeId, usize)> = vec![(r, 0)];
            while let Some((id, k)) = stack.pop() {
                let range = span(&pchild_start, id.index());
                let kids = &pchild_flat[range];
                if k < kids.len() {
                    let child = kids[k];
                    stack.push((id, k + 1));
                    stack.push((child, 0));
                } else {
                    post_order.push(id);
                }
            }
        }

        Self {
            participates,
            hash,
            height,
            pdesc,
            subtree_end,
            pchild_start,
            pchild_flat,
            content_start,
            content_flat,
            pparent,
            index_in_pparent,
            post_order,
            root,
        }
    }

    /// Number of nodes in the underlying tree, participating or not.
    #[must_use]
    pub fn len(&self) -> usize {
        self.participates.len()
    }

    /// Always false for a tree from [`sm_cst::parse`] — even an empty file has
    /// a root node.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.participates.is_empty()
    }

    /// Whether this node takes part in matching. See the module docs.
    #[must_use]
    pub fn participates(&self, id: NodeId) -> bool {
        self.participates[id.index()]
    }

    /// Number of participating nodes.
    #[must_use]
    pub fn matchable_count(&self) -> usize {
        self.post_order.len()
    }

    /// The structural hash: kind, leaf text for
    /// [`Language::significant_text`] kinds, arity, and the children's hashes
    /// in order — over the *projected* tree, so comments and punctuation are
    /// not folded in.
    ///
    /// Equal hashes are a *candidate* isomorphism, never a conclusion:
    /// [`crate::structurally_equal`] confirms every top-down match.
    #[must_use]
    pub fn hash(&self, id: NodeId) -> u64 {
        self.hash[id.index()]
    }

    /// Height in the projected tree, **1-based**: a participating node with no
    /// participating children has height 1.
    ///
    /// This is the ASE'14 paper's convention, which is what
    /// [`crate::MatchConfig::min_height`] is expressed in: `min_height = 1`
    /// admits bare leaves to the top-down phase, `min_height = 2` excludes
    /// them. (GumTree's implementation uses a 0-based height and a
    /// `st_minprio` of 1 for the same policy — see docs/prior-art.md §7.)
    #[must_use]
    pub fn height(&self, id: NodeId) -> u32 {
        self.height[id.index()]
    }

    /// Number of participating strict descendants.
    #[must_use]
    pub fn descendant_count(&self, id: NodeId) -> u32 {
        self.pdesc[id.index()]
    }

    /// Number of participating nodes in this subtree, including `id` itself.
    #[must_use]
    pub fn subtree_size(&self, id: NodeId) -> u32 {
        self.pdesc[id.index()] + 1
    }

    /// Participating children, in source order.
    #[must_use]
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        let i = id.index();
        &self.pchild_flat[self.pchild_start[i] as usize..self.pchild_start[i + 1] as usize]
    }

    /// The node's content sequence: participating children interleaved with the
    /// anonymous tokens between them, comments dropped. See [`ContentItem`].
    #[must_use]
    pub fn content(&self, id: NodeId) -> &[ContentItem] {
        let i = id.index();
        &self.content_flat[self.content_start[i] as usize..self.content_start[i + 1] as usize]
    }

    /// Nearest participating strict ancestor, or `None` at the projected root.
    #[must_use]
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.pparent[id.index()]
    }

    /// Index of `id` among its participating parent's participating children.
    #[must_use]
    pub fn index_in_parent(&self, id: NodeId) -> u32 {
        self.index_in_pparent[id.index()]
    }

    /// Whether `descendant` lies strictly inside `id`'s subtree.
    ///
    /// `O(1)`: preorder numbering makes a subtree a contiguous ID range.
    #[must_use]
    pub fn contains(&self, id: NodeId, descendant: NodeId) -> bool {
        id.0 < descendant.0 && descendant.0 < self.subtree_end[id.index()]
    }

    /// The half-open ID range covering `id` and all its descendants.
    #[must_use]
    pub fn subtree_range(&self, id: NodeId) -> std::ops::Range<u32> {
        id.0..self.subtree_end[id.index()]
    }

    /// Participating nodes in post-order.
    #[must_use]
    pub fn post_order(&self) -> &[NodeId] {
        &self.post_order
    }

    /// The root of the projected tree.
    #[must_use]
    pub fn root(&self) -> Option<NodeId> {
        self.root
    }

    /// Walk from `id` up through participating ancestors, `id` excluded.
    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut cur = self.parent(id);
        std::iter::from_fn(move || {
            let next = cur?;
            cur = self.parent(next);
            Some(next)
        })
    }
}

/// Whether two subtrees are structurally identical over the projected tree.
///
/// This is the belt-and-braces check behind the structural hash. The hash is 64
/// bits over the whole subtree, so on a 100k-node file a birthday collision is
/// around `10^-9` — small, but a collision would not produce a wrong *diff*, it
/// would produce a wrong *merge*, and SPEC.md §0.4 says a wrong merge is never
/// an acceptable answer. The verification is `O(size)` and it runs only when
/// the hashes already agree, immediately before an `O(size)` pairwise mapping
/// walk we were going to do anyway; measurably it is free.
///
/// Equality means: same kind name, the same content sequence — matching
/// anonymous tokens in matching positions, participating children equal
/// recursively — and the same bytes for [`Language::significant_text`] leaves.
#[must_use]
pub fn structurally_equal(
    src: &SourceTree,
    src_metrics: &TreeMetrics,
    dst: &SourceTree,
    dst_metrics: &TreeMetrics,
    lang: &dyn Language,
    a: NodeId,
    b: NodeId,
) -> bool {
    let mut stack = vec![(a, b)];
    while let Some((x, y)) = stack.pop() {
        let nx = src.node(x);
        let ny = dst.node(y);
        if nx.kind != ny.kind {
            return false;
        }
        if src_metrics.children(x).is_empty()
            && lang.significant_text(nx.kind)
            && src.node_bytes(x) != dst.node_bytes(y)
        {
            return false;
        }

        let cx = src_metrics.content(x);
        let cy = dst_metrics.content(y);
        if cx.len() != cy.len() {
            return false;
        }
        for (&ix, &iy) in cx.iter().zip(cy.iter()) {
            match (ix, iy) {
                (ContentItem::Node(p), ContentItem::Node(q)) => stack.push((p, q)),
                // Kind IDs come from one grammar (`match_trees` requires both
                // trees to share a language), so comparing them is exact.
                (ContentItem::Token(t), ContentItem::Token(u)) if t == u => {}
                _ => return false,
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{TreeMetrics, structurally_equal};
    use sm_cst::{Language, NodeId, SourceTree};

    fn java() -> &'static dyn Language {
        sm_cst::languages::detect(std::path::Path::new("x.java")).expect("java")
    }

    fn parse(src: &str) -> (SourceTree, TreeMetrics) {
        let tree = sm_cst::parse(src.as_bytes(), java()).expect("parse");
        assert!(!tree.has_errors(), "test snippet must be valid Java: {src}");
        let metrics = TreeMetrics::compute(&tree, java());
        (tree, metrics)
    }

    fn root_hash(src: &str) -> u64 {
        let (_, metrics) = parse(src);
        metrics.hash(NodeId::ROOT)
    }

    fn class(body: &str) -> String {
        format!("class C {{ {body} }}")
    }

    #[test]
    fn identical_sources_hash_identically() {
        assert_eq!(
            root_hash(&class("int f() { return 1 + 2; }")),
            root_hash(&class("int f() { return 1 + 2; }"))
        );
    }

    /// The case that motivates [`super::ContentItem`]: the operator is an
    /// anonymous token, so dropping anonymous tokens would make these two
    /// isomorphic and let a merge silently turn a `+` into a `-`.
    #[test]
    fn a_different_operator_is_a_different_structure() {
        assert_ne!(
            root_hash(&class("int f() { return 1 + 2; }")),
            root_hash(&class("int f() { return 1 - 2; }"))
        );
        assert_ne!(
            root_hash(&class("void f(int i) { i++; }")),
            root_hash(&class("void f(int i) { i--; }"))
        );
        assert_ne!(
            root_hash(&class("void f(int i) { i += 1; }")),
            root_hash(&class("void f(int i) { i -= 1; }"))
        );
    }

    /// `modifiers` holds nothing but anonymous tokens, so without them a
    /// `private` field and a `public` one would be indistinguishable.
    #[test]
    fn modifiers_are_part_of_the_structure() {
        assert_ne!(
            root_hash(&class("public int x;")),
            root_hash(&class("private int x;"))
        );
        assert_ne!(
            root_hash(&class("public int x;")),
            root_hash(&class("public static int x;"))
        );
    }

    #[test]
    fn identifier_and_literal_text_is_part_of_the_structure() {
        assert_ne!(root_hash(&class("int x;")), root_hash(&class("int y;")));
        assert_ne!(
            root_hash(&class("int x = 1;")),
            root_hash(&class("int x = 2;"))
        );
        assert_ne!(
            root_hash(&class("String s = \"a\";")),
            root_hash(&class("String s = \"b\";"))
        );
    }

    /// Reindentation must never break a match — SPEC.md §1 names the
    /// "wrap a block in `if`" case as a headline false conflict.
    #[test]
    fn whitespace_is_not_part_of_the_structure() {
        assert_eq!(
            root_hash("class C {\n  int f() {\n    return 1;\n  }\n}"),
            root_hash("class C {\n        int f() {\n                return 1;\n        }\n}")
        );
        assert_eq!(
            root_hash("class C { int x ; }"),
            root_hash("class C {int x;}")
        );
    }

    /// Comments must never break a match either: they ride along with their
    /// owner through `sm-cst`'s trivia attachment.
    #[test]
    fn comments_are_not_part_of_the_structure() {
        assert_eq!(
            root_hash(&class("int f() { return 1; }")),
            root_hash(&class("int f() { /* why */ return 1; // yes\n }"))
        );
    }

    #[test]
    fn structural_equality_agrees_with_the_hash() {
        let (a, am) = parse(&class("int f() { return 1 + 2; }"));
        let (b, bm) = parse(&class("int f() { return 1 + 2; }"));
        assert!(structurally_equal(
            &a,
            &am,
            &b,
            &bm,
            java(),
            NodeId::ROOT,
            NodeId::ROOT
        ));

        let (c, cm) = parse(&class("int f() { return 1 - 2; }"));
        assert!(!structurally_equal(
            &a,
            &am,
            &c,
            &cm,
            java(),
            NodeId::ROOT,
            NodeId::ROOT
        ));
    }

    #[test]
    fn heights_are_one_based_and_comments_do_not_raise_them() {
        let (tree, metrics) = parse("class C {}");
        // The root of a compilation unit holding one empty class: program >
        // class_declaration > identifier.
        assert_eq!(metrics.height(NodeId::ROOT), 3);
        assert!(metrics.matchable_count() < tree.len());

        let (_, with_comment) = parse("// hi\nclass C {}");
        assert_eq!(with_comment.height(NodeId::ROOT), 3);
    }

    #[test]
    fn descendants_are_a_contiguous_id_range() {
        let (tree, metrics) = parse(&class("int f() { return 1; }"));
        for id in tree.ids() {
            for other in tree.ids() {
                let by_range = metrics.contains(id, other);
                let by_walk = tree.descendants(id).any(|d| d == other);
                assert_eq!(by_range, by_walk, "{id} contains {other}?");
            }
        }
    }

    #[test]
    fn post_order_puts_children_before_parents() {
        let (_, metrics) = parse(&class("int f() { return 1; }"));
        let mut seen = std::collections::BTreeSet::new();
        for &id in metrics.post_order() {
            for &child in metrics.children(id) {
                assert!(seen.contains(&child), "{child} came after its parent {id}");
            }
            seen.insert(id);
        }
        assert_eq!(seen.len(), metrics.matchable_count());
    }

    #[test]
    fn an_empty_file_still_has_a_participating_root() {
        let (_, metrics) = parse("");
        assert_eq!(metrics.root(), Some(NodeId::ROOT));
        assert_eq!(metrics.matchable_count(), 1);
        assert_eq!(metrics.height(NodeId::ROOT), 1);
        assert_eq!(metrics.descendant_count(NodeId::ROOT), 0);
    }
}
