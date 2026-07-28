//! The matching visualiser behind `sm match`.
//!
//! SPEC.md §4.3 is emphatic that this is not optional: *"Build the eyeball tool
//! — matching bugs are almost invisible in assertions and obvious visually."*
//!
//! It lives in `sm-match` rather than in `sm-cli` for the same reason
//! `render_parse` lives in `sm-cst` (PROGRESS.md decision 14): a snapshot test
//! can then assert on exactly the bytes the user sees without shelling out to a
//! binary.
//!
//! Only *participating* nodes are shown — the ones that can take part in a
//! matching at all. Rendering the anonymous tokens would triple the height of
//! the output with rows that are structurally incapable of being interesting.

use std::fmt::Write as _;

use serde::Serialize;
use sm_cst::{Language, NodeId, SourceTree};

use crate::config::MatchConfig;
use crate::matching::Matching;
use crate::metrics::TreeMetrics;

/// ANSI SGR sequences. Written out rather than pulled from a crate: this is
/// nine escape sequences and a terminal-detection call, and SPEC.md §9 is
/// hostile to dependencies that are not carrying weight.
const RESET: &str = "\x1b[0m";
const RED: &str = "\x1b[31m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
/// Cycled by pair number so neighbouring matches are visually separable.
const PAIR_COLORS: [&str; 6] = [
    "\x1b[36m", // cyan
    "\x1b[32m", // green
    "\x1b[33m", // yellow
    "\x1b[35m", // magenta
    "\x1b[34m", // blue
    "\x1b[96m", // bright cyan
];

/// Knobs for [`render_side_by_side`].
///
/// The column headers come from [`Side::path`], not from here.
#[derive(Clone, Copy, Debug)]
pub struct VisualizeOptions {
    /// Total terminal width. Each column gets `(width - 3) / 2`.
    pub width: usize,
    /// Emit ANSI colour. The caller decides (tty detection, `--color`).
    pub color: bool,
    /// Maximum characters of node text to show, for
    /// [`Language::significant_text`] kinds.
    pub max_text_len: usize,
}

impl Default for VisualizeOptions {
    fn default() -> Self {
        Self {
            width: 160,
            color: false,
            max_text_len: 24,
        }
    }
}

/// Per-node move flags for the source tree, indexed by [`NodeId`].
///
/// A matched pair `(a, b)` is a **move** when its context changed, in either of
/// two ways:
///
/// - **Re-parented** — `a`'s participating parent is not matched to `b`'s. This
///   is the headline case: a method that migrated to another class body, a
///   statement pulled out of a loop.
/// - **Reordered** — the parents *do* correspond, but the node's rank among its
///   **matched** siblings differs. Ranking among matched siblings rather than
///   among all siblings is what stops a plain insertion elsewhere in the list
///   from flagging every following sibling as moved.
///
/// Roots are never moves. This is a display classification for `sm match`; M3
/// owns the real `Move` edit operation and may well want a richer notion (the
/// distinction between re-parenting and reordering matters for the emitter's
/// reindentation, for instance).
#[must_use]
pub fn move_flags(
    src_metrics: &TreeMetrics,
    dst_metrics: &TreeMetrics,
    matching: &Matching,
) -> Vec<bool> {
    let src_rank = matched_sibling_ranks(src_metrics, |id| matching.is_src_matched(id));
    let dst_rank = matched_sibling_ranks(dst_metrics, |id| matching.is_dst_matched(id));

    let mut flags = vec![false; src_metrics.len()];
    for (a, b) in matching.iter() {
        let reparented = match (src_metrics.parent(a), dst_metrics.parent(b)) {
            (None, None) => false,
            (Some(pa), Some(pb)) => matching.dst_of(pa) != Some(pb),
            _ => true,
        };
        let reordered = !reparented && src_rank[a.index()] != dst_rank[b.index()];
        flags[a.index()] = reparented || reordered;
    }
    flags
}

