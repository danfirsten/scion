# Prior art for `semantic-merge` — verified state as of July 2026

**Status:** research note. Written to satisfy SPEC §0 rule 8 ("Verify prior art rather than trusting the
summary in §2"). Every non-obvious claim carries a source. Read §8 ("Implications") and §9 ("What is
actually novel here") if you read nothing else.

## 0. Method and caveats

Two classes of evidence are used, and they are labelled throughout:

- **[primary]** — read directly from source I cloned and inspected. This is the strongest evidence and
  most of the algorithmic detail below is of this kind.
- **[secondary]** — obtained from search-engine result summaries because the host was unreachable.

Repositories inspected directly (clone date 2026-07-27):

| Repo | Commit / version inspected |
|---|---|
| `qundao/mirror-mergiraf` (GitHub mirror of `codeberg.org/mergiraf/mergiraf`) | `3e61fe7`, 2026-07-26, version 0.18.0 |
| `GumTreeDiff/gumtree` | HEAD 2026-07-22, v4.0.0 line |
| `Wilfred/difftastic` | HEAD 2026-07-24, `Cargo.toml` version 0.70.0 |
| `ASSERT-KTH/spork` | HEAD, v0.5.0 line, incl. `replication/` result CSVs |
| `index.crates.io` | live registry index for crate versions |

**Egress caveat (important, affects reproducibility of this note):** the sandbox this was researched in
allows outbound HTTPS only to `github.com`, `raw.githubusercontent.com`, `gitlab.com` and
`index.crates.io`. `arxiv.org`, `hal.science`, `mergiraf.org`, `codeberg.org`, `lwn.net`, `dl.acm.org`
and `crates.io` (the web UI) were all blocked by the egress proxy (HTTP 403 on CONNECT). Where a paper
PDF could not be fetched, its claims are marked **[secondary]** and were cross-checked against a primary
source wherever one existed — e.g. Spork's paper-level numbers were checked against the actual result
CSVs committed in the Spork repo. Anyone re-running this from an unrestricted network should re-verify
the **[secondary]** items.

---

## 1. GumTree (Falleri et al., ASE 2014) — and what changed since

Sources:
- Paper: Falleri, Morandat, Blanc, Martinez, Monperrus, *Fine-grained and accurate source code
  differencing*, ASE 2014. <https://doi.org/10.1145/2642937.2642982>,
  PDF at <https://hal.science/hal-01054552/document> **[secondary — host blocked]**
- Implementation: <https://github.com/GumTreeDiff/gumtree> **[primary]**

### 1.1 The algorithm as published

Two phases (this part of SPEC §4.3 is accurate):

1. **Top-down greedy isomorphic subtree matching.** Height-indexed priority queues over both trees;
   always pop the tallest unmatched subtrees; match subtrees whose structural hash is identical, and map
   all their descendants pairwise.
2. **Bottom-up container matching.** For each unmatched internal node in post-order, find the candidate
   in the other tree maximising the **dice coefficient** over already-matched descendants; match if
   `dice > minDice` **and kinds are equal**; then run a "last chance" recovery pass on the small
   unmatched subtrees underneath.

The paper's three constants are **`minHeight = 2`, `minDice = 0.5`, `maxSize = 100`**, and the paper
explicitly says these "have been chosen according to our expertise" and that other values could perform
differently. **[secondary — confirmed by search snippet of the paper text; PDF host blocked]**

### 1.2 What the current implementation actually does (this differs from the paper)

Read from `core/src/main/java/com/github/gumtreediff/matchers/` at HEAD. **[primary]**

**The default matcher is no longer the paper's algorithm.** `CompositeMatchers.java` registers:

| id | composition | registry priority |
|---|---|---|
| `gumtree-simple` | `GreedySubtreeMatcher` + `SimpleBottomUpMatcher` | **MAXIMUM (= the default)** |
| `gumtree-classic` | `GreedySubtreeMatcher` + `GreedyBottomUpMatcher` | HIGH — *this* is the ASE'14 algorithm |
| `gumtree-simple-stable` | `GreedySubtreeMatcher` + `SimpleMarriageBottomUpMatcher` | HIGH |
| `gumtree-hybrid` | `GreedySubtreeMatcher` + `HybridBottomUpMatcher` | (registered) |
| `gumtree-simple-auto`, `gumtree-simple-auto-st` | auto-tuning wrappers, see §1.4 | HIGH |

The CHANGELOG entry for **v4.0.0 ("Ginkgo")** states plainly: *"Simple is now the default matcher"* and
*"Added auto matchers which automatically select the best parameters for the input"*. It also announces
*"New tree-sitter based tree generator with support for a wide range of languages"* and *"Dropped
external parsers that have a tree-sitter counterpart (except srcml)"* — i.e. **GumTree itself has moved
to tree-sitter** (`gen.treesitter-ng/`, 19 language generators incl. Java, TS, TSX, Rust, Go).
<https://github.com/GumTreeDiff/gumtree/blob/main/CHANGELOG.md> **[primary]**

**Actual default constants in code:**

- `AbstractSubtreeMatcher`: `DEFAULT_MIN_PRIORITY = 1`, priority calculator `"height"`.
- `TreeMetricComputer`: a **leaf gets `height = 0`**, an inner node gets `height = max(child heights)+1`.
  Therefore `minPriority = 1` means "subtrees of height ≥ 1", i.e. bare leaves are excluded. This is
  *semantically identical* to the paper's `minHeight = 2` under the paper's 1-based leaf height. **Do not
  read the number 1 as a loosening — it is the same policy.** **[primary]**
- `GreedyBottomUpMatcher` (the classic bottom-up): `DEFAULT_SIM_THRESHOLD = 0.5`,
  `DEFAULT_SIZE_THRESHOLD = 1000`. Note **1000, not the paper's 100** — the recovery pass (an exact
  Zhang-Shasha TED via `ZsMatcher`) now fires on much larger subtrees than the paper described.
  **[primary]**
- `SimpleBottomUpMatcher` (the *default* bottom-up): `simThreshold = NaN`, which triggers an **adaptive
  threshold** computed per candidate pair:
  `threshold = 1 / (1 + ln(|desc(candidate)| + |desc(t)|))`, and similarity is **Chawathe** similarity,
  not dice. Its `lastChanceMatch` is a cheap trio — `lcsEqualMatching`, `lcsStructureMatching`,
  `histogramMatching` — with **no TED and no size threshold at all**. **[primary]**
- Configurable knobs are the enum `ConfigurationOptions`: `bu_minsim`, `bu_minsize`, `st_minprio`,
  `st_priocalc` (`"size"` or `"height"`), plus ChangeDistiller-specific ones. **[primary]**

**Candidate restriction (both bottom-up matchers):** `getDstCandidates` walks up from each already-matched
descendant and only admits an ancestor if `parent.getType() == src.getType()` and the ancestor is not
already mapped and is not the root. **Same-kind is a hard constraint, not a tiebreak.** **[primary]**

**Ambiguity handling in the top-down phase** is richer than SPEC §4.3 assumes. `GreedySubtreeMatcher`
collects ambiguous (many-to-many isomorphic) groups, sorts groups by largest source subtree descending,
then within a group sorts candidate mappings by `MappingComparators.FullMappingComparator`, which applies
in order: **matched-siblings similarity → parents similarity → position-in-parents similarity → textual
position distance → absolute position distance**, and greedily takes them. SPEC's "disambiguate by
parent-context similarity; ties → defer" is a reasonable simplification but is not what GumTree does.
**[primary]**

### 1.3 Hyperparameter studies

Martinez, Falleri, Monperrus, *Hyperparameter Optimization for AST Differencing*, IEEE TSE 49(10):
4814–4828 (2023); preprint <https://arxiv.org/abs/2011.10268>. The approach ("DAT") searches GumTree's
configuration space per-input and reports **improved edit scripts in 21.8% of evaluated cases**. The
bottom-up matcher itself is one of the hyperparameters, with values Greedy / Simple / Clique.
**[secondary — arXiv blocked]**

### 1.4 Auto-tuning is now in GumTree itself

