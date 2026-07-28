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

## Status: M4 — the merge driver works and is installable

`sm merge` is a real git merge driver: it merges Java and TypeScript on the
tree, writes conflict markers when it cannot, falls back to `git merge-file`
whenever it is unsure, and is wrapped so that a panic or a timeout degrades to a
line merge rather than to a damaged file.

What is **not** done is the part that matters most: the evaluation. SPEC.md §6
asks for a resolve rate and an incorrect-resolve rate over thousands of mined
merge conflicts, and until that exists you should read the paragraph above as a
description of the code, not as a claim about quality.

> **Evaluation pending (M5).** No resolve-rate or incorrect-resolve-rate numbers
> are published yet. They land in M5, together with the failure-mode analysis
> and a comparison against Mergiraf.

Name binding — the part that could catch a rename/use collision that git and
every other syntactic merge tool miss — is M6 and does not exist yet. Nothing in
this tool currently understands what a name refers to.

## Install

Requires a stable Rust toolchain (1.85 or newer — the workspace is on edition
2024) and git. Git **2.44 or newer** is recommended: older versions do not
expand the `%S` / `%X` / `%Y` conflict-label placeholders, and the driver then
falls back to generic `ours` / `base` / `theirs` labels.

```sh
cargo build --release          # binary at target/release/sm
```

Then, from inside the repository you want to use it in:

```sh
sm install-driver --local --langs java,ts,tsx
```

That writes, via `git config`, into `.git/config`:

```ini
[merge "semantic"]
    name = semantic-merge: AST-aware three-way merge
    driver = /abs/path/to/sm merge %O %A %B %L %P %S %X %Y
    recursive = text
```

and prints the `.gitattributes` lines you still need. Add them and commit them —
they are a project decision, not a per-clone one:

```
*.java merge=semantic
*.ts merge=semantic
*.tsx merge=semantic
```

Or let the command do it: `sm install-driver --local --write-attributes
.gitattributes`. Re-running it never duplicates a rule.

Other flags: `--global` writes to `~/.gitconfig` instead; `--driver-path` sets
the command git runs (it defaults to the absolute path of the binary you
invoked, because `sm` is often not on the `PATH` of a GUI client or a hook);
`--recursive binary|text|semantic` picks the driver used for merging virtual
ancestors in a criss-cross merge; `--dry-run` prints what it would do.

### Uninstall

```sh
sm install-driver --local --uninstall
```

then delete the `merge=semantic` lines from `.gitattributes`. Removing either
one alone is safe: without the config git falls back to its own merge with a
warning, and without the attributes the config is simply never consulted.

## `sm merge`

You should not need to run this by hand; git runs it. The contract:

```sh
sm merge <base> <ours> <theirs> [marker-size] [pathname] [base-label] [ours-label] [theirs-label]
```

corresponding to git's `%O %A %B %L %P %S %X %Y`. **The result is written to
`<ours>` — `%A` — which is both an input and the destination.** It is replaced
atomically: a temporary in the same directory, `fsync`, then `rename`, so the
file is either entirely the old bytes or entirely the new ones, never a prefix.

| exit code | meaning | what is in `%A` |
|---|---|---|
| `0` | merged cleanly | the merged file |
| `1` | conflicts remain | the merged file, with conflict markers |
| `2` | the driver failed | **untouched** |

Exit 2 is the safety valve. Git treats any non-zero exit as "conflicts remain"
and keeps whatever is in `%A`, so leaving it untouched degrades to exactly the
state of never having installed the driver.

Useful flags: `--timeout-ms N` (default 5000, `0` disables), `--max-bytes N`
(default 5 MiB), `--diff3` for conflict markers that include the ancestor,
`--output PATH` to write somewhere other than `%A`, `--no-fast-path` and
`--line-merge-only` to force one route or the other, and `--debug-json PATH` for
a machine-readable record of what the driver did and how long each stage took.

## How it decides

**The line merge runs first, always.** `git merge-file` is invoked on the three
inputs; if it comes back conflict-free and its output parses, that is the answer
and no tree is ever built. The consequences are worth being explicit about:

- On files git can already merge — the overwhelming majority — the output is
  **byte-identical to what you would have got without this tool**. The
  regression risk on that population is zero by construction, not by testing.
- The tree machinery only ever sees files git could not merge, which is where it
  can help and where a slower path is affordable.

