//! The terminal renderer behind `sm diff` (SPEC.md §4.4, milestone M3).
//!
//! The exit criterion for M3 is that this output is *obviously* better than
//! `git diff` on moved functions and reindented blocks, so the layout is a
//! deliverable rather than an implementation detail — which is why it lives
//! here, next to a snapshot test, instead of in `sm-cli` where it could only be
//! tested through a subprocess. Same argument `sm-cst` made for `render_parse`
//! (PROGRESS.md decision 14) and `sm-match` for `render_side_by_side`.
//!
//! # What the layout is trying to do
//!
//! A line diff has one vocabulary — *this line is gone, this line is new* — and
//! has to spend it on everything. Three of the four things an edit script knows
//! are therefore invisible to it: that a declaration **moved**, that a block was
//! **reindented** without changing, and that a set of statements was
//! **wrapped** rather than rewritten. Each gets its own shape here:
//!
//! - A move is **one header line**, not a deletion plus an insertion. When the
//!   subtree is byte-identical it says `[unchanged]`, and when it differs only
//!   in leading whitespace it says `[body unchanged, reindented +4]` — the
//!   killer case, where `git diff` reports every line of the block as modified.
//! - An insertion whose subtree contains matched nodes prints only the lines
//!   that are genuinely new and renders the matched regions as nested move
//!   headers *in place*. Wrapping three statements in an `if` is then the
//!   insertion of two lines plus one "moved here", not seven changed lines.
//! - An update prints the old and the new line with the changed leaf named, and
//!   highlighted in colour.
//!
//! difftastic (docs/prior-art.md §3) is the quality reference. The two ideas
//! taken from it are that nesting-depth changes are *priced and reported*
//! rather than hidden, and that the interesting unit of a diff is a node, not a
//! line.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use sm_cst::{Language, NodeId, SourceTree};
use sm_match::{Matching, TreeMetrics};

use crate::lines::{LineIndex, indent_width};
use crate::script::{EditOp, EditScript, MoveKind, Summary};

/// ANSI SGR sequences, written out for the same reason `sm-match` writes them
/// out: this is a handful of escapes, not a dependency.
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
/// Reverse video, for the changed run inside an updated line.
const INVERSE: &str = "\x1b[7m";

/// One side of a diff: the tree, its side tables and the label to print.
///
/// Grouped into a struct so the two sides' three arguments cannot be transposed
/// silently — the same reasoning as [`sm_match::visualize::Side`].
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
}

/// Everything the renderer needs: both sides, the matching they came from and
/// the script derived from it.
#[derive(Clone, Copy)]
pub struct DiffView<'a> {
    pub src: Side<'a>,
    pub dst: Side<'a>,
    pub matching: &'a Matching,
    pub lang: &'a dyn Language,
    pub script: &'a EditScript,
}

/// Display knobs. Nothing here changes the edit script, only how it reads.
#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    /// Emit ANSI colour. The caller decides (tty detection, `--color`).
    pub color: bool,
    /// Maximum source lines printed for one hunk before the middle is elided.
    pub max_hunk_lines: usize,
    /// Maximum characters of node text shown in a header label.
    pub max_label_len: usize,
    /// Fold moves of subtrees this small — when they are otherwise unchanged
    /// and carry no nested hunks — into one trailing summary line.
    ///
    /// The motivation is not tidiness. `sm-match`'s permissive profile matches
    /// height-1 subtrees, so a Java file will happily pair one method's
    /// `modifiers` (a lone `public`) with another's, and every such pair is a
    /// perfectly true re-parent that says nothing a reader wants to know. The
    /// operations are still in the [`EditScript`] and still in the counts; only
    /// the *display* folds them. `0` disables the fold.
    pub fold_moves_below: u32,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            color: false,
            max_hunk_lines: 40,
            max_label_len: 40,
            fold_moves_below: 3,
        }
    }
}

// ---------------------------------------------------------------------------
// Units and the hunk forest
// ---------------------------------------------------------------------------

/// The operations that share one anchor, e.g. the `Move` and the `Update` of a
/// declaration that moved *and* was renamed.
#[derive(Clone, Debug)]
struct Unit {
    src: Option<NodeId>,
    dst: Option<NodeId>,
    ops: Vec<usize>,
    children: Vec<usize>,
    owner: Option<usize>,
}

