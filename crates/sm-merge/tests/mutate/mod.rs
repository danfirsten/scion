//! Structured mutations: the generator behind `proptest_merge.rs`.
//!
//! # Why not random text
//!
//! A generator that perturbs bytes produces files that do not parse, and a
//! merge of three files that do not parse tests nothing this project cares
//! about — the driver refuses those inputs outright (`sm-cli`'s `merge` module,
//! fallback rung 4). What has to be generated is the situation the merge
//! algorithm exists for: **two plausible edits to the same program**, of the
//! kinds people actually make.
//!
//! So every mutation here is defined over the *parsed tree* and rewrites a
//! chosen node's byte range. `MutationKind` is the taxonomy, and it was chosen
//! from SPEC.md §1 and §4.5 rather than from what was easy:
//!
//! | kind | the merge behaviour it exercises |
//! |---|---|
//! | [`MutationKind::RenameIdentifier`] | update/update on a leaf; and, with M6 in mind, the rename/use pair |
//! | [`MutationKind::ChangeLiteral`] | the smallest possible update; convergence and divergence |
//! | [`MutationKind::InsertStatement`] | ordered child-list insertion |
//! | [`MutationKind::InsertMember`] | unordered child-list insertion — the commonest false conflict |
//! | [`MutationKind::DeleteStatement`] | delete/update escalation |
//! | [`MutationKind::DeleteMember`] | delete/update at declaration granularity |
//! | [`MutationKind::MoveMember`] | move/update, the headline row of the decision table |
//! | [`MutationKind::MoveMemberAcrossContainers`] | reparenting and the covering rule |
//! | [`MutationKind::WrapInIf`] | the wrapped-block case: reindentation plus a new home for old statements |
//! | [`MutationKind::AddImport`] | the set merge, including the duplicate-insertion path |
//! | [`MutationKind::EditComment`] | trivia riding its owner, and `CommentEdit` |
//! | [`MutationKind::Reindent`] | a formatting-only change, which must lose to a content change |
//! | [`MutationKind::BlankLine`] | whitespace ownership at a gap |
//!
//! # The one hard rule
//!
//! **A mutation must produce a file that still parses.** Everything else is
//! negotiable; that is not, because the properties over clean merges are only
//! meaningful when the inputs are programs. Each mutation is written to be
//! syntax-preserving by construction, and [`apply_all`] verifies the result and
//! discards a mutation whose output has parse errors rather than propagating
//! it. `proptest_merge.rs` asserts that discards are rare, so a mutation that
//! quietly stopped applying cannot hide.
//!
//! # Determinism
//!
//! No RNG here at all. A [`Mutation`] carries integer *selectors* which are
//! reduced modulo the number of available sites, so proptest generates plain
//! `u16`s and gets a well-defined mutation for every value — which is what
//! makes shrinking produce something a human can read.

#![allow(dead_code)]

use std::ops::Range;

use sm_cst::{Language, NodeId, SourceTree};

/// What to do. The payload is a selector, not an index: it is taken modulo the
/// number of candidate sites, so every value is valid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MutationKind {
    RenameIdentifier,
    ChangeLiteral,
    InsertStatement,
    InsertMember,
    DeleteStatement,
    DeleteMember,
    MoveMember,
    MoveMemberAcrossContainers,
    WrapInIf,
    AddImport,
    EditComment,
    Reindent,
    BlankLine,
}

impl MutationKind {
    /// Every kind, for a proptest selector.
    pub const ALL: [Self; 13] = [
        Self::RenameIdentifier,
        Self::ChangeLiteral,
        Self::InsertStatement,
        Self::InsertMember,
        Self::DeleteStatement,
        Self::DeleteMember,
        Self::MoveMember,
        Self::MoveMemberAcrossContainers,
        Self::WrapInIf,
        Self::AddImport,
        Self::EditComment,
        Self::Reindent,
        Self::BlankLine,
    ];

