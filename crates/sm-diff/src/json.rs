//! The `sm diff --json` wire format.
//!
//! Hand-written DTOs rather than `derive(Serialize)` on [`EditOp`], for the
//! reason `sm-cst` gives for `JsonTree` (PROGRESS.md decision 15): the format
//! other tools consume should be a deliberate decision, not a side effect of an
//! internal refactor. Versioned with [`DiffReport::SCHEMA_VERSION`].
//!
//! Each operation carries both the node id — the stable handle M4/M5/M6 work
//! with — and the byte range and line span, so a consumer that only wants to
//! point at source never has to load the tree.

use serde::Serialize;
use sm_cst::{NodeId, SourceTree};

use crate::lines::LineIndex;
use crate::render::{DiffView, Side};
use crate::script::{EditOp, Summary};

/// Where one node is, in every coordinate system a consumer might want.
#[derive(Clone, Debug, Serialize)]
pub struct NodeRef {
    pub id: u32,
    pub kind: String,
    pub start_byte: u32,
    pub end_byte: u32,
    pub start_line: usize,
    pub end_line: usize,
}

impl NodeRef {
    fn new(tree: &SourceTree, index: &LineIndex, id: NodeId) -> Self {
        let node = tree.node(id);
        let lines = index.lines_of(node.byte_range.clone());
        Self {
            id: id.0,
            kind: node.kind.to_owned(),
            start_byte: node.byte_range.start,
            end_byte: node.byte_range.end,
            start_line: *lines.start(),
            end_line: *lines.end(),
        }
    }
}

/// One operation, flattened for the wire.
#[derive(Clone, Debug, Serialize)]
pub struct OpJson {
    /// `insert`, `delete`, `update` or `move`.
    pub op: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub src: Option<NodeRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dst: Option<NodeRef>,
    /// `reparent` or `reorder`; only on a `move`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub move_kind: Option<&'static str>,
    /// The inserted node's parent in the destination tree; only on an `insert`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dst_parent: Option<u32>,
    /// Index among that parent's participating children; only on an `insert`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<usize>,
}

/// What each file is.
#[derive(Clone, Debug, Serialize)]
pub struct FileInfo {
    pub path: String,
    pub language: String,
    pub nodes: usize,
    pub lines: usize,
    pub has_errors: bool,
}

impl FileInfo {
    fn new(side: Side<'_>, index: &LineIndex) -> Self {
        Self {
            path: side.path.to_owned(),
            language: side.tree.language_name().to_owned(),
            nodes: side.tree.len(),
            lines: index.line_count(),
            has_errors: side.tree.has_errors(),
        }
    }
}

/// The `--json` payload.
#[derive(Clone, Debug, Serialize)]
pub struct DiffReport {
    pub schema_version: u32,
    pub src: FileInfo,
    pub dst: FileInfo,
    pub summary: Summary,
    pub ops: Vec<OpJson>,
}

impl DiffReport {
    /// Current `schema_version`. Bump on any incompatible change.
    pub const SCHEMA_VERSION: u32 = 1;

    #[must_use]
    pub fn new(view: &DiffView<'_>) -> Self {
        let src_lines = LineIndex::new(view.src.tree.source());
        let dst_lines = LineIndex::new(view.dst.tree.source());
        let a = |id: NodeId| NodeRef::new(view.src.tree, &src_lines, id);
        let b = |id: NodeId| NodeRef::new(view.dst.tree, &dst_lines, id);

        let ops = view
            .script
            .ops
            .iter()
            .map(|op| OpJson {
                op: op.name(),
                src: op.src().map(a),
                dst: op.dst().map(b),
                move_kind: match op {
                    EditOp::Move { kind, .. } => Some(kind.as_str()),
                    _ => None,
                },
                dst_parent: match op {
                    EditOp::Insert { dst_parent, .. } => dst_parent.map(|p| p.0),
                    _ => None,
                },
                position: match op {
                    EditOp::Insert { position, .. } => Some(*position),
                    _ => None,
                },
            })
            .collect();

        Self {
            schema_version: Self::SCHEMA_VERSION,
            src: FileInfo::new(view.src, &src_lines),
            dst: FileInfo::new(view.dst, &dst_lines),
            summary: view.script.summary,
            ops,
        }
    }
}
