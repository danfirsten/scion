# Project Spec: `semantic-merge` — an AST-aware three-way merge driver for git

**How to use this document:** paste it into Claude Code as the opening message of a fresh session, then say: *"Read this spec. Confirm your understanding, then begin M0. Do not skip ahead."* Keep the file at the repo root as `SPEC.md` so it can be re-read in later sessions.

---

## 0. Rules of engagement (read first, these override convenience)

1. **Work in milestones. Do not skip ahead.** Each milestone (M0–M6) ends with a working binary, passing tests, and a git commit. Do not begin M(n+1) until M(n)'s exit criteria are met and I have confirmed.
2. **No stubs in shipped paths.** No `todo!()`, `unimplemented!()`, or "this would be implemented here" comments in any code path the binary can reach. If something is out of scope for the current milestone, the code must not call into it.
3. **Falsifiability first.** The test corpus (M1) gets built *before* the merge algorithm. This project is uniquely prone to producing code that looks correct and is not. Never let correctness be unmeasurable.
4. **A conflict is always an acceptable answer. A wrong merge never is.** When confidence is low, when parsing fails, when the situation is ambiguous — fall back to git's line merge or emit a conflict. Optimize for a low incorrect-merge rate, not a high resolve rate.
5. **Never reformat untouched code.** Output is produced by splicing byte ranges out of the input files. Whole-file pretty-printing is an instant adoption killer and is forbidden.
6. **Maintain `PROGRESS.md`** at the repo root: current milestone, what's done, what's next, every non-obvious decision and its rationale, every known limitation. Update it at the end of every working session. This spans many sessions and it is the handoff.
7. **Ask before inventing scope.** If a design decision is genuinely ambiguous and consequential, stop and ask rather than picking silently.
8. **Verify prior art rather than trusting the summary in §2.** Search for the current state of GumTree, Mergiraf, difftastic, and Spork before writing the matcher. Some of this has moved recently.

---

## 1. What we are building

Git merges **lines of text**. It has no model of what the code means. Two consequences:

- **False conflicts.** Move a function, and any edit to its body on another branch conflicts. Wrap a block in `if (…) { }` and the reindent makes every line inside look modified. Two branches adding an import at the same spot conflict, every time.
- **False clean merges — the dangerous case.** Branch A renames `getUser`; branch B adds a call to `getUser`. The edits touch different lines, git merges clean, the build breaks. Line-based merge cannot see this by construction.

`semantic-merge` is a drop-in git merge driver that parses all three versions with tree-sitter, matches nodes structurally, merges on the tree rather than on lines, and (in M6) resolves names so it can catch the second class of failure.

**Success is a number, not a demo.** The headline deliverable is an evaluation over thousands of real merge conflicts mined from open-source history, reporting resolve rate *and* incorrect-resolve rate. Design every earlier milestone in service of being able to run that evaluation credibly.

---

## 2. Prior art — read before implementing

This is not greenfield. Understand what exists and be explicit about what is new.

| Project | What it does | Relationship |
|---|---|---|
| **GumTree** (Falleri et al., ASE 2014) | The canonical AST diff/matching algorithm | We reimplement this in M2. Read the paper. |
| **difftastic** | Structural diff, read-only | Reference for output quality; does not merge |
| **Mergiraf** | tree-sitter-based merge driver | Closest existing work. Study it. Benchmark against it. |
| **Spork** / **jdime** | Structured merge for Java, research-grade | Source of evaluation methodology |

**Everything above is syntactic.** The name-binding layer in M6 — merging with an understanding of what refers to what — is where the genuinely novel contribution is. Frame the project accordingly and do not overclaim about M0–M5.

---

## 3. Technology decisions (settled — do not relitigate)

- **Language: Rust.** First-class tree-sitter bindings, single fast binary, and merge drivers are invoked constantly so latency is user-visible.
- **Parser: tree-sitter.** Error-tolerant, incremental, uniform grammar interface, and the CST retains every byte including comments and whitespace with source ranges.
- **Target language: Java first.** Cleanest grammar for this purpose, largest volume of public merge history, and the most prior work to benchmark against. Design every crate to be language-agnostic behind a `Language` trait; add TypeScript in M5 only to prove the abstraction holds.
- **Integration: git merge driver.** Do not fork git. Do not write a git reimplementation.

### Crate layout

