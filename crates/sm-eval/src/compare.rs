//! How a replayed merge is graded against the human resolution, and the two
//! ground-truth-free criteria that need no human resolution at all.
//!
//! # The two equality notions SPEC.md §6.2 requires
//!
//! **Byte-exact** is exactly that: `output == resolved`, byte for byte. It is
//! the only notion that needs no parser and the only one nobody can argue with,
//! and it is unfairly harsh — a human who reindented one block while resolving
//! fails it.
//!
//! **AST-equal modulo formatting** is the interesting one, and the whole value
//! of reporting it depends on saying precisely what it ignores. Here is the
//! complete list:
//!
//! | ignored | counted |
//! |---|---|
//! | whitespace *between* tokens, including indentation and blank lines | every token's text |
//! | the *layout* of a comment: leading/trailing whitespace on each of its lines | every comment's text, and the order comments appear in |
//! | the exact byte offsets of everything | the tree's shape, kind for kind |
//!
//! **Comments count.** That is a deliberate choice and it is the strict one:
//! [`sm_match::structurally_equal`], which does the structural half of the
//! work, drops comments before it compares (they are `extra` nodes and do not
//! participate in a matching), so taking it alone would score "we deleted a
//! Javadoc block the human kept" as a correct resolution. A merge driver that
//! silently drops comments is broken, so the metric must be able to see it.
//! The comment-blind variant is computed too and reported as a sub-line —
//! [`Comparison::ast_equal_ignoring_comments`] — so the report can say how many
//! of the AST-equal-modulo-comments cases differ *only* in comments. That
//! number is the honest measure of how much the choice costs.
//!
//! Comment *layout* is ignored because the emitter is explicitly allowed to
//! rewrite it: reindenting a block that changed nesting depth rewrites the
//! leading whitespace of a block comment's interior lines (see `sm-emit`), so
//! counting it would penalise the one transform SPEC.md §4.6 sanctions.
//!
//! # The ground-truth-free criteria (docs/prior-art.md §8.3.3)
//!
//! Mori & Hashimoto (ASE 2025) grade a merge without a human resolution on two
//! questions, and they are the answer to "the human resolution is not ground
//! truth for what was *correct*":
//!
//! * **parsable** — the output is a syntactically valid program.
//! * **universal** — every hunk of the output is traceable to an input; nothing
//!   was invented.
//!
//! We approximate universality as **zero synthesized bytes** (the driver
//! reports this, and its own self-check refuses to ship a clean merge that
//! synthesized anything but a token separator) **plus token authenticity**:
//! every token of the output is a token of one of the three inputs. The second
//! half is recomputed here from the four trees rather than trusted from the
//! driver's record, which is the point — it is the check that still works when
//! the driver's own is wrong, and it is computable for the `git merge-file`
//! control arm too, which reports no counters at all.
//!
//! Both criteria are only meaningful for a **conflict-free** output. A file
//! with conflict markers in it does not parse and its markers are synthesized
//! by construction, so scoring one would only measure whether it conflicted.

use std::collections::HashSet;

use sm_cst::{Language, NodeId, SourceTree};
use sm_match::{TreeMetrics, structurally_equal};

/// Everything one output/resolution pair is graded on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Comparison {
    /// `output == resolved`, byte for byte.
    pub byte_exact: bool,
    /// The strict notion: same tree, same tokens, same comments. Whitespace and
    /// comment layout differ freely. See the module docs.
    pub ast_equal: bool,
    /// The same, with comments dropped entirely. Always implied by
    /// [`Self::ast_equal`]; the two differ exactly when the merge and the human
    /// agreed on the code and disagreed on the comments.
    pub ast_equal_ignoring_comments: bool,
    /// Both trees parsed, so the AST verdicts above mean something.
    pub comparable: bool,
    /// The output parsed without syntax errors.
    pub output_parsable: bool,
    /// The human resolution parsed without syntax errors. Not our problem when
    /// it does not, but it changes what the AST verdict can say, so it is
    /// recorded.
    pub resolved_parsable: bool,
}

