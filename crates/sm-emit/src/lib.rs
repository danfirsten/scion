//! Source-range splicing serializer (SPEC.md §4.6, milestone M4).
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use sm_emit::{EmitOptions, emit};
//! use sm_merge::{MergeConfig, merge};
//!
//! let lang = sm_cst::languages::detect("Foo.java".as_ref()).expect("java");
//! let base = sm_cst::parse(&std::fs::read("base.java")?, lang)?;
//! let ours = sm_cst::parse(&std::fs::read("ours.java")?, lang)?;
//! let theirs = sm_cst::parse(&std::fs::read("theirs.java")?, lang)?;
//!
//! let outcome = merge(&base, &ours, &theirs, lang, &MergeConfig::for_language(lang));
//! let result = emit(
//!     &outcome.tree,
//!     &base,
//!     &ours,
//!     &theirs,
//!     lang,
//!     &EmitOptions::default(),
//! );
//! std::fs::write("merged.java", &result.bytes)?;
//! # Ok(()) }
//! ```
//!
//! # The contract
//!
//! Output is produced by **copying byte ranges out of the three inputs**. There
//! is no pretty-printer here and there must never be one (SPEC.md §0.5). The
//! only bytes this crate can write that came from no input are conflict
//! markers, which are visible by construction, and [`sm_merge::Gap::Synthesized`],
//! which the merge does not currently produce. [`EmitResult::synthesized_bytes`]
//! reports the second so a test can assert on it rather than assume it.
//!
//! The one transformation applied to copied bytes is reindentation, described
//! in the `reindent` module: replace the leading whitespace of a line, nothing
//! else, and only when the spliced node's nesting depth changed.
//!
//! # Whitespace ownership
//!
//! Decided by the merge, not here — see `sm-merge`'s crate docs, §9. This crate
//! reads [`sm_merge::MergedTree::lead`] for the gap before each child and
//! [`sm_merge::layout::frame`] for the container's own head and tail. The three
//! tile the container exactly, which is asserted in `sm-merge`'s `layout` tests.
//!
//! ```text
//!            extent(container)
//! ┌──────────────────────────────────────────────────┐
//! │ head │ lead₀ │ item₀ │ lead₁ │ item₁ │ ... │ tail │
//! └──────────────────────────────────────────────────┘
//! ```
//!
//! A container emitted as [`sm_merge::MergedNode::Rebuilt`] writes its own head
//! and tail, and for each merged child writes that child's lead followed by the
//! child. A [`sm_merge::MergedNode::Splice`] writes its node's whole extent —
//! comments included, which is what makes a Javadoc travel with the method it
//! documents.
//!
//! **The root is special**: its extent is the whole file rather than whatever
//! tree-sitter reports for the root node, so leading and trailing blank lines
//! are preserved. That is what makes SPEC.md §4.6's invariant exact.
//!
//! # Conflict markers
//!
//! Standard git markers, with the size git passes as `%L` and the labels it
//! passes as `%X` / `%Y` / `%S`. [`ConflictStyle::Diff3`] adds the ancestor
//! block.
//!
//! Line discipline, which is fiddlier than it looks and is what makes the
//! output reparse:
//!
//! - Before a marker block the emitter rewinds over trailing spaces and tabs
//!   (the indentation the gap was about to place the conflicting element at)
//!   and starts a new line if it is not already at one.
//! - Each side's block starts at the beginning of the line its first node sits
//!   on, so the conflicting code keeps its own indentation, and is terminated
//!   with a newline if it does not already end in one.
//! - The closing `>>>>>>>` line is written **without** a trailing newline, and
//!   the next thing written supplies it — usually the following element's gap,
//!   which begins with one anyway. At end of file the emitter adds it. This is
//!   what stops a blank line appearing after every conflict.
//!
//! # Token separation
//!
//! A splicing emitter's characteristic failure is writing two ranges with
//! nothing between them, so that the last byte of one and the first byte of the
//! next lex as a single token. `sm-merge`'s corpus dry run found it happening
//! for real — `public` + `interface` → `publicinterface`, `static` + `int` →
//! `staticint` — and the `staticint` form *parses*, so nothing downstream
//! notices.
//!
//! The invariant this crate now maintains:
//!
//! > **Between two adjacent emitted items there is at least one byte, whenever
//! > the last byte of the first and the first byte of the second would lex
//! > together.**
//!
//! It is defended in two places, and the order matters:
//!
//! 1. **`sm-merge` picks a gap that is actually valid** (its crate docs, §9): a
//!    copied gap describes a relationship to a particular predecessor, and when
//!    the merge changes that predecessor an *empty* gap is discarded in favour
//!    of a whitespace-only one from a revision where the relationship existed.
//!    This is where the fix belongs — the separator is then a real byte from a
//!    real revision, indented the way that revision indents it, and the output
//!    is still a pure splice.
//! 2. **This crate is the backstop.** If two items still meet with no bytes
//!    between them and [`fuses`] says they would lex together, one space is
//!    written and counted in [`EmitResult::synthesized_separators`]. SPEC.md §5
//!    permits "an explicitly synthesized token"; counting it separately is what
//!    lets the driver keep asserting that nothing *else* was invented.
//!
//! [`fuses`] is deliberately a **curated pair list**, not "both bytes are
//! punctuation". Java and TypeScript both write token pairs that must stay
//! adjacent — `new ArrayList<>()` puts `<` next to `>`, `Map<String,List<X>>`
//! puts `>` next to `>` — and a rule that separated those would corrupt output
//! to prevent a problem that is not there. Missing a fusion is caught by `sm
//! merge`'s own token self-check and costs a fallback; inventing a separator
//! that is not needed changes bytes. The list errs towards the first.
//!
//! # The invariant
//!
//! *Zero conflicts and one side entirely unchanged ⇒ output is byte-identical
//! to the other side's input.* It is not defended here; it falls out. If one
//! side is unchanged, the merge's root decision is a single
//! [`sm_merge::MergedNode::Splice`] of the other side, and splicing the root
//! copies that file. `tests/properties.rs` asserts it anyway, over every
//! scenario and in both directions, because "falls out" is a claim and claims
//! get tested.

