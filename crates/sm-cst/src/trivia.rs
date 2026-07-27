//! The trivia attachment pass (SPEC.md §4.2).
//!
//! tree-sitter exposes comments as `extra` nodes, which the grammar allows
//! almost anywhere. That means a comment lands wherever the parser happened to
//! be, as a sibling of whatever else was being parsed — and *which* node a
//! comment belongs to is a question the grammar does not answer. This pass
//! answers it, so that later milestones can move a method and take its Javadoc
//! with it.
//!
//! Attachment **annotates**; it never restructures. Every comment keeps its
//! position in its parent's [`crate::Node::children`], because removing it would
//! break [`crate::invariants::check_byte_coverage`], the mechanical proof that
//! no byte was dropped. All this pass writes is
//! [`crate::Node::leading_trivia`] / [`crate::Node::trailing_trivia`] and the
//! matching [`Attachment`] back-pointer, via the [`SourceTree`] mutators that
//! keep the two directions consistent.
//!
//! # The policy
//!
//! The pass runs **independently within each parent's child list**. A comment
//! can only ever attach to one of its own siblings; it never attaches across a
//! parent boundary. This matters because tree-sitter nests extras as deep as
//! the parse happened to be: the same-looking comment can be a child of
//! `program`, `class_body`, `block`, `modifiers` or `argument_list`, and the
//! reachable candidates differ accordingly. See "Known imperfections" below for
//! the cases where that placement is surprising.
//!
//! A **candidate owner** is a sibling that is *named*, *not itself a comment*,
//! and not a zero-width `MISSING` node inserted by error recovery. Anonymous
//! tokens (`{`, `;`, `)`) are never owners, and neither are comments — a comment
//! cannot own a comment, so a run of comments chains through to the eventual
//! code owner instead.
//!
//! For each comment, in this order:
//!
//! 1. **Trailing.** If the nearest preceding candidate owner `P` *ends on the
//!    same line the comment starts on*, and only comments sit between them, the
//!    comment becomes `Trailing(P)`:
//!    ```java
//!    int x = 1; // like this
//!    ```
//! 2. **Leading.** Otherwise, if the comment can reach a following candidate
//!    owner `O` through a chain of gaps that are whitespace-only and contain at
//!    most [`TriviaConfig::max_leading_gap_newlines`] newlines each, the comment
//!    becomes `Leading(O)`:
//!    ```java
//!    // comment
//!    void f() {}
//!    ```
//! 3. **Floating.** Otherwise it stays [`Attachment::Floating`] and lives on
//!    only at its structural position: a banner comment surrounded by blank
//!    lines, a comment at the end of a block, a file that is nothing but
//!    comments.
//!
//! # Precise definitions
//!
//! **Lines.** A [`LineIndex`] is built over the raw source bytes; the line
//! number of an offset is the number of `\n` bytes before it. `\r\n` therefore
//! counts as exactly one line break (the `\r` is ordinary whitespace on the
//! preceding line, and the `\n` ends it). A **lone `\r`** — classic Mac OS 9
//! line endings — is *not* a line break: such a file reads as a single line, so
//! every comment in it trails its predecessor. That is a deliberate, documented
//! wrong-but-predictable answer rather than a special case nobody would ever
//! exercise; `\n` and `\r\n` cover every line ending a Java file has had for
//! twenty-five years.
//!
//! Because a line number counts the `\n`s before an offset, the number of
//! newlines strictly between two offsets is exactly the difference of their line
//! numbers, which is how the gap rule below is evaluated.
//!
//! **"Same line" (rule 1)** means `line(P.end) == line(C.start)` — equivalently,
//! no `\n` anywhere between the owner's last byte and the comment's first. Note
//! which endpoints those are: for a multi-line block comment, the trailing check
//! uses the comment's **start** line, so
//! ```java
//! foo(); /* a
//!           b */
//! ```
//! still trails `foo()`.
//!
//! **"At most one newline" (rule 2)** is evaluated per gap: the bytes between
//! the comment's **end** and the next node's **start** must be *only* whitespace
//! (space, tab, form feed, `\r`, `\n` — exactly the JLS whitespace set) and must
//! contain no more than `max_leading_gap_newlines` `\n` bytes. With the default
//! of 1, a blank line — two newlines — breaks attachment. Zero newlines
//! qualifies too, so `/* c */ int x;` attaches leading to the field.
//!
//! **Runs.** Consecutive comments chain: each link of the chain is checked
//! separately, comment-end to next-node-start, and the whole run attaches to the
//! same following owner in source order.
//!
//! ```java
//! // a
//! // b
//! void f() {}   // both `a` and `b` are Leading(f)
//! ```
//! ```java
//! // a
//!
//! // b
//! void f() {}   // `a` floats (blank line); `b` is Leading(f)
//! ```
//! Checking each link separately, rather than measuring from the first comment
//! to the owner, is what makes a multi-line block comment in the middle of a run
//! harmless: its own interior newlines are never counted as a gap.
//!
//! **The tie-break.** Rule 1 is tried before rule 2, so a comment that could be
//! read either way goes to the *preceding* node:
//! ```java
//! int x = 1; // trailing
//! void f() {}
//! ```
//! `// trailing` is `Trailing(int x = 1)`, **not** `Leading(f)`. Trailing wins
//! because a comment on the same line as code is overwhelmingly about that code,
//! and because the alternative would drag end-of-line comments along whenever
//! the *next* declaration moves — a visibly wrong result in exactly the case
//! (moved declarations) this project exists to get right.
//!
//! A comment that loses rule 1 to a trailing attachment is still *transparent*
//! for the chain in rule 2: in
//! ```java
//! int x = 1; // t
//! // b
//! void f() {}
//! ```
//! `t` trails the field and `b` still leads `f`.
//!
//! # Known imperfections
//!
//! The goal is to be wrong *predictably*, so these are enumerated rather than
//! papered over:
//!
//! - **A trailing comment that actually describes the next line.** `// see
//!   below` written at the end of a line attaches backwards. Unfixable without
//!   reading English; the tie-break above is the deliberate choice.
//! - **Comments between an annotation and the rest of a signature** are children
//!   of `modifiers`, not of the class body. There the only candidate owners are
//!   the annotations themselves (`marker_annotation`, `annotation`); the
//!   keywords `public`/`static` are anonymous tokens and cannot own anything. So
//!   `@Override\n// why\npublic void f()` leaves `// why` floating inside
//!   `modifiers`. A Javadoc block written *above* the annotations is unaffected:
//!   it is a sibling of `method_declaration` in the class body and attaches to
//!   the method as expected, which is the case that actually matters.
//! - **Comments in argument lists and array initialisers attach forwards.**
//!   `foo(a, // about a\n b)` puts the comment after the `,`, an anonymous
//!   token, which blocks rule 1 (only comments may intervene), so the comment
//!   becomes `Leading(b)`. Allowing rule 1 to skip anonymous tokens would fix
//!   this case and break `for (…; i++) // c` into a comment on `i++`; the strict
//!   reading was chosen because it is the one that can be stated in a sentence.
//! - **A comment on the opening line of a body** — `class A { // c` — has no
//!   preceding candidate owner (the `{` is anonymous), so it leads the first
//!   member instead of trailing the class header.
//! - **A comment at the end of a body floats.** The `}` is anonymous, so there
//!   is nothing after it to lead, and a preceding statement on an earlier line
//!   cannot claim it. This is intentional: such a comment is usually about the
//!   block as a whole.
//! - **A file-level header followed by a blank line floats**, and one glued to
//!   the `package` declaration attaches to it. Both are predictable; the blank
//!   line is the author's own signal about which they meant.

