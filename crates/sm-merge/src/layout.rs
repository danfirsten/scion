//! Byte-level layout of a node: its extent, its content items, and the frame a
//! rebuilt container keeps around them.
//!
//! This is public because `sm-emit` must agree with the merge, byte for byte,
//! about where a node starts and stops. Two independent implementations of
//! "where does this node's Javadoc begin" would eventually disagree, and the
//! symptom would be a duplicated or dropped comment in merged output. There is
//! one definition, here, and both crates call it.
//!
//! The three concepts, and how they tile the source:
//!
//! ```text
//!            extent(container)
//! ┌──────────────────────────────────────────────────┐
//! │ head │ lead₀ │ item₀ │ lead₁ │ item₁ │ ... │ tail │
//! └──────────────────────────────────────────────────┘
//! ```
//!
//! - `head` runs from the container's extent start to the container's own first
//!   byte, so it is exactly the comments attached in front of it.
//! - `leadᵢ` runs from the previous item's extent end (or the container's first
//!   byte, for `lead₀`) to item `i`'s extent start. It holds the whitespace
//!   between siblings and any comment the trivia pass left floating.
//! - `itemᵢ` is a child's extent: the child plus the comments attached to it.
//! - `tail` runs from the last item's extent end to the container's extent end.
//!
//! Every byte of the container is in exactly one of those, which is what makes
//! "recompose the child list" a splice rather than a reprint.

use std::ops::Range;

use sm_cst::{Language, NodeId, SourceTree};

/// Whether this node is a comment — an `extra`, or a kind the language calls a
/// comment.
#[must_use]
pub fn is_trivia(tree: &SourceTree, lang: &dyn Language, id: NodeId) -> bool {
    let node = tree.node(id);
    node.is_extra || lang.is_comment(node.kind)
}

/// A node's byte range widened over the comments attached to it.
///
/// `sm-cst`'s trivia pass attaches a comment to a *sibling*, so a method's
/// Javadoc lies outside the method's byte range. Every decision in the merge
/// and every splice in the emitter is expressed over the extent instead, which
/// is what makes a comment travel with the declaration it documents.
#[must_use]
pub fn extent(tree: &SourceTree, id: NodeId) -> Range<u32> {
    let node = tree.node(id);
    let mut start = node.byte_range.start;
    let mut end = node.byte_range.end;
    for &c in node.leading_trivia.iter().chain(&node.trailing_trivia) {
        let r = &tree.node(c).byte_range;
        start = start.min(r.start);
        end = end.max(r.end);
    }
    start..end
}

/// The extent to use for a node emitted as a whole file.
///
/// tree-sitter's root node is not guaranteed to cover leading or trailing
/// whitespace, and SPEC.md §4.6's byte-identity invariant is stated over the
/// *file*. So the root's extent is the file.
#[must_use]
pub fn outer_extent(tree: &SourceTree, id: NodeId, whole_file: bool) -> Range<u32> {
    if whole_file {
        0..tree.source().len() as u32
    } else {
        extent(tree, id)
    }
}

/// A container's content items: every child except comments and zero-width
/// `MISSING` repairs, in source order, with extents clamped so that they never
/// overlap.
///
/// Anonymous tokens are kept. See the crate docs, §1.
#[must_use]
pub fn content_items(
    tree: &SourceTree,
    lang: &dyn Language,
    container: NodeId,
) -> Vec<(NodeId, Range<u32>)> {
    let mut out = Vec::new();
    let mut floor = tree.node(container).byte_range.start;
    for child in tree.children(container) {
        if tree.node(child).is_missing || is_trivia(tree, lang, child) {
            continue;
        }
        let raw = extent(tree, child);
        let start = raw.start.max(floor);
        let end = raw.end.max(start);
        floor = end;
        out.push((child, start..end));
    }
    out
}

/// The `(head, tail)` a rebuilt container keeps from its own revision.
///
/// A container with no content items has no place to put children, so the whole
/// extent is head and the tail is empty; the merge never rebuilds such a node.
#[must_use]
pub fn frame(
    tree: &SourceTree,
    lang: &dyn Language,
    container: NodeId,
    whole_file: bool,
) -> (Range<u32>, Range<u32>) {
    let outer = outer_extent(tree, container, whole_file);
    let items = content_items(tree, lang, container);
    let Some((_, last)) = items.last() else {
        return (outer.clone(), outer.end..outer.end);
    };
    let head_end = tree.node(container).byte_range.start.max(outer.start);
    let tail_start = last.end.clamp(head_end, outer.end);
    (outer.start..head_end, tail_start..outer.end)
}

#[cfg(test)]
mod tests {
    use super::{content_items, extent, frame};
    use sm_cst::{Language, NodeId};

    fn java() -> &'static dyn Language {
        sm_cst::languages::detect(std::path::Path::new("x.java")).expect("java")
    }

    #[test]
    fn head_lead_items_and_tail_tile_the_container() {
        let src = "// hdr\nclass C {\n  int x;\n  // note\n  void f() {}\n}\n";
        let tree = sm_cst::parse(src.as_bytes(), java()).expect("parse");

        for id in tree.ids() {
            let items = content_items(&tree, java(), id);
            if items.is_empty() {
                continue;
            }
            let whole_file = id == NodeId::ROOT;
            let (head, tail) = frame(&tree, java(), id, whole_file);
            let outer = super::outer_extent(&tree, id, whole_file);

            // Reconstruct: head, then each lead + item, then tail.
            let mut rebuilt: Vec<u8> = Vec::new();
            rebuilt.extend_from_slice(&src.as_bytes()[head.start as usize..head.end as usize]);
            let mut cursor = head.end;
            for (_, item) in &items {
                rebuilt.extend_from_slice(&src.as_bytes()[cursor as usize..item.start as usize]);
                rebuilt.extend_from_slice(&src.as_bytes()[item.start as usize..item.end as usize]);
                cursor = item.end;
            }
            rebuilt.extend_from_slice(&src.as_bytes()[tail.start as usize..tail.end as usize]);

            assert_eq!(
                rebuilt,
                &src.as_bytes()[outer.start as usize..outer.end as usize],
                "container {id} ({}) did not tile",
                tree.node(id).kind
            );
        }
    }

    #[test]
    fn an_extent_covers_the_attached_comment() {
        let src = "class C {\n  // note\n  void f() {}\n}\n";
        let tree = sm_cst::parse(src.as_bytes(), java()).expect("parse");
        let method = tree
            .ids()
            .find(|&id| tree.node(id).kind == "method_declaration")
            .expect("method");
        let e = extent(&tree, method);
        assert_eq!(
            &src[e.start as usize..e.end as usize],
            "// note\n  void f() {}"
        );
    }

    #[test]
    fn the_root_extent_is_the_whole_file_including_trailing_blank_lines() {
        let src = "class C {}\n\n\n";
        let tree = sm_cst::parse(src.as_bytes(), java()).expect("parse");
        assert_eq!(
            super::outer_extent(&tree, NodeId::ROOT, true),
            0..src.len() as u32
        );
    }
}