Only when the line merge conflicts does the structural merge run: parse all
three revisions, match nodes structurally against the ancestor, merge the trees,
and emit by **splicing byte ranges out of the input files**. Untouched code is
never reprinted or reformatted; the only transformation applied to copied bytes
is a leading-whitespace adjustment when a block's nesting depth changed.

### When it gives up

Any of these sends the merge to `git merge-file` and uses its result verbatim,
including its exit code:

1. `%P` has no extension we have a grammar for.
2. An input is larger than `--max-bytes`.
3. **Any of the three inputs has a syntax error.** A tool that guesses at broken
   input is worse than one that declines.
4. The structural merge exceeds `--timeout-ms`.
5. Anything panics — the whole structural path runs inside `catch_unwind`.
6. The output fails a self-check. A clean merge must emit no conflict markers,
   must synthesize no bytes, must itself parse, and **must not contain a token
   that appears in none of the three inputs** — the last one catches a real
   splicing bug where two adjacent tokens fuse into one (`static int` becoming
   `staticint`) in output that still parses.

If `git merge-file` *also* fails, the driver writes nothing, leaves `%A` exactly
as it found it, and exits 2.

## `sm parse`, `sm match`, `sm diff`

Inspection tools for the layers underneath, not part of the merge path.

```sh
sm parse <file> [--no-trivia] [--json] [--max-text N]   # the concrete syntax tree
sm match <a> <b>                                        # the structural matching
sm diff  <a> <b>                                        # the edit script, moves reported as moves
```

```
$ sm parse crates/sm-cst/tests/fixtures/typical.java
crates/sm-cst/tests/fixtures/typical.java  language=java  nodes=230  parse=0.312ms
program [0..1308]
  line_comment [0..45] extra "// Copyright 2026 the semantic-merge aut"...
  package_declaration [93..123]
    ...
```

`sm parse` exits 0 even on a file with syntax errors — the parse succeeded and
the tree is faithful — and prints a warning. That same warning is the signal the
merge driver uses to fall back.

## Supported languages

Java and TypeScript/TSX. TypeScript exists to test whether the `Language`
abstraction actually holds, and it does carry the full pipeline, but Java is
where the design decisions were made and where the evaluation will run.

## Honest limitations

- **No published numbers yet.** See the status section. Everything below is a
  known shortcoming rather than a measured one.
- **Whole-file fallback only.** A single unparseable or pathological file
  degrades entirely to a line merge; there is no per-method fallback, so one bad
  region costs the whole file's structural merge.
- **No name resolution.** The false-clean-merge class of bug — rename on one
  branch, new use on the other — is *not* caught. That is M6 and it is the only
  genuinely novel part of the project.
- **`class_body` is merged as an unordered set**, which is not strictly true:
  instance field initialisers and static blocks run in textual order.
- **Two branches adding methods with the same signature at different places
  both apply**, where Spork and Mergiraf would conflict.
- **Multi-line leaves are atomic**: a block comment or text block edited on both
  sides conflicts wholesale instead of merging line by line.
- **Comment placement is a heuristic.** It is documented and predictable (see
  `crates/sm-cst/src/trivia.rs`), and it is sometimes wrong.
- **Parsing is the latency floor**, at roughly 2.5 MB/s. On mined real-world
  conflicts the driver's median end-to-end time is tens of milliseconds and its
  p99 a few hundred; the fast path is a few milliseconds.

## Build and test

```sh
cargo build --release
cargo test --workspace
```

The test suite is three layers, per SPEC.md §5: unit and `insta` snapshot tests
per crate; property tests over generated three-way merges
(`crates/sm-merge/tests/proptest_merge.rs`) asserting the identity laws,
symmetry, idempotence, parse stability and byte preservation; and end-to-end
tests that drive the real binary, including `crates/sm-cli/tests/dogfood.rs`,
which installs the driver in a throwaway repository and runs `git merge`.

## Documentation

`SPEC.md` is the plan, `PROGRESS.md` is the running record of decisions and
limitations, and `docs/prior-art.md` is a verified survey of GumTree, Mergiraf,
difftastic and Spork. The crate-level doc comments on `sm-merge`, `sm-emit` and
`sm-cli`'s `merge` module are the design records for the merge algorithm, the
emitter and the driver respectively; read those before changing them.

## License

MIT OR Apache-2.0.