impl Comparison {
    /// True when the two agree on the code and disagree only about comments.
    #[must_use]
    pub fn comment_only_difference(&self) -> bool {
        self.ast_equal_ignoring_comments && !self.ast_equal
    }
}

/// Grade `output` against `resolved`.
///
/// `lang` must be the language both are written in. A file that does not parse,
/// or that parses with `ERROR` nodes in it, is not AST-equal to anything — the
/// tree is not a description of a program at that point — and
/// [`Comparison::comparable`] records which side was at fault.
#[must_use]
pub fn compare(output: &[u8], resolved: &[u8], lang: &dyn Language) -> Comparison {
    let byte_exact = output == resolved;

    let out_tree = parse_clean(output, lang);
    let res_tree = parse_clean(resolved, lang);
    let output_parsable = out_tree.is_some();
    let resolved_parsable = res_tree.is_some();

    let (Some(out_tree), Some(res_tree)) = (out_tree, res_tree) else {
        return Comparison {
            byte_exact,
            // A byte-exact pair is equal under every notion, including when
            // neither side parses. Saying otherwise would be arithmetic
            // nonsense: `byte_exact && !ast_equal` is not a state that exists.
            ast_equal: byte_exact,
            ast_equal_ignoring_comments: byte_exact,
            comparable: false,
            output_parsable,
            resolved_parsable,
        };
    };

    let out_metrics = TreeMetrics::compute(&out_tree, lang);
    let res_metrics = TreeMetrics::compute(&res_tree, lang);
    let structural = structurally_equal(
        &out_tree,
        &out_metrics,
        &res_tree,
        &res_metrics,
        lang,
        NodeId::ROOT,
        NodeId::ROOT,
    );
    let comments_equal = structural && comments(&out_tree, lang) == comments(&res_tree, lang);

    Comparison {
        byte_exact,
        ast_equal: comments_equal,
        ast_equal_ignoring_comments: structural,
        comparable: true,
        output_parsable,
        resolved_parsable,
    }
}

/// Parse, and treat a tree containing `ERROR` nodes as a failure.
fn parse_clean(bytes: &[u8], lang: &dyn Language) -> Option<SourceTree> {
    sm_cst::parse(bytes, lang)
        .ok()
        .filter(|tree| !tree.has_errors())
}

/// Every comment in the file, in document order, with each of its lines
/// stripped of leading and trailing ASCII whitespace.
///
/// The stripping is what makes this "modulo formatting": a block comment whose
/// continuation lines were reindented is the same comment.
/// Sorted by byte offset rather than by arena order: trivia attachment moves
/// comments around in the child lists, and document order is the only ordering
/// that means the same thing in two independently parsed files.
fn comments(tree: &SourceTree, lang: &dyn Language) -> Vec<String> {
    let mut out: Vec<(u32, String)> = Vec::new();
    for id in tree.ids() {
        let node = tree.node(id);
        if !lang.is_comment(node.kind) {
            continue;
        }
        out.push((
            node.byte_range.start,
            normalize_comment(tree.node_bytes(id)),
        ));
    }
    out.sort_by_key(|(start, _)| *start);
    out.into_iter().map(|(_, text)| text).collect()
}

