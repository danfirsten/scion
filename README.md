# semantic-merge

Git merges lines of text. It has no model of what the code means, and that costs
you twice: it raises **false conflicts** — move a method and every edit to its
body on another branch conflicts; two branches adding an import at the same spot
conflict every time — and, worse, it produces **false clean merges**, where one
branch renames `getUser`, another adds a call to `getUser`, the edits touch
different lines, git merges without complaint and the build breaks.
`semantic-merge` is a git merge driver that parses all three versions with
tree-sitter, matches nodes structurally, and merges on the tree rather than on
lines — and it carries a name-resolution layer that re-resolves every reference
in the candidate merge, so it can catch the second class of failure too.

## Status: M4 + M6 — the merge driver works, is installable, and checks names

`sm merge` is a real git merge driver: it merges Java and TypeScript on the
tree, writes conflict markers when it cannot, falls back to `git merge-file`
whenever it is unsure, and is wrapped so that a panic or a timeout degrades to a
line merge rather than to a damaged file. On a clean merge it also re-resolves
every name in the result and warns about the ones the merge broke.

What is **not** done is the part that matters most: the evaluation. SPEC.md §6
asks for a resolve rate and an incorrect-resolve rate over thousands of mined
merge conflicts, and until that exists you should read the paragraph above as a
description of the code, not as a claim about quality.

> **Evaluation pending (M5).** No resolve-rate or incorrect-resolve-rate numbers
> are published yet. They land in M5, together with the failure-mode analysis
> and a comparison against Mergiraf.

Name binding — the part that catches a rename/use collision that git and every
other syntactic merge tool miss — exists and runs inside the driver by default,
in report-only mode. What it can see is bounded in a way that has to be stated
up front: **a merge driver is handed one file at a time**, so it catches the
*within-file* case and nothing cross-file. How often that fires on real merges
is one of the numbers M5 will produce. See "The semantic check" below.

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

To turn the semantic check into a **gate** rather than a warning, add
`--semantic=conflict` to the driver line — it is a one-word edit to the config
`install-driver` wrote, and it is per-repository:

```sh
git config merge.semantic.driver \
    "$(command -v sm) merge %O %A %B %L %P %S %X %Y --semantic=conflict"
```

Read "The semantic check" below before you do: in that mode git marks the file
as conflicted even though the file itself has no conflict markers in it.

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
`--semantic off|report|conflict` (default `report`, see below), `--output PATH`
to write somewhere other than `%A`, `--no-fast-path` and `--line-merge-only` to
force one route or the other, and `--debug-json PATH` for a machine-readable
record of what the driver did and how long each stage took.

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
   must invent no bytes beyond a token separator (below), must itself parse, and
   **must not contain a token that appears in none of the three inputs**.

That last check exists because of a real splicing bug: two adjacent spliced
ranges written with nothing between them, so `static` and `int` came out as
`staticint` — in output that parses perfectly well, because `staticint` is a
valid type name. The bug itself is fixed, in the merge (which now checks that a
copied gap still describes the pair of items it is between) with a lexical
backstop in the emitter (which will write one space rather than let two tokens
fuse, and counts it). The check stays anyway: it re-derives its answer from the
four syntax trees rather than trusting the fix, and the whole symptom of this
class of bug is output that looks fine.

If `git merge-file` *also* fails, the driver writes nothing, leaves `%A` exactly
as it found it, and exits 2.

## The semantic check

Git cannot see that one branch renamed `getUser` while the other added a call to
`getUser`. Neither can a purely syntactic tree merge. So after the tree merge
comes out **clean**, the driver builds a scope tree for each revision *and for
the merged result*, re-resolves every reference, and reports the ones whose
binding the merge changed.

Everything it reports is **differential**: a name is only reported when it
resolved in the branch it came from and stops resolving, or starts resolving
somewhere else, once the two branches are put together. A name that resolved to
nothing in its own branch — a library name, an inherited member, anything from
another file — is silent. That rule is what keeps the false-positive rate near
zero despite the resolver being deliberately simple.

