# PROGRESS

## Current milestone: M0 — Scaffold + CST layer (complete)

## Session log

### Session 1 (2026-07-27)

**Status:** M0 scaffold and CST core complete. The trivia attachment pass — the
one remaining M0 exit criterion — is designed for but deliberately not
implemented; the data model, the renderer path and the mutation API for it are
all in place and tested.

**Environment notes:**
- Repository is `danfirsten/scion`; the workspace lives at the repo root (not in a
  `semantic-merge/` subdirectory as the spec's layout sketch shows — the repo root *is*
  the project root).
- Development branch: `claude/semantic-merge-spec-8d684v`.
- Toolchain: Rust 1.94.1 stable. Workspace is edition 2024, `rust-version = 1.85`.

**Done:**
- SPEC.md committed at repo root.
- Cargo workspace at the repo root with all eight crates under `crates/`.
  `sm-cli` produces the `sm` binary.
- `sm-cst`: arena (`SourceTree`/`Node`/`NodeId`), `parse()`, `Language` trait,
  `JavaLanguage`, extension-keyed language registry, structural invariant checks,
  the `sm parse` renderer and the versioned JSON schema.
- `sm-cli`: `sm parse <file> [--no-trivia] [--json] [--max-text N]`.
- Seven Java fixtures and 45 tests: invariants (a)–(d) over every fixture,
  grammar-inventory guards on every configured kind name, an `insta` snapshot of
  the renderer, and JSON schema tests.
- `.github/workflows/ci.yml`: fmt, clippy (`-D warnings`), test.
- README.md.

**Not done (next up in M0):**
- The trivia attachment pass and its test suite. *(Done in session 2.)*

### Session 2 (2026-07-27)

**Status:** M0 complete. The trivia attachment pass is implemented, wired into
`parse()`, documented and tested; every M0 exit criterion is met.

**Done:**
- `crates/sm-cst/src/trivia.rs`: the attachment pass. `attach_trivia(&mut
  SourceTree, &dyn Language, &TriviaConfig)`, a `LineIndex` over the source
  bytes, and a module-level doc comment that *is* the policy specification —
  the rules, the precise definitions, the tie-break and the enumerated known
  imperfections.
- `TriviaConfig { max_leading_gap_newlines: u32 = 1, attach_trailing_same_line:
  bool = true }`, plus `parse_with_trivia_config()`. `parse()` keeps its
  signature and uses `TriviaConfig::DEFAULT`.
- `crates/sm-cst/tests/trivia.rs`: 40 tests. Table-driven over small inline
  snippets — each asserts the *whole* list of comments in the snippet and where
  each one landed, so a policy change shows up as a diff of the policy. Every
  table assertion also re-runs `invariants::check_all` and a back-pointer
  consistency check, because the one thing attachment must never do is
  restructure the tree.
- `crates/sm-cst/tests/fixtures/trivia_gallery.java`: one valid-Java fixture
  exercising every rule, with each comment labelled `g01`…`g29` and annotated
  in-place with the answer the policy gives. Two `insta` snapshots over it: the
  rendered tree, and the attachment table in prose.
- Test count 45 → 85. The existing `render.rs` and `json_schema.rs` tests that
  hand-drove the trivia seam were adjusted: `render.rs` now detaches everything
  first (so it still tests the *printer* rather than the policy), and
  `json_schema.rs` now asserts all three `Attachment` variants against real
  pass output.
- `render__typical_class.snap` re-accepted: 7 lines moved, none added or
  removed. Three Javadoc blocks, one block comment and one line comment moved
  under the declarations they lead; `// mutable, set by rename()` moved under
  the field it trails; the two copyright header lines stayed floating at their
  structural position.

## Decisions

