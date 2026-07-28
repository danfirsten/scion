//! Per-revision side tables: extents, trivia-aware hashes and content items.
//!
//! Everything here is a `Vec` indexed by [`NodeId`], the pattern `sm-cst`'s
//! dense preorder IDs are built for.
//!
//! # Extents: a node plus the comments that belong to it
//!
//! `sm-cst`'s trivia pass attaches each comment to a *sibling*, which means a
//! node's Javadoc lives **outside** its byte range. The merge never wants the
//! byte range on its own: if a method moves, its Javadoc must move with it. So
//! every decision in this crate is expressed over the node's **extent** — its
//! byte range widened to cover its attached leading and trailing comments.
//!
//! # Two hashes, because "unchanged" has to include comments
//!
//! [`sm_match::TreeMetrics::hash`] is deliberately blind to comments and
//! whitespace: that is what lets a reindented, re-commented method still match.
//! It is exactly the wrong test for "did this side change anything", though —
//! under it, rewriting a comment is a no-op, and a merge that concluded
//! "ours is unchanged, take theirs wholesale" would silently drop the rewrite.
//!
//! So this module computes a second hash, [`SideTables::full_hash`], that folds
//! in every comment in the subtree *and* the node's own attached comments. The
//! two are used for different questions:
//!
//! | question | test |
//! |---|---|
//! | is this side byte-for-byte unchanged? | extent bytes equal |
//! | did this side change anything a human would see? | `full_hash` equal |
//! | did the two sides converge on the same code? | `TreeMetrics::hash` equal, confirmed by `structurally_equal` |
//!
//! Comment text is compared **trimmed** of surrounding ASCII whitespace, so
//! reindenting a comment is a formatting change rather than a content change —
//! consistent with the treatment of code.

use std::ops::Range;

use sm_cst::{Language, NodeId, SourceTree};
use sm_match::TreeMetrics;

use crate::layout;

/// One entry of a container's content sequence.
///
/// This is [`sm_match::ContentItem`] with the node ID kept for the anonymous
/// tokens too, because the emitter has to splice their bytes and a `kind_id`
/// cannot be spliced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Item {
    /// The child node.
    pub node: NodeId,
    /// Whether this child participates in matching — i.e. whether it is an
    /// *element* rather than an anonymous token.
    pub is_element: bool,
    /// The child's extent: its bytes plus its attached comments, clamped so
    /// that the extents of one container's items never overlap.
    pub extent: Range<u32>,
}

/// Splitmix64's finalizer. Same choice, same reason, as `sm-match`: a stable
/// hash, not one that changes under a toolchain upgrade.
#[inline]
const fn mix(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    x
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
const TRIVIA_SALT: u64 = 0x2545_f491_4f6c_dd1d;

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = FNV_OFFSET;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

fn trim(bytes: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = bytes.len();
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    &bytes[start..end]
}

/// Derived tables for one of the three revisions.
pub(crate) struct SideTables<'a> {
    pub tree: &'a SourceTree,
    pub lang: &'a dyn Language,
    pub metrics: TreeMetrics,
    extent: Vec<Range<u32>>,
    full: Vec<u64>,
    own_trivia: Vec<u64>,
}

impl<'a> SideTables<'a> {
    pub fn build(tree: &'a SourceTree, lang: &'a dyn Language) -> Self {
        let metrics = TreeMetrics::compute(tree, lang);
        Self::with_metrics(tree, metrics, lang)
    }