    pub const fn tag(self) -> &'static str {
        match self {
            Self::RenameIdentifier => "rename",
            Self::ChangeLiteral => "literal",
            Self::InsertStatement => "insert_stmt",
            Self::InsertMember => "insert_member",
            Self::DeleteStatement => "delete_stmt",
            Self::DeleteMember => "delete_member",
            Self::MoveMember => "move_member",
            Self::MoveMemberAcrossContainers => "move_across",
            Self::WrapInIf => "wrap_if",
            Self::AddImport => "add_import",
            Self::EditComment => "edit_comment",
            Self::Reindent => "reindent",
            Self::BlankLine => "blank_line",
        }
    }
}

/// One mutation: what to do, where, and with which of the fixed templates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Mutation {
    pub kind: MutationKind,
    /// Which candidate site, modulo the number available.
    pub site: u16,
    /// Which template / how far to move / which new name. Also modular.
    pub variant: u16,
}

impl std::fmt::Display for Mutation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}[{}/{}]", self.kind.tag(), self.site, self.variant)
    }
}

/// Apply a sequence of mutations, skipping any that has no site or that would
/// break the parse.
///
/// Returns the mutated source and the mutations that actually applied.
#[must_use]
pub fn apply_all(
    source: &str,
    lang: &'static dyn Language,
    mutations: &[Mutation],
) -> (String, Vec<Mutation>) {
    let mut current = source.to_owned();
    let mut applied = Vec::new();
    for m in mutations {
        let Ok(tree) = sm_cst::parse(current.as_bytes(), lang) else {
            break;
        };
        if tree.has_errors() {
            break;
        }
        let Some(next) = apply_one(&current, &tree, lang, *m) else {
            continue;
        };
        if next == current {
            continue;
        }
        // The hard rule. A mutation that breaks the parse is dropped, not kept.
        match sm_cst::parse(next.as_bytes(), lang) {
            Ok(t) if !t.has_errors() => {
                current = next;
                applied.push(*m);
            }
            _ => {}
        }
    }
    (current, applied)
}

// ------------------------------------------------------------ the mutations

fn apply_one(
    src: &str,
    tree: &SourceTree,
    lang: &'static dyn Language,
    m: Mutation,
) -> Option<String> {
    let java = lang.name() == "java";
    match m.kind {
        MutationKind::RenameIdentifier => rename_identifier(src, tree, m),
        MutationKind::ChangeLiteral => change_literal(src, tree, m),
        MutationKind::InsertStatement => insert_statement(src, tree, m, java),
        MutationKind::InsertMember => insert_member(src, tree, m, java),
        MutationKind::DeleteStatement => delete_in(src, tree, m, &["block"]),
        MutationKind::DeleteMember => delete_in(src, tree, m, body_kinds(java)),
        MutationKind::MoveMember => move_member(src, tree, m, body_kinds(java)),
        MutationKind::MoveMemberAcrossContainers => move_across(src, tree, m, body_kinds(java)),
        MutationKind::WrapInIf => wrap_in_if(src, tree, m),
        MutationKind::AddImport => add_import(src, tree, m, java),
        MutationKind::EditComment => edit_comment(src, tree, m),
        MutationKind::Reindent => reindent(src, tree, m),
        MutationKind::BlankLine => blank_line(src, tree, m),
    }
}

const fn body_kinds(java: bool) -> &'static [&'static str] {
    if java {
        &["class_body", "interface_body", "enum_body_declarations"]
    } else {
        &["class_body", "statement_block"]
    }
}

fn rename_identifier(src: &str, tree: &SourceTree, m: Mutation) -> Option<String> {
    let sites = nodes_of_kind(tree, &["identifier", "type_identifier"]);
    let id = pick(&sites, m.site)?;
    let range = tree.node(id).byte_range.clone();
    let old = &src[range.start as usize..range.end as usize];
    // Suffixing keeps the result a legal identifier whatever the original was,
    // and keeps the rename visible in a failure message.
    let new = format!("{old}_r{}", m.variant % 4);
    Some(splice(src, &range, &new))
}