| # | Decision | Rationale |
|---|---|---|
| 1 | Workspace at repo root of `scion`, crates under `crates/` | The repo root is the project root; spec's `semantic-merge/` wrapper directory would just add a level of nesting. |
| 2 | `tree-sitter 0.26.11` + `tree-sitter-java 0.23.5` | Latest of each on crates.io. The grammar is ABI 14, which the 0.26 runtime loads; verified by parsing every fixture. The two crates version independently, so the pairing has to be checked, not assumed. |
| 3 | Edition 2024, `resolver = "3"`, MSRV 1.85 | New project, no legacy constraints. |
| 4 | Source stored as `Vec<u8>`, never `String` | tree-sitter reports byte offsets and the emitter splices byte ranges; the JLS does not require Java source to be UTF-8, and lossy-decoding at the front door would silently rewrite bytes in a tool whose first rule is never to lose data. Display goes through `from_utf8_lossy`; the merge path never decodes. |
| 5 | `byte_range: Range<u32>`, sources > `u32::MAX` rejected with an error | Halves arena memory; merge-driver latency is user-visible. Rejecting is honest, truncating would be a correctness bug. |
| 6 | Node IDs assigned in preorder | Gives the spec's "stable ID within its own tree" for free, plus `parent < child` (so ascending ID order is a topological order) and the property that a node's descendants are a contiguous ID range. Both are asserted by tests so later milestones can rely on them. |
| 7 | Anonymous tokens and `extra` nodes are kept in `children`; `named_children()` is the filtered view | The emitter needs punctuation to splice; the matcher wants named nodes only. Keeping both avoids a second tree. |
| 8 | Syntax errors return `Ok` with `has_errors()`, not `Err` | tree-sitter is error-tolerant and such a tree still accounts for every byte. `Err` is reserved for hard failures (grammar ABI mismatch, source too large, no tree). M4 reads `has_errors()` to decide fallback. |
| 9 | `ChildListKind` has a third variant, `PartiallyUnordered { unordered_kinds }` | Java's `program` node holds the package declaration, the imports and the type declarations in one child list. The imports commute; the package declaration must stay first. A two-valued enum would force either a lost optimisation or a merge that can hoist a class above the package statement. |
| 10 | `class_body` / `interface_body` / `enum_body_declarations` / `annotation_type_body` are Unordered; everything else defaults to Ordered | Members resolve by name, and disjoint member additions from two branches are the most common false conflict in Java history. Documented caveat: field initialisers and `static_initializer` blocks *do* run in textual order — see Known limitations. |
| 11 | `enum_body` is Ordered | Enum constant order defines `ordinal()` and `values()`, and ordinals get persisted. This is exactly the kind of "looks like a set, is not" trap that produces a silently wrong merge. `formal_parameters` and `argument_list` are ordered for the same class of reason. |
| 12 | `JavaLanguage::ORDERED_CONTAINERS` exists even though Ordered is the default | It changes no behaviour; it records which containers were considered and rejected, and the test suite asserts those names still exist in the grammar — which makes the reasoning checkable instead of folkloric. |
| 13 | Every configured kind name is checked against the grammar's node-kind inventory in a test | A typo or an upstream rename would silently disable a rule with no symptom other than worse merges. `tests/language_config.rs` catches both. |
| 14 | The `sm parse` renderer lives in `sm-cst`, not `sm-cli` | Lets the snapshot test assert on exactly the bytes the user sees without shelling out to a binary. |
| 15 | JSON output goes through a hand-written `JsonTree` DTO, not `derive(Serialize)` on `Node` | Makes the wire format a deliberate decision instead of a side effect of internal refactoring. Versioned via `schema_version`. |
| 16 | Trivia attachment was *modelled* one session before it was implemented (decisions 18–28) | `leading_trivia` / `trailing_trivia` / `Attachment` exist, `attach_leading`/`attach_trailing`/`detach` maintain both directions, and the renderer already suppresses attached comments at their structural position and prints them under their owner. Tests drive that path by hand, so the pass inherits a seam already known to work. |
| 17 | `sm parse` exits 0 on a file with syntax errors, 2 only for an unreadable file or unknown extension | Parsing succeeded; the tree is faithful. A broken pipe is also treated as success, so `sm parse Big.java \| head` behaves. |
| 18 | Trivia attachment runs **per parent child list**; a comment never attaches across a parent boundary | tree-sitter nests `extra` nodes wherever the parse happened to be, so the same-looking comment can be a child of `program`, `class_body`, `block`, `modifiers` or `argument_list`. Scoping the pass to one child list is the only rule that stays true at every one of those placements, and it is what stops a comment at the end of a method body from claiming the *next method* one newline below the closing brace. |
| 19 | Candidate owners are **named, non-comment, non-`MISSING`** siblings | Anonymous tokens (`{`, `;`, `,`, `)`) carry no identity a matcher can follow, so an attachment to one could not survive M2. A comment cannot own a comment, so runs chain through to the eventual code owner. `MISSING` nodes are zero-width synthesised repairs with no bytes for the emitter to splice. |
| 20 | **Trailing beats leading** when both rules match | A comment on the same line as code is overwhelmingly about that code. The alternative drags end-of-line comments along whenever the *next* declaration moves — visibly wrong in exactly the moved-declaration case the project exists to get right. |
| 21 | Rule 1 (trailing) requires **only comments** between the owner and the comment; an anonymous token blocks it | This is SPEC.md §4.2's "only whitespace/other comments between" read literally, and it makes both rules state the same intervention condition, which is what lets the policy be described in a sentence. The cost is `foo(a, // about a` — the `,` blocks the trailing rule, so the comment leads `b` instead. The benefit is `for (…; i++) // c` leading the loop body instead of becoming a comment on `i++`. Both are in the module's known-imperfections list. |
| 22 | Rule 2 (leading) is a **chain of per-link gap checks**, not one measurement from the first comment to the owner | It is what makes a run of comments attach as a unit, and it makes a multi-line block comment in the middle of a run harmless: its own interior newlines are never mistaken for a gap. `// a\n// b\nvoid f()` attaches both to `f`; `// a\n\n// b\nvoid f()` floats `a` and attaches `b`. |
| 23 | A gap qualifies iff it is **whitespace only** and holds ≤ `max_leading_gap_newlines` newlines | "Whitespace" is `is_ascii_whitespace` — space, tab, form feed, CR, LF — which is exactly the JLS whitespace set. Anything else in a gap means bytes the grammar did not turn into a node, and crossing them would be a guess. |
| 24 | Lines are counted by `\n` only; `\r\n` is one break and a **lone `\r` is not a break at all** | `\n` and `\r\n` are the only line endings Java source has had in twenty-five years, and treating `\r` as ordinary whitespace keeps the rule to one sentence. The documented consequence: a classic Mac OS 9 file reads as a single line, so every comment in it trails its predecessor — wrong, but predictably wrong, which is the standard SPEC.md §4.2 sets. |
| 25 | The trailing check uses the comment's **start** offset; the leading gap check uses its **end** | A block comment that opens on the owner's last line trails it however far it runs (`foo(); /* a\n b */`), and a block comment that ends one line above a declaration leads it however far back it started. Using one endpoint for both would make multi-line comments attach by accident of length. |
| 26 | A `LineIndex` (offsets of line starts, binary-searched) rather than ad-hoc newline scans | Line numbers are O(1) and the code reads as the policy prose does (`line(P.end) == line(C.start)`). It also gives the newline count in a gap for free, since a line number *is* the count of `\n`s before an offset. One `Vec<u32>` per parse, built in one pass — negligible against the per-node `Vec`s the arena already allocates. |
| 27 | Attachment is planned first and applied second, and every comment gets exactly one decision — including `Floating`, which detaches | Makes the pass idempotent and re-runnable under a different config, which is what `parse_with_trivia_config` and the M5 constant sweep need. Applying decisions in child-list order is also what keeps `leading_trivia`/`trailing_trivia` in source order. |
| 28 | `TriviaConfig` has exactly two fields | Both change an answer the pass gives on real Java: the newline ceiling is the knob a codebase that separates banners from their sections by a blank line would turn, and disabling same-line trailing is the knob for a house style where end-of-line comments annotate what follows. Nothing was added on the grounds that it might one day be useful. |