fn normalize_comment(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The ASE-2025-style universality check: is every token of `output` a token of
/// one of the inputs?
///
/// Returns the tokens that are not, capped at `limit` so a pathological case
/// cannot fill the replay log. An empty result means the output is universal on
/// this axis.
///
/// The exclusions match the driver's own `fabricated_token` — comments, tokens
/// containing a newline, whitespace-only and empty leaves — and for the same
/// reason: the reindenter is allowed to rewrite the leading whitespace of a
/// multi-line leaf, so those bytes legitimately differ from every input.
#[must_use]
pub fn fabricated_tokens(output: &SourceTree, inputs: &[&SourceTree], limit: usize) -> Vec<String> {
    let mut known: HashSet<&[u8]> = HashSet::new();
    for tree in inputs {
        for id in tree.leaves() {
            known.insert(tree.node_bytes(id));
        }
    }
    let mut found = Vec::new();
    for id in output.leaves() {
        let node = output.node(id);
        if node.is_extra || node.kind.contains("comment") {
            continue;
        }
        let text = output.node_bytes(id);
        if text.is_empty()
            || text.contains(&b'\n')
            || text.iter().all(u8::is_ascii_whitespace)
            || known.contains(text)
        {
            continue;
        }
        found.push(String::from_utf8_lossy(text).into_owned());
        if found.len() >= limit {
            break;
        }
    }
    found
}

/// A cheap size-of-difference signal, in the spirit of Spork's `diff_size`
/// (docs/prior-art.md §8.3.2): the number of lines that are in one file and not
/// the other, counted as a multiset.
///
/// Not an edit distance — it is symmetric, order-insensitive and linear, which
/// is what makes it affordable over twenty thousand cases. Zero means the two
/// files have the same lines in some order; a small number means the resolution
/// was nearly ours. It exists to rank the incorrect-resolution gallery, not to
/// be quoted as a metric.
#[must_use]
pub fn line_diff_size(a: &[u8], b: &[u8]) -> u64 {
    use std::collections::HashMap;
    let mut counts: HashMap<&[u8], i64> = HashMap::new();
    for line in a.split(|&c| c == b'\n') {
        *counts.entry(line).or_insert(0) += 1;
    }
    for line in b.split(|&c| c == b'\n') {
        *counts.entry(line).or_insert(0) -= 1;
    }
    counts.values().map(|v| v.unsigned_abs()).sum()
}

#[cfg(test)]
mod tests {
    use super::{Comparison, compare, fabricated_tokens, line_diff_size};
    use sm_cst::Language;

    fn java() -> &'static dyn Language {
        sm_cst::languages::detect(std::path::Path::new("x.java")).expect("java is registered")
    }

    fn cmp(a: &str, b: &str) -> Comparison {
        compare(a.as_bytes(), b.as_bytes(), java())
    }

    #[test]
    fn identical_files_are_equal_under_every_notion() {
        let c = cmp(
            "class C { int f() { return 1; } }",
            "class C { int f() { return 1; } }",
        );
        assert!(c.byte_exact && c.ast_equal && c.ast_equal_ignoring_comments && c.comparable);
        assert!(!c.comment_only_difference());
    }

    #[test]
    fn whitespace_differences_are_ignored() {
        let c = cmp(
            "class C {\n  int f() {\n    return 1;\n  }\n}\n",
            "class C { int f() { return 1; } }",
        );
        assert!(!c.byte_exact, "the bytes differ");
        assert!(c.ast_equal, "reindentation must not count as a difference");
        assert!(c.ast_equal_ignoring_comments);
    }

    #[test]
    fn a_changed_token_is_not_ast_equal() {
        let c = cmp(
            "class C { int f() { return 1; } }",
            "class C { int f() { return 2; } }",
        );
        assert!(!c.ast_equal && !c.ast_equal_ignoring_comments);
        assert!(c.comparable);
    }

    #[test]
    fn an_operator_change_is_not_ast_equal() {
        // The case that motivates `ContentItem::Token`: both sides are
        // `binary_expression(identifier, identifier)`.
        let c = cmp(
            "class C { int f(int a, int b) { return a + b; } }",
            "class C { int f(int a, int b) { return a - b; } }",
        );
        assert!(
            !c.ast_equal_ignoring_comments,
            "an anonymous token must count"
        );
    }

    #[test]
    fn a_dropped_comment_is_a_comment_only_difference() {
        let c = cmp(
            "class C { int f() { return 1; } }",
            "class C { /* keep me */ int f() { return 1; } }",
        );
        assert!(
            !c.ast_equal,
            "comments count: a dropped comment is a difference"
        );
        assert!(c.ast_equal_ignoring_comments, "the code itself is the same");
        assert!(c.comment_only_difference());
    }

    #[test]
    fn an_edited_comment_is_a_comment_only_difference() {
        let c = cmp(
            "class C { // was one\n int f() { return 1; } }",
            "class C { // was two\n int f() { return 1; } }",
        );
        assert!(!c.ast_equal);
        assert!(c.comment_only_difference());
    }

    #[test]
    fn reindenting_a_block_comment_is_not_a_difference() {
        let c = cmp(
            "class C {\n    /*\n     * doc\n     */\n    int f() { return 1; }\n}\n",
            "class C {\n/*\n* doc\n*/\nint f() { return 1; }\n}\n",
        );
        assert!(c.ast_equal, "comment layout is formatting, not content");
        assert!(!c.byte_exact);
    }

    #[test]
    fn reordered_comments_are_a_difference() {
        let c = cmp(
            "class C { // a\n // b\n int f() { return 1; } }",
            "class C { // b\n // a\n int f() { return 1; } }",
        );
        assert!(
            !c.ast_equal,
            "comment order is part of the comment sequence"
        );
        assert!(c.ast_equal_ignoring_comments);
    }

    #[test]
    fn unparsable_output_is_never_ast_equal() {
        let c = cmp(
            "class C { int f() { return 1; }",
            "class C { int f() { return 1; } }",
        );
        assert!(!c.comparable);
        assert!(!c.output_parsable);
        assert!(c.resolved_parsable);
        assert!(!c.ast_equal);
    }

    #[test]
    fn byte_exact_survives_both_sides_being_unparsable() {
        // Two identical files that do not parse are still identical, and a
        // metric that said otherwise would make `correct >= byte_exact` false.
        let c = cmp("class C { int f() {", "class C { int f() {");
        assert!(c.byte_exact && c.ast_equal && c.ast_equal_ignoring_comments);
        assert!(!c.comparable);
    }

    #[test]
    fn conflict_markers_make_an_output_unparsable() {
        let c = cmp(
            "class C {\n<<<<<<< ours\n int a = 1;\n=======\n int a = 2;\n>>>>>>> theirs\n}\n",
            "class C {\n int a = 1;\n}\n",
        );
        assert!(!c.output_parsable);
        assert!(!c.ast_equal);
    }

    fn tree(src: &str) -> sm_cst::SourceTree {
        sm_cst::parse(src.as_bytes(), java()).expect("parse")
    }

    #[test]
    fn a_spliced_output_has_no_fabricated_tokens() {
        let base = tree("class C { int a = 1; }");
        let ours = tree("class C { int a = 2; }");
        let theirs = tree("class C { static int a = 1; }");
        let out = tree("class C { static int a = 2; }");
        assert!(fabricated_tokens(&out, &[&base, &ours, &theirs], 8).is_empty());
    }

    #[test]
    fn a_fused_token_is_caught() {
        // The real M4b bug: `static` and `int` spliced with nothing between
        // them. It parses — `staticint` reads as a type name — so only the
        // token check sees it.
        let base = tree("class C { int a = 1; }");
        let ours = tree("class C { int a = 2; }");
        let theirs = tree("class C { static int a = 1; }");
        let out = tree("class C { staticint a = 2; }");
        assert_eq!(
            fabricated_tokens(&out, &[&base, &ours, &theirs], 8),
            vec!["staticint".to_owned()]
        );
    }

    #[test]
    fn the_fabricated_token_list_is_capped() {
        let base = tree("class C { }");
        let out = tree("class C { int aa = 1; int bb = 2; int cc = 3; }");
        assert_eq!(fabricated_tokens(&out, &[&base], 2).len(), 2);
    }

    #[test]
    fn line_diff_size_is_zero_for_a_permutation_and_grows_with_difference() {
        assert_eq!(line_diff_size(b"a\nb\nc", b"c\nb\na"), 0);
        assert_eq!(line_diff_size(b"a\nb", b"a\nb"), 0);
        // One line only in the left, one only in the right.
        assert_eq!(line_diff_size(b"a\nb", b"a\nc"), 2);
    }
}