fn change_literal(src: &str, tree: &SourceTree, m: Mutation) -> Option<String> {
    let sites = nodes_of_kind(
        tree,
        &[
            "decimal_integer_literal",
            "number",
            "string_literal",
            "string",
            "true",
            "false",
        ],
    );
    let id = pick(&sites, m.site)?;
    let range = tree.node(id).byte_range.clone();
    let old = &src[range.start as usize..range.end as usize];
    let new = if old.starts_with('"') || old.starts_with('\'') {
        // Rewrite the interior, keeping whatever quote style was there.
        let q = &old[..1];
        format!("{q}v{}{q}", m.variant % 7)
    } else if old == "true" {
        "false".to_owned()
    } else if old == "false" {
        "true".to_owned()
    } else {
        format!("{}", 100 + u32::from(m.variant % 9))
    };
    Some(splice(src, &range, &new))
}

/// Statement templates. All are legal in a Java *and* a TypeScript block.
const STATEMENTS: [&str; 5] = [
    "log(1);",
    "count = count + 1;",
    "if (ready) { run(); }",
    "helper();",
    "total += 2;",
];

fn insert_statement(src: &str, tree: &SourceTree, m: Mutation, _java: bool) -> Option<String> {
    let sites = nodes_of_kind(tree, &["block", "statement_block"]);
    let id = pick(&sites, m.site)?;
    let open = tree.node(id).byte_range.start as usize;
    // Just after the `{`.
    let at = open + 1;
    let indent = indent_of_line_containing(src, open);
    let text = STATEMENTS[m.variant as usize % STATEMENTS.len()];
    Some(format!("{}\n{indent}  {text}{}", &src[..at], &src[at..]))
}

/// Member templates, per language.
const JAVA_MEMBERS: [&str; 4] = [
    "void generated0() {\n  step();\n}",
    "int generated1 = 7;",
    "private String generated2 = \"x\";",
    "void generated3(int n) {\n  use(n);\n}",
];
const TS_MEMBERS: [&str; 4] = [
    "generated0() {\n  step();\n}",
    "generated1 = 7;",
    "generated2: string = \"x\";",
    "generated3(n: number) {\n  use(n);\n}",
];

fn insert_member(src: &str, tree: &SourceTree, m: Mutation, java: bool) -> Option<String> {
    let sites = nodes_of_kind(tree, body_kinds(java));
    let id = pick(&sites, m.site)?;
    let open = tree.node(id).byte_range.start as usize;
    let at = open + 1;
    let indent = indent_of_line_containing(src, open);
    let members = if java { JAVA_MEMBERS } else { TS_MEMBERS };
    let text = members[m.variant as usize % members.len()].replace('\n', &format!("\n{indent}  "));
    Some(format!("{}\n{indent}  {text}\n{}", &src[..at], &src[at..]))
}

/// Delete a named child of a container of one of `container_kinds`, together
/// with the whitespace in front of it — which is what a human deleting a
/// declaration does, and what keeps the result formatted.
fn delete_in(
    src: &str,
    tree: &SourceTree,
    m: Mutation,
    container_kinds: &[&str],
) -> Option<String> {
    let (_, child) = pick_container_child(tree, m.site, container_kinds, 1)?;
    let range = extent_with_leading_gap(src, tree, child);
    Some(splice(src, &range, ""))
}

fn move_member(
    src: &str,
    tree: &SourceTree,
    m: Mutation,
    container_kinds: &[&str],
) -> Option<String> {
    let (container, child) = pick_container_child(tree, m.site, container_kinds, 2)?;
    let siblings = named_children(tree, container);
    let from = siblings.iter().position(|c| *c == child)?;
    let to = (from + 1 + (m.variant as usize % siblings.len().max(1))) % siblings.len();
    if to == from {
        return None;
    }
    let cut = extent_with_leading_gap(src, tree, child);
    let anchor = siblings[to];
    let insert_at = extent_with_leading_gap(src, tree, anchor).start;
    cut_and_paste(src, &cut, insert_at)
}