## Known limitations

- **`class_body` is treated as an unordered set, and that is not strictly true.**
  Instance field initialisers and `static_initializer` blocks execute in textual
  order, so permuting two members where one initialiser reads the other changes
  behaviour. The bet is that the merge algorithm will only ever *insert* into
  these lists rather than permute them, and that a set merge of disjoint
  insertions preserves existing relative order. M5's divergence analysis is where
  this gets tested; the fix, if needed, is to demote `class_body` to
  `PartiallyUnordered`.
- **`annotation_argument_list` is Ordered although named element-value pairs are
  formally order-insensitive.** The single-element form `@Anno(x)` is positional
  and shares the node kind, so the conservative answer was taken.
- **`SCOPE_INTRODUCING` is a first draft.** It is the list from the spec and has
  not been validated against real name-resolution requirements; M6 will revise it
  (`switch` blocks, `try`-with-resources and record compact constructors all
  arguably belong).
- **Parsing throughput is ~2.5 MB/s**, i.e. about 130 ms for a 326 KB / 134k-node
  file, against a p99 budget of 1 s per file (SPEC.md §6.2). Real Java files are
  far smaller (~1 ms for the largest fixture), so this is fine today, but the
  arena build allocates a `Vec` per node and that is the obvious first thing to
  fix if the budget is ever threatened. Not optimised now — there is no corpus
  yet to optimise against.