fn matched_sibling_ranks(metrics: &TreeMetrics, is_matched: impl Fn(NodeId) -> bool) -> Vec<u32> {
    let mut rank = vec![u32::MAX; metrics.len()];
    for i in 0..metrics.len() {
        let id = NodeId(i as u32);
        if !metrics.participates(id) {
            continue;
        }
        let mut next = 0;
        for &c in metrics.children(id) {
            if is_matched(c) {
                rank[c.index()] = next;
                next += 1;
            }
        }
    }
    rank
}

/// How many nodes of each kind went unmatched.
#[derive(Clone, Debug, Serialize)]
pub struct KindCount {
    pub kind: String,
    pub count: usize,
}

/// The footer numbers.
#[derive(Clone, Debug, Serialize)]
pub struct MatchSummary {
    /// Every node in the arena, anonymous tokens and comments included.
    pub src_nodes: usize,
    pub dst_nodes: usize,
    /// Nodes eligible to be matched at all — see the [`crate::metrics`] docs.
    pub src_matchable: usize,
    pub dst_matchable: usize,
    pub matched: usize,
    pub moved: usize,
    /// Percentage of *matchable* nodes that found a partner.
    pub src_matched_pct: f64,
    pub dst_matched_pct: f64,
    /// The ten commonest unmatched kinds, descending, ties broken by kind name.
    pub unmatched_src_by_kind: Vec<KindCount>,
    pub unmatched_dst_by_kind: Vec<KindCount>,
}

/// How many entries the by-kind histograms keep.
const TOP_KINDS: usize = 10;

#[must_use]
pub fn summarize(src: Side<'_>, dst: Side<'_>, matching: &Matching) -> MatchSummary {
    let (src_metrics, dst_metrics) = (src.metrics, dst.metrics);
    let (src, dst) = (src.tree, dst.tree);
    let moved = move_flags(src_metrics, dst_metrics, matching)
        .iter()
        .filter(|&&f| f)
        .count();

    let pct = |part: usize, whole: usize| {
        if whole == 0 {
            100.0
        } else {
            100.0 * part as f64 / whole as f64
        }
    };

    MatchSummary {
        src_nodes: src.len(),
        dst_nodes: dst.len(),
        src_matchable: src_metrics.matchable_count(),
        dst_matchable: dst_metrics.matchable_count(),
        matched: matching.len(),
        moved,
        src_matched_pct: pct(matching.len(), src_metrics.matchable_count()),
        dst_matched_pct: pct(matching.len(), dst_metrics.matchable_count()),
        unmatched_src_by_kind: unmatched_kinds(src, src_metrics, |id| matching.is_src_matched(id)),
        unmatched_dst_by_kind: unmatched_kinds(dst, dst_metrics, |id| matching.is_dst_matched(id)),
    }
}