mod options;
mod reindent;

pub use options::{ConflictStyle, EmitOptions, EmitResult};

use std::ops::Range;

use sm_cst::{Language, NodeId, SourceTree};
use sm_merge::{Conflict, Gap, MergedId, MergedNode, MergedTree, Side, layout};

use crate::reindent::{
    Shift, apply, line_base_indent, line_indent, output_indent, protected_ranges,
};

/// Render a merged tree to bytes.
///
/// The three trees must be the ones the merge ran on; the merged tree's
/// provenance pointers are `NodeId`s into them.
#[must_use]
pub fn emit(
    tree: &MergedTree,
    base: &SourceTree,
    ours: &SourceTree,
    theirs: &SourceTree,
    lang: &dyn Language,
    opts: &EmitOptions,
) -> EmitResult {
    let mut emitter = Emitter {
        sources: [base.source(), ours.source(), theirs.source()],
        trees: [base, ours, theirs],
        protected: [
            protected_ranges(base, lang),
            protected_ranges(ours, lang),
            protected_ranges(theirs, lang),
        ],
        lang,
        opts,
        out: Vec::with_capacity(ours.source().len() + 64),
        conflict_count: 0,
        synthesized_bytes: 0,
        synthesized_separators: 0,
        reindented_lines: 0,
        pending_newline: false,
    };
    emitter.node(tree, tree.root(), true, None);
    if emitter.pending_newline {
        emitter.out.push(b'\n');
    }
    EmitResult {
        bytes: emitter.out,
        conflict_count: emitter.conflict_count,
        synthesized_bytes: emitter.synthesized_bytes,
        synthesized_separators: emitter.synthesized_separators,
        reindented_lines: emitter.reindented_lines,
    }
}

/// Would these two bytes, written next to each other, lex as one token?
///
/// A **sound-by-omission** approximation: every pair here definitely fuses, and
/// pairs that merely might are left out on purpose. See the crate docs, "Token
/// separation", for why that direction is the safe one.
///
/// - Two identifier bytes. Letters, digits, `_`, `$` and anything ≥ `0x80`
///   (Java and TypeScript identifiers are Unicode, and a UTF-8 lead or
///   continuation byte is never a delimiter). This is the case the corpus hit:
///   `static` + `int`, `public` + `interface`.
/// - Anything that opens or closes a **comment**: `//`, `/*`, `*/`. The worst
///   outcome in the family — it would swallow the rest of the line.
/// - `++` and `--`, which change an expression's meaning rather than break it.
/// - A trailing `=` after an operator, making `==`, `<=`, `+=` and friends.
/// - `&&`, `||`, `::`, `..`, `->`, `=>`.
///
/// Deliberately excluded: `<` followed by `>` (`new ArrayList<>()`), `>`
/// followed by `>` (`Map<String, List<X>>`), and `<` followed by `<`. Those
/// pairs are written adjacent in ordinary source, so separating them would be
/// the emitter corrupting correct output.
#[must_use]
pub fn fuses(a: u8, b: u8) -> bool {
    const fn ident(c: u8) -> bool {
        c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80
    }
    if ident(a) && ident(b) {
        return true;
    }
    matches!(
        (a, b),
        (b'/', b'/' | b'*')
            | (b'*', b'/')
            | (b'+', b'+')
            | (b'-', b'-')
            | (
                b'=' | b'!' | b'<' | b'>' | b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^',
                b'='
            )
            | (b'&', b'&')
            | (b'|', b'|')
            | (b':', b':')
            | (b'.', b'.')
            | (b'-' | b'=', b'>')
    )
}