- **No `Deserialize` for `SourceTree`.** The JSON schema is write-only for now;
  nothing needs to read a tree back, and `Node::kind` being `&'static str` would
  need an interning step to support it.
- **Java only.** TypeScript is M5.

### Where trivia attachment is knowingly wrong

SPEC.md §4.2 asks for a heuristic that is wrong *predictably*. These are the
places it is wrong. All of them are covered by a test that pins the current
answer, so none of them can change silently. The full statement lives in the
module doc of `crates/sm-cst/src/trivia.rs`.

- **A trailing comment that actually describes the next line.** `// see below`
  written at the end of a line attaches backwards, to the code it sits beside.
  Not fixable without reading English; the trailing-beats-leading tie-break is
  the deliberate choice (decision 20).
- **A comment between an annotation and the rest of a signature floats.**
  `@Override` / `// why` / `public void f()` puts the comment inside the
  method's own `modifiers` node, where the only candidate owners are the
  annotations themselves — `public` and `static` are anonymous tokens. Checked
  against tree-sitter-java's actual tree shape rather than assumed. **The case
  that matters is unaffected:** a Javadoc block written *above* the annotations
  is a sibling of `method_declaration` in the class body, and attaches to the
  method exactly as expected, because tree-sitter-java nests the annotations
  inside `modifiers` inside `method_declaration` rather than leaving them
  between the comment and the method. Tested both ways.
- **Comments in argument lists and array initialisers attach forwards.**
  `foo(a, // about a` leads `b` rather than trailing `a`, because the `,` is an
  anonymous token and rule 1 requires only comments to intervene (decision 21).
- **A comment on a body's opening line leads the first member** rather than
  trailing the header: in `class A { // c`, the `{` is anonymous, so nothing
  precedes the comment that could own it.
- **A comment at the end of a body floats.** There is nothing after it but the
  anonymous `}`, and a statement on an earlier line cannot claim it. Usually
  right — such comments are about the block as a whole — but a genuine trailing
  note on the last statement of a block, written on the following line, floats
  when it should trail.
- **A lone-`\r` (Mac OS 9) file reads as a single line**, so every comment in it
  trails its predecessor (decision 24).
- **Nothing understands comment *content*.** `// ---- section ----` banners,
  `// fall through`, commented-out code and `@formatter:off` pragmas are all
  treated as ordinary comments. A banner glued to the declaration below it will
  be dragged along if that declaration moves. Content-aware rules were
  deliberately not invented; if the M5 divergence analysis shows this costing
  real merges, the fix is a `Language`-level "comment is a banner" predicate,
  not a change to the attachment rules.

## Trivia attachment: where it lives

Implemented in session 2. Kept here as a map rather than as a handoff.

- **Policy** — the module doc of `crates/sm-cst/src/trivia.rs` is the
  specification: the three rules, the precise definitions of "same line" and "at
  most one newline", the tie-break with its rationale, and the enumerated known
  imperfections. Read it before changing anything here.
- **Data model** — `crates/sm-cst/src/arena.rs`. `Node::leading_trivia`,
  `Node::trailing_trivia`, `Node::attachment`. Comments stay in their parent's
  `children` list; attachment annotates and never restructures, because removal
  would break `invariants::check_byte_coverage`.
- **Write API** — `SourceTree::attach_leading`, `attach_trailing`, `detach`,
  which keep the forward index and the back-pointer consistent. The pass goes
  through them; so should anything else.
- **Entry points** — `parse()` runs the pass with `TriviaConfig::DEFAULT`;
  `parse_with_trivia_config()` takes an explicit policy; `attach_trivia()` can
  be re-run on an existing tree and fully replaces the previous answer.
- **Tests** — `crates/sm-cst/tests/trivia.rs` (40), plus the
  `trivia_gallery.java` fixture and its two snapshots. `render.rs` still tests
  the printer in isolation by detaching everything first.

## Next

- M0's exit criteria are met: `sm parse Foo.java` prints a readable tree with
  byte ranges and attached comments, and trivia attachment has its own passing
  test suite. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets
  -- -D warnings` and `cargo test --workspace` (85 tests) are green.
- Commit, then await user confirmation before starting M1 (corpus miner).