```
semantic-merge/
├── SPEC.md, PROGRESS.md, README.md
├── crates/
│   ├── sm-cst/     # tree-sitter wrapper; stable node IDs; trivia attachment; Language trait
│   ├── sm-match/   # GumTree matching (top-down + bottom-up)
│   ├── sm-diff/    # edit-script derivation, including MOVE as first-class
│   ├── sm-merge/   # three-way tree merge + conflict model
│   ├── sm-emit/    # source-range splicing serializer, reindentation
│   ├── sm-bind/    # scope tree + name resolution (M6)
│   ├── sm-cli/     # merge driver, diff viewer
│   └── sm-eval/    # corpus mining + replay harness + metrics
└── corpus/         # gitignored; mined test cases
```

Suggested deps: `tree-sitter`, `tree-sitter-java`, `clap`, `thiserror`, `anyhow`, `serde`/`serde_json`, `rayon`, `insta` (snapshot tests), `proptest`, `indicatif`. Use `git2` or shell out to `git` for mining — shelling out is fine and simpler.

---

## 4. Architecture

### 4.1 Pipeline

```
base.java  ─┐
ours.java  ─┼─► parse (CST) ─► match(base,ours) ─┐
theirs.java─┘                  match(base,theirs)─┴─► 3-way tree merge ─► emit ─► merged.java
                                                          │
                                                     conflicts? ─► fall back / emit markers
```

### 4.2 `sm-cst` — the CST layer

- Wrap tree-sitter trees in an arena (`Vec<Node>` + indices). Every node carries: kind, byte range, parent, children, and a **stable ID within its own tree**.
- Retain *all* bytes. Whitespace and comments are not discarded — the emitter needs them.
- **Trivia attachment.** tree-sitter exposes comments as `extra` nodes. Heuristic: a comment attaches to the *following* named sibling if separated by at most one newline; otherwise to the *preceding* sibling if on the same line; otherwise it floats in the parent's child list. Make this a documented, configurable policy with its own test suite. It will be wrong sometimes; the goal is that it is wrong *predictably*. This kills naive implementations — take it seriously.
- **`Language` trait.** Per-language config: which node kinds are "ordered" sequences (statement lists) vs "unordered" sets (imports, class members, struct fields); which kinds are scope-introducing (for M6); which kinds are identifiers/references.

### 4.3 `sm-match` — GumTree

Two phases. Implement exactly, then tune constants against the corpus.

**Phase 1, top-down (greedy isomorphic subtree matching):**
- Maintain height-indexed priority queues over both trees; always process the tallest unmatched subtrees first.
- Two subtrees are *isomorphic* if their kind-and-text structure hashes identically. Precompute a structural hash per node.
- If a subtree in T₁ has exactly one isomorphic candidate in T₂, match them and all descendants pairwise.
- If multiple candidates, disambiguate by parent-context similarity (dice of already-matched ancestors); ties → defer.
- Ignore subtrees below `MIN_HEIGHT` (start at 2) to avoid matching trivial leaves.

**Phase 2, bottom-up (container recovery):**
- For each unmatched internal node in T₁ (post-order), find the candidate in T₂ maximising the **dice coefficient**:
  `dice(a,b) = 2·|matched descendants shared| / (|desc(a)| + |desc(b)|)`
- Match if `dice > MIN_DICE` (start at 0.5) and kinds are compatible.
- After matching a pair, run a cheap recovery pass on their small unmatched subtrees (histogram/optimal matching on subtrees under `MAX_SIZE`, start at 100).

**Constants (`MIN_HEIGHT`, `MIN_DICE`, `MAX_SIZE`) must be tunable via config**, because M5 will sweep them against the corpus.

**Verification:** snapshot tests (`insta`) on hand-written pairs, plus a CLI subcommand that renders a matching side-by-side in the terminal so it can be eyeballed. Build the eyeball tool — matching bugs are almost invisible in assertions and obvious visually.

### 4.4 `sm-diff` — edit scripts

From a matching, derive: `Insert`, `Delete`, `Update` (same node, changed text), and critically **`Move`** (matched node, different parent or different position). Move is the operation line-diff structurally cannot express and is the source of most of our wins. Represent an edit script as an ordered list of operations over stable node IDs.

### 4.5 `sm-merge` — the three-way merge

Given `M_ours: base→ours` and `M_theirs: base→theirs`, compose through base to relate ours↔theirs.

Classify each base node's fate in each branch: `Unchanged | Updated | Moved | Deleted | MovedAndUpdated`. Then, per node:

| ours | theirs | result |
|---|---|---|
| Unchanged | any | take theirs |
| any | Unchanged | take ours |
| Updated(x) | Updated(x) | take x |
| Updated(x) | Updated(y), x≠y | **conflict** |
| Moved(p) | Updated(x) | **apply both** ← the headline win |
| Moved(p) | Moved(q), p≠q | **conflict** |
| Deleted | Updated | **conflict** (classic delete/modify) |
| Deleted | Deleted | delete |
| Deleted | Unchanged | delete |

**Child list merging** is the subtle part and where wrong answers come from:

- **Ordered lists** (statement sequences): perform a diff3-style sequence merge where elements are identified by *node matching*, not by position. Disjoint insertions at different anchor points merge cleanly. Insertions from both branches at the *same* anchor → **conflict** (do not guess an interleaving; be conservative).
- **Unordered sets** (imports, class members, top-level declarations): merge as sets. Disjoint additions from both sides both apply, no conflict. This alone eliminates a large fraction of real-world conflicts. Deduplicate identical additions.

**Conflict representation:** an internal `Conflict { base, ours, theirs, span, reason }` — not text markers. The emitter renders markers only at the end. Keep the reason enum rich; M5 analysis depends on knowing *why* things conflicted.

### 4.6 `sm-emit` — serialization

- Every node in the merged tree carries a provenance pointer: a byte range in base, ours, or theirs. Emit by **copying those bytes**. Synthesize text only where structurally unavoidable.
- **Reindentation:** when a node's nesting depth changes, apply a controlled indentation transform to its emitted block — adjust leading whitespace per line, do not reprint.
- **Conflicts:** emit standard `<<<<<<< ours` / `=======` / `>>>>>>> theirs` markers at the smallest enclosing sensible boundary (statement or declaration, not expression), honouring the marker size git passes in `%L`.
- **Invariant to assert in tests:** if there are zero conflicts and zero changes from one side, output is byte-identical to the input from the other side.

### 4.7 `sm-cli` — the git merge driver

Register via `.gitattributes`:
```
*.java merge=semantic
```
and `.git/config`:
```
[merge "semantic"]
    name = semantic AST merge
    driver = sm merge %O %A %B %L %P
    recursive = binary
```
`%O` = ancestor, `%A` = ours **and the path the result must be written to**, `%B` = theirs, `%L` = marker size, `%P` = pathname. **Exit 0 = merged cleanly; non-zero = conflicts remain.** Get this contract exactly right — writing the result to the wrong path silently destroys work.

**Mandatory fallback:** if any of the three files fails to parse, or the file exceeds a size/time budget, or any internal invariant fails, shell out to `git merge-file` and return its result. Wrap the whole driver so a panic degrades to line merge rather than data loss. Add `--timeout` with a default around 5s.

Also ship `sm diff <a> <b>` (structural diff viewer) and `sm match <a> <b>` (matching visualizer).

---

## 5. Testing strategy

Three layers, all required.

**1. Unit + snapshot tests.** Hand-written cases per component. `insta` snapshots for matchings, edit scripts, and merge outputs.

**2. Property tests (`proptest`) — these catch real bugs, implement them early:**
- `merge(B, X, B) == X` — identity on one side
- `merge(B, B, Y) == Y`
- `merge(B, X, X) == X` — convergent edits
- `merge(B, B, B) == B`
- **Symmetry:** `merge(B,X,Y)` conflicts ⟺ `merge(B,Y,X)` conflicts (marker order may differ; the *decision* must not)
- **Idempotence:** merging a clean result against itself is a no-op
- **Parse stability:** any conflict-free output must itself parse without errors
- **Byte preservation:** every byte of output is traceable to an input range or an explicitly synthesized token

**3. The corpus replay (M1 + M5).** The real test. See below.

---

## 6. The evaluation — this is the portfolio artifact

### 6.1 Mining

For each target repo (pick 30–50 large, active Java repos):
1. `git log --merges` → merge commits `M` with parents `P1`, `P2`.
2. `B = git merge-base P1 P2`.
3. Re-run the merge in a scratch worktree to find which files git conflicted on.
4. For each such file, record the case: `(base, ours, theirs, human_resolution)` where `human_resolution` is that file's content in `M`.

Also record the **clean** cases (git merged without conflict) — needed to measure regressions.

**Filter out contaminated cases.** Humans often make unrelated edits during a merge. Exclude or flag any case where the resolution contains content present in neither parent nor base. Document how many were excluded and why; an honest denominator is what makes the number credible.