struct Emitter<'a> {
    sources: [&'a [u8]; 3],
    trees: [&'a SourceTree; 3],
    protected: [Vec<Range<u32>>; 3],
    lang: &'a dyn Language,
    opts: &'a EmitOptions,
    out: Vec<u8>,
    conflict_count: usize,
    synthesized_bytes: usize,
    synthesized_separators: usize,
    reindented_lines: usize,
    /// The last thing written was a `>>>>>>>` line with no newline after it.
    pending_newline: bool,
}

const fn slot(side: Side) -> usize {
    match side {
        Side::Base => 0,
        Side::Ours => 1,
        Side::Theirs => 2,
    }
}

impl<'a> Emitter<'a> {
    // ------------------------------------------------------------ raw writing

    fn write(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if self.pending_newline {
            if bytes[0] != b'\n' {
                self.out.push(b'\n');
            }
            self.pending_newline = false;
        }
        self.out.extend_from_slice(bytes);
    }

    /// Copy a byte range from one revision, applying `shift` to line starts.
    fn span(&mut self, side: Side, range: &Range<u32>, shift: Option<&Shift>) {
        if range.start >= range.end {
            return;
        }
        let i = slot(side);
        let (start, end) = (range.start as usize, range.end as usize);
        match shift {
            None => {
                if self.pending_newline {
                    let first = self.sources[i][start];
                    if first != b'\n' {
                        self.out.push(b'\n');
                    }
                    self.pending_newline = false;
                }
                self.out.extend_from_slice(&self.sources[i][start..end]);
            }
            Some(shift) => {
                if self.pending_newline {
                    if self.sources[i][start] != b'\n' {
                        self.out.push(b'\n');
                    }
                    self.pending_newline = false;
                }
                let mut buf = Vec::with_capacity(end - start);
                let lines = apply(
                    &mut buf,
                    &self.sources[i][start..end],
                    range.start,
                    &self.protected[i],
                    shift,
                );
                self.reindented_lines += lines;
                self.out.extend_from_slice(&buf);
            }
        }
    }

    /// The reindentation shift for a node about to be written at the output's
    /// current position, or `None` if it does not apply.
    fn shift_for(&self, side: Side, extent: &Range<u32>) -> Option<Shift> {
        if !self.opts.reindent {
            return None;
        }
        let target = output_indent(&self.out);
        let source = line_base_indent(self.sources[slot(side)], extent.start as usize);
        (target != source).then(|| Shift {
            from: source.to_vec(),
            to: target.to_vec(),
        })
    }

    // ------------------------------------------------------------- the walker

    fn gap(&mut self, gap: &Gap, shift: Option<&Shift>) {
        match gap {
            Gap::Copied { side, range } => self.span(*side, range, shift),
            Gap::Synthesized(bytes) => {
                self.synthesized_bytes += bytes.len();
                let bytes = bytes.clone();
                self.write(&bytes);
            }
        }
    }

    /// Emit one merged node. `inherited` is the enclosing rebuilt container's
    /// shift, which its children's gaps are placed under.
    fn node(&mut self, tree: &MergedTree, id: MergedId, is_root: bool, inherited: Option<&Shift>) {
        match tree.node(id) {
            MergedNode::Splice { side, node } => {
                let extent = layout::outer_extent(self.trees[slot(*side)], *node, is_root);
                let shift = self.shift_for(*side, &extent);
                self.span(*side, &extent, shift.as_ref().or(inherited));
            }
            MergedNode::Rebuilt {
                side,
                node,
                children,
            } => {
                let (head, tail) =
                    layout::frame(self.trees[slot(*side)], self.lang, *node, is_root);
                let outer = layout::outer_extent(self.trees[slot(*side)], *node, is_root);
                let shift = self.shift_for(*side, &outer).or_else(|| inherited.cloned());
                self.span(*side, &head, shift.as_ref());
                for &child in children {
                    let lead = tree.lead(child).clone();
                    // Where this child's first byte lands, so the separator
                    // backstop can look at the join afterwards. Taken before
                    // the gap, because an empty gap is exactly the case at
                    // issue.
                    let join = self.out.len();
                    self.gap(&lead, shift.as_ref());
                    self.node(tree, child, false, shift.as_ref());
                    self.separate_at(join);
                }
                self.span(*side, &tail, shift.as_ref());
            }
            MergedNode::Conflict(conflict) => self.conflict(conflict),
        }
    }

    /// The token-separation backstop (crate docs, "Token separation").
    ///
    /// `join` is the offset the child's gap was about to be written at. If
    /// nothing was written there — the gap was empty — the bytes on either side
    /// of `join` are the two items' touching tokens, and if they would fuse a
    /// single space goes in between.
    ///
    /// Inserting rather than checking ahead is what makes this exact: "the
    /// first byte this child will write" is a recursive question over splices,
    /// rebuilt frames and empty ranges, and the answer is simply `out[join]`
    /// once the child has been written. The cost is one `Vec::insert` in a case
    /// that is already a bug, and none at all otherwise.
    fn separate_at(&mut self, join: usize) {
        if join == 0 || join >= self.out.len() {
            return;
        }
        if fuses(self.out[join - 1], self.out[join]) {
            self.out.insert(join, b' ');
            self.synthesized_bytes += 1;
            self.synthesized_separators += 1;
        }
    }

    // ------------------------------------------------------------- conflicts

    fn conflict(&mut self, conflict: &Conflict) {
        self.conflict_count += 1;
        self.start_of_line();

        let size = self.opts.effective_marker_size();
        let marker = |ch: u8, label: &str| -> Vec<u8> {
            let mut line = vec![ch; size];
            if !label.is_empty() {
                line.push(b' ');
                line.extend_from_slice(label.as_bytes());
            }
            line.push(b'\n');
            line
        };

        let ours_label = self.opts.ours_label.clone();
        let theirs_label = self.opts.theirs_label.clone();
        let base_label = self.opts.base_label.clone();

        let open = marker(b'<', &ours_label);
        self.write(&open);
        self.side_block(Side::Ours, &conflict.ours);

        if self.opts.style == ConflictStyle::Diff3 {
            let mid = marker(b'|', &base_label);
            self.write(&mid);
            if let Some(base) = &conflict.base {
                self.side_block(Side::Base, base);
            }
        }

        let sep = marker(b'=', "");
        self.write(&sep);
        self.side_block(Side::Theirs, &conflict.theirs);

        let mut close = vec![b'>'; size];
        if !theirs_label.is_empty() {
            close.push(b' ');
            close.extend_from_slice(theirs_label.as_bytes());
        }
        self.write(&close);
        // Deliberately no newline: see the crate docs, "Conflict markers".
        self.pending_newline = true;
    }

    /// Rewind over the indentation a gap just placed, and make sure the next
    /// byte written starts a line.
    fn start_of_line(&mut self) {
        if self.pending_newline {
            self.out.push(b'\n');
            self.pending_newline = false;
        }
        while matches!(self.out.last(), Some(b' ' | b'\t')) {
            self.out.pop();
        }
        if !self.out.is_empty() && self.out.last() != Some(&b'\n') {
            self.out.push(b'\n');
        }
    }

    /// One side of a conflict: the nodes' bytes, extended back to the start of
    /// the line the first one sits on, terminated by a newline.
    fn side_block(&mut self, side: Side, nodes: &[NodeId]) {
        let Some(range) = self.node_run(side, nodes) else {
            return;
        };
        self.span(side, &range, None);
        if self.out.last() != Some(&b'\n') {
            self.out.push(b'\n');
        }
    }

    /// The byte range covering a run of sibling nodes, widened backwards to the
    /// start of the line so the block keeps its indentation.
    fn node_run(&self, side: Side, nodes: &[NodeId]) -> Option<Range<u32>> {
        let tree = self.trees[slot(side)];
        let mut it = nodes.iter().map(|&n| layout::extent(tree, n));
        let first = it.next()?;
        let span = it.fold(first, |acc, r| acc.start.min(r.start)..acc.end.max(r.end));
        let source = self.sources[slot(side)];
        let start = line_indent(source, span.start as usize)
            .map_or(span.start, |indent| span.start - indent.len() as u32);
        Some(start..span.end)
    }
}