impl Unit {
    fn has(&self, script: &EditScript, name: &'static str) -> bool {
        self.ops.iter().any(|&i| script.ops[i].name() == name)
    }

    fn move_kind(&self, script: &EditScript) -> Option<MoveKind> {
        self.ops.iter().find_map(|&i| match script.ops[i] {
            EditOp::Move { kind, .. } => Some(kind),
            _ => None,
        })
    }
}

/// Group the script's operations into units and nest them.
///
/// A unit's owner is the innermost other unit that *contains* it: by source
/// ancestry when it has a source node, and otherwise — the case that matters —
/// by destination ancestry, which is what puts a subtree that moved *into* an
/// inserted wrapper underneath that wrapper's hunk.
fn build_units(view: &DiffView<'_>) -> Vec<Unit> {
    let script = view.script;
    let mut units: Vec<Unit> = Vec::new();
    let mut by_anchor: BTreeMap<(Option<NodeId>, Option<NodeId>), usize> = BTreeMap::new();

    for (i, op) in script.ops.iter().enumerate() {
        let anchor = (op.src(), op.dst());
        match by_anchor.get(&anchor) {
            Some(&u) => units[u].ops.push(i),
            None => {
                by_anchor.insert(anchor, units.len());
                units.push(Unit {
                    src: anchor.0,
                    dst: anchor.1,
                    ops: vec![i],
                    children: Vec::new(),
                    owner: None,
                });
            }
        }
    }

    let mut src_unit = vec![usize::MAX; view.src.tree.len()];
    let mut dst_unit = vec![usize::MAX; view.dst.tree.len()];
    for (u, unit) in units.iter().enumerate() {
        if let Some(a) = unit.src {
            src_unit[a.index()] = u;
        }
        if let Some(b) = unit.dst {
            dst_unit[b.index()] = u;
        }
    }

    let owners: Vec<Option<usize>> = (0..units.len())
        .map(|u| {
            let via_src = units[u].src.and_then(|a| {
                view.src
                    .metrics
                    .ancestors(a)
                    .find_map(|p| (src_unit[p.index()] != usize::MAX).then(|| src_unit[p.index()]))
            });
            let owner = via_src.or_else(|| {
                units[u].dst.and_then(|b| {
                    view.dst.metrics.ancestors(b).find_map(|p| {
                        (dst_unit[p.index()] != usize::MAX).then(|| dst_unit[p.index()])
                    })
                })
            });
            owner.filter(|&o| o != u)
        })
        .collect();
    for (unit, owner) in units.iter_mut().zip(owners) {
        unit.owner = owner;
    }

    // Ownership across two trees is not obviously acyclic — a pair of moves
    // could in principle each contain the other, one by source and one by
    // destination. Rather than argue that it cannot happen, break any cycle.
    for u in 0..units.len() {
        let mut seen = vec![false; units.len()];
        let mut cur = u;
        while let Some(next) = units[cur].owner {
            if seen[next] || next == u {
                units[cur].owner = None;
                break;
            }
            seen[next] = true;
            cur = next;
        }
    }

    for u in 0..units.len() {
        if let Some(owner) = units[u].owner {
            units[owner].children.push(u);
        }
    }
    units
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

struct Painter {
    color: bool,
}

impl Painter {
    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("{code}{text}{RESET}")
        } else {
            text.to_owned()
        }
    }
}

struct Renderer<'a> {
    view: DiffView<'a>,
    opts: RenderOptions,
    paint: Painter,
    src_lines: LineIndex,
    dst_lines: LineIndex,
    units: Vec<Unit>,
    /// Units withheld by [`RenderOptions::fold_moves_below`], reported as one
    /// line at the end.
    folded: Vec<usize>,
    out: String,
}

/// Render the whole diff.
///
/// The output is a header naming the two files, then one block per top-level
/// hunk in [`EditScript::ops`] order, then the `--stat` line.
#[must_use]
pub fn render(view: &DiffView<'_>, opts: &RenderOptions) -> String {
    let mut r = Renderer {
        view: *view,
        opts: *opts,
        paint: Painter { color: opts.color },
        src_lines: LineIndex::new(view.src.tree.source()),
        dst_lines: LineIndex::new(view.dst.tree.source()),
        units: build_units(view),
        folded: Vec::new(),
        out: String::new(),
    };
    r.run();
    r.out
}

