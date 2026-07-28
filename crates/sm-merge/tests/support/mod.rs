//! Shared helpers for `sm-merge`'s integration tests.

#![allow(dead_code)]

use std::path::Path;

use sm_cst::{Language, NodeId, SourceTree};
use sm_merge::{MergeConfig, MergeOutcome, MergedId, MergedNode, Side, layout, merge};

#[must_use]
pub fn java() -> &'static dyn Language {
    sm_cst::languages::detect(Path::new("x.java")).expect("java is registered")
}

#[must_use]
pub fn typescript() -> &'static dyn Language {
    sm_cst::languages::detect(Path::new("x.ts")).expect("typescript is registered")
}

#[must_use]
pub fn parse(src: &str, lang: &dyn Language) -> SourceTree {
    let tree = sm_cst::parse(src.as_bytes(), lang).expect("parse");
    assert!(
        !tree.has_errors(),
        "test fixture must be syntactically valid:\n{src}"
    );
    tree
}

/// One three-way merge, with the trees kept alive alongside the outcome.
pub struct Run {
    pub base: SourceTree,
    pub ours: SourceTree,
    pub theirs: SourceTree,
    pub outcome: MergeOutcome,
}

impl Run {
    #[must_use]
    pub fn java(base: &str, ours: &str, theirs: &str) -> Self {
        Self::with(base, ours, theirs, java(), &MergeConfig::default())
    }

    #[must_use]
    pub fn typescript(base: &str, ours: &str, theirs: &str) -> Self {
        Self::with(base, ours, theirs, typescript(), &MergeConfig::default())
    }

    #[must_use]
    pub fn with(
        base: &str,
        ours: &str,
        theirs: &str,
        lang: &'static dyn Language,
        cfg: &MergeConfig,
    ) -> Self {
        let base = parse(base, lang);
        let ours = parse(ours, lang);
        let theirs = parse(theirs, lang);
        let outcome = merge(&base, &ours, &theirs, lang, cfg);
        Self {
            base,
            ours,
            theirs,
            outcome,
        }
    }

    /// `(reason tag, node kind)` for every conflict, in preorder.
    #[must_use]
    pub fn conflicts(&self) -> Vec<(&str, &str)> {
        self.outcome
            .conflicts
            .iter()
            .map(|c| (c.reason.tag(), c.kind.as_str()))
            .collect()
    }

    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.outcome.is_clean()
    }

    pub fn tree(&self, side: Side) -> &SourceTree {
        match side {
            Side::Base => &self.base,
            Side::Ours => &self.ours,
            Side::Theirs => &self.theirs,
        }
    }

    /// The fate of the first base node of the given kind, on both sides.
    #[must_use]
    pub fn fates_of(&self, kind: &str) -> Option<(sm_merge::Fate, sm_merge::Fate)> {
        let id = self
            .base
            .ids()
            .find(|&id| self.base.node(id).kind == kind)?;
        Some((
            self.outcome.ours_fates[id.index()],
            self.outcome.theirs_fates[id.index()],
        ))
    }

    /// The node kinds of one merged node's children, in emission order.
    #[must_use]
    pub fn child_kinds(&self, id: MergedId) -> Vec<String> {
        self.outcome
            .tree
            .children(id)
            .iter()
            .map(|&c| match self.outcome.tree.node(c) {
                MergedNode::Splice { side, node } | MergedNode::Rebuilt { side, node, .. } => {
                    self.tree(*side).node(*node).kind.to_owned()
                }
                MergedNode::Conflict(conflict) => format!("<conflict {}>", conflict.reason),
            })
            .collect()
    }

    /// The node kinds of the merged root's children, in emission order.
    #[must_use]
    pub fn root_child_kinds(&self) -> Vec<String> {
        self.child_kinds(self.outcome.tree.root())
    }

    /// A stable, diffable rendering of the merged tree, for snapshots.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.render_node(self.outcome.tree.root(), 0, true, &mut out);
        out
    }

    fn render_node(&self, id: MergedId, depth: usize, is_root: bool, out: &mut String) {
        use std::fmt::Write as _;
        let pad = "  ".repeat(depth);
        let lead = match self.outcome.tree.lead(id) {
            sm_merge::Gap::Copied { side, range } if range.start < range.end => {
                format!(" lead={}{:?}", side.name(), self.text(*side, range.clone()))
            }
            sm_merge::Gap::Copied { .. } => String::new(),
            sm_merge::Gap::Synthesized(bytes) => {
                format!(" lead=SYNTH{:?}", String::from_utf8_lossy(bytes))
            }
        };
        match self.outcome.tree.node(id) {
            MergedNode::Splice { side, node } => {
                let range = layout::outer_extent(self.tree(*side), *node, is_root);
                let _ = writeln!(
                    out,
                    "{pad}splice {} {} {:?}{lead}",
                    side.name(),
                    self.tree(*side).node(*node).kind,
                    self.text(*side, range),
                );
            }
            MergedNode::Rebuilt {
                side,
                node,
                children,
            } => {
                let _ = writeln!(
                    out,
                    "{pad}rebuilt {} {}{lead}",
                    side.name(),
                    self.tree(*side).node(*node).kind,
                );
                for &child in children {
                    self.render_node(child, depth + 1, false, out);
                }
            }
            MergedNode::Conflict(conflict) => {
                let _ = writeln!(out, "{pad}conflict {}{lead}", conflict.reason);
                for (label, side, nodes) in [
                    (
                        "base",
                        Side::Base,
                        conflict.base.clone().unwrap_or_default(),
                    ),
                    ("ours", Side::Ours, conflict.ours.clone()),
                    ("theirs", Side::Theirs, conflict.theirs.clone()),
                ] {
                    let _ = writeln!(
                        out,
                        "{pad}  {label}: {}",
                        if nodes.is_empty() {
                            "<none>".to_owned()
                        } else {
                            nodes
                                .iter()
                                .map(|&n| {
                                    format!(
                                        "{:?}",
                                        self.text(side, layout::extent(self.tree(side), n))
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(" ")
                        }
                    );
                }
            }
        }
    }

    fn text(&self, side: Side, range: std::ops::Range<u32>) -> String {
        let source = self.tree(side).source();
        String::from_utf8_lossy(&source[range.start as usize..range.end as usize]).into_owned()
    }
}

/// Every node kind in a tree, for asserting a fixture really contains what the
/// test claims it does.
#[must_use]
pub fn find_kind(tree: &SourceTree, kind: &str) -> Option<NodeId> {
    tree.ids().find(|&id| tree.node(id).kind == kind)
}