fn unmatched_kinds(
    tree: &SourceTree,
    metrics: &TreeMetrics,
    is_matched: impl Fn(NodeId) -> bool,
) -> Vec<KindCount> {
    let mut counts: std::collections::BTreeMap<&'static str, usize> =
        std::collections::BTreeMap::new();
    for &id in metrics.post_order() {
        if !is_matched(id) {
            *counts.entry(tree.node(id).kind).or_default() += 1;
        }
    }
    let mut out: Vec<KindCount> = counts
        .into_iter()
        .map(|(kind, count)| KindCount {
            kind: kind.to_owned(),
            count,
        })
        .collect();
    // Descending by count; the BTreeMap already made the name order stable, and
    // a stable sort preserves it within a count.
    out.sort_by(|a, b| b.count.cmp(&a.count));
    out.truncate(TOP_KINDS);
    out
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct FileInfo {
    pub path: String,
    pub language: String,
    pub nodes: usize,
    pub matchable: usize,
    pub has_errors: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PairInfo {
    pub src_id: u32,
    pub dst_id: u32,
    pub kind: String,
    pub src_start: u32,
    pub src_end: u32,
    pub dst_start: u32,
    pub dst_end: u32,
    /// See [`move_flags`].
    pub moved: bool,
}

/// The `--json` payload.
///
/// Versioned for the same reason `sm-cst`'s `JsonTree` is (PROGRESS.md decision
/// 15): the wire format should be a deliberate decision, not a side effect of a
/// refactor.
#[derive(Clone, Debug, Serialize)]
pub struct MatchReport {
    pub schema_version: u32,
    pub config: MatchConfig,
    pub src: FileInfo,
    pub dst: FileInfo,
    pub summary: MatchSummary,
    pub pairs: Vec<PairInfo>,
}

/// One side of a report: the tree, its side tables and the path to show.
///
/// Grouped into a struct because the alternative is an eight-argument
/// constructor in which the two sides' three arguments can be transposed
/// silently.
#[derive(Clone, Copy)]
pub struct Side<'a> {
    pub tree: &'a SourceTree,
    pub metrics: &'a TreeMetrics,
    pub path: &'a str,
}

impl<'a> Side<'a> {
    #[must_use]
    pub fn new(tree: &'a SourceTree, metrics: &'a TreeMetrics, path: &'a str) -> Self {
        Self {
            tree,
            metrics,
            path,
        }
    }

    fn info(self) -> FileInfo {
        FileInfo {
            path: self.path.to_owned(),
            language: self.tree.language_name().to_owned(),
            nodes: self.tree.len(),
            matchable: self.metrics.matchable_count(),
            has_errors: self.tree.has_errors(),
        }
    }
}

impl MatchReport {
    /// Current `schema_version`.
    pub const SCHEMA_VERSION: u32 = 1;

    #[must_use]
    pub fn new(src: Side<'_>, dst: Side<'_>, matching: &Matching, cfg: &MatchConfig) -> Self {
        let flags = move_flags(src.metrics, dst.metrics, matching);
        let pairs = matching
            .iter()
            .map(|(a, b)| {
                let na = src.tree.node(a);
                let nb = dst.tree.node(b);
                PairInfo {
                    src_id: a.0,
                    dst_id: b.0,
                    kind: na.kind.to_owned(),
                    src_start: na.byte_range.start,
                    src_end: na.byte_range.end,
                    dst_start: nb.byte_range.start,
                    dst_end: nb.byte_range.end,
                    moved: flags[a.index()],
                }
            })
            .collect();

        Self {
            schema_version: Self::SCHEMA_VERSION,
            config: *cfg,
            src: src.info(),
            dst: dst.info(),
            summary: summarize(src, dst, matching),
            pairs,
        }
    }
}

// ---------------------------------------------------------------------------
// Side-by-side rendering
// ---------------------------------------------------------------------------

/// One rendered node, before alignment.
struct Row {
    id: NodeId,
    depth: usize,
}

fn preorder_rows(metrics: &TreeMetrics) -> Vec<Row> {
    let mut rows = Vec::with_capacity(metrics.matchable_count());
    let Some(root) = metrics.root() else {
        return rows;
    };
    let mut stack = vec![(root, 0usize)];
    while let Some((id, depth)) = stack.pop() {
        rows.push(Row { id, depth });
        for &c in metrics.children(id).iter().rev() {
            stack.push((c, depth + 1));
        }
    }
    rows
}

/// Render both trees side by side, matched pairs sharing a number and a colour.
///
/// Matched pairs land on the same line wherever the two preorders allow it.
/// Where they do not — a declaration that moved backwards, say — the two
/// members appear in their own blocks, still sharing their number and colour,
/// with the source side carrying an `M`. Unmatched nodes are red and carry a
/// `·` instead of a number.
///
/// # The alignment
///
/// A two-cursor walk, not an optimal one. When the two cursors point at
/// partners, they are emitted together. Otherwise each cursor knows the row its
/// partner sits on, and the walk emits a **run** of one-sided rows to close the
/// cheaper of the two gaps in one go. Emitting a run rather than a single row
/// is what keeps a moved method rendered as one block on each side, instead of
/// as two interleaved combs. `i + j` strictly increases on every iteration, so
/// the walk always terminates.
#[must_use]
pub fn render_side_by_side(
    src: Side<'_>,
    dst: Side<'_>,
    lang: &dyn Language,
    matching: &Matching,
    opts: &VisualizeOptions,
) -> String {
    let (src_metrics, dst_metrics) = (src.metrics, dst.metrics);
    let (src_label, dst_label) = (src.path, dst.path);
    let (src, dst) = (src.tree, dst.tree);
    let left = preorder_rows(src_metrics);
    let right = preorder_rows(dst_metrics);

    let mut left_pos = vec![usize::MAX; src.len()];
    for (i, row) in left.iter().enumerate() {
        left_pos[row.id.index()] = i;
    }
    let mut right_pos = vec![usize::MAX; dst.len()];
    for (j, row) in right.iter().enumerate() {
        right_pos[row.id.index()] = j;
    }

    // Pair numbers, assigned in source preorder so they read top to bottom.
    let mut pair_no = vec![usize::MAX; src.len()];
    for (n, (a, _)) in matching.iter().enumerate() {
        pair_no[a.index()] = n + 1;
    }
    let flags = move_flags(src_metrics, dst_metrics, matching);

    let col = (opts.width.saturating_sub(3) / 2).max(24);
    let mut out = String::with_capacity((left.len() + right.len()) * (col + 4));

    let header = format!(
        "{:<col$} | {}",
        truncate(src_label, col),
        truncate(dst_label, col)
    );
    if opts.color {
        let _ = writeln!(out, "{BOLD}{header}{RESET}");
    } else {
        let _ = writeln!(out, "{header}");
    }
    let _ = writeln!(out, "{}", "-".repeat(col * 2 + 3));

    // Everything a cell needs, so the three call sites below stay readable.
    let left_cell = |row: &Row| {
        let n = (pair_no[row.id.index()] != usize::MAX).then(|| pair_no[row.id.index()]);
        cell(src, lang, row, n, flags[row.id.index()], opts, col)
    };
    let right_cell = |row: &Row| {
        let partner = matching.src_of(row.id);
        let n = partner
            .map(|a| pair_no[a.index()])
            .filter(|&n| n != usize::MAX);
        let moved = partner.is_some_and(|a| flags[a.index()]);
        cell(dst, lang, row, n, moved, opts, col)
    };

    let mut i = 0;
    let mut j = 0;
    while i < left.len() || j < right.len() {
        if i < left.len() && j < right.len() && matching.dst_of(left[i].id) == Some(right[j].id) {
            let _ = writeln!(out, "{} | {}", left_cell(&left[i]), right_cell(&right[j]));
            i += 1;
            j += 1;
            continue;
        }

        // How many rows the other side would have to emit alone before this
        // cursor's partner comes up. `None` means "nothing to wait for": either
        // the cursor is spent, or its node is unmatched, or its partner is a
        // row we have already passed.
        let left_gap = (i < left.len())
            .then(|| matching.dst_of(left[i].id).map(|b| right_pos[b.index()]))
            .flatten()
            .filter(|&t| t > j)
            .map(|t| t - j);
        let right_gap = (j < right.len())
            .then(|| matching.src_of(right[j].id).map(|a| left_pos[a.index()]))
            .flatten()
            .filter(|&t| t > i)
            .map(|t| t - i);

        match (left_gap, right_gap) {
            // Neither is waiting. Both are unmatched (or their partners are
            // behind us), so pair them on one line: two red cells side by side
            // read as "this changed", which is what they are.
            (None, None) => {
                let l = if i < left.len() {
                    let c = left_cell(&left[i]);
                    i += 1;
                    c
                } else {
                    pad("", col)
                };
                let r = if j < right.len() {
                    let c = right_cell(&right[j]);
                    j += 1;
                    c
                } else {
                    String::new()
                };
                let _ = writeln!(out, "{l} | {r}");
            }
            // Left is waiting and right is not: let the right side through.
            (Some(_), None) => {
                let _ = writeln!(out, "{:<col$} | {}", "", right_cell(&right[j]));
                j += 1;
            }
            (None, Some(_)) => {
                let _ = writeln!(out, "{} |", left_cell(&left[i]));
                i += 1;
            }
            // Both are waiting. Close the cheaper gap in one run, so a moved
            // declaration renders as one block per side rather than as two
            // interleaved combs.
            (Some(gl), Some(gr)) => {
                if gl <= gr {
                    for r in &right[j..j + gl] {
                        let _ = writeln!(out, "{:<col$} | {}", "", right_cell(r));
                    }
                    j += gl;
                } else {
                    for r in &left[i..i + gr] {
                        let _ = writeln!(out, "{} |", left_cell(r));
                    }
                    i += gr;
                }
            }
        }
    }

    // Trailing padding on the right-hand column is invisible but it dirties
    // snapshots and `git diff`. In colour mode the line ends with the reset
    // sequence, so the padding survives and the columns stay aligned.
    let mut trimmed = String::with_capacity(out.len());
    for line in out.lines() {
        trimmed.push_str(line.trim_end());
        trimmed.push('\n');
    }
    trimmed
}

/// Render one node into a fixed-width, optionally coloured cell.
///
/// Matched cells are tagged with their pair number and coloured by it (six
/// colours, cycled, so adjacent pairs are distinguishable); unmatched cells are
/// red and tagged `·`; a moved pair keeps its colour and adds a bold `M`.
fn cell(
    tree: &SourceTree,
    lang: &dyn Language,
    row: &Row,
    pair: Option<usize>,
    moved: bool,
    opts: &VisualizeOptions,
    col: usize,
) -> String {
    let node = tree.node(row.id);

    let tag = match pair {
        Some(n) => format!("{n:>5}"),
        None => "    \u{b7}".to_owned(),
    };

    let mut body = String::new();
    for _ in 0..row.depth.min(24) {
        body.push_str("  ");
    }
    body.push_str(node.kind);
    if lang.significant_text(node.kind) {
        let text = one_line(&tree.node_text(row.id), opts.max_text_len);
        let _ = write!(body, " {text}");
    }
    if moved {
        body.push_str(" M");
    }

    let padded = pad(
        &format!("{tag} {}", truncate(&body, col.saturating_sub(6))),
        col,
    );
    if !opts.color {
        return padded;
    }
    match pair {
        None => format!("{RED}{padded}{RESET}"),
        Some(n) => {
            let hue = PAIR_COLORS[(n - 1) % PAIR_COLORS.len()];
            if moved {
                format!("{BOLD}{hue}{padded}{RESET}")
            } else {
                format!("{hue}{padded}{RESET}")
            }
        }
    }
}

/// Collapse a node's text to one line and cap its length.
fn one_line(text: &str, max: usize) -> String {
    let mut out = String::with_capacity(max + 2);
    out.push('"');
    for (n, ch) in text.chars().enumerate() {
        if n >= max {
            out.push('…');
            break;
        }
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '"' => out.push_str("\\\""),
            c if c.is_control() => out.push('.'),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn pad(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len() + width - len);
    out.push_str(s);
    for _ in len..width {
        out.push(' ');
    }
    out
}

/// The summary block printed under the two columns.
#[must_use]
pub fn render_summary(summary: &MatchSummary, cfg: &MatchConfig, color: bool) -> String {
    let mut out = String::new();
    let (dim, reset) = if color { (DIM, RESET) } else { ("", "") };

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{dim}profile: min_height={} min_dice={} max_size={}{reset}",
        cfg.min_height, cfg.min_dice, cfg.max_size
    );
    let _ = writeln!(
        out,
        "a: {} nodes ({} matchable)   b: {} nodes ({} matchable)",
        summary.src_nodes, summary.src_matchable, summary.dst_nodes, summary.dst_matchable
    );
    let _ = writeln!(
        out,
        "matched: {} pairs — {:.1}% of a, {:.1}% of b   moves: {}",
        summary.matched, summary.src_matched_pct, summary.dst_matched_pct, summary.moved
    );

    for (label, kinds) in [
        ("unmatched in a", &summary.unmatched_src_by_kind),
        ("unmatched in b", &summary.unmatched_dst_by_kind),
    ] {
        if kinds.is_empty() {
            let _ = writeln!(out, "{label}: none");
            continue;
        }
        let list: Vec<String> = kinds
            .iter()
            .map(|k| format!("{}×{}", k.kind, k.count))
            .collect();
        let _ = writeln!(out, "{label} (top {}): {}", kinds.len(), list.join(", "));
    }
    out
}