`AutoMatchers.java` **[primary]** implements the DAT idea in-tree. `propertiesForSimple()` sweeps
`bu_minsim ∈ {0.1, 0.3, 0.5, 0.7, 0.9}` × `st_minprio ∈ {5,4,3,2,1}` (25 configurations), runs
`SimpleGumtreeStable` for each, generates an edit script with `SimplifiedChawatheScriptGenerator`, and
**keeps the configuration that yields the smallest edit script**. `gumtree-simple-auto` does this with a
`parallelStream`. This is a directly transferable trick and a strong endorsement of SPEC's "constants
must be tunable".

### 1.5 Matchers that beat GumTree

- **RefactoringMiner-based AST diff** — Alikhanifard & Tsantalis, *A Novel Refactoring and Semantic Aware
  Abstract Syntax Tree Differencing Tool and a Benchmark for Evaluating the Accuracy of Diff Tools*,
  ACM TOSEM 2024. <https://arxiv.org/abs/2403.05939>, <https://dl.acm.org/doi/10.1145/3696002>. It names
  five concrete GumTree failure modes: (1) no multi-mapping support, (2) matching semantically
  incompatible AST nodes, (3) ignoring language clues, (4) no refactoring awareness, (5) no commit-level
  diff. It ships **the first benchmark of AST node mappings: 800 bug-fixing commits + 188 refactoring
  commits**, and compares against GumTree 3.0 (greedy and simple), IJM, MTDiff and GumTree 2.1.0,
  reporting considerably higher precision and recall especially on refactoring commits.
  **[secondary]**
- **HyperDiff / HyperAST** — Le Dilavrec, Khelladi, Blouin, Jézéquel, *HyperDiff: Computing Source Code
  Diffs at Scale*, ESEC/FSE 2023. <https://dl.acm.org/doi/10.1145/3611643.3616312>,
  <https://inria.hal.science/hal-04189855/>. **This is a Rust reimplementation of GumTree** over a
  Merkle-DAG AST built from Git + tree-sitter, "lazified" so subtrees are only materialised on demand.
  Reported ×1.2–×12.7 CPU speedup end-to-end (up to ×226 on intermediate phases), ×4.5 lower memory per
  AST node, and **99.3% identical diffs vs GumTree**. Code: <https://github.com/quentinLeDilavrec/HyperAST>.
  **[secondary]**

### 1.6 Rust implementations of GumTree

There is **no general-purpose `gumtree` crate on crates.io.** The two real Rust implementations are:

1. **Mergiraf's `src/tree_matcher.rs`** — a clean, self-contained ~600-line implementation of
   *gumtree-classic* (see §2.2). GPL-3.0. **[primary]**
2. **HyperAST** — research-grade, tightly coupled to its own DAG store. **[secondary]**