/// The `--stat` one-liner.
#[must_use]
pub fn render_stat(summary: &Summary) -> String {
    fn plural(n: usize, word: &str) -> String {
        if n == 1 {
            format!("{n} {word}")
        } else {
            format!("{n} {word}s")
        }
    }
    format!(
        "{}, {}, {}, {} ({} reparent, {} reorder)",
        plural(summary.inserts, "insert"),
        plural(summary.deletes, "delete"),
        plural(summary.updates, "update"),
        plural(summary.moves, "move"),
        summary.reparents,
        summary.reorders,
    )
}

impl Renderer<'_> {
    fn run(&mut self) {
        let a = self
            .paint
            .paint(RED, &format!("--- a  {}", self.view.src.path));
        let b = self
            .paint
            .paint(GREEN, &format!("+++ b  {}", self.view.dst.path));
        let _ = writeln!(self.out, "{a}");
        let _ = writeln!(self.out, "{b}");

        if self.view.script.is_empty() {
            let _ = writeln!(self.out);
            let _ = writeln!(
                self.out,
                "{}",
                self.paint
                    .paint(DIM, "no structural changes (formatting and comments aside)")
            );
            return;
        }

        let roots: Vec<usize> = (0..self.units.len())
            .filter(|&u| self.units[u].owner.is_none())
            .collect();
        for u in roots {
            if self.foldable(u) {
                self.folded.push(u);
                continue;
            }
            let _ = writeln!(self.out);
            self.unit(u, 0);
        }
        self.fold_line();

        let _ = writeln!(self.out);
        let _ = writeln!(
            self.out,
            "{}",
            self.paint
                .paint(DIM, &render_stat(&self.view.script.summary))
        );
    }

    fn indent(&self, depth: usize) -> String {
        "  ".repeat(depth)
    }

    /// Whether a unit is a move too small and too unchanged to be worth a hunk.
    /// See [`RenderOptions::fold_moves_below`].
    fn foldable(&self, u: usize) -> bool {
        let unit = &self.units[u];
        let script = self.view.script;
        let Some(a) = unit.src else { return false };
        unit.children.is_empty()
            && unit.ops.iter().all(|&i| script.ops[i].name() == "move")
            && self.view.src.metrics.subtree_size(a) < self.opts.fold_moves_below
            && unit.dst.is_some_and(|b| {
                self.view.src.tree.node_bytes(a) == self.view.dst.tree.node_bytes(b)
            })
    }

    /// Render a child hunk, or set it aside for the fold line.
    fn child(&mut self, c: usize, depth: usize) {
        if self.foldable(c) {
            self.folded.push(c);
        } else {
            self.unit(c, depth);
        }
    }

    fn fold_line(&mut self) {
        if self.folded.is_empty() {
            return;
        }
        let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
        for &u in &self.folded {
            let id = self.units[u]
                .src
                .expect("a foldable unit has a source node");
            *counts.entry(self.view.src.tree.node(id).kind).or_default() += 1;
        }
        let kinds: Vec<String> = counts
            .iter()
            .map(|(kind, n)| format!("{kind}×{n}"))
            .collect();
        let note = format!(
            "… and {} unchanged small node{} that merely changed place: {}",
            self.folded.len(),
            if self.folded.len() == 1 { "" } else { "s" },
            kinds.join(", "),
        );
        let _ = writeln!(self.out);
        let _ = writeln!(self.out, "{}", self.paint.paint(DIM, &note));
    }

    /// Dispatch on what the unit is. Order matters: a pair that both moved and
    /// changed is reported as a move whose body was edited, because *where it
    /// went* is the fact a line diff could not have told you.
    fn unit(&mut self, u: usize, depth: usize) {
        let script = self.view.script;
        if self.units[u].has(script, "insert") {
            self.insert_unit(u, depth);
        } else if self.units[u].has(script, "delete") {
            self.delete_unit(u, depth);
        } else if self.units[u].has(script, "move") {
            self.move_unit(u, depth);
        } else {
            self.update_unit(u, depth);
        }
    }

    // -- headers ----------------------------------------------------------

    /// `kind "text"` where the node's own source fits on one short line, and
    /// `kind "name"` otherwise, where the name is the first identifier under
    /// it. A `formal_parameters` reads better as `"(String id)"` than as
    /// `"String"`, and a `method_declaration` — whose text is its whole body —
    /// reads better as its name.
    fn label(&self, side: Side<'_>, id: NodeId) -> String {
        let node = side.tree.node(id);
        let own = side.tree.node_text(id);
        let text = if own.chars().count() <= self.opts.max_label_len && !own.contains('\n') {
            Some(own.into_owned())
        } else {
            self.name_of(side, id)
        };
        match text {
            Some(t) => format!("{} {}", node.kind, quote(&t, self.opts.max_label_len)),
            None => node.kind.to_owned(),
        }
    }

    /// The declared name of a node, if it has one.
    ///
    /// The first identifier among the node's participating children, or among
    /// the children of a `*_declarator` child. Two narrow rules rather than a
    /// search: a `method_declaration` carries its name directly, and a
    /// `field_declaration` or `local_variable_declaration` carries it one level
    /// down inside a `variable_declarator` — a naming convention
    /// `tree-sitter-java` and `tree-sitter-typescript` share. Anything wider
    /// starts labelling a `block` with the first identifier that happens to
    /// appear inside it, which is worse than no label at all.
    fn name_of(&self, side: Side<'_>, id: NodeId) -> Option<String> {
        let first_identifier = |parent: NodeId| {
            side.metrics
                .children(parent)
                .iter()
                .find(|&&c| self.view.lang.is_identifier(side.tree.node(c).kind))
                .map(|&c| side.tree.node_text(c).into_owned())
        };
        first_identifier(id).or_else(|| {
            side.metrics
                .children(id)
                .iter()
                .filter(|&&c| side.tree.node(c).kind.ends_with("declarator"))
                .find_map(|&c| first_identifier(c))
        })
    }

    fn src_span(&self, id: NodeId) -> String {
        let r = self
            .src_lines
            .lines_of(self.view.src.tree.node(id).byte_range.clone());
        span_text('a', *r.start(), *r.end())
    }

    fn dst_span(&self, id: NodeId) -> String {
        let r = self
            .dst_lines
            .lines_of(self.view.dst.tree.node(id).byte_range.clone());
        span_text('b', *r.start(), *r.end())
    }

    // -- insert / delete --------------------------------------------------

    fn insert_unit(&mut self, u: usize, depth: usize) {
        let dst = self.units[u].dst.expect("an insert has a destination node");
        let head = format!(
            "{}+ {:<8} {}   {}",
            self.indent(depth),
            "insert",
            self.label(self.view.dst, dst),
            self.dst_span(dst),
        );
        let painted = self.paint.paint(GREEN, &head);
        let _ = writeln!(self.out, "{painted}");
        self.one_sided_body(u, depth, /* inserted */ true);
    }

    fn delete_unit(&mut self, u: usize, depth: usize) {
        let src = self.units[u].src.expect("a delete has a source node");
        let head = format!(
            "{}- {:<8} {}   {}",
            self.indent(depth),
            "delete",
            self.label(self.view.src, src),
            self.src_span(src),
        );
        let painted = self.paint.paint(RED, &head);
        let _ = writeln!(self.out, "{painted}");
        self.one_sided_body(u, depth, /* inserted */ false);
    }

    /// Print the lines of an inserted (or deleted) subtree, skipping the
    /// regions occupied by nodes that merely moved in (or out) and rendering
    /// each of those as a nested hunk at the place it belongs.
    ///
    /// This is what turns "wrap three statements in an `if`" from seven changed
    /// lines into two new lines and a "moved here".
    fn one_sided_body(&mut self, u: usize, depth: usize, inserted: bool) {
        // Everything that reads a borrowed `LineIndex` happens first, so that
        // rendering the nested hunks below only needs `&mut self`.
        let (rows, at_line, elided) = self.one_sided_plan(u, depth, inserted);

        let mut at_line = at_line;
        for (line, row) in rows {
            let _ = writeln!(self.out, "{row}");
            // Nested hunks are spliced in *after* the line they start on, so
            // that a "moved here" reads as the body of the wrapper whose
            // opening line precedes it.
            let due: Vec<usize> = at_line
                .range(..=line)
                .flat_map(|(_, cs)| cs.iter().copied())
                .collect();
            at_line.retain(|&k, _| k > line);
            for c in due {
                self.child(c, depth + 1);
            }
        }
        for (_, cs) in std::mem::take(&mut at_line) {
            for c in cs {
                self.child(c, depth + 1);
            }
        }
        if elided {
            let note = format!("{}      … more lines elided", self.indent(depth + 1));
            let painted = self.paint.paint(DIM, &note);
            let _ = writeln!(self.out, "{painted}");
        }
    }

    /// The printable rows of a one-sided hunk, where its nested hunks splice
    /// in, and whether anything was cut for length.
    #[allow(clippy::type_complexity)]
    fn one_sided_plan(
        &self,
        u: usize,
        depth: usize,
        inserted: bool,
    ) -> (Vec<(usize, String)>, BTreeMap<usize, Vec<usize>>, bool) {
        let (side, index, sign, color) = if inserted {
            (self.view.dst, &self.dst_lines, '+', GREEN)
        } else {
            (self.view.src, &self.src_lines, '-', RED)
        };
        let root = if inserted {
            self.units[u].dst.expect("insert")
        } else {
            self.units[u].src.expect("delete")
        };
        let range = side.tree.node(root).byte_range.clone();

        // Maximal matched subtrees inside: content that came from the other
        // file rather than being written anew.
        let mut foreign: Vec<std::ops::Range<u32>> = Vec::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let matched = if inserted {
                self.view.matching.is_dst_matched(id)
            } else {
                self.view.matching.is_src_matched(id)
            };
            if id != root && matched {
                foreign.push(side.tree.node(id).byte_range.clone());
                continue;
            }
            stack.extend(side.metrics.children(id).iter().rev().copied());
        }

        // Where each child hunk splices in: at the first line of its own span
        // on this side.
        let mut at_line: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for &c in &self.units[u].children {
            let node = if inserted {
                self.units[c].dst
            } else {
                self.units[c].src
            };
            if let Some(n) = node {
                let first = *index.lines_of(side.tree.node(n).byte_range.clone()).start();
                at_line.entry(first).or_default().push(c);
            }
        }

        let mut rows: Vec<(usize, String)> = Vec::new();
        let mut elided = false;
        for line in index.lines_of(range.clone()) {
            let span = index.line_span(line);
            let (runs, whole) = novel_runs(side.tree.source(), &span, &range, &foreign);
            if runs.is_empty() {
                continue;
            }
            if rows.len() >= self.opts.max_hunk_lines {
                elided = true;
                continue;
            }
            let text = index.line_text(side.tree.source(), line);
            // A line whose *whole* content is new prints plainly. A line that
            // also carries unchanged code — the signature line a new body opens
            // on, the tail of a statement whose head was rewritten — gets the
            // new runs marked, because claiming the whole line is new would be
            // the very thing this viewer exists not to do.
            let body = self.line_body(side.tree.source(), &text, &span, &runs, whole, color);
            let head = self.paint.paint(
                color,
                &format!("{}{:>5} {sign} ", self.indent(depth + 1), line),
            );
            rows.push((line, format!("{head}{body}")));
        }
        (rows, at_line, elided)
    }

    // -- move -------------------------------------------------------------

    fn move_unit(&mut self, u: usize, depth: usize) {
        let script = self.view.script;
        let a = self.units[u].src.expect("a move has a source node");
        let b = self.units[u].dst.expect("a move has a destination node");
        let kind = self.units[u].move_kind(script).expect("a move op");
        let edited = !self.units[u].children.is_empty() || self.units[u].has(script, "update");
        let tag = self.move_tag(a, b, edited);

        let head = format!(
            "{}~ {:<8} {}   {} → {}   [{}]",
            self.indent(depth),
            kind.as_str(),
            self.label(self.view.src, a),
            self.src_span(a),
            self.dst_span(b),
            tag,
        );
        let painted = self.paint.paint(if edited { YELLOW } else { CYAN }, &head);
        let _ = writeln!(self.out, "{painted}");

        if self.units[u].has(script, "update") {
            self.update_lines(u, depth + 1);
        }
        for c in self.units[u].children.clone() {
            self.child(c, depth + 1);
        }
    }

    /// `[unchanged]`, `[body unchanged, reindented +4]` or `[edited]`.
    ///
    /// The middle one is the whole point of the exercise: `sm-match`'s
    /// structural hash excludes whitespace and comments, so an equal hash on a
    /// pair whose bytes differ says *nothing about this subtree changed except
    /// how it is laid out*. A line diff cannot make that statement.
    fn move_tag(&self, a: NodeId, b: NodeId, edited: bool) -> String {
        if edited {
            return "edited".to_owned();
        }
        let sa = self.view.src.tree.node_bytes(a);
        let sb = self.view.dst.tree.node_bytes(b);
        if sa == sb {
            return "unchanged".to_owned();
        }
        if self.view.src.metrics.hash(a) != self.view.dst.metrics.hash(b) {
            return "edited".to_owned();
        }
        let ia = self.line_indent(&self.src_lines, self.view.src.tree, a);
        let ib = self.line_indent(&self.dst_lines, self.view.dst.tree, b);
        if ia == ib {
            "body unchanged, whitespace or comments differ".to_owned()
        } else {
            let delta = ib as isize - ia as isize;
            format!("body unchanged, reindented {delta:+}")
        }
    }

    fn line_indent(&self, index: &LineIndex, tree: &SourceTree, id: NodeId) -> usize {
        let line = *index.lines_of(tree.node(id).byte_range.clone()).start();
        indent_width(&index.line_text(tree.source(), line))
    }

    // -- update -----------------------------------------------------------

    fn update_unit(&mut self, u: usize, depth: usize) {
        let a = self.units[u].src.expect("an update has a source node");
        let b = self.units[u].dst.expect("an update has a destination node");
        let change = self.text_change(a, b);
        let what = if change.is_empty() {
            self.label(self.view.src, a)
        } else {
            // The arrow already shows both texts; repeating one in the label
            // would just make the line longer.
            self.view.src.tree.node(a).kind.to_owned()
        };
        let head = format!(
            "{}! {:<8} {}   {} → {}{}",
            self.indent(depth),
            "change",
            what,
            self.src_span(a),
            self.dst_span(b),
            change,
        );
        let painted = self.paint.paint(YELLOW, &head);
        let _ = writeln!(self.out, "{painted}");
        self.update_lines(u, depth + 1);

        // Children whose lines the two blocks above already showed would only
        // repeat themselves; the rest (a subtree that moved out of an updated
        // container, say) still need a hunk.
        let printed_src = self
            .src_lines
            .lines_of(self.view.src.tree.node(a).byte_range.clone());
        let printed_dst = self
            .dst_lines
            .lines_of(self.view.dst.tree.node(b).byte_range.clone());
        for c in self.units[u].children.clone() {
            let inside_src = self.units[c].src.is_some_and(|n| {
                let r = self
                    .src_lines
                    .lines_of(self.view.src.tree.node(n).byte_range.clone());
                printed_src.contains(r.start()) && printed_src.contains(r.end())
            });
            let inside_dst = self.units[c].dst.is_some_and(|n| {
                let r = self
                    .dst_lines
                    .lines_of(self.view.dst.tree.node(n).byte_range.clone());
                printed_dst.contains(r.start()) && printed_dst.contains(r.end())
            });
            if !(inside_src || inside_dst) {
                self.child(c, depth + 1);
            }
        }
    }

    /// `"total" → "sum"` when both sides are short leaves, otherwise nothing.
    fn text_change(&self, a: NodeId, b: NodeId) -> String {
        if !self.view.src.metrics.children(a).is_empty() {
            return String::new();
        }
        let ta = self.view.src.tree.node_text(a);
        let tb = self.view.dst.tree.node_text(b);
        if ta == tb {
            return String::new();
        }
        format!(
            "   {} → {}",
            quote(&ta, self.opts.max_label_len),
            quote(&tb, self.opts.max_label_len)
        )
    }

    /// The old lines, then the new ones, with the changed node highlighted.
    fn update_lines(&mut self, u: usize, depth: usize) {
        let a = self.units[u].src.expect("update src");
        let b = self.units[u].dst.expect("update dst");
        let rows = self.side_rows(self.view.src.tree, &self.src_lines, a, '-', RED, depth);
        for row in rows {
            let _ = writeln!(self.out, "{row}");
        }
        let rows = self.side_rows(self.view.dst.tree, &self.dst_lines, b, '+', GREEN, depth);
        for row in rows {
            let _ = writeln!(self.out, "{row}");
        }
    }

    fn side_rows(
        &self,
        tree: &SourceTree,
        index: &LineIndex,
        id: NodeId,
        sign: char,
        color: &str,
        depth: usize,
    ) -> Vec<String> {
        let node_range = tree.node(id).byte_range.clone();
        let mut rows = Vec::new();
        let lines = index.lines_of(node_range.clone());
        let total = lines.end() - lines.start() + 1;
        for (n, line) in lines.enumerate() {
            if n >= self.opts.max_hunk_lines {
                rows.push(self.paint.paint(
                    DIM,
                    &format!(
                        "{}      … {} more lines",
                        self.indent(depth),
                        total - self.opts.max_hunk_lines
                    ),
                ));
                break;
            }
            let span = index.line_span(line);
            let text = index.line_text(tree.source(), line).into_owned();
            let (runs, whole) = novel_runs(tree.source(), &span, &node_range, &[]);
            let body = self.line_body(tree.source(), &text, &span, &runs, whole, color);
            let head = self
                .paint
                .paint(color, &format!("{}{:>5} {sign} ", self.indent(depth), line));
            rows.push(format!("{head}{body}"));
        }
        rows
    }

    /// One rendered source line: marked when the marking would actually say
    /// something, plain otherwise.
    ///
    /// Marking earns its keep when a *small, contiguous* part of a line
    /// changed — a renamed identifier, the `{` a new body opens on. When the
    /// change is scattered across several runs, or is nearly the whole line,
    /// the brackets add noise and no information, so the line is printed
    /// plainly and the `+`/`-` carries the meaning.
    fn line_body(
        &self,
        source: &[u8],
        text: &str,
        span: &std::ops::Range<u32>,
        runs: &[std::ops::Range<u32>],
        whole: bool,
        color: &str,
    ) -> String {
        let code = |r: &std::ops::Range<u32>| {
            (r.start..r.end)
                .filter(|&i| !source[i as usize].is_ascii_whitespace())
                .count()
        };
        let line_code = code(span);
        let worth_marking = runs.len() == 1 && code(&runs[0]) * 5 < line_code * 4;
        if whole || !worth_marking {
            self.paint.paint(color, text)
        } else {
            self.mark_runs(text, span.start, runs, color)
        }
    }

    /// Print `text` with the byte ranges in `runs` called out.
    ///
    /// Reverse video in colour, `\u{27e8}\u{27e9}` brackets without it. The plain form
    /// matters: `--color never` is what a snapshot test and a piped `sm diff`
    /// see, and a word-level attribution that only exists in a terminal is one
    /// that cannot be asserted on.
    fn mark_runs(
        &self,
        text: &str,
        line_start: u32,
        runs: &[std::ops::Range<u32>],
        color: &str,
    ) -> String {
        let mut out = String::with_capacity(text.len() + runs.len() * 8);
        let mut cursor = 0usize;
        for r in runs {
            let lo = (r.start.saturating_sub(line_start) as usize).min(text.len());
            let hi = (r.end.saturating_sub(line_start) as usize).min(text.len());
            // Lossy decoding can move byte offsets; when it has, fall back to
            // printing the line unmarked rather than slicing mid-character.
            if lo < cursor || lo >= hi || !text.is_char_boundary(lo) || !text.is_char_boundary(hi) {
                continue;
            }
            out.push_str(&text[cursor..lo]);
            if self.opts.color {
                out.push_str(BOLD);
                out.push_str(INVERSE);
            } else {
                out.push('\u{27e8}');
            }
            out.push_str(&text[lo..hi]);
            if self.opts.color {
                out.push_str(RESET);
                out.push_str(color);
            } else {
                out.push('\u{27e9}');
            }
            cursor = hi;
        }
        out.push_str(&text[cursor..]);
        self.paint.paint(color, &out)
    }
}

