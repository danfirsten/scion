//! The scenario corpus and the harness both `sm-emit` test suites run over.
//!
//! The corpus is deliberately *one* table. Every property test — symmetry,
//! determinism, idempotence, parse stability, byte preservation — runs over all
//! of it, so adding a scenario adds it to every property at once, and a
//! scenario that is only interesting to one of them still gets checked by the
//! rest.

#![allow(dead_code)]

use std::path::Path;

use sm_cst::{Language, SourceTree};
use sm_emit::{EmitOptions, EmitResult, emit};
use sm_merge::{MergeConfig, MergeOutcome, merge};

#[must_use]
pub fn java() -> &'static dyn Language {
    sm_cst::languages::detect(Path::new("x.java")).expect("java is registered")
}

#[must_use]
pub fn typescript() -> &'static dyn Language {
    sm_cst::languages::detect(Path::new("x.ts")).expect("typescript is registered")
}

#[must_use]
pub fn lang_for(name: &str) -> &'static dyn Language {
    match name {
        "java" => java(),
        "ts" => typescript(),
        other => panic!("unknown language {other}"),
    }
}

/// One hand-written three-way merge case.
pub struct Scenario {
    /// Test-visible name; also the key in the gallery snapshot.
    pub name: &'static str,
    /// `"java"` or `"ts"`.
    pub lang: &'static str,
    pub base: &'static str,
    pub ours: &'static str,
    pub theirs: &'static str,
}

impl Scenario {
    #[must_use]
    pub fn language(&self) -> &'static dyn Language {
        lang_for(self.lang)
    }

    /// The same scenario with the two sides swapped.
    #[must_use]
    pub fn mirrored(&self) -> Self {
        Self {
            name: self.name,
            lang: self.lang,
            base: self.base,
            ours: self.theirs,
            theirs: self.ours,
        }
    }
}

/// A merged scenario: the three trees, the merge outcome and the emitted bytes.
pub struct Merged {
    pub base: SourceTree,
    pub ours: SourceTree,
    pub theirs: SourceTree,
    pub outcome: MergeOutcome,
    pub result: EmitResult,
    lang: &'static dyn Language,
}

impl Merged {
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.result.bytes).into_owned()
    }

    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.outcome.is_clean()
    }

    #[must_use]
    pub fn reasons(&self) -> Vec<String> {
        self.outcome
            .conflicts
            .iter()
            .map(|c| format!("{}@{}", c.reason.tag(), c.kind))
            .collect()
    }

    /// Reparse the emitted bytes. Only meaningful for conflict-free output.
    #[must_use]
    pub fn reparse(&self) -> SourceTree {
        sm_cst::parse(&self.result.bytes, self.lang).expect("reparse")
    }
}

/// Merge and emit with the default configuration.
#[must_use]
pub fn run(base: &str, ours: &str, theirs: &str, lang: &'static dyn Language) -> Merged {
    run_with(base, ours, theirs, lang, &EmitOptions::default())
}

/// Merge and emit with explicit emitter options.
#[must_use]
pub fn run_with(
    base: &str,
    ours: &str,
    theirs: &str,
    lang: &'static dyn Language,
    opts: &EmitOptions,
) -> Merged {
    run_bytes(
        base.as_bytes(),
        ours.as_bytes(),
        theirs.as_bytes(),
        lang,
        opts,
    )
}

/// Merge and emit from raw bytes, for fixtures that are not valid UTF-8 text in
/// a Rust literal (CRLF cases, encoding cases).
#[must_use]
pub fn run_bytes(
    base: &[u8],
    ours: &[u8],
    theirs: &[u8],
    lang: &'static dyn Language,
    opts: &EmitOptions,
) -> Merged {
    let base = sm_cst::parse(base, lang).expect("parse base");
    let ours = sm_cst::parse(ours, lang).expect("parse ours");
    let theirs = sm_cst::parse(theirs, lang).expect("parse theirs");
    let outcome = merge(
        &base,
        &ours,
        &theirs,
        lang,
        &MergeConfig::for_language(lang),
    );
    let result = emit(&outcome.tree, &base, &ours, &theirs, lang, opts);
    Merged {
        base,
        ours,
        theirs,
        outcome,
        result,
        lang,
    }
}

#[must_use]
pub fn run_scenario(sc: &Scenario) -> Merged {
    run(sc.base, sc.ours, sc.theirs, sc.language())
}