Other tree-sitter Rust diff tools are *not* GumTree: `diffsitter` uses LCS over syntax-tree leaves
(<https://github.com/afnanenayet/diffsitter>), difftastic uses Dijkstra (§3).

> **Consequence for M2:** we are not duplicating an existing crate. Writing a reusable, permissively
> licensed GumTree in Rust is itself a modest contribution. Mergiraf's version is **GPL-3.0** — read it
> for understanding, but do not copy code into an MIT/Apache project.

---

## 2. Mergiraf — the closest competitor, studied in depth

Home: <https://mergiraf.org/> · Canonical repo: <https://codeberg.org/mergiraf/mergiraf> ·
GitHub mirror used here: <https://github.com/qundao/mirror-mergiraf>

### 2.1 Current state **[primary, from the mirror at `3e61fe7`]**

| | |
|---|---|
| Version | **0.18.0** (tagged 2026-07-14; v0.17.0 was 2026-05-07) |
| License | **GPL-3.0-only** |
| Rust edition | 2024 |
| tree-sitter core | **`tree-sitter = "0.26"`** |
| Languages | **29 programming languages + 13 declarative formats** (`doc/src/languages.md`), incl. Java, TypeScript, Rust, Go, Kotlin, Scala, Solidity, SystemVerilog, Erlang (added 2026-07-26) |
| Activity | 50 commits in 2026 YTD on the mirror's shallow history; last commit the day before this note; releases roughly every 2 months |
| Governance | `GOVERNANCE.md` + `members.yml` — a multi-person team, not a solo project |
| Test corpus | 55 end-to-end example directories under `examples/`, plus `tests/working.rs`, `tests/failing.rs`, `src/snapshots/` |

Adoption signal: **Spork's own README now says** *"If you're looking for a production ready tool for
AST-based GIT merge, we recommend [mergiraf](https://mergiraf.org/)."*
<https://github.com/ASSERT-KTH/spork> **[primary]**. LWN covered it 2025-10-31
(<https://lwn.net/Articles/1042355/>) **[secondary]**.

### 2.2 The matcher: yes, it is GumTree — specifically *gumtree-classic* **[primary]**

`src/tree_matcher.rs` declares `TreeMatcher { min_height, sim_threshold, use_rted, max_recovery_size }`
and its doc comment says "The `GumTree` classic matching algorithm". Structure:

- **Top-down** (`top_down_pass`): two height-indexed `PriorityList`s, synchronised by popping the taller
  side; stops when `min(peek_max_1, peek_max_2) < min_height`. Within an equal-height batch, it computes
  the set of **duplicated hashes on each side** and matches a pair only if it is *isomorphic AND not
  ambiguous on either side*; ambiguous pairs go into an unused `auxiliary` matching and the nodes are
  re-opened (children pushed back). **This is strictly more conservative than GumTree**, which
  disambiguates greedily via `FullMappingComparator` (§1.2).
- Between the phases it **truncates** both trees, replacing every exactly-matched subtree by a stub, so
  the bottom-up pass runs over a much smaller tree. Nice trick; worth stealing for performance.
- **Bottom-up** (`bottom_up_pass`): post-order over unmatched non-leaf nodes; `find_candidates` walks
  ancestors of matched descendants and admits only ancestors with `left_node.kind == ancestor.kind`
  that are unmatched and non-root; picks max `dice`; requires `sim > sim_threshold`. There is a debug
  branch that logs "near-miss" candidates with `sim > 0.75 * sim_threshold` — a good idea for our
  matcher-debugging CLI.
- **Recovery** (`last_chance_match`): if `use_rted` and both stripped subtrees have `size <=
  max_recovery_size`, run a real **tree edit distance** (`tree_edit_distance` crate) and convert the
  edit script to matches; otherwise fall back to `match_subtrees_linearly`, described in the code as
  "poor man's approximation … linear complexity", which indexes children by `(kind, signature)`.

**The constants used in production** (`src/structured.rs`) — and note there are **two different matcher
configurations**:

```rust
let primary_matcher = TreeMatcher {          // base<->left and base<->right
    min_height: 1, sim_threshold: 0.4, max_recovery_size: 100, use_rted: true,
};
let auxiliary_matcher = TreeMatcher {        // left<->right
    min_height: 2, sim_threshold: 0.6, max_recovery_size: 100, use_rted: false,
};
```

The documented rationale (`doc/src/architecture.md`): matchings *involving base* use "less conservative
parameters" plus TED recovery, because a missed match there costs a spurious conflict; the **left↔right**
matching uses *stricter* parameters because "any false positive in this matching can have quite
detrimental effects on the merged result", and it only matters when both sides made similar changes,
which is rare. **This asymmetry is a real design insight and SPEC §4.3 does not have it.**

### 2.3 The merge: 3DM / PCS triples, not a node-fate table **[primary]**

`doc/src/architecture.md` opens: *"Mergiraf broadly follows the architecture of spork"*, and Spork in turn
implements **Tancred Lindholm's 3DM** (<https://doi.org/10.1145/1030397.1030399>). Pipeline:

1. **Parse** all three revisions. Line endings normalised to `\n` first and restored at the end.
   **Multi-line leaves (block comments, multi-line strings) are split into per-line subnodes** so they
   merge line-by-line rather than atomically. If *any* revision fails to parse, abort to line merge.
2. **Match** all three pairs (base↔left, base↔right, left↔right) — see §2.2.
3. **Class mapping**: equivalence classes over the union of the three matchings; a "leader" is elected per
   class with priority **base > left > right**.
4. **PCS encoding**: each tree becomes a set of `(parent, child, successor)` triples over leaders, with
   `⊣`/`⊢` sentinels per child list and a virtual root `⊥`.
5. **Changeset merge**: tag each triple with its revision; union; delete base triples inconsistent with a
   left/right triple. Three inconsistency rules are given explicitly (same parent+child different
   successor; same parent+successor different child; shared child/successor but different parents).
6. **Rebuild** the tree from the triples, recursively from `⊥`. Remaining inconsistencies become
   conflicts *or* are resolved on the fly.
7. **Delete/modify check** (post-pass, see §2.5).
8. **Duplicate-signature check** (post-pass, see §2.6).
9. **Render** (see §2.7).

The merged tree node types are worth copying wholesale into our conflict model:
`ExactTree` (byte-identical to some revision's node), `Conflict` (holds all three sides),
`LineBasedMerge` (text from a diff3 of that node's source in three revisions), `MixedTree` (internal),
`CommutativeChildSeparator` (synthesised separator text). **Note `LineBasedMerge` as a *node type*: the
fallback to line merging is per-subtree, not whole-file.** SPEC §4.7 only has a whole-file fallback.

### 2.4 Child-list merging: "commutative parents" with *groups*, not flat sets **[primary]**

SPEC §4.5 says "unordered sets (imports, class members) — merge as sets". Mergiraf's model is meaningfully
richer, and the extra structure is load-bearing. `LangProfile` (`src/lang_profile.rs`) carries:

```
atomic_nodes          // kinds treated as opaque leaves
commutative_parents   // kinds whose child order does not matter
signatures            // how to key children of a commutative parent
flattened_nodes
extra_comment_nodes   // kinds treated like comments for bundling
injections            // tree-sitter injection query for embedded languages
allow_parse_errors
```

A `CommutativeParent` carries **left delimiter, separator, right delimiter** (so re-rendering can
synthesise correct punctuation) and, crucially, may be `restricted_to(Vec<ChildrenGroup>)` — children
commute **only within their group**, not across groups. The Java profile makes this concrete
(`src/supported_langs.rs`):

- `program` — groups: `[module_declaration]`, `[package_declaration]`, `[import_declaration]` (separator
  `"\n"`), `[class_declaration, record_declaration, interface_declaration,
  annotation_type_declaration, enum_declaration]`. So **an import can be reordered among imports but can
  never float past the class declaration.** A naive "top level is a set" implementation would happily
  emit an import after a class.
- `class_body` — groups: field declarations; nested type declarations; constructors/methods/compact
  constructors. Comment in the source: *"strictly speaking, this isn't true (order can be accessed via
  reflection)"* — an honest note that this is a heuristic.
- `modifiers` — `[visibility, modifier]` and `[marker_annotation, annotation]` as separate groups.
- `throws`, `catch_type`, `type_list`, `annotation_argument_list`.
- `atomic_nodes: ["import_declaration"]` — imports are never merged *internally*.

`signature(...)` definitions give each commutative child a key built from field/child paths, e.g.
`method_declaration` is keyed on `[name]` **and** `[parameters → formal_parameter → type]` **and**
`[parameters → spread_parameter → identifier]` — i.e. **the Java erasure signature**. `annotation` and
`marker_annotation` deliberately get an *empty* signature with the comment *"annotations can be
repeatable, so we can't use the name as key"*.

When a conflict arises under a commutative parent, the resolution rule is explicitly asymmetric-then-
symmetrised (`doc/src/architecture.md`): compute the children the **right** side deletes vs base and the
children it adds; then take the **left** child list, remove the right's deletions, append the right's
additions.

### 2.5 Delete/modify: Mergiraf fixes a real 3DM bug **[primary]**

Plain 3DM silently keeps the deleting side and drops the other side's edits — filed against Spork as
<https://github.com/ASSERT-KTH/spork/issues/529>. Mergiraf's post-pass: track deleted nodes while
rebuilding; at the end, for each deleted element `E` that the other revision modified, try to compute a
**covering** — a set `D` of descendants of `E` that (a) still exist in the deleting revision (i.e. were
moved, not deleted) and (b) contain all of the other revision's changes to `E`. If a covering exists, the
changes survived elsewhere and no conflict is emitted; otherwise a `LineBasedMerge` conflict is inserted
at `E`'s parent.

> This is exactly SPEC §4.5's `Deleted | Updated → conflict` row, but with a genuinely clever escape
> hatch for the move-out-of-scope case. **Copy this.**

### 2.6 Duplicate-signature post-pass **[primary]**

One pass over the merged tree; for each commutative parent, group children with identical signatures; if
a group has >1 member, replace them with a single conflict at the position of the first. Catches "both
sides added a method with the same erasure" even though the additions were at non-conflicting positions.
Spork does the same thing but hard-codes it for Java (`doc/src/related-work.md`).

### 2.7 Trivia / comments — Mergiraf's policy vs SPEC §4.2 **[primary]**

Implemented in `src/ast/bundle_comments.rs` as an explicit state machine
(`SingleNonComment | CollectingComments | BundlingCommentsFromBelow`). Key facts:

- The distance metric is **`distance_to` = number of `\n` characters in the source between two nodes**
  (after LF normalisation), and **`is_close_enough_to` = `distance <= 1`**. This is *exactly* the
  heuristic SPEC §4.2 proposes ("at most one newline"). Independent confirmation that the heuristic is
  the right starting point.
- Preference order: a comment block bundles into the **following** node (comments-from-above) when close
  enough; a trailing comment bundles into the **preceding** node (comments-from-below); when both are
  possible, the recorded `distance` to the preceding node is compared against the distance to the
  following node and the closer wins. The doc comment gives the tricky case `A, // comment` / `B` where
  the comment is closer to the `,` than to `B` and should therefore not be bundled at all.
- **Scope limitation worth knowing:** `bundle_comments` is called on **the children of a commutative
  parent** — bundling is a device to keep documentation attached during *commutative* merging, not a
  universal trivia-attachment layer. Mergiraf does not have a general trivia-ownership model; whitespace
  is not in the tree at all and is re-synthesised at render time by "imitating the whitespace from the
  original revisions".
- `LangProfile::extra_comment_nodes` lets a language declare non-`extra` kinds that should behave like
  comments.

> SPEC §4.2 is **more ambitious** than Mergiraf here (universal trivia attachment with byte-exact
> retention). That is a defensible differentiator, but it is also the thing SPEC itself warns "kills naive
> implementations". Mergiraf's narrower scope is evidence that the general problem is hard enough that a
> mature project chose to sidestep it.

### 2.8 Fast mode — the idea SPEC is missing **[primary]**

From `doc/src/architecture.md`:

1. Run **git's line-based merge first**. If it is conflict-free, return it. Done — extremely fast, and by
   construction zero regression risk versus git.
2. If there are conflicts, **do not throw the line merge away**. Reconstruct *fictional* base/left/right
   revisions from the conflicted file by selecting sides of each conflict hunk. Because they come from one
   file, every syntactic element lying entirely in a merged (non-conflicted) region can be matched for
   free. Seed the tree matcher with that initial matching (`TreeMatcher::match_trees` takes
   `initial_matching: Option<&ApproxExactMatching>`), which "speeds it up significantly".
3. Same machinery powers `mergiraf solve`, which resolves conflict markers in an already-conflicted file
   **without access to the original three revisions** — a genuinely useful UX mode.
4. Documented limitation: fast mode cannot resolve **moved-and-edited elements**; that needs the real
   three revisions.

### 2.9 Git merge driver registration **[primary]**

```ini
[merge "mergiraf"]
    name = mergiraf
    driver = mergiraf merge --git %O %A %B -s %S -x %X -y %Y -p %P -l %L
```
plus `* merge=mergiraf` in gitattributes. Docs say "For best results, use Git v2.44.0 or newer."

Details SPEC §4.7 gets wrong or omits:

- **`%S` / `%X` / `%Y` exist** and are the conflict-marker *labels* for base / ours / theirs. Mergiraf maps
  them to `-s/--base-name`, `-x/--left-name`, `-y/--right-name`, with source comments saying "the choice
  of 's' is inherited from Git's merge driver interface". SPEC's driver line only uses `%O %A %B %L %P` and
  will therefore emit worse conflict markers than git's own.
- **Old-git detection trick:** `let old_git_detected = base_name.as_deref().is_some_and(|n| n == "%S");`
  — a git older than 2.44 does not expand `%S`, so the literal string `"%S"` arrives as the argument.
  Mergiraf detects that and falls back to defaults. **We must do this or we will write `%S` into users'
  conflict markers on older git.**
- **`--git` flag is a separate, explicit mode** meaning "overwrite the left revision in place", and it
  `conflicts_with = "output"`. Making the destructive behaviour opt-in at the CLI level is much safer than
  inferring it. SPEC's `driver = sm merge %O %A %B %L %P` has no such guard.
- **Exit codes** (`src/lib.rs`): `EXIT_SUCCESS = 0`, `EXIT_MERGE_HAS_CONFLICTS = 1`, and for the `solve`
  subcommand `EXIT_SOLVE_FAILED = 1`, `EXIT_SOLVE_HAS_CONFLICTS = 2`.
- **Timeout defaults:** `timeout.unwrap_or(if fast { 5000 } else { 10000 })` milliseconds, and `--timeout 0`
  disables. SPEC's 5s default matches fast mode; full structured merge gets 10s.
- There is an **enabling environment variable** (`ENABLING_ENV_VAR`) and a `mergiraf report`
  bug-reporting path (`src/bug_reporter.rs`) that packages up a failing case. Good ergonomics to copy for
  M5's incorrect-resolution gallery.

### 2.10 Published evaluation

**Mergiraf has published no evaluation of its own.** There is no benchmark, evaluation or results section
in the repo docs or README **[primary — grepped]**. The only third-party numbers come from the LastMerge
paper (§5.1): *"Mergiraf misses 42% fewer false negatives than Spork"*, with generic tools showing runtime
comparable to language-specific ones **[secondary]**.

> **This is the single biggest opening for our project.** The nearest competitor, at version 0.18 with
> a multi-person team and 42 languages, has never published a resolve-rate / incorrect-rate number. SPEC
> §6 is therefore not just a portfolio artifact — it is a contribution that does not currently exist for
> this class of tool. It also means we can and should benchmark *against* Mergiraf directly.

---

## 3. difftastic

Repo: <https://github.com/Wilfred/difftastic> · Manual: `manual/src/` **[primary]**

- **Version 0.70 unreleased at HEAD** (2026-07-24); 0.69 released 2026-04-30, 0.68 2026-03-16,
  0.67 2025-11-16. Actively maintained, roughly quarterly releases.
- **62 language/format entries** in `manual/src/languages_supported.md`.
- Uses **`tree-sitter = "0.26.8"`** and **`tree-sitter-language = "0.1.7"`** with a large set of vendored
  and crates.io grammars — another confirmation of the version-decoupling story in §6.

**Algorithm** (`manual/src/diffing.md`): diffing is a **shortest-path problem on a DAG built lazily**. A
vertex is a *pair of positions*, one in each syntax tree. Edges are the possible moves — "this node is
novel on the left", "novel on the right", "the nodes match", "enter an unchanged delimiter", "enter a
novel delimiter". Dijkstra finds the minimum-cost route; the graph is never materialised (that would be
exponential), neighbours are generated on demand.

**Edge costs** (`src/diff/graph.rs`) — these are the tuned heart of the tool and are directly instructive
for M3's rendering quality:

| Edge | Cost |
|---|---|
| `UnchangedNode { depth_difference, probably_punctuation }` | cheapest (varies with depth difference) |
| `EnterUnchangedDelimiter { depth_difference }` | `100 + min(40, depth_difference)` |
| `NovelAtomLHS` / `NovelAtomRHS` | `300` |
| `EnterNovelDelimiterLHS` / `RHS` | `300` |
| `ReplacedComment { levenshtein_pct }` / `ReplacedString` | `500 + (100 - levenshtein_pct)` |

Two design points to steal:
1. **Depth difference is priced.** Matching a node that moved a long way in nesting depth is penalised but
   not forbidden. This is exactly the "wrapped in an `if`" case SPEC §1 calls out.
2. **Comments and strings are matched by Levenshtein similarity, not equality**, and the cost is tuned to
   sit just under `2 × NovelAtom` so that "replaced comment" beats "delete + insert" but only barely.

**Safety valves** (`src/options.rs`, `src/main.rs`): `DEFAULT_GRAPH_LIMIT = 3_000_000` vertices
(`DFT_GRAPH_LIMIT`); on exceeding it difftastic reports `"exceeded DFT_GRAPH_LIMIT"` and **falls back to a
line diff**. There are also `DFT_PARSE_ERROR_LIMIT` and `DFT_BYTE_LIMIT`. Three independent budgets, each
with a named env var and a graceful degradation — a good model for our `--timeout`/size budget.

`manual/src/tree_diffing.md` is also a well-curated survey of the space (json-diff, GumTree, Tristan
Hume's A* tree diff, Autochrome, graphtage, diffsitter, sdiff) and is worth reading in full.

---

## 4. Spork and jdime — evaluation methodology for M5

### 4.1 Spork **[primary for the numbers below]**

Larsen, Falleri, Baudry, Monperrus, *Spork: Structured Merge for Java with Formatting Preservation*,
IEEE TSE, <https://doi.org/10.1109/TSE.2022.3143766>, preprint <https://arxiv.org/abs/2202.05329>.
Repo: <https://github.com/ASSERT-KTH/spork>. Master's thesis: *Spork: Move-enabled structured merge for
Java with GumTree and 3DM*, <http://urn.kb.se/resolve?urn=urn:nbn:se:kth:diva-281960>.

Architecture: **Spoon** (Java AST) + **GumTree** (matching, via `gumtree-spoon-ast-diff`) + **3DM**
(merge). Same three-layer shape we are building, with tree-sitter substituted for Spoon.

**Mining methodology and dataset**, reconstructed from `replication/` in the repo:

- `candidate_projects.txt` — **589 candidate projects** meeting the selection criteria.
- `buildable_candidates.txt` — **359** of those were buildable on the experiment machine (they needed a
  build for the bytecode-equivalence experiment).
- `projects.csv` — **119 projects** finally used, with per-project columns
  `num_merge_scenarios, num_file_merges, cloc, num_stars, num_core_contributors`.
- `stats_collection_file_merge_results.csv` — **3,480 rows = 1,740 file merges × 2 tools**, with columns:
  `project, merge_dir, merge_commit, base_blob, left_blob, right_blob, expected_blob, replayed_blob,
  merge_cmd, outcome, git_diff_size, num_conflicts, conflict_size, runtime`.

**Outcome distribution, computed directly from that CSV** (each tool over the same 1,740 file merges):

| tool | success | conflict | fail |
|---|---|---|---|
| spork | 1,500 (86.2%) | 206 (11.8%) | 34 (2.0%) |
| jdime | 1,490 (85.6%) | 227 (13.0%) | 23 (1.3%) |

Important framing point: **these 1,740 are *all* file merges in the sampled merge scenarios, not just the
ones git conflicted on.** So 86% "success" is *not* comparable to SPEC §6.2's resolve rate, which is
conditioned on git having conflicted. Do not put these numbers side by side without saying so.

Metric definitions Spork used, in effect: `outcome ∈ {success, conflict, fail}` (fail = crash/timeout);
`git_diff_size` = size of the diff between the replayed merge and `expected_blob` (the human resolution),
which is their **correctness/formatting** proxy; `num_conflicts` / `conflict_size`; `runtime`. Note the
README's warning that **JDime does not report conflicts unless stats collection is enabled**, which slows
it, so they ran the experiment twice — an honesty detail worth imitating.

The `replication_package.tar.gz` (v0.5.0 release asset) contains the actual merged files for every case.

### 4.2 jdime **[secondary]**

Apel, Liebig, Brandl, Lengauer, Kästner, *Structured merge with auto-tuning: balancing precision and
performance*, ASE 2012, <https://dl.acm.org/doi/10.1145/2351676.2351694>; extended as *Balancing precision
and performance in structured merge*, ASE Journal 2015,
<https://link.springer.com/article/10.1007/s10515-014-0151-5>. Repo: <https://github.com/se-sic/jdime>.

Core idea: **switch between unstructured and structured merge depending on whether a conflict occurs** —
run cheap line merge, and only escalate to structured merge on the conflicting regions. (Mergiraf's "fast
mode" is the same idea rediscovered, §2.8.)

Datasets: the 2012 paper used **8 Java projects, 7–10 merge scenarios each**; the extended study used
**50 real-world Java projects, 434 merge scenarios, >51 MLOC**. Each merge scenario is three directories
(left, base, right). Headline result: auto-tuning is **up to 92× faster than pure structured merge, ~10×
on average**, with precision close to structured merge.

### 4.3 A newer and stricter evaluation framework — read this before designing M5

Mori & Hashimoto, *On the Correctness of Software Merge*, ASE 2025, pp. 2338–2349,
<https://doi.org/10.1109/ASE63991.2025.00193>, preprint <https://arxiv.org/abs/2607.07987>. **[secondary]**

They argue that "matches the human resolution" is the wrong criterion and propose two **syntactic**
criteria a merge result must satisfy:

- **Parsable** — syntactically valid per the language grammar.
- **Universal** — the result incorporates *all and only* the edit operations occurring in each branch,
  with edits common to both branches applied exactly once.

Scale: **43,774 file merge scenarios from 76 open-source Java projects**; they report that existing tools
including Git produce a number of incorrect results while their tool `d3j` produces none. They further
compare against **2,582 developer-resolved merges** and **2,459 merge scenarios involving 21 refactoring
types**.

> SPEC §6.2's honesty caveat ("the human resolution is ground truth for what was committed, not for what
> was correct") is exactly the gap this paper closes. Adopting *parsable* + *universal* as a second,
> ground-truth-free metric would make our evaluation stronger than Spork's and directly comparable to the
> current state of the art. SPEC already mandates the parsability property test; universality is a small
> extension of the same machinery.

### 4.4 Other datasets worth knowing **[secondary]**

- **s3m / Cavalcanti et al.**, *Evaluating and improving semistructured merge*, OOPSLA 2017 — the origin of
  much of the Java merge-scenario mining methodology. <https://github.com/guilhermejccavalcanti/s3m>
- **ConGra**, <https://arxiv.org/abs/2409.14121> — a graded benchmark for automatic conflict resolution.
- **Merge-Bench**, <https://arxiv.org/abs/2605.25890> — LLM conflict-resolution benchmark (out of scope per
  SPEC §9, but it defines the dataset our numbers will be compared against by readers).

---

## 5. Other AST/tree-sitter merge tools the spec does not mention

### 5.1 LastMerge (2025) **[secondary]**

*LastMerge: A language-agnostic structured tool for code integration*, <https://arxiv.org/abs/2507.19687>
(July 2025). A generic structured merge tool built on **tree-sitter CSTs** with a thin per-language
configuration interface — i.e. the same thesis as Mergiraf and as us. Its evaluation compares LastMerge
and Mergiraf (generic) against jDime and Spork (Java-specific):

- LastMerge reports **15% fewer false positives than jDime**.
- Mergiraf misses **42% fewer false negatives than Spork**.
- Both generic tools have **runtime comparable to the language-specific tools**.
- ~**10% difference rate** between Java-specific tools and their generic counterparts, mostly from
  implementation details.
- Conclusion: *"no evidence that generic structured merge significantly impacts merge accuracy"*.

Note their vocabulary: **false positive = a conflict reported that should not have been** (over-conflict),
**false negative = a conflict not reported that should have been** (silently wrong merge). Under SPEC
§6.2's names, false negatives ≈ incorrect-resolve, false positives ≈ regression/over-conflict. Say which
convention we use.

A related public repo is <https://github.com/ace-design/git-merge-adv> ("New merging tool for Python and
Java source code", tree-sitter CSTs for Java + Python `ast` for Python, custom data structure,
git merge driver integration), though it does not identify itself as LastMerge.

### 5.2 Weave — a live, popular competitor that appeared recently **[secondary, from its GitHub README]**

<https://github.com/Ataraxy-Labs/weave>. Git merge driver, tree-sitter based, **28+ languages**,
**dual Apache-2.0 / MIT**, ~1.2k stars, Homebrew-installable.

Algorithm per its README: parse → extract **entities** (functions, classes, methods, JSON keys) → match
entities by **identity (name + type + scope)** → three-way resolve at entity granularity → reconstruct.
This is **semistructured / FSTMerge-shaped**, coarser than Mergiraf's full-CST merge.

Its README states, unprompted, that this is **"*not* full semantic analysis like name binding or reference
resolution"** — see §9.

Its published benchmark is **31 scenarios** (31/31 clean vs git's 15/31; 100% vs Mergiraf's 83%). At n=31
with no incorrect-merge rate reported, this is a demo, not an evaluation. It is nonetheless the number a
reader will have seen, so our report should mention it and contrast the sample sizes.

### 5.3 The rest of the landscape

From `jelmer/awesome-merge-drivers` (<https://github.com/jelmer/awesome-merge-drivers>) and Mergiraf's
`doc/src/related-work.md` **[primary]**:

| Tool | Languages | Paper / link |
|---|---|---|
| spork | Java | <https://arxiv.org/abs/2202.05329> (2023) |
| automerge-ptm | Java | *Enhancing Precision of Structured Merge by Proper Tree Matching* (2019) |
| jsFSTMerge | JavaScript ES5 | Tavares dissertation (2018) |
| s3m | Java | OOPSLA 2017 |
| jdime | Java | ASE 2012 |
| FSTMerge | Java, C#, Python | FSE 2011 |
| IntelliMerge | Java (multi-file) | <https://dl.acm.org/doi/10.1145/3360596> (OOPSLA 2019) |
| static-semantic-merge | Java | <https://github.com/spgroup/static-semantic-merge> |
| plume-lib/merging | Java imports, annotations, version numbers | <https://github.com/plume-lib/merging> |
| Weave | 28+ | <https://github.com/Ataraxy-Labs/weave> |

Mergiraf's related-work page explicitly excludes ML/LLM merge approaches on the grounds that *"none of
them appear to be open source so far"*.

---

## 6. The tree-sitter Rust crate ecosystem — settle this before M0

All facts here read live from `index.crates.io` on 2026-07-27 **[primary]**.

### 6.1 Current versions

| Crate | Latest | Depends on |
|---|---|---|
| `tree-sitter` | **0.26.11** | `tree-sitter-language ^0.1`, `streaming-iterator ^0.1.9`, `regex`, `regex-syntax`, `wasmtime-c-api ^36`; build-deps `cc ^1.2.48`, `bindgen ^0.72`; **`rust-version = "1.77"`** |
| `tree-sitter-language` | **0.1.7** | (no deps) |
| `tree-sitter-java` | **0.23.5** | normal: `tree-sitter-language ^0.1`; build: `cc ^1.1`; **dev-only: `tree-sitter ^0.24`** |
| `tree-sitter-typescript` | **0.23.2** | normal: `tree-sitter-language ^0.1`; build: `cc ^1.1`; **dev-only: `tree-sitter ^0.24`** |

### 6.2 Do grammar crate versions have to match the core crate version? **No — not since `tree-sitter-language`.**

This was a genuine problem historically: <https://github.com/tree-sitter/tree-sitter/issues/3095>
documents the `expected tree_sitter::Language, found a different tree_sitter::Language` failure caused by
two copies of the core crate in the dependency graph.

The fix is the **`tree-sitter-language` crate**: grammar crates now export a `LanguageFn` from
`tree-sitter-language` and depend *only* on that crate, not on `tree-sitter` core. The core crate provides
`impl From<LanguageFn> for Language`. Evidence:

- `tree-sitter-java 0.23.5`'s only **normal** dependency is `tree-sitter-language ^0.1`; its dependency on
  `tree-sitter ^0.24` is **`kind: dev`** (tests only) and therefore does not constrain downstream builds.
- **Mergiraf 0.18.0** builds `tree-sitter = "0.26"` against grammar crates at versions 0.3, 0.5, 0.7,
  0.23, 0.24, 0.25, 1.1, 1.2 simultaneously. **[primary — `Cargo.toml`]**
- **difftastic 0.70** does the same with `tree-sitter = "0.26.8"` + `tree-sitter-language = "0.1.7"` and
  ~62 grammars. **[primary]**

The real compatibility constraint is the **parser ABI version**, not the crate version. tree-sitter 0.25
bumped the internal ABI to **15** and requires a `tree-sitter.json` in grammar repos.
<https://github.com/tree-sitter/tree-sitter/releases/tag/v0.25.0> **[secondary]**

**Practical guidance for M0:** depend on `tree-sitter = "0.26"` and `tree-sitter-java = "0.23"`, and do
*not* try to "match" the numbers. If a grammar fails with a language-version error, the fix is a newer
grammar crate, not an older core.

### 6.3 API shape at 0.24–0.26 (what an arena wrapper touches)

Confirmed against the official Rust binding README
(<https://raw.githubusercontent.com/tree-sitter/tree-sitter/master/lib/binding_rust/README.md>) and
against Mergiraf's actual usage in `src/ast.rs` **[primary]**:

```rust
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator, Tree, TreeCursor, Range};

let mut parser = Parser::new();
parser.set_language(&tree_sitter_java::LANGUAGE.into())?;   // &Language, and LANGUAGE is a LanguageFn
let tree = parser.parse(source, None).unwrap();
let mut cursor = tree.walk();                                // TreeCursor for child traversal
let mut qc = QueryCursor::new();                             // QueryMatches is a StreamingIterator
```

Changes to be aware of when writing the `sm-cst` arena wrapper:

- **`LANGUAGE.into()` (0.23+).** Grammar crates export `LANGUAGE: LanguageFn`, not a
  `fn language() -> Language`. `set_language` takes `&Language`.
- **`StreamingIterator` (0.24+).** `QueryMatches` no longer implements `Iterator`; it implements
  `StreamingIterator`. `tree_sitter` **re-exports `StreamingIterator` and `StreamingIteratorMut`** as of
  0.25, so you do not need to add `streaming-iterator` yourself. Mergiraf imports it straight from
  `tree_sitter`.
- **`TreeCursor` for traversal.** `Node::children(&mut cursor)` needs a cursor. For a one-shot arena build
  this is fine — walk once with a single cursor and never touch the tree-sitter API again.
- **Cancellation changed in 0.25.** `set_timeout_micros` and the cancellation flag are **deprecated** in
  favour of a **progress callback** passed via `parse_with_options`. Relevant to SPEC §4.7's `--timeout`:
  prefer the progress callback, or (like Mergiraf) enforce the timeout at the merge level rather than
  inside the parser.
- `ts_node_child_containing_descendant` removed in 0.25; use `child_with_descendant`.
- Node no longer carries the language name ("Drop language name from node").

Source for the 0.25 breaking-change list: <https://github.com/tree-sitter/tree-sitter/releases/tag/v0.25.0>
**[secondary]**. Note: `CHANGELOG.md` is **not** present at the repo root on `master` (404 on raw fetch) —
use the GitHub Releases pages.

### 6.4 Grammar-crate choice for Java

Mergiraf does **not** use the official `tree-sitter-java`; it uses **`tree-sitter-java-orchard = "0.5"`**
(from the `grammar-orchard` collective, which also supplies its Rust, Python, Dart, Go-mod grammars, and
supplies difftastic's Clojure grammar). **[primary — both `Cargo.toml`s]** Worth evaluating: the orchard
forks are maintained specifically for tooling that needs stable, current grammars. Start with the official
`tree-sitter-java 0.23.5` and switch only if we hit concrete grammar bugs — but know the option exists and
that the closest competitor chose it.

---

## 7. Quick-reference constants table

| Constant | ASE'14 paper | GumTree `gumtree-classic` today | GumTree default (`gumtree-simple`) | Mergiraf primary (base↔side) | Mergiraf auxiliary (left↔right) | SPEC §4.3 |
|---|---|---|---|---|---|---|
| min height | `minHeight = 2` (leaf = 1) | `st_minprio = 1` (leaf = 0) — same policy | same | `min_height = 1` (leaf-inclusive) | `min_height = 2` | `MIN_HEIGHT = 2` |
| bottom-up similarity | `minDice = 0.5` (dice) | `bu_minsim = 0.5` (dice) | adaptive `1/(1+ln(|desc(c)|+|desc(t)|))`, Chawathe sim | `sim_threshold = 0.4` (dice) | `sim_threshold = 0.6` (dice) | `MIN_DICE = 0.5` |
| recovery size cap | `maxSize = 100` | `bu_minsize = 1000`, exact Zhang-Shasha TED | none (LCS + histogram only) | `max_recovery_size = 100`, RTED | RTED off | `MAX_SIZE = 100` |
| candidate kind constraint | same kind | same kind (hard) | same kind (hard) | same kind (hard) | same kind (hard) | "kinds compatible" |

---

## 8. Implications for `semantic-merge`

### 8.1 M2 — the matcher

1. **Implement `gumtree-classic`, not `gumtree-simple`.** The classic algorithm is what Mergiraf uses and
   what the merge literature is calibrated on; `gumtree-simple`'s adaptive threshold has no published
   merge-quality evidence. Start with `MIN_HEIGHT = 2` (paper semantics), `MIN_DICE = 0.5`,
   `MAX_SIZE = 100` as SPEC says — those are right — and treat `bu_minsize = 1000` as an alternative to
   sweep in M5, not a default.
2. **Adopt Mergiraf's two-configuration asymmetry — this is the highest-value single change to SPEC §4.3.**
   Matchings against base should be *permissive* (`min_height = 1`, `dice > 0.4`, TED recovery on) because
   a miss costs a false conflict; the ours↔theirs matching should be *strict* (`min_height = 2`,
   `dice > 0.6`, no TED) because a false positive there corrupts the merge. SPEC's single set of constants
   is the wrong shape. Make the matcher take a config struct, and instantiate it twice.
3. **Kind equality is a hard precondition for bottom-up candidates**, not a soft "compatible kinds" check.
   All three reference implementations agree.
4. **Be conservative about ambiguous top-down matches.** GumTree greedily disambiguates via a five-level
   comparator (siblings sim → parents sim → position-in-parent → textual position → absolute position);
   Mergiraf instead **refuses to match any subtree whose hash is duplicated on either side** and re-opens
   its children. For a *merge* tool (SPEC rule 4: a conflict is acceptable, a wrong merge is not),
   Mergiraf's conservatism is the correct default. Implement the GumTree comparator behind a flag so M5
   can measure whether it helps.
5. **Truncate between phases.** Replace each exactly-matched subtree with a stub before the bottom-up pass.
   Mergiraf does this; it makes the O(n²)-ish candidate search tractable on real files.
6. **Build the auto-tuner now, cheaply.** GumTree's `AutoMatchers` sweeps 5×5 configurations and keeps the
   one minimising edit-script size. That is ~20 lines on top of what M2 already needs and it turns M5's
   constant sweep from a research task into a `rayon` loop.
7. **Instrument near-misses.** Mergiraf logs candidates with `sim > 0.75 × threshold`. Wire that into the
   `sm match` visualiser SPEC mandates — near-misses are where the tuning signal is.
8. Trivia: `distance <= 1 newline` is independently validated as the right attachment heuristic. Keep
   SPEC §4.2's policy; be aware that Mergiraf's is *narrower* (bundling only under commutative parents),
   so our general trivia model is unproven territory and needs the test suite SPEC already demands.

### 8.2 M4 — the merge

**Things Mergiraf gets right that we should copy:**

1. **Fast mode.** Run `git merge-file` first; return it unchanged if clean. This makes the regression rate
   in SPEC §6.2 *structurally* zero for the clean-merge population, makes p50 latency trivial, and reduces
   the structured path to the cases that actually matter. If conflicts remain, reconstruct fictional
   revisions from the conflicted file and use them to **seed the matcher**. SPEC does not have this and it
   is the biggest architectural idea we are missing.
2. **Per-subtree line-merge fallback (`LineBasedMerge` as a merged-tree node type).** SPEC only has a
   whole-file fallback. A localised fallback lets one gnarly method degrade to diff3 while the rest of the
   file merges structurally.
3. **The delete/modify "covering" check.** Do not just emit `Deleted × Updated → conflict`. First check
   whether the modified descendants survive elsewhere in the deleting revision (i.e. the element was moved,
   not deleted); if the other side's changes are all covered, merge cleanly. This is a real resolve-rate
   win that also *avoids* a wrong answer.
4. **Signature-keyed duplicate detection as a post-pass.** After building the merged tree, look for two
   children of a commutative parent with the same signature and turn them into a conflict. For Java the
   signature is the erasure: name + parameter types. Both Spork and Mergiraf do this; it catches
   both-sides-added-the-same-method.
5. **Commutative parents need `ChildrenGroup`s, delimiters and separators.** SPEC §4.5's flat
   "unordered sets" will let an import migrate past a class declaration. Model the Java `program` node as
   ordered *groups* of internally-commutative children, and carry `(prefix, separator, suffix)` strings so
   the emitter can synthesise punctuation. Also mark `import_declaration` **atomic** — never merge inside
   one.
6. **Normalise line endings to LF on input and restore the original style on output.** Cheap, and it
   removes a whole class of spurious diffs.
7. **Split multi-line leaves (block comments, multi-line strings) into per-line subnodes** so they merge
   line-wise instead of conflicting atomically.
8. **Driver contract:** use `%O %A %B -s %S -x %X -y %Y -p %P -l %L`; detect the literal string `"%S"` as
   the signal for git < 2.44 and fall back to default labels; make the destructive in-place write an
   explicit `--git` flag that conflicts with `--output`. Timeouts: 5s fast path, 10s structured, `0` to
   disable.

**Things to do differently / where SPEC is already better:**

9. **The PCS/3DM question.** Mergiraf and Spork both encode trees as parent-child-successor triples,
   merge the triple sets, and rebuild. SPEC §4.5's per-node fate table is a *different* formulation. It is
   more legible and it makes the `Moved × Updated → apply both` rule explicit, but it has not been shown to
   work at scale, and the known 3DM pathologies (delete/modify, §2.5) are exactly the cases the fate table
   also has to get right. **This is a genuine, consequential design fork and SPEC rule 7 says to flag it
   rather than pick silently.** Recommendation: keep the fate table as the *specification* of intent, but
   plan for the child-list reconstruction to be the hard part either way; read Spork §3.4 (the taxonomy of
   PCS inconsistencies) before writing `sm-merge`.
10. **Byte-exact provenance emission (SPEC §4.6) is stronger than Mergiraf's approach.** Mergiraf does not
    keep whitespace in the tree at all — it re-synthesises inter-token whitespace at render time by
    "imitating the whitespace from the original revisions". SPEC's "copy byte ranges, never reprint"
    invariant is strictly more faithful and is a defensible product differentiator. Keep it.
11. **Conflict granularity.** Mergiraf can generate sub-line conflicts and offers `--compact` to render
    them inside a line. SPEC §4.6 mandates statement/declaration boundaries. SPEC's choice is safer;
    consider `--compact` as a later opt-in.
12. **Ship a bug-report packager.** Mergiraf's `mergiraf report` bundles a failing case. Ours should do the
    same and feed M5's incorrect-resolution gallery directly.

### 8.3 M5 — the evaluation

**Numbers to compare against.**

| Source | What it measures | Number |
|---|---|---|
| Spork replication CSV (n=1,740 file merges, 119 projects) | outcome over *all* file merges | spork 86.2% success / 11.8% conflict / 2.0% fail; jdime 85.6% / 13.0% / 1.3% |
| LastMerge paper (2025) | relative | Mergiraf **42% fewer false negatives** than Spork; LastMerge **15% fewer false positives** than jDime; ~10% divergence generic vs language-specific |
| Mori & Hashimoto, ASE 2025 | parsable + universal, n=43,774 scenarios, 76 projects | `d3j` reports zero incorrect; Git reports "a number" |
| Weave README | 31 scenarios | 31/31 vs git 15/31; 100% vs Mergiraf 83% |
| Mergiraf | — | **no published evaluation exists** |

**Concrete recommendations:**

1. **Report on both denominators.** SPEC §6.2's resolve rate is conditioned on git conflicting; Spork's
   86% is over all file merges. Report both so the comparison is honest and so a reader can place us.
2. **Adopt Spork's per-case record schema verbatim.** `project, merge_commit, base_blob, left_blob,
   right_blob, expected_blob, replayed_blob, tool, outcome, diff_size, num_conflicts, conflict_size,
   runtime`. It is a proven schema, it makes our CSV diffable against theirs, and `diff_size` against the
   human resolution is a better correctness signal than a binary equal/not-equal.
3. **Add the ground-truth-free metrics from Mori & Hashimoto: parsable and universal.** SPEC already
   requires the parse-stability property test; promote it to a reported metric, and add universality
   (every edit from each branch appears exactly once; no edit appears that came from neither). This is the
   answer to "the human resolution isn't ground truth" and it lets us report on the ~40k scale where human
   resolutions are unreliable.
4. **Benchmark against Mergiraf directly on the same corpus.** It is installable, fast, GPL (so we can run
   it, we just cannot copy it), and it has no numbers of its own. Running the same corpus through
   `git merge-file`, Mergiraf, and `sm` is the single most valuable table in the report.
5. **Fix the false-positive/false-negative vocabulary in the report.** LastMerge uses FP = spurious
   conflict, FN = missed conflict (silently wrong merge). SPEC uses "regression rate" and
   "incorrect-resolve rate". State the mapping explicitly in the README.
6. **Target repo count:** SPEC says 30–50 Java repos; Spork used 119 projects (from 589 candidates, 359
   buildable), jdime's extended study 50 projects / 434 scenarios, Mori & Hashimoto 76 projects / 43,774
   scenarios. SPEC's ≥20 repos / ≥2,000 cases (M1 exit) is a reasonable floor but is at the low end;
   aim for Spork-comparable or better, and **publish the funnel** (candidates → filtered → used) the way
   Spork's `candidate_projects.txt` / `buildable_candidates.txt` / `projects.csv` chain does.
7. **Latency:** difftastic's three independent budgets (`DFT_GRAPH_LIMIT` = 3M vertices,
   `DFT_PARSE_ERROR_LIMIT`, `DFT_BYTE_LIMIT`) with graceful line-diff degradation is the model to copy for
   SPEC §6.2's p99 < 1s target.

---

## 9. What is actually novel here

SPEC §2 asserts: *"Everything above is syntactic. The name-binding layer in M6 … is where the genuinely
novel contribution is."* **That claim as written is too strong and would not survive review.** Here is
what the record actually shows.

### 9.1 Confirmed: no *merge tool* does merge-time reference re-resolution

- **Mergiraf** — verified by reading the source. `src/signature.rs` computes signatures from **syntactic
  key paths** (field names, child kinds) with equality defined as "quasi-isomorphism"; there is no scope
  tree, no symbol table, no `sm-bind` analogue anywhere in `src/`. Its only "semantic" step is duplicate-
  signature detection among siblings of a commutative parent. **[primary]**
- **Weave** — its own README states the entity matching is *"not full semantic analysis like name binding
  or reference resolution"*. **[secondary — but it is the tool's own claim]**
- **Spork** — Spoon-based, so a reference model exists in the underlying library, but `grep` over
  `src/main/java` finds `getReference`/`resolve` only in the GumTree-Spoon bridge, the CLI and the pretty
  printer — not in the merge logic. Its Java-specific semantics is, like Mergiraf's, duplicate-signature
  detection. **[primary]**
- **jdime / FSTMerge / s3m** — semistructured; they match declarations by name and signature, which is not
  the same as resolving *uses* to *declarations*.
- **difftastic** — read-only diff, no name analysis.
- **LastMerge** — described as generic structured merge over tree-sitter CSTs with a thin config
  interface; no semantic layer reported. **[secondary]**

**So: as a feature of a git merge driver, "re-resolve every reference in the merged tree and flag
references that now resolve to nothing or to a different declaration" appears to be unimplemented.** That
part of SPEC's claim holds.

### 9.2 But the *problem* is well studied, and one tool does essentially this analysis

The rename-in-A / new-call-in-B failure is known in the literature as a **build conflict** (or
compile-time semantic conflict), and it has dedicated tooling:

- **Bucond** — Towqir, Shen et al., *Detecting Build Conflicts in Software Merge for Java Programs via
  Static Analysis*, **ASE 2022**, <https://dl.acm.org/doi/10.1145/3551349.3556950>. Models base, left and
  right each as a graph, extracts entity-related edits (e.g. class renaming), and flags **cross-branch
  edit combinations that violate def-use constraints between program entities** — the paper's own example
  is *"one branch adds a reference to field F while the other branch removes F"*, which is precisely
  SPEC's motivating example. **57 patterns covering 97% of the build conflicts observed; 100% precision,
  88–100% recall.** **[secondary]**
- **IntelliMerge** — Shen et al., OOPSLA 2019, <https://dl.acm.org/doi/10.1145/3360596>. Builds Program
  Element Graphs for the three versions, detects refactorings (renames, moves) and uses them to improve
  matching *and* merging. Reported to handle some build conflicts, though the Bucond paper notes it
  detected only four, all of type "Import: remove def vs. add use". **[secondary]**
- **static-semantic-merge** (`spgroup`) — <https://github.com/spgroup/static-semantic-merge>. Runs
  **after a textually conflict-free merge** and uses SOOT-based static analysis to detect interference,
  optionally reverting the merge. The *architectural position* — post-merge semantic check on a clean
  merge — is exactly SPEC M6's. **[secondary, plus its own README]**
- **SAM / TOM** — semantic conflict detection by generating unit tests as partial specifications,
  <https://arxiv.org/html/2310.02395>, <https://www.sciencedirect.com/science/article/pii/S0164121224001158>.
  **[secondary]**
- **RefFilter** (2025) — refactoring-aware static analysis that cuts semantic-conflict false positives by
  ~32%, <https://arxiv.org/abs/2510.01960>. **[secondary]**
- **The Effect of Pointer Analysis on Semantic Conflict Detection** (2025),
  <https://arxiv.org/abs/2507.20081>. **[secondary]**

### 9.3 How to frame the contribution without overclaiming

**Do not say:** "no existing tool detects semantic merge conflicts" or "git and every existing structural
merge tool miss this" (SPEC §7, M6). Bucond detects this class of conflict with 100% precision, and
the semantic-conflict-detection subfield is roughly a decade old with an active 2025–2026 publication
stream.

**Do say**, and each clause is defensible against the evidence above:

> Semantic conflict detection exists as a research subfield, but it lives *outside* the merge — as a
> separate Java-specific, SOOT/graph/test-generation-based post-hoc analysis (Bucond, IntelliMerge,
> static-semantic-merge, SAM). No production merge driver performs it. `semantic-merge` integrates
> name-binding-based conflict detection **into the merge driver itself**: it re-resolves references in the
> candidate merged tree using the same language-agnostic tree-sitter substrate that produced the merge,
> in-process, within a sub-second latency budget, and reports the result as an ordinary merge conflict.

The novel claims that survive scrutiny are therefore:

1. **Integration and position in the pipeline** — inside the merge driver, on the candidate merged tree,
   before the result is written, not as a separate post-merge CI/analysis step.
2. **Language-agnosticism** — every prior semantic-conflict tool is Java-only and most require a
   compilable project and/or a build. Ours needs only a tree-sitter grammar and a per-language scope
   config, and must work on a single file in a detached merge-driver invocation with no classpath.
3. **Latency and determinism** — sub-second, no build, no solver, no LLM (SPEC §9).
4. **Measurement** — a corpus-scale measurement of how often merge-time reference breakage actually
   occurs in real merges, alongside a resolve/incorrect-rate evaluation of the syntactic merge, which
   Mergiraf (the closest tool) has never published at all.

Point 4 is arguably the most defensible of the four, and it is also the one SPEC §6 already commits to.

**One honest limitation to state up front in M6:** a single-file merge driver sees one file. The
canonical rename-in-A / call-in-B case is frequently *cross-file*, and cross-file is exactly where Bucond
and IntelliMerge operate. Within-file detection is real and worth measuring, but the README must say
which subset of the problem is in scope, or a knowledgeable reader will ask in the first five minutes.

---

## 10. Source index

**Primary (inspected directly):**
- <https://github.com/qundao/mirror-mergiraf> — Mergiraf 0.18.0 source, docs, Cargo.toml, examples
- <https://github.com/GumTreeDiff/gumtree> — matchers, ConfigurationOptions, AutoMatchers, CHANGELOG
- <https://github.com/Wilfred/difftastic> — `manual/src/diffing.md`, `manual/src/tree_diffing.md`, `src/diff/graph.rs`, `src/options.rs`, CHANGELOG
- <https://github.com/ASSERT-KTH/spork> — README, `replication/` CSVs
- <https://index.crates.io/tr/ee/tree-sitter> (and `-java`, `-typescript`, `tree-sitter-language`)
- <https://raw.githubusercontent.com/tree-sitter/tree-sitter/master/lib/binding_rust/README.md>
- <https://github.com/jelmer/awesome-merge-drivers>
- <https://github.com/Ataraxy-Labs/weave>
- <https://github.com/spgroup/static-semantic-merge>
- <https://github.com/ace-design/git-merge-adv>

**Secondary (search-summarised; host blocked from this sandbox):**
- GumTree paper — <https://hal.science/hal-01054552/document>, <https://doi.org/10.1145/2642937.2642982>
- Hyperparameter Optimization for AST Differencing — <https://arxiv.org/abs/2011.10268>
- RefactoringMiner AST diff + benchmark — <https://arxiv.org/abs/2403.05939>, <https://dl.acm.org/doi/10.1145/3696002>
- HyperDiff — <https://inria.hal.science/hal-04189855/>, <https://dl.acm.org/doi/10.1145/3611643.3616312>, <https://github.com/quentinLeDilavrec/HyperAST>
- Spork paper — <https://arxiv.org/abs/2202.05329>, <https://doi.org/10.1109/TSE.2022.3143766>
- jdime — <https://dl.acm.org/doi/10.1145/2351676.2351694>, <https://link.springer.com/article/10.1007/s10515-014-0151-5>, <https://github.com/se-sic/jdime>
- On the Correctness of Software Merge (d3j) — <https://arxiv.org/abs/2607.07987>, <https://doi.org/10.1109/ASE63991.2025.00193>
- LastMerge — <https://arxiv.org/abs/2507.19687>
- Bucond — <https://dl.acm.org/doi/10.1145/3551349.3556950>
- IntelliMerge — <https://dl.acm.org/doi/10.1145/3360596>
- SAM / semantic conflicts via unit tests — <https://arxiv.org/html/2310.02395>
- RefFilter — <https://arxiv.org/abs/2510.01960>
- Pointer analysis & semantic conflict detection — <https://arxiv.org/abs/2507.20081>
- ConGra — <https://arxiv.org/abs/2409.14121>
- tree-sitter v0.25.0 release notes — <https://github.com/tree-sitter/tree-sitter/releases/tag/v0.25.0>
- tree-sitter grammar versioning issue — <https://github.com/tree-sitter/tree-sitter/issues/3095>
- LWN on Mergiraf — <https://lwn.net/Articles/1042355/>
- Mergiraf docs (rendered) — <https://mergiraf.org/architecture.html>, <https://mergiraf.org/related-work.html>, <https://mergiraf.org/usage.html>