/// The genuinely new runs of one line of a one-sided hunk.
///
/// A byte is *novel* when it lies inside `region` and outside every range in
/// `foreign` — the subtrees that merely moved here rather than being written.
/// Returns the maximal novel runs that contain at least one non-whitespace
/// byte, and whether they account for **every** non-whitespace byte of the
/// line.
///
/// This is the rule that decides which lines of an inserted subtree are new at
/// all, and how much of each one is. The wrapped-block case leans on both
/// halves: the statements the wrapper now contains produce no runs and are not
/// printed, while the signature line the new body opens on produces one run
/// holding a single `{`.
fn novel_runs(
    source: &[u8],
    line: &std::ops::Range<u32>,
    region: &std::ops::Range<u32>,
    foreign: &[std::ops::Range<u32>],
) -> (Vec<std::ops::Range<u32>>, bool) {
    let novel_at = |i: u32| {
        i >= region.start && i < region.end && !foreign.iter().any(|f| f.start <= i && i < f.end)
    };

    let mut runs: Vec<std::ops::Range<u32>> = Vec::new();
    let mut outside_code = false;
    let mut open: Option<std::ops::Range<u32>> = None;
    let mut has_code = false;
    for i in line.start..line.end {
        let code = !source[i as usize].is_ascii_whitespace();
        if novel_at(i) {
            has_code |= code;
            match &mut open {
                Some(r) => r.end = i + 1,
                None => open = Some(i..i + 1),
            }
        } else {
            outside_code |= code;
            if let Some(r) = open.take()
                && has_code
            {
                runs.push(r);
            }
            has_code = false;
        }
    }
    if let Some(r) = open
        && has_code
    {
        runs.push(r);
    }
    (runs, !outside_code)
}