/// The corpus.
///
/// Every case here is named for the behaviour it pins, and every one of them is
/// something SPEC.md or docs/prior-art.md calls out by name.
#[must_use]
pub fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "disjoint_edits_in_one_method",
            lang: "java",
            base: "class C {\n  void a() {\n    int i = 1;\n    int j = 2;\n  }\n}\n",
            ours: "class C {\n  void a() {\n    int i = 11;\n    int j = 2;\n  }\n}\n",
            theirs: "class C {\n  void a() {\n    int i = 1;\n    int j = 22;\n  }\n}\n",
        },
        Scenario {
            name: "both_add_a_different_method",
            lang: "java",
            base: "class C {\n  void a() {}\n}\n",
            ours: "class C {\n  void a() {}\n\n  void ours() {}\n}\n",
            theirs: "class C {\n  void a() {}\n\n  void theirs() {}\n}\n",
        },
        Scenario {
            name: "both_add_the_same_method",
            lang: "java",
            base: "class C {\n  void a() {}\n}\n",
            ours: "class C {\n  void a() {}\n\n  void shared() { s(); }\n\n  void ourOwn() {}\n}\n",
            theirs: "class C {\n  void a() {}\n\n  void shared() { s(); }\n\n  void theirOwn() {}\n}\n",
        },
        Scenario {
            name: "both_add_an_import_at_the_same_spot",
            lang: "java",
            base: "package p;\n\nimport java.util.List;\n\nclass C {}\n",
            ours: "package p;\n\nimport java.util.List;\nimport java.util.Map;\n\nclass C {}\n",
            theirs: "package p;\n\nimport java.util.List;\nimport java.util.Set;\n\nclass C {}\n",
        },
        Scenario {
            name: "import_replaced_on_one_side_added_on_the_other",
            lang: "java",
            base: "import java.util.HashMap;\n\nclass C {}\n",
            ours: "import java.util.Optional;\n\nclass C {}\n",
            theirs: "import java.util.HashMap;\nimport java.util.List;\n\nclass C {}\n",
        },
        Scenario {
            name: "we_move_a_method_they_edit_its_body",
            lang: "java",
            base: "class C {\n  void a() {\n    x();\n  }\n\n  void b() {\n    y();\n  }\n}\n",
            ours: "class C {\n  void b() {\n    y();\n  }\n\n  void a() {\n    x();\n  }\n}\n",
            theirs: "class C {\n  void a() {\n    x();\n    z();\n  }\n\n  void b() {\n    y();\n  }\n}\n",
        },
        Scenario {
            name: "update_update_the_same_literal",
            lang: "java",
            base: "class C {\n  int x = 1;\n}\n",
            ours: "class C {\n  int x = 2;\n}\n",
            theirs: "class C {\n  int x = 3;\n}\n",
        },
        Scenario {
            name: "delete_versus_modify",
            lang: "java",
            base: "class C {\n  void a() { x(); }\n\n  void b() {}\n}\n",
            ours: "class C {\n  void b() {}\n}\n",
            theirs: "class C {\n  void a() { x(); z(); }\n\n  void b() {}\n}\n",
        },
        Scenario {
            name: "both_delete_the_same_member",
            lang: "java",
            base: "class C {\n  void a() {}\n\n  void b() {}\n}\n",
            ours: "class C {\n  void b() {}\n}\n",
            theirs: "class C {\n  void b() {}\n}\n",
        },
        Scenario {
            name: "wrapped_block_with_an_inner_edit",
            lang: "java",
            base: "class C {\n  void a() {\n    x();\n    y();\n  }\n}\n",
            ours: "class C {\n  void a() {\n    if (ok) {\n      x();\n      y();\n    }\n  }\n}\n",
            theirs: "class C {\n  void a() {\n    x();\n    y2();\n  }\n}\n",
        },
        Scenario {
            name: "enum_constants_inserted_on_both_sides",
            lang: "java",
            base: "enum E {\n  A,\n  B\n}\n",
            ours: "enum E {\n  A,\n  X,\n  B\n}\n",
            theirs: "enum E {\n  A,\n  Y,\n  B\n}\n",
        },
        Scenario {
            name: "reorder_versus_edit_in_a_class_body",
            lang: "java",
            base: "class C {\n  void a() { x(); }\n\n  void b() { y(); }\n\n  void c() { z(); }\n}\n",
            ours: "class C {\n  void c() { z(); }\n\n  void a() { x(); }\n\n  void b() { y(); }\n}\n",
            theirs: "class C {\n  void a() { x(); }\n\n  void b() { y2(); }\n\n  void c() { z(); }\n}\n",
        },
        Scenario {
            name: "comment_edited_on_one_side_code_on_the_other",
            lang: "java",
            base: "class C {\n  // old\n  void a() { x(); }\n}\n",
            ours: "class C {\n  // new\n  void a() { x(); }\n}\n",
            theirs: "class C {\n  // old\n  void a() { x(); z(); }\n}\n",
        },
        Scenario {
            name: "a_trailing_comment_rides_its_moved_owner",
            lang: "java",
            base: "class C {\n  int a = 1; // about a\n\n  int b = 2;\n}\n",
            ours: "class C {\n  int b = 2;\n\n  int a = 1; // about a\n}\n",
            theirs: "class C {\n  int a = 1; // about a\n\n  int b = 22;\n}\n",
        },
        Scenario {
            name: "a_javadoc_rides_its_moved_owner",
            lang: "java",
            base: "class C {\n  /** Doc for a. */\n  void a() {}\n\n  void b() {}\n}\n",
            ours: "class C {\n  void b() {}\n\n  /** Doc for a. */\n  void a() {}\n}\n",
            theirs: "class C {\n  /** Doc for a. */\n  void a() {}\n\n  void b() { changed(); }\n}\n",
        },
        Scenario {
            name: "text_block_interior_is_never_reindented",
            lang: "java",
            base: "class C {\n  void a() {\n    String s = \"\"\"\n        keep\n          me\n        \"\"\";\n  }\n}\n",
            ours: "class C {\n  void a() {\n    if (ok) {\n      String s = \"\"\"\n        keep\n          me\n        \"\"\";\n    }\n  }\n}\n",
            theirs: "class C {\n  void a() {\n    String s = \"\"\"\n        keep\n          me\n        \"\"\";\n    after();\n  }\n}\n",
        },
        Scenario {
            name: "divergent_operators",
            lang: "java",
            base: "class C {\n  void f(int i) {\n    i = 1;\n  }\n}\n",
            ours: "class C {\n  void f(int i) {\n    i += 1;\n  }\n}\n",
            theirs: "class C {\n  void f(int i) {\n    i -= 1;\n  }\n}\n",
        },
        Scenario {
            name: "insertions_at_the_same_anchor_of_a_statement_list",
            lang: "java",
            base: "class C {\n  void a() {\n    p();\n    q();\n  }\n}\n",
            ours: "class C {\n  void a() {\n    p();\n    ours();\n    q();\n  }\n}\n",
            theirs: "class C {\n  void a() {\n    p();\n    theirs();\n    q();\n  }\n}\n",
        },
        Scenario {
            name: "disjoint_insertions_into_a_statement_list",
            lang: "java",
            base: "class C {\n  void a() {\n    p();\n    q();\n  }\n}\n",
            ours: "class C {\n  void a() {\n    first();\n    p();\n    q();\n  }\n}\n",
            theirs: "class C {\n  void a() {\n    p();\n    q();\n    last();\n  }\n}\n",
        },
        Scenario {
            name: "a_reformat_on_one_side_an_edit_on_the_other",
            lang: "java",
            base: "class C { int f() { return 1; } }\n",
            ours: "class C {\n  int f() {\n    return 1;\n  }\n}\n",
            theirs: "class C { int f() { return 2; } }\n",
        },
        Scenario {
            name: "unicode_identifiers_and_literals",
            lang: "java",
            base: "class C {\n  String s = \"héllo\";\n  int café = 1;\n}\n",
            ours: "class C {\n  String s = \"héllo wörld\";\n  int café = 1;\n}\n",
            theirs: "class C {\n  String s = \"héllo\";\n  int café = 2;\n}\n",
        },
        Scenario {
            name: "typescript_class_members",
            lang: "ts",
            base: "class C {\n  a() { return 1; }\n}\n",
            ours: "class C {\n  a() { return 1; }\n  b() { return 2; }\n}\n",
            theirs: "class C {\n  a() { return 1; }\n  c() { return 3; }\n}\n",
        },
        Scenario {
            name: "typescript_named_imports_commute",
            lang: "ts",
            base: "import { a } from \"m\";\n\nexport const x = a;\n",
            ours: "import { a, b } from \"m\";\n\nexport const x = a;\n",
            theirs: "import { a, c } from \"m\";\n\nexport const x = a;\n",
        },
        Scenario {
            name: "typescript_conflicting_constants",
            lang: "ts",
            base: "const x = 1;\n",
            ours: "const x = 2;\n",
            theirs: "const x = 3;\n",
        },
        Scenario {
            name: "no_trailing_newline",
            lang: "java",
            base: "class C {\n  int x = 1;\n}",
            ours: "class C {\n  int x = 2;\n}",
            theirs: "class C {\n  int x = 1;\n  int y = 9;\n}",
        },
        Scenario {
            name: "leading_and_trailing_blank_lines",
            lang: "java",
            base: "\n\nclass C {\n  int x = 1;\n}\n\n\n",
            ours: "\n\nclass C {\n  int x = 2;\n}\n\n\n",
            theirs: "\n\nclass C {\n  int x = 1;\n  int y = 9;\n}\n\n\n",
        },
    ]
}

/// The CRLF corpus, kept separate because the fixtures have to be built rather
/// than written as literals with visible carriage returns.
#[must_use]
pub fn crlf(text: &str) -> Vec<u8> {
    text.replace('\n', "\r\n").into_bytes()
}