use crate::arena::{Attachment, NodeId, SourceTree};
use crate::language::Language;

/// The knobs on the trivia attachment policy (SPEC.md §4.2 requires it to be
/// "documented, configurable").
///
/// Deliberately small. Every field here changes an answer the pass gives on real
/// Java; nothing is included on the grounds that it might one day be useful.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TriviaConfig {
    /// How many newlines a gap between a comment and the node it would lead may
    /// contain. Default `1`, i.e. the two may be on consecutive lines but a
    /// blank line between them breaks the attachment.
    ///
    /// `0` restricts leading attachment to the same line (`/* c */ int x;`).
    /// Raising it lets a comment reach across blank lines, which is what a
    /// codebase that separates a section banner from its section by a blank line
    /// would want.
    pub max_leading_gap_newlines: u32,

    /// Whether rule 1 runs at all — whether a comment starting on the line a
    /// preceding sibling ends on trails that sibling. Default `true`.
    ///
    /// Setting it to `false` makes every comment either leading or floating,
    /// which is the right model for a language or a house style where end-of-line
    /// comments annotate what follows.
    pub attach_trailing_same_line: bool,
}

impl TriviaConfig {
    /// The default policy: a one-newline leading gap, same-line trailing on.
    /// This is what [`crate::parse`] uses.
    pub const DEFAULT: Self = Self {
        max_leading_gap_newlines: 1,
        attach_trailing_same_line: true,
    };
}