```
$ sm merge ... # (git runs this)
semantic-merge: warning: Repo.java: `ours` renamed method `getUser` to
  `fetchUser`; the reference to `getUser` from `theirs` still says `getUser`,
  which no longer resolves in the merged result.
semantic-merge: warning: Repo.java: 1 semantic conflict (broken_reference);
  the merge itself was clean
```

| `--semantic` | what happens |
|---|---|
| `off` | not run |
| `report` (default) | findings printed to stderr; **exit code unchanged** |
| `conflict` | findings printed, and the driver exits 1 |

`report` is the default on purpose: a merge driver that starts refusing merges
on a new heuristic is a merge driver that gets uninstalled.

**What `conflict` mode actually does to your working tree.** There is no textual
disagreement here — both branches' edits belong in the result and both are in
it — so writing conflict markers would be inventing a choice that does not
exist, and would destroy the merged file you need to look at. Instead the clean
merged text is written to the file and the driver exits 1. Git treats that as a
content conflict, so:

- `git merge` prints `CONFLICT (content): Merge conflict in Repo.java` and does
  not commit;
- `git status` lists the file under "Unmerged paths" as "both modified";
- all three stages stay in the index, so `git checkout --ours/--theirs` and
  `git merge --abort` work normally;
- **the file itself has no conflict markers in it.**

The tradeoff is that last line: a user who ignores stderr sees a file git calls
conflicted with nothing visibly wrong in it. That is why it is opt-in. The
resolution is ordinary — read the explanation, fix the call or decide it is
fine, `git add`, `git commit`.

### What it does not see

- **Anything cross-file.** Git invokes a merge driver per path, with three blobs
  and no project. If the rename is in `UserService.java` and the new call is in
  `OrderController.java`, nothing here will notice. This is the big one, and it
  is a property of where the check runs, not of the analysis.
- **The fast path skips it.** When `git merge-file` produced a clean result that
  parses, the driver ships those bytes without building a tree, and there is no
  merge plan to check. A line-clean merge is exactly where a broken reference
  hides, so this is a real gap — accepted for now because it is also what every
  other tool ships, and because running it there would cost three parses and a
  merge on every invocation. `sm check` and `--no-fast-path` both reach it, which
  is how M5 will measure the population before anyone pays for it online.
- **No types and no inheritance.** `user.getName()` is a member reference and is
  never resolved; a name inherited from a superclass in another file resolves to
  nothing. Both are false *negatives* — a missed conflict, never a wrong one.

The full list of simplifications, each with the direction it fails in, is in
`crates/sm-bind/src/lib.rs`.

## `sm parse`, `sm match`, `sm diff`, `sm check`

Inspection tools for the layers underneath, not part of the merge path.

```sh
sm parse <file> [--no-trivia] [--json] [--max-text N]   # the concrete syntax tree
sm match <a> <b>                                        # the structural matching
sm diff  <a> <b>                                        # the edit script, moves reported as moves
sm check <base> <ours> <theirs> [--json]                # the semantic check, on three files
```

`sm check` runs the same check the driver runs, on three files you name, with no
git repository involved — which is both how M5's corpus scan drives it and the
shortest way to see the feature work. It exits 1 when it finds something:

```
$ sm check base.java ours.java theirs.java
tree merge: clean
references: 11 (resolved in origin 5, unresolved in origin 6), skipped conflict regions: 0
semantic conflicts: 1

  [broken_reference] `getUser`
  reference: theirs "getUser"
  was: method `getUser` in theirs scope `class_declaration`
  `ours` renamed method `getUser` to `fetchUser`; the reference to `getUser`
  from `theirs` still says `getUser`, which no longer resolves in the merged result.
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
- **Name resolution is single-file and skips the fast path.** The
  false-clean-merge class of bug is caught *within one file*, and only on merges
  git could not do on its own. See "The semantic check".
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