/// Remove `cut` and reinsert its text at `insert_at` (an offset in the
/// *original* source). Returns `None` if the destination is inside the cut,
/// which would be meaningless.
fn cut_and_paste(src: &str, cut: &Range<u32>, insert_at: u32) -> Option<String> {
    if cut.start <= insert_at && insert_at < cut.end {
        return None;
    }
    let text = &src[cut.start as usize..cut.end as usize];
    let mut out = String::with_capacity(src.len() + text.len());
    if insert_at <= cut.start {
        out.push_str(&src[..insert_at as usize]);
        out.push_str(text);
        out.push_str(&src[insert_at as usize..cut.start as usize]);
        out.push_str(&src[cut.end as usize..]);
    } else {
        out.push_str(&src[..cut.start as usize]);
        out.push_str(&src[cut.end as usize..insert_at as usize]);
        out.push_str(text);
        out.push_str(&src[insert_at as usize..]);
    }
    Some(out)
}

fn move_across(
    src: &str,
    tree: &SourceTree,
    m: Mutation,
    container_kinds: &[&str],
) -> Option<String> {
    let containers: Vec<NodeId> = nodes_of_kind(tree, container_kinds)
        .into_iter()
        .filter(|c| !named_children(tree, *c).is_empty())
        .collect();
    if containers.len() < 2 {
        return None;
    }
    let from_container = containers[m.site as usize % containers.len()];
    let to_container = containers
        [(m.site as usize + 1 + m.variant as usize % (containers.len() - 1)) % containers.len()];
    if from_container == to_container {
        return None;
    }
    // Never move a container into itself or into its own descendant.
    if contains(tree, from_container, to_container) || contains(tree, to_container, from_container)
    {
        return None;
    }
    let source_children = named_children(tree, from_container);
    let child = *source_children.get(m.variant as usize % source_children.len())?;
    if contains(tree, child, to_container) {
        return None;
    }
    let cut = extent_with_leading_gap(src, tree, child);
    // Just inside the destination container's `{`.
    let insert_at = tree.node(to_container).byte_range.start + 1;
    cut_and_paste(src, &cut, insert_at)
}