impl Default for TriviaConfig {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Byte offset → line number, for the "same line" and "how many newlines
/// between" questions the policy is written in terms of.
///
/// Lines are delimited by `\n` only; see the module docs for what that means for
/// `\r\n` and for a lone `\r`.
struct LineIndex {
    /// Offset of the first byte of each line. Always starts with `0`, so it is
    /// never empty and a lookup always finds a predecessor.
    line_starts: Vec<u32>,
}

impl LineIndex {
    fn new(source: &[u8]) -> Self {
        // One entry per line; 32 bytes per line is a decent guess for source
        // code and only affects how many times this grows.
        let mut line_starts = Vec::with_capacity(source.len() / 32 + 1);
        line_starts.push(0);
        for (offset, &byte) in source.iter().enumerate() {
            if byte == b'\n' {
                // `offset` fits in u32 because `parse` rejects larger sources.
                line_starts.push(offset as u32 + 1);
            }
        }
        Self { line_starts }
    }

    /// The 0-based line the byte at `offset` sits on, i.e. the number of `\n`
    /// bytes before it. Defined for `offset == source.len()` as well, which is
    /// where a comment that ends the file ends.
    fn line_of(&self, offset: u32) -> u32 {
        // `line_starts[0] == 0 <= offset`, so the partition point is at least 1.
        (self.line_starts.partition_point(|&start| start <= offset) - 1) as u32
    }
}

/// What a sibling is, from the point of view of a comment looking for an owner.
#[derive(Clone, Copy)]
enum Role {
    /// A comment. Never an owner; transparent to a run of comments.
    Comment,
    /// A candidate owner: named, not a comment, and spanning real bytes.
    Owner,
    /// Anything else — an anonymous token, or a zero-width `MISSING` node. Not
    /// an owner, and not transparent: it breaks a run.
    Opaque,
}

/// Attach every comment in `tree` to a sibling, per the policy in the module
/// docs.
///
/// Called by [`crate::parse`] with [`TriviaConfig::DEFAULT`], so every tree in
/// the system arrives attached. It is idempotent and safe to re-run with a
/// different config: each comment is given exactly one decision, and a
/// `Floating` decision detaches whatever was there before.
pub fn attach_trivia(tree: &mut SourceTree, lang: &dyn Language, config: &TriviaConfig) {
    // Plan first, mutate second: the plan reads the whole tree, and the
    // mutators need `&mut`. Decisions are collected in source order per parent,
    // which is what makes `leading_trivia`/`trailing_trivia` come out in source
    // order — the mutators push.
    let mut plan: Vec<(NodeId, Attachment)> = Vec::new();
    {
        let index = LineIndex::new(tree.source());
        for parent in tree.ids() {
            plan_for_parent(tree, lang, config, &index, parent, &mut plan);
        }
    }

    for (comment, decision) in plan {
        match decision {
            Attachment::Leading(owner) => tree.attach_leading(owner, comment),
            Attachment::Trailing(owner) => tree.attach_trailing(owner, comment),
            Attachment::Floating => tree.detach(comment),
        }
    }
}

/// Decide every comment in one parent's child list, appending to `plan` in
/// source order.
fn plan_for_parent(
    tree: &SourceTree,
    lang: &dyn Language,
    config: &TriviaConfig,
    index: &LineIndex,
    parent: NodeId,
    plan: &mut Vec<(NodeId, Attachment)>,
) {
    let children = &tree.node(parent).children;
    // Most nodes have no comment among their children, and this runs on every
    // node of every parse, so bail out before allocating anything.
    if !children
        .iter()
        .any(|&id| lang.is_comment(tree.node(id).kind))
    {
        return;
    }

    let roles: Vec<Role> = children
        .iter()
        .map(|&id| {
            let node = tree.node(id);
            if lang.is_comment(node.kind) {
                Role::Comment
            } else if node.is_named && !node.is_missing {
                Role::Owner
            } else {
                Role::Opaque
            }
        })
        .collect();

    // Pass 1, right to left: rule 2. `owner` is the candidate a comment sitting
    // immediately to the left would lead, and `next_start` is the offset its gap
    // is measured against — the owner's start, or the start of the comment that
    // already chained to it.
    let mut leading: Vec<Option<NodeId>> = vec![None; children.len()];
    let mut owner: Option<NodeId> = None;
    let mut next_start: u32 = 0;
    for i in (0..children.len()).rev() {
        let node = tree.node(children[i]);
        match roles[i] {
            Role::Comment => {
                let reaches = owner.is_some()
                    && gap_qualifies(
                        tree.source(),
                        index,
                        config,
                        node.byte_range.end,
                        next_start,
                    );
                if reaches {
                    leading[i] = owner;
                    next_start = node.byte_range.start;
                } else {
                    // Nothing further left can reach past a comment that could
                    // not itself reach.
                    owner = None;
                }
            }
            Role::Owner => {
                owner = Some(children[i]);
                next_start = node.byte_range.start;
            }
            Role::Opaque => owner = None,
        }
    }

    // Pass 2, left to right: rule 1, then the tie-break. `previous` is the
    // nearest preceding candidate owner with only comments in between — an
    // anonymous token clears it, a comment does not.
    let mut previous: Option<NodeId> = None;
    for i in 0..children.len() {
        let id = children[i];
        match roles[i] {
            Role::Comment => {
                let start_line = index.line_of(tree.node(id).byte_range.start);
                let trailing = previous
                    .filter(|_| config.attach_trailing_same_line)
                    .filter(|&candidate| {
                        index.line_of(tree.node(candidate).byte_range.end) == start_line
                    });
                let decision = match (trailing, leading[i]) {
                    (Some(candidate), _) => Attachment::Trailing(candidate),
                    (None, Some(candidate)) => Attachment::Leading(candidate),
                    (None, None) => Attachment::Floating,
                };
                plan.push((id, decision));
            }
            Role::Owner => previous = Some(id),
            Role::Opaque => previous = None,
        }
    }
}

/// Whether the bytes in `from..to` are a gap a leading attachment may cross:
/// whitespace only, and no more newlines than the config allows.
fn gap_qualifies(
    source: &[u8],
    index: &LineIndex,
    config: &TriviaConfig,
    from: u32,
    to: u32,
) -> bool {
    // Siblings do not overlap (`invariants::check_sibling_order`), but a
    // malformed tree must not panic this pass.
    if to < from || to as usize > source.len() {
        return false;
    }
    // `is_ascii_whitespace` is space, tab, form feed, CR and LF — exactly the
    // JLS whitespace set. Anything else means the gap holds something the
    // grammar did not turn into a node, and crossing it would be a guess.
    if !source[from as usize..to as usize]
        .iter()
        .all(u8::is_ascii_whitespace)
    {
        return false;
    }
    index.line_of(to) - index.line_of(from) <= config.max_leading_gap_newlines
}
