# PROGRESS

## Current milestone: M0 — Scaffold + CST layer (core done; trivia attachment outstanding)

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
- The trivia attachment pass and its test suite. See "Handoff: trivia
  attachment" below.

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
| 16 | Trivia attachment is *modelled* in M0 but not implemented | `leading_trivia` / `trailing_trivia` / `Attachment` exist, `attach_leading`/`attach_trailing`/`detach` maintain both directions, and the renderer already suppresses attached comments at their structural position and prints them under their owner. Tests drive that path by hand, so the pass inherits a seam already known to work. |
| 17 | `sm parse` exits 0 on a file with syntax errors, 2 only for an unreadable file or unknown extension | Parsing succeeded; the tree is faithful. A broken pipe is also treated as success, so `sm parse Big.java \| head` behaves. |

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

## Handoff: trivia attachment (the next piece of M0)

The policy to implement is SPEC.md §4.2: a comment attaches to the *following*
named sibling if separated by at most one newline; otherwise to the *preceding*
sibling if on the same line; otherwise it floats. It must be documented,
configurable and have its own test suite.

Everything it needs is already in place:

- **Data model** — `crates/sm-cst/src/arena.rs`. `Node::leading_trivia`,
  `Node::trailing_trivia`, `Node::attachment` (`Attachment::{Leading, Trailing,
  Floating}`). All comments parse as `Floating` with empty trivia vectors.
- **Write API** — `SourceTree::attach_leading`, `attach_trailing`, `detach`.
  These keep the forward index and the back-pointer consistent; do not reach
  around them.
- **Where to invoke it** — `crates/sm-cst/src/parse.rs`, in `parse()`, after
  `SourceTree::new(...)` and before the `Ok(...)`. Add a `mod trivia;` to
  `lib.rs` and call it there so every tree in the system is attached.
- **Rendering** — already done. `crates/sm-cst/src/render.rs` collects every
  attached comment ID, suppresses it at its structural position and prints it as
  a `leading:`/`trailing:` line indented under its owner. Do not touch the
  printer. `crates/sm-cst/tests/render.rs::attached_trivia_renders_under_its_owner_and_only_there`
  drives this path by hand and asserts the line count is unchanged.
- **What must not change** — comments stay in their parent's `children` list.
  Attachment annotates the tree; it never removes nodes, because removal would
  break `invariants::check_byte_coverage`, which the test suite runs over every
  fixture.
- **Fixtures** — `crates/sm-cst/tests/fixtures/` already covers Javadoc, line and
  block comments, trailing same-line comments, a comment-only file and an empty
  file. `only_comment.java` is the degenerate case: three comments and no named
  sibling to attach any of them to.

## Next

- Implement trivia attachment + its test suite; then M0's exit criteria are met.
- Commit, then await user confirmation before starting M1 (corpus miner).