    pub fn with_metrics(
        tree: &'a SourceTree,
        metrics: TreeMetrics,
        lang: &'a dyn Language,
    ) -> Self {
        let n = tree.len();
        let is_comment: Vec<bool> = tree
            .ids()
            .map(|id| layout::is_trivia(tree, lang, id))
            .collect();

        // Extents come from `layout`, which `sm-emit` also uses: one definition
        // of where a node begins and ends, so the two crates cannot drift.
        let mut extent: Vec<Range<u32>> = tree.ids().map(|id| layout::extent(tree, id)).collect();
        // The root's extent is the whole file, whatever tree-sitter reports for
        // it. That is what makes "one side is byte-for-byte unchanged" an exact
        // test at the top level, which is SPEC.md §4.6's invariant.
        if n > 0 {
            extent[0] = layout::outer_extent(tree, NodeId::ROOT, true);
        }

        // Trivia hashes. `sub` covers the comments strictly inside a subtree;
        // `own` adds the comments attached to the node itself, which live
        // outside it. One reverse scan, because `parent < child`.
        let mut sub = vec![0u64; n];
        for i in (0..n).rev() {
            let mut h = TRIVIA_SALT;
            for &c in &tree.node(NodeId(i as u32)).children {
                let contribution = if is_comment[c.index()] {
                    hash_bytes(trim(tree.node_bytes(c)))
                } else {
                    sub[c.index()]
                };
                h = mix(h.rotate_left(7) ^ contribution);
            }
            sub[i] = h;
        }

        let mut own_trivia = vec![0u64; n];
        let mut full = vec![0u64; n];
        for id in tree.ids() {
            let node = tree.node(id);
            let mut h = TRIVIA_SALT;
            for &c in &node.leading_trivia {
                h = mix(h.rotate_left(11) ^ hash_bytes(trim(tree.node_bytes(c))));
            }
            h = mix(h.rotate_left(3) ^ 1);
            for &c in &node.trailing_trivia {
                h = mix(h.rotate_left(11) ^ hash_bytes(trim(tree.node_bytes(c))));
            }
            own_trivia[id.index()] = h;
            full[id.index()] = mix(metrics
                .hash(id)
                .rotate_left(17)
                .wrapping_add(mix(h ^ sub[id.index()])));
        }

        Self {
            tree,
            lang,
            metrics,
            extent,
            full,
            own_trivia,
        }
    }

    /// The node's byte range widened over its attached comments.
    pub fn extent(&self, id: NodeId) -> Range<u32> {
        self.extent[id.index()].clone()
    }

    /// The extent's bytes.
    pub fn extent_bytes(&self, id: NodeId) -> &[u8] {
        let r = &self.extent[id.index()];
        &self.tree.source()[r.start as usize..r.end as usize]
    }

    /// Content hash *plus* every comment in the subtree and on the node itself.
    pub fn full_hash(&self, id: NodeId) -> u64 {
        self.full[id.index()]
    }

    /// Hash of just this node's own attached comments.
    pub fn own_trivia_hash(&self, id: NodeId) -> u64 {
        self.own_trivia[id.index()]
    }

    /// The container's content sequence: every child except comments and
    /// zero-width `MISSING` repairs, in source order, with extents clamped to
    /// be non-overlapping and ascending.
    ///
    /// Anonymous tokens are **kept**. That is the whole trick that lets one
    /// sequence merge handle both `{ a; b; }` → `{ a; b; c; }` (an element
    /// inserted, separators unchanged) and `x = 1` → `x += 1` (an element list
    /// unchanged, a token replaced) — and refuse the second when both sides do
    /// it differently, instead of silently splicing one side's operator.
    pub fn items(&self, container: NodeId) -> Vec<Item> {
        layout::content_items(self.tree, self.lang, container)
            .into_iter()
            .map(|(node, extent)| Item {
                node,
                is_element: self.tree.node(node).is_named,
                extent,
            })
            .collect()
    }

    /// The gap that precedes `items[idx]`: from the previous item's extent end,
    /// or from the container's first byte for the first item.
    pub fn lead(&self, container: NodeId, items: &[Item], idx: usize) -> Range<u32> {
        let start = if idx == 0 {
            self.tree.node(container).byte_range.start
        } else {
            items[idx - 1].extent.end
        };
        let end = items[idx].extent.start.max(start);
        start..end
    }
}
