# semantic-merge

Git merges lines of text. It has no model of what the code means, and that costs
you twice: it raises **false conflicts** — move a method and every edit to its
body on another branch conflicts; two branches adding an import at the same spot
conflict every time — and, worse, it produces **false clean merges**, where one
branch renames `getUser`, another adds a call to `getUser`, the edits touch
different lines, git merges without complaint and the build breaks.
`semantic-merge` is a git merge driver that parses all three versions with
tree-sitter, matches nodes structurally, and merges on the tree rather than on
lines — with a name-resolution layer planned so it can catch the second class of
failure too.

## Status: M0 — scaffold and CST layer

Nothing here merges anything yet. What works today is the concrete syntax tree
layer and a `sm parse` command to inspect it:

- `sm-cst` parses Java into a preorder arena with exact byte ranges, keeps every
  byte of the input (whitespace, comments, punctuation), and exposes the
  language-agnostic `Language` trait that the rest of the pipeline is written
  against.
- **Trivia attachment** decides which declaration each comment belongs to — a
  Javadoc block leads the method below it, `// like this` trails the statement it
  sits on, a banner between blank lines floats. The policy is documented and
  configurable (`TriviaConfig`), and has its own test suite; the comment nodes
  themselves never move, so no byte is ever lost.
- The structural invariants — range containment, sibling ordering, byte
  coverage, preorder ID numbering — are enforced by tests over a fixture corpus,
  not just asserted in prose.
- The other six crates (`sm-match`, `sm-diff`, `sm-merge`, `sm-emit`, `sm-bind`,
  `sm-eval`) are empty. Each has a doc comment describing the role it will play
  and no code, so nothing can accidentally depend on a stub.

Not yet implemented, in the order it is coming: corpus mining (M1), GumTree
matching (M2), structural diff (M3), **the three-way merge and the git driver
(M4)**, the evaluation (M5), and name binding (M6). See `SPEC.md` for the full
plan and `PROGRESS.md` for where things actually stand.

## Build

Requires a stable Rust toolchain (1.85 or newer — the workspace is on edition
2024).

```sh
cargo build --release      # binary at target/release/sm
cargo test --workspace
```

## `sm parse`

```sh
sm parse <file> [--no-trivia] [--json] [--max-text N]
```

Prints the concrete syntax tree: one node per line, indented by depth, with each
node's kind and byte range, and the source text of nodes whose text carries
meaning (identifiers, literals, comments).

```
$ sm parse crates/sm-cst/tests/fixtures/typical.java
crates/sm-cst/tests/fixtures/typical.java  language=java  nodes=230  parse=0.312ms
program [0..1308]
  line_comment [0..45] extra "// Copyright 2026 the semantic-merge aut"...
  package_declaration [93..123]
    package [93..100]
    scoped_identifier [101..122]
      ...
```

- `--no-trivia` hides comments. A display option only; the tree always retains
  them.
- `--json` dumps the arena as JSON with a versioned schema, for tooling.
- Exit code is `0` on success and `2` for an unreadable file or an unsupported
  file type. A file with **syntax errors still exits 0**: tree-sitter is
  error-tolerant and the tree is a faithful description of the file. A warning is
  printed, and it is that same signal the merge driver will use to fall back to
  git's line merge.

## Merge

`sm merge` does not exist yet. It lands in **M4**, together with the git driver
registration, the conflict model and the mandatory fallback to `git merge-file`.
Until then this repository is a parser and an inspection tool, and installing it
as a merge driver would do nothing useful.

## Supported languages

Java only. TypeScript arrives in M5, specifically to test whether the `Language`
abstraction actually holds.

## License

MIT OR Apache-2.0.