/// Wrap a run of 1–3 consecutive statements in `if (…) { }`, reindenting them.
///
/// This is the case SPEC.md §1 opens with and the one a line merge cannot
/// survive, so it is worth the extra care of doing the reindent properly.
fn wrap_in_if(src: &str, tree: &SourceTree, m: Mutation) -> Option<String> {
    let blocks: Vec<NodeId> = nodes_of_kind(tree, &["block", "statement_block"])
        .into_iter()
        .filter(|b| !named_children(tree, *b).is_empty())
        .collect();
    let block = *blocks.get(m.site as usize % blocks.len().max(1))?;
    let stmts = named_children(tree, block);
    if stmts.is_empty() {
        return None;
    }
    let start = m.variant as usize % stmts.len();
    let run = 1 + (m.variant as usize / 3) % 3;
    let end = (start + run).min(stmts.len());
    let first = stmts[start];
    let last = stmts[end - 1];

    let from = line_start(src, tree.node(first).byte_range.start as usize);
    let to = tree.node(last).byte_range.end as usize;
    // A statement that shares its line with something else would be split.
    if src[from..tree.node(first).byte_range.start as usize]
        .bytes()
        .any(|b| !b.is_ascii_whitespace())
    {
        return None;
    }
    let indent = indent_of_line_containing(src, tree.node(first).byte_range.start as usize);
    let body: String = src[from..to]
        .lines()
        .map(|l| {
            if l.trim().is_empty() {
                l.to_owned()
            } else {
                format!("  {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "{}{indent}if (wrapped) {{\n{body}\n{indent}}}{}",
        &src[..from],
        &src[to..]
    ))
}

const JAVA_IMPORTS: [&str; 4] = [
    "import java.util.Map;",
    "import java.util.Set;",
    "import java.util.Optional;",
    "import java.util.List;",
];
const TS_IMPORTS: [&str; 4] = [
    "import { alpha } from \"./alpha\";",
    "import { beta } from \"./beta\";",
    "import { gamma } from \"./gamma\";",
    "import { delta } from \"./delta\";",
];

fn add_import(src: &str, tree: &SourceTree, m: Mutation, java: bool) -> Option<String> {
    let imports = nodes_of_kind(tree, &["import_declaration", "import_statement"]);
    let templates = if java { JAVA_IMPORTS } else { TS_IMPORTS };
    let text = templates[m.variant as usize % templates.len()];
    let at = if let Some(last) = imports.last() {
        tree.node(*last).byte_range.end as usize
    } else if java {
        // After the package declaration if there is one, else at the top.
        nodes_of_kind(tree, &["package_declaration"])
            .first()
            .map_or(0, |p| tree.node(*p).byte_range.end as usize)
    } else {
        0
    };
    if at == 0 {
        Some(format!("{text}\n{src}"))
    } else {
        Some(format!("{}\n{text}{}", &src[..at], &src[at..]))
    }
}

fn edit_comment(src: &str, tree: &SourceTree, m: Mutation) -> Option<String> {
    let sites = nodes_of_kind(tree, &["line_comment", "comment", "block_comment"]);
    let id = pick(&sites, m.site)?;
    let range = tree.node(id).byte_range.clone();
    let old = &src[range.start as usize..range.end as usize];
    // Preserve the comment's own syntax; only its prose changes.
    let new = if old.starts_with("/*") {
        format!("/* edited {} */", m.variant % 5)
    } else if old.starts_with("//") {
        format!("// edited {}", m.variant % 5)
    } else {
        return None;
    };
    Some(splice(src, &range, &new))
}

/// A formatting-only change: indent one statement's line two spaces further.
fn reindent(src: &str, tree: &SourceTree, m: Mutation) -> Option<String> {
    let sites: Vec<NodeId> = nodes_of_kind(tree, &["block", "statement_block"])
        .into_iter()
        .flat_map(|b| named_children(tree, b))
        .collect();
    let id = pick(&sites, m.site)?;
    let start = tree.node(id).byte_range.start as usize;
    let from = line_start(src, start);
    // Only when the statement owns its line, or we would be inserting spaces
    // into the middle of an expression — or worse, inside a string.
    if src[from..start].bytes().any(|b| !b.is_ascii_whitespace()) {
        return None;
    }
    Some(format!("{}  {}", &src[..from], &src[from..]))
}

/// Another formatting-only change: a blank line before a declaration.
fn blank_line(src: &str, tree: &SourceTree, m: Mutation) -> Option<String> {
    let sites: Vec<NodeId> = nodes_of_kind(tree, &["class_body", "block", "statement_block"])
        .into_iter()
        .flat_map(|b| named_children(tree, b))
        .collect();
    let id = pick(&sites, m.site)?;
    let start = tree.node(id).byte_range.start as usize;
    let from = line_start(src, start);
    if from == 0 || src[from..start].bytes().any(|b| !b.is_ascii_whitespace()) {
        return None;
    }
    Some(format!("{}\n{}", &src[..from], &src[from..]))
}

// -------------------------------------------------------------- tree helpers

fn nodes_of_kind(tree: &SourceTree, kinds: &[&str]) -> Vec<NodeId> {
    (0..tree.len())
        .map(|i| NodeId(u32::try_from(i).expect("tree fits in u32")))
        .filter(|id| kinds.contains(&tree.node(*id).kind))
        .collect()
}

fn pick(sites: &[NodeId], selector: u16) -> Option<NodeId> {
    if sites.is_empty() {
        return None;
    }
    Some(sites[selector as usize % sites.len()])
}

/// Named, non-comment children — the things a person would call "the members".
fn named_children(tree: &SourceTree, parent: NodeId) -> Vec<NodeId> {
    tree.node(parent)
        .children
        .iter()
        .copied()
        .filter(|c| {
            let n = tree.node(*c);
            n.is_named && !n.is_extra && !n.kind.contains("comment")
        })
        .collect()
}

/// A container with at least `min_children` named children, and one of them.
fn pick_container_child(
    tree: &SourceTree,
    selector: u16,
    container_kinds: &[&str],
    min_children: usize,
) -> Option<(NodeId, NodeId)> {
    let containers: Vec<NodeId> = nodes_of_kind(tree, container_kinds)
        .into_iter()
        .filter(|c| named_children(tree, *c).len() >= min_children)
        .collect();
    if containers.is_empty() {
        return None;
    }
    let container = containers[selector as usize % containers.len()];
    let children = named_children(tree, container);
    let child = children[(selector as usize / 3) % children.len()];
    Some((container, child))
}

fn contains(tree: &SourceTree, outer: NodeId, inner: NodeId) -> bool {
    let a = tree.node(outer).byte_range.clone();
    let b = tree.node(inner).byte_range.clone();
    a.start <= b.start && b.end <= a.end
}

/// A node's range widened backwards over the whitespace and comments that
/// belong to it — the unit a person cuts when they move a declaration.
fn extent_with_leading_gap(src: &str, tree: &SourceTree, id: NodeId) -> Range<u32> {
    let node = tree.node(id);
    let mut start = node.byte_range.start as usize;
    let line = line_start(src, start);
    if src[line..start].bytes().all(|b| b.is_ascii_whitespace()) {
        start = line;
        // Swallow one preceding blank line, so moving a member does not leave a
        // double blank behind it.
        let mut probe = start;
        while probe > 0 {
            let prev = line_start(src, probe - 1);
            if src[prev..probe].trim().is_empty() {
                probe = prev;
            } else {
                break;
            }
        }
        start = probe;
    }
    u32::try_from(start).expect("fits")..node.byte_range.end
}

fn line_start(src: &str, at: usize) -> usize {
    src[..at].rfind('\n').map_or(0, |i| i + 1)
}

fn indent_of_line_containing(src: &str, at: usize) -> String {
    let from = line_start(src, at);
    src[from..at]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

fn splice(src: &str, range: &Range<u32>, text: &str) -> String {
    let mut out = String::with_capacity(src.len() + text.len());
    out.push_str(&src[..range.start as usize]);
    out.push_str(text);
    out.push_str(&src[range.end as usize..]);
    out
}

// -------------------------------------------------------------- the base pool

/// One starting program.
pub struct Base {
    pub name: &'static str,
    /// A `Language::name`, i.e. `"java"` or `"typescript"`.
    pub lang: &'static str,
    pub source: &'static str,
}

impl Base {
    #[must_use]
    pub fn language(&self) -> &'static dyn Language {
        sm_cst::languages::by_name(self.lang).expect("a registered language")
    }
}

/// The pool the generator starts from.
///
/// Deliberately small, hand-written and *realistic*: files with imports, a
/// class body with several members, comments in the places comments go, nested
/// blocks, and a TypeScript pair so the generator exercises the second grammar
/// rather than assuming Java. Random *programs* would be a different project;
/// what matters here is that the shapes the merge algorithm branches on are all
/// present.
#[must_use]
pub fn bases() -> Vec<Base> {
    vec![
        Base {
            name: "service",
            lang: "java",
            source: "package app;\n\
                     \n\
                     import java.util.List;\n\
                     \n\
                     /** A service. */\n\
                     class Service {\n\
                     \x20 private int count = 0;\n\
                     \n\
                     \x20 // resets everything\n\
                     \x20 void reset() {\n\
                     \x20   count = 0;\n\
                     \x20   log(0);\n\
                     \x20 }\n\
                     \n\
                     \x20 int total(List xs) {\n\
                     \x20   int total = 0;\n\
                     \x20   for (int i = 0; i < 3; i++) {\n\
                     \x20     total += i;\n\
                     \x20   }\n\
                     \x20   return total;\n\
                     \x20 }\n\
                     \n\
                     \x20 void log(int n) {\n\
                     \x20   System.out.println(n);\n\
                     \x20 }\n\
                     }\n",
        },
        Base {
            name: "two_classes",
            lang: "java",
            source: "package app;\n\
                     \n\
                     import java.util.List;\n\
                     import java.util.Map;\n\
                     \n\
                     class Alpha {\n\
                     \x20 int a = 1;\n\
                     \n\
                     \x20 void step() {\n\
                     \x20   a = a + 1;\n\
                     \x20 }\n\
                     }\n\
                     \n\
                     class Beta {\n\
                     \x20 int b = 2;\n\
                     \n\
                     \x20 void run() {\n\
                     \x20   helper();\n\
                     \x20   b = b + 1;\n\
                     \x20 }\n\
                     \n\
                     \x20 void helper() {\n\
                     \x20   b = 0;\n\
                     \x20 }\n\
                     }\n",
        },
        Base {
            name: "nested_blocks",
            lang: "java",
            source: "class Deep {\n\
                     \x20 boolean ready = true;\n\
                     \n\
                     \x20 void go(int n) {\n\
                     \x20   if (ready) {\n\
                     \x20     run();\n\
                     \x20     n = n + 1;\n\
                     \x20   }\n\
                     \x20   while (n > 0) {\n\
                     \x20     n = n - 1;\n\
                     \x20     log(n);\n\
                     \x20   }\n\
                     \x20 }\n\
                     \n\
                     \x20 void run() {\n\
                     \x20   /* body */\n\
                     \x20   log(1);\n\
                     \x20 }\n\
                     \n\
                     \x20 void log(int n) {}\n\
                     }\n",
        },
        Base {
            name: "small",
            lang: "java",
            source: "class Small {\n\
                     \x20 int x = 1;\n\
                     \x20 int y = 2;\n\
                     }\n",
        },
        Base {
            name: "ts_module",
            lang: "typescript",
            source: "import { alpha } from \"./alpha\";\n\
                     \n\
                     // the widget\n\
                     export class Widget {\n\
                     \x20 count = 0;\n\
                     \n\
                     \x20 step() {\n\
                     \x20   this.count = this.count + 1;\n\
                     \x20   log(this.count);\n\
                     \x20 }\n\
                     \n\
                     \x20 reset() {\n\
                     \x20   this.count = 0;\n\
                     \x20 }\n\
                     }\n\
                     \n\
                     export function log(n: number) {\n\
                     \x20 console.log(n);\n\
                     }\n",
        },
        Base {
            name: "ts_two_classes",
            lang: "typescript",
            source: "import { alpha } from \"./alpha\";\n\
                     import { beta } from \"./beta\";\n\
                     \n\
                     class One {\n\
                     \x20 a = 1;\n\
                     \n\
                     \x20 step() {\n\
                     \x20   this.a = this.a + 1;\n\
                     \x20 }\n\
                     }\n\
                     \n\
                     class Two {\n\
                     \x20 b = 2;\n\
                     \n\
                     \x20 run() {\n\
                     \x20   helper();\n\
                     \x20   this.b = this.b + 1;\n\
                     \x20 }\n\
                     \n\
                     \x20 helper() {\n\
                     \x20   this.b = 0;\n\
                     \x20 }\n\
                     }\n",
        },
    ]
}