fn span_text(side: char, first: usize, last: usize) -> String {
    if first == last {
        format!("{side}:{first}")
    } else {
        format!("{side}:{first}-{last}")
    }
}

/// One line, quoted, capped, control characters escaped.
fn quote(text: &str, max: usize) -> String {
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

// ---------------------------------------------------------------------------
// The stable text dump the snapshot tests assert on
// ---------------------------------------------------------------------------

/// Render the edit script itself, one operation per line, in script order.
///
/// This is the form the snapshot tests pin. It carries node ids, kinds, line
/// spans and the text of any renamed leaf — enough to see *what* an operation
/// says without re-deriving it, and nothing that varies between runs.
#[must_use]
pub fn render_script(view: &DiffView<'_>) -> String {
    let src_lines = LineIndex::new(view.src.tree.source());
    let dst_lines = LineIndex::new(view.dst.tree.source());
    let a_at = |id: NodeId| {
        let r = src_lines.lines_of(view.src.tree.node(id).byte_range.clone());
        span_text('a', *r.start(), *r.end())
    };
    let b_at = |id: NodeId| {
        let r = dst_lines.lines_of(view.dst.tree.node(id).byte_range.clone());
        span_text('b', *r.start(), *r.end())
    };

    let mut out = String::new();
    let _ = writeln!(out, "summary: {}", render_stat(&view.script.summary));
    if view.script.is_empty() {
        let _ = writeln!(out, "(empty edit script)");
        return out;
    }
    for op in &view.script.ops {
        match *op {
            EditOp::Insert {
                dst,
                dst_parent,
                position,
            } => {
                let parent = dst_parent.map_or_else(|| "-".to_owned(), |p| format!("b#{}", p.0));
                let _ = writeln!(
                    out,
                    "insert   b#{:<4} {:<24} {:<12} parent={parent} pos={position}",
                    dst.0,
                    view.dst.tree.node(dst).kind,
                    b_at(dst),
                );
            }
            EditOp::Delete { src } => {
                let _ = writeln!(
                    out,
                    "delete   a#{:<4} {:<24} {:<12}",
                    src.0,
                    view.src.tree.node(src).kind,
                    a_at(src),
                );
            }
            EditOp::Update { src, dst } => {
                let text = if view.src.metrics.children(src).is_empty() {
                    format!(
                        "   {} → {}",
                        quote(&view.src.tree.node_text(src), 32),
                        quote(&view.dst.tree.node_text(dst), 32)
                    )
                } else {
                    String::new()
                };
                let _ = writeln!(
                    out,
                    "update   a#{:<4} → b#{:<4} {:<24} {:<12} → {:<12}{text}",
                    src.0,
                    dst.0,
                    view.src.tree.node(src).kind,
                    a_at(src),
                    b_at(dst),
                );
            }
            EditOp::Move { src, dst, kind } => {
                let _ = writeln!(
                    out,
                    "move     a#{:<4} → b#{:<4} {:<24} {:<12} → {:<12}   {}",
                    src.0,
                    dst.0,
                    view.src.tree.node(src).kind,
                    a_at(src),
                    b_at(dst),
                    kind.as_str(),
                );
            }
        }
    }
    out
}