### 6.2 Metrics

| Metric | Meaning | Target |
|---|---|---|
| **Resolve rate** | git conflicted, we produced a clean merge | maximize |
| **Correct-resolve rate** | of those, output matches human resolution | maximize |
| **Incorrect-resolve rate** | of those, output differs from human | **< 1% — the metric that matters** |
| **Correct decline** | git conflicted, we conflicted too | acceptable |
| **Regression rate** | git merged clean, we conflicted | ≈ 0 |
| **Divergence** | both merged clean, results differ | **≈ 0 — investigate every instance** |
| **Latency** | p50 / p99 per file | p99 < 1s |

Compare on two equality notions: byte-exact, and AST-equal-modulo-formatting. Report both.

**Honesty requirement:** the human resolution is ground truth for *what was committed*, not for *what was correct*. State this limitation in the README. It is the first thing a knowledgeable reader will ask about, and pre-empting it reads as rigor.

### 6.3 Output

`sm-eval report` produces a markdown + JSON report: the table above, per-repo breakdown, conflict-reason histogram, and a sample of incorrect resolutions with diffs. The incorrect-resolution gallery is the most valuable debugging asset in the project.

---

## 7. Milestones

### M0 — Scaffold + CST layer
Cargo workspace, all crates stubbed out at the boundary level, CI running `fmt`/`clippy`/`test`. `sm-cst` parses Java, builds the arena, attaches trivia, exposes the `Language` trait.
**Exit:** `sm parse Foo.java` prints a readable tree with byte ranges and attached comments. Trivia attachment has its own passing test suite. Committed.

### M1 — Corpus miner (before the algorithm — do not reorder)
`sm-eval mine --repos repos.txt --out corpus/`. Clones, walks merge commits, extracts cases, filters contaminated ones, writes a manifest.
**Exit:** ≥ 2,000 conflicted-file cases on disk from ≥ 20 repos, with a summary of counts and exclusions. Committed.

### M2 — Matching
GumTree top-down + bottom-up. Config-driven constants. Snapshot tests. `sm match a.java b.java` visualizer.
**Exit:** matcher runs over 500 corpus pairs without panic; visual inspection of 20 hand-picked cases (renames, moves, wrapped blocks) is correct. Committed.

### M3 — Structural diff + viewer
Edit scripts with moves. Terminal renderer.
**Exit:** `sm diff` produces obviously-better-than-`git diff` output on moved functions and reindented blocks. Screenshot it — this is the first demo-able moment. Committed.

### M4 — Three-way merge + emitter + git driver
Conflict model, ordered/unordered child merging, splicing emitter, driver registration with hard fallback. Full property test suite.
**Exit:** all property tests pass; installed as your own merge driver; you have used it on a real repo and it has not lost data. Committed.

### M5 — Run the evaluation
Replay the full corpus. Tune constants. Hunt every divergence and incorrect resolution. Add TypeScript to prove language-agnosticism. Write the report.
**Exit:** published numbers hitting the §6.2 targets, plus a written analysis of the failure modes. **This is the deliverable the whole project exists to produce.** Committed and written up.

### M6 — Name binding + semantic conflicts (the novel part)
`sm-bind` builds a scope tree per version and resolves identifiers to declarations. After producing a candidate merge, re-resolve every reference in the merged tree:
- reference resolves to nothing → **semantic conflict** (something was renamed or deleted out from under it)
- reference resolves to a *different declaration* than it did in the branch it came from → **semantic conflict**

This catches the rename-in-A / new-call-in-B case that git and every existing structural merge tool miss.
**Exit:** a constructed test suite of semantic-conflict scenarios all caught; a scan of the corpus for real instances, with however many you find written up as case studies. Even a handful of real-world hits is a strong result. Committed.

---

## 8. Definition of done

- `cargo install` → working `sm` binary, documented driver setup in the README
- Java and TypeScript supported through the same trait
- Never loses data; degrades to git line merge on any failure
- Evaluation report published with resolve rate, incorrect rate < 1%, and honest limitations
- `PROGRESS.md` complete; README leads with the numbers, not the architecture

---

## 9. Anti-goals

Not a git reimplementation. Not a merge GUI. Not a formatter. Not a linter. Not a language server. Not an LLM-powered resolver — the value here is determinism and provability, and an LLM in the merge path destroys both. Resist all of these.

---

**Begin with M0. Confirm your understanding of this spec first, flag anything you think is wrong or underspecified, then start.**
