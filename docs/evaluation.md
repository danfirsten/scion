# Evaluation

`semantic-merge` replayed over 16,238 real Java merge conflicts and 4,835 real
clean merges mined from 62 open-source repositories (SPEC.md §6, milestone M5).

The tables below are machine-produced by `sm-eval report` from the replay log;
the prose around them is hand-written, which is why the reproduce command writes
the generated report to `/tmp` rather than over this file. `docs/evaluation.json` carries the same
numbers in machine-readable form, plus the incorrect-resolution gallery.

**Reproduce:**

```
cargo build --release
sm-eval replay --corpus corpus/ --sm target/release/sm --out /tmp/replay --control
sm-eval report --replay /tmp/replay --out /tmp/report.md --json docs/evaluation.json
```

---

## 1. Headline

| Metric | Value | Denominator | SPEC §6.2 target |
|---|---:|---|---|
| **Resolve rate** | **52.34%** | 16,238 gradeable conflicted cases | maximize |
| **Correct-resolve rate** (AST-equal) | 55.23% | 8,499 clean results | maximize |
| **Correct-resolve rate** (byte-exact) | 44.16% | 8,499 clean results | — |
| **Incorrect-resolve rate** | **44.77%** | 8,499 clean results | **< 1% — missed** |
| — of those, differing only in comments | 36.08% | 3,805 incorrect | — |
| Incorrect per conflicted file | 23.43% | 16,238 gradeable cases | — |
| **Correct decline** | 47.66% | 16,238 gradeable cases | acceptable |
| **Regression rate** vs `git merge-file` | **0.00%** (0 of 4,781) | clean cases the line merge also merged | ≈ 0 — met |
| Regression rate vs `git merge-tree` | 0.81% (39 of 4,835) | all clean cases | — |
| **Divergence** vs `git merge-file` | **0.02%** (1 of 4,781) | clean cases the line merge also merged | ≈ 0 — met |
| Divergence vs `git merge-tree` | 0.17% (8 of 4,835) | all clean cases | — |
| **Latency** p50 / p90 / p99 | **24 / 93 / 304 ms** | 25,671 invocations | p99 < 1000 ms — met |
| Parsable (ASE 2025) | 99.99% | 8,499 clean results | — |
| Universal (ASE 2025) | 99.99% | 8,499 clean results | — |
| `git merge-file` control, same inputs | resolved **0** of 16,109 | gradeable cases the line merge saw | — |

**Three of the four targets are met and one is not.** Regression, divergence and
latency are comfortably inside their budgets. The resolve rate is a real number
on a hard population — git's own line merge resolves *none* of these files. The
incorrect-resolve rate misses its target by a wide margin, and §3 is the analysis
of why; the short version is that the measured 44.77% is dominated by three
classes that are not wrong merges, and a hand audit of a 30-case sample puts the
genuinely-broken-output rate at roughly **1 in 30 of the incorrect population** —
an estimate with a wide interval, and still above 1%.

Nothing below is adjusted to improve a number. Where a different denominator or a
different equality notion tells a different story, both are reported.

---

## 2. Populations and denominators

The corpus (`docs/corpus-summary.md`) holds 34,811 cases. They are not all the
same kind of thing, and mixing them is how an evaluation ends up quoting a number
nobody can reproduce.

| bucket | cases | replayed | in a rate? |
|---|---:|---:|---|
| `gradeable` | 16,238 | 16,238 | **yes — every conflicted-case rate** |
| `contaminated` | 4,598 | 4,598 | reported separately (§9) |
| `clean` | 5,640 | 4,835 | yes — regression and divergence |
| `incomplete` | 4,520 | 0 | no |
| `rename` | 3,815 | 0 | no |

* **gradeable** — git conflicted, all of base/ours/theirs *and* the human
  resolution exist, and the contamination filter did not flag the case.
* **contaminated** — the same, but the human's committed resolution contains
  lines present in no input, i.e. they did unrelated work while merging. Kept and
  reported apart (SPEC §6.1).
* **rename** — the corpus's `add_none` shape. `git merge-tree` names the
  conflicted path as it exists on one side; when the other side *moved* the file,
  the path resolves to nothing in base and nothing on that side, so the case has
  one revision and a resolution. **These are rename conflicts, not three-way
  merges**, and grading them as three-way merges would measure the wrong thing.
  They are counted and not replayed. Following `git diff -M` rename detection in
  the miner, so these become real three-way cases, is concrete future work.
* **incomplete** — add/add (no base), delete/modify, modify/delete, or no
  resolution. Real merge situations the driver must survive, not gradeable
  against ground truth as three-way merges.
* 805 clean cases lack one of the three inputs and were not replayed.

### 2.1 Which "git" the rates are conditioned on

The corpus's *"git conflicted here"* label comes from `git merge-tree`
(merge-ort) at mining time. The driver's fallback is `git merge-file`, the
classic three-way line merge. **They are different algorithms and they disagree
on real cases**, so both readings are reported:

* Of the 16,238 gradeable cases, `git merge-file` also conflicts on **16,109**.
  On the other 129 the driver's fast path returns git's own bytes, so counting
  them as resolves of ours would be scoring git's work. Conditioned on the
  narrower denominator the headline barely moves: resolve **51.96%**,
  incorrect-resolve **45.23%**.
* Of the 4,835 clean cases, `git merge-file` also merges **4,781** cleanly. All
  39 "regressions" are among the 54 where it does *not* — see §6.

### 2.2 The two equality notions, stated exactly

**Byte-exact** is `output == resolved`, byte for byte.

**AST-equal modulo formatting** parses both files and compares them
structurally. What it ignores and what it counts:

| ignored | counted |
|---|---|
| whitespace *between* tokens: indentation, blank lines, line breaks | every token's text, including anonymous operator tokens |
| the *layout* of a comment — leading/trailing whitespace on each of its lines | every comment's text, and the order comments appear in |
| byte offsets | the tree's shape, kind for kind |

**Comments count.** That is the strict choice and it is deliberate: the
structural comparator underneath (`sm_match::structurally_equal`) drops comments,
because comments do not participate in a matching, so using it alone would score
"we deleted a Javadoc block the human kept" as a correct resolution. A merge
driver that silently drops comments is broken, so the metric has to be able to
see it. The comment-blind variant is computed too and reported as the sub-line
"differing only in comments" — 36.08% of the incorrect population, which is what
the choice costs. Comment *layout* is ignored because reindenting a block
comment's interior lines is the one transform SPEC §4.6 sanctions.

A file that does not parse, or that parses with `ERROR` nodes, is not AST-equal
to anything unless it is byte-identical.

**Parsable** and **universal** are the ground-truth-free criteria from Mori &
Hashimoto (docs/prior-art.md §8.3.3). Parsable: the output parses with no error
nodes. Universal is approximated as zero synthesized bytes **plus token
authenticity** — every token of the output is a token of one of the three
inputs — recomputed by the harness from the four trees rather than trusted from
the driver's own counters. Both are scored only on conflict-free output; a file
with markers in it does not parse and its markers are synthesized by
construction.

---

## 3. The incorrect-resolve rate: what the 44.77% is made of

This is the number SPEC §6.2 calls "the metric that matters" and the target is
under 1%. The measured value is 44.77% of clean results (3,805 of 8,499). That is
the honest number and it stays the headline. What follows is the analysis.

### 3.1 Automated taxonomy over all 3,805

Every incorrect resolution was classified by comparing the multiset of
whitespace-trimmed lines in our output with the human's, and asking where the
lines that differ came from.

| class | count | share | median lines in disagreement |
|---|---:|---:|---:|
| `comments_only` — the code is identical, the comments are not | 1,373 | 36.1% | 0 |
| `mixed_disagreement` — content differs in both directions | 765 | 20.1% | 5 |
| `reorder_only` — identical line multiset, different order | 712 | 18.7% | 0 |
| `we_kept_extra_content` — we have lines the human does not | 293 | 7.7% | 4 |
| `we_kept_ours_human_dropped` | 179 | 4.7% | 1 |
| `we_kept_theirs_human_dropped` | 170 | 4.5% | 1 |
| `we_dropped_content` | 108 | 2.8% | 3 |
| `we_dropped_ours` | 96 | 2.5% | 1 |
| `we_dropped_theirs` | 96 | 2.5% | 1 |
| `indentation_only` | 13 | 0.3% | 8 |

Two classes account for 55% of the population and neither is a wrong program.

### 3.2 Defect class 1 — a both-sides edit to a *floating* comment is invisible (1,280 cases, 34% of all incorrect)

Of the 1,373 comment-only differences, **1,280 differ only in the copyright
header**, and 1,182 of those come from three merge commits in the Spring projects
that changed `Copyright 2012-2025` to `Copyright 2012-present` across the whole
tree while the other branch was bumping the year.

```diff
  /*
- * Copyright 2012-present the original author or authors.   <- the human
+ * Copyright 2012-2025 the original author or authors.      <- us
```

Base says `2012-2024`, ours says `2012-2025`, theirs says `2012-present`. **Both
sides edited the same comment differently, so this must be a conflict** — and for
a comment *attached* to a declaration it already is: `comment_edit` is
`sm-merge`'s single most common conflict reason in this replay (1,612 hits on
`package_declaration` alone, §7). It does not fire here because the Spring header
is followed by a blank line, so the trivia policy leaves it **floating** rather
than attaching it to `package_declaration`, and a floating comment lives in the
*gap* before the following element. Gaps come from the container's frame side
(`sm-merge`'s crate docs §8–§9), which is ours by default, so their edit is
silently discarded.

**Recommended fix (not made here):** make floating comments first-class,
text-keyed elements of a commutative child list, so a both-sides edit becomes an
ordinary `CommentEdit` conflict exactly as it already does for an attached one.
This is a change to `sm-merge`'s child-list model with real blast radius, which
is why it is written down rather than made at the end of the milestone whose job
is to measure. It would convert ~1,280 wrong outputs in this corpus into
conflicts.

### 3.3 Defect class 2 — insertion order in commutative containers (712 cases, 18.7%)

`reorder_only` means our output and the human's contain exactly the same lines in
a different order. 331 of the 712 are import-list order; 381 are class members or
statements.

```diff
  import org.graalvm.collections.EconomicSet;
- import org.graalvm.compiler.serviceprovider.JavaVersionUtil;   <- the human keeps it here
  import com.oracle.svm.core.util.UserError;
  ...
+ import org.graalvm.compiler.serviceprovider.JavaVersionUtil;   <- we move it here
```

The cause is `sm-merge`'s rule for a conflicting region of a commutative
container: *"our order is preserved; their additions follow"*. It is
deterministic and it never loses an element, but it does not reproduce where a
line merge — and therefore the human — would have put the insertion. In Java the
result is semantically identical, since member and import order carry no meaning,
but it is a visible difference and the AST-equal notion counts it, correctly: the
metric is not allowed to decide that reordering somebody's file is free.

**Recommended fix (not made here):** anchor an insertion to its predecessor in
the contributing revision rather than appending it after our whole run.

### 3.4 Defect class 3 — an update applied to a mis-aligned sibling (rare, and the one that is genuinely dangerous)

`apache/accumulo`, `LiveTServerSet.java`, merge `3258c83cfb77`:

```diff
- private LiveTServersSnapshot tServersSnapshot = null;        <- the human
+ private final LiveTServersSnapshot tServersSnapshot = null;  <- us
```

Their branch added `final` to three *other* fields (`current`,
`currentInstances`, `locklessServers`); our branch added the new field
`tServersSnapshot`, which is reassigned later. The child-list alignment paired
their modifier edit with our inserted field, and the result does not compile.
This is the class SPEC §0.4 rules out categorically, and it is the class the hand
audit in §4 finds at roughly 1 case in 30 of the incorrect population.

### 3.5 The class that is *not* a defect: the human made a further judgement

`apache/shardingsphere`, merge `579cfabeaa09`. Our branch changed
`new DataRepository(ds).demo()` to `demo(true)`; their branch changed
`DataRepository` to `JDBCRepository`. We produced
`new JDBCRepository(ds).demo(true)` — both edits applied, which is exactly the
"apply both" win the whole project exists for. The human committed
`new JDBCRepository(ds).demo()`, silently dropping the other branch's argument.

Under the metric this is an incorrect resolution. Under any reading of what a
merge *should* do, ours is the better answer. SPEC §6.2 names this exactly: the
human resolution is ground truth for what was **committed**, not for what was
**correct**.

### 3.6 Concentration

1,548 distinct merge commits produce the 3,805 incorrect resolutions, and the ten
largest contribute 35.7% of them. `spring-projects/spring-boot` alone supplies
1,038 of the 8,499 clean results, 97.4% of them graded incorrect, almost entirely
the copyright header of §3.2. This is why the unweighted per-repository mean is
reported next to the pooled number: they happen to agree here (pooled incorrect
44.77%, per-repo mean 45.46% over the 49 repositories with ≥ 20 gradeable cases),
but the agreement is a fact to be checked, not assumed.

**Sensitivity, clearly labelled as such and not the headline:** removing the
1,280 copyright-header cases from both numerator and denominator gives resolve
48.26% and incorrect-resolve 34.98%.

---

## 4. Hand audit of 30 sampled incorrect resolutions

30 incorrect resolutions were drawn with a seeded hash (`seed = 20260728`) and
each was read against its four input files. SPEC §6.2's honesty requirement is
that the human resolution is not an oracle for correctness, so each case was
asked a different question: *is our output a program a reviewer would call
broken?*

| verdict | n | share |
|---|---:|---:|
| **Comment / copyright-header difference only** (§3.2) | 10 | 33% |
| **Declaration or import order only** — semantically identical Java (§3.3) | 9 | 30% |
| **Defensible union: we applied a branch's edit the human discarded** (§3.5) | 10 | 33% |
| **Genuine defect: output a reviewer would reject** (§3.4) | 1 | 3% |

The single defect is the `accumulo` mis-aligned `final` of §3.4.

**What this implies, with the interval stated.** One hit in thirty gives a point
estimate of 3.3% of incorrect resolutions being genuinely broken, i.e. about 127
of 3,805, i.e. roughly **1.5% of clean results**. The 95% Wilson interval on 1/30
is 0.6%–16.7%, so the corresponding interval on the true wrong-merge rate is
roughly **0.7%–7.5% of clean results**. Two honest conclusions follow:

1. the headline 44.77% overstates the harm by about an order of magnitude, and
2. even the bottom of the interval sits at or above SPEC's 1% target. **The
   target is missed.** A larger audit is the obvious next measurement; 30 cases
   is too few to put a tight bound on a rate this small.

---

## 5. What was changed in response, and what it bought

One fix was made during M5, in the driver, chosen because it is confined to the
existing fallback ladder and cannot introduce a new failure mode.

**Duplicate-declaration self-check** (`crates/sm-cli/src/merge/dedup.rs`,
fallback rung 7b). `sm-merge`'s docs already named the limitation: two sides
adding a member with the same signature and different bodies merge to two
members. `alibaba/druid`'s `DataSourceHolder` is the corpus instance —

```java
public void restart() { dataSource.restart(); }
public void restart() { dataSource.resetState(); }
```

— which parses, and whose every token came from a real input, so neither the
reparse check nor the token-authenticity check sees it. The new rung counts
declaration keys (enclosing-name chain, kind, name, whitespace-stripped parameter
text) in the output and in each input, and refuses a clean merge that holds more
copies of a key than any input did. Refusing means falling back to
`git merge-file`, whose bytes are already in hand.

Measured over the full corpus, before and after:

| | before | after | delta |
|---|---:|---:|---:|
| resolved | 8,724 | 8,499 | −225 |
| **correct** | **4,694** | **4,694** | **0** |
| incorrect | 4,030 | 3,805 | −225 |
| incorrect-resolve rate | 46.19% | 44.77% | −1.42 pt |

**Every one of the 225 merges it declined was previously graded incorrect, and
not one correct resolution was lost.** That is as clean a result as this kind of
check can produce, and it is what SPEC §0.4 asks for: a conflict in place of a
wrong answer.

The other two defect classes (§3.2, §3.3) were **not** fixed. Both are changes to
`sm-merge`'s child-list and trivia model, both have a large blast radius, and
neither is a safe thing to land at the end of the milestone that exists to
measure. They are written up above with the mechanism and the recommended repair.

---

## 6. Regression and divergence — every instance investigated

SPEC §6.2 says to investigate every divergence. There are nine, and here is all
of it.

| outcome | count | share of 4,835 |
|---|---:|---:|
| identical to git's committed result | 4,788 | 99.03% |
| diverged (both clean, bytes differ) | 8 | 0.17% |
| regressed (we conflicted, `git merge-tree` did not) | 39 | 0.81% |
| driver error | 0 | 0.00% |
| — took the fast path | 4,777 | 98.80% |
| — the driver's own `git merge-file` was also clean | 4,781 | 98.88% |
| — — **regressions among those** | **0** | **0.00%** |
| — — **divergences among those** | **1** | **0.02%** |

**Regressions: zero, structurally and empirically.** The driver runs
`git merge-file` first and returns its output unchanged when it is clean and
parses (the fast path, docs/prior-art.md §2.8). So "git merged it clean and we
broke it" is impossible for the population where the line merge is clean, and the
replay confirms it: all 39 regressions are among the 54 cases where
`git merge-file` conflicts and `git merge-tree` does not. Those 39 are
ort-versus-merge-file disagreements — merge-ort has rename detection and a
different hunk model — not a regression against the tool the driver would
otherwise have handed the user.

**Divergences: one, and it is git's.** Eight cases have clean output whose bytes
differ from the merge commit's; seven took the semantic path because
`git merge-file` conflicted, so git's own answer differed too. The one remaining
case is `Netflix/conductor`'s `TaskResult.java` at merge `5a5826bd7384`, which
took the **fast path** — meaning our output is `git merge-file`'s output, byte
for byte:

```java
public Status getStatus() { return status; }

/**
 * @return the status
 */
public Status getStatus() { return status; }
```

The classic line merge duplicated a method; merge-ort did not. We faithfully
returned the line merge's answer. **Zero divergences are attributable to the tree
merge.** It is also a good advertisement for the duplicate check of §5, which
does not run on the fast path — the fast path deliberately builds no tree —
extending it there is cheap and is recorded as future work.

---

## 7. Conflict reasons

Why the 7,739 declined cases declined, by `reason@kind`, top 25 of 198:

| reason@kind | count | share |
|---|---:|---:|
| `comment_edit@package_declaration` | 1612 | 11.40% |
| `delete_update@method_declaration` | 997 | 7.05% |
| `move_move@class_declaration` | 979 | 6.92% |
| `update_delete@method_declaration` | 663 | 4.69% |
| `ordered_insert_collision@expression_statement` | 640 | 4.53% |
| `update_update@expression_statement` | 636 | 4.50% |
| `comment_edit@method_declaration` | 513 | 3.63% |
| `delete_update@expression_statement` | 456 | 3.22% |
| `update_delete@expression_statement` | 420 | 2.97% |
| `update_update@local_variable_declaration` | 391 | 2.76% |
| `delete_update@local_variable_declaration` | 381 | 2.69% |
| `ordered_insert_collision@local_variable_declaration` | 366 | 2.59% |
| `update_update@method_declaration` | 348 | 2.46% |
| `update_delete@local_variable_declaration` | 337 | 2.38% |
| `ordered_insert_collision@method_declaration` | 304 | 2.15% |
| `move_delete@class_declaration` | 279 | 1.97% |
| `ordered_insert_collision@if_statement` | 267 | 1.89% |
| `delete_update@class_declaration` | 260 | 1.84% |
| `delete_move@class_declaration` | 221 | 1.56% |
| `comment_edit@class_declaration` | 184 | 1.30% |
| `delete_update@if_statement` | 182 | 1.29% |
| `update_delete@if_statement` | 182 | 1.29% |
| `move_move@method_declaration` | 179 | 1.27% |
| `update_update@class_declaration` | 174 | 1.23% |
| `update_delete@class_declaration` | 164 | 1.16% |
| _173 more_ | 3007 | 21.26% |

`comment_edit` is the largest single family, and one repository dominates it:
`OpenAPITools/openapi-generator` declines 1,465 of its 1,466 gradeable cases,
every one of them `comment_edit@package_declaration`. It is a generated-code
repository whose files carry a regenerated banner comment that both branches
rewrite differently — a genuine both-sides comment edit, correctly refused. That
single repository costs about nine points of pooled resolve rate, which is the
clearest illustration of why the per-repository mean is reported alongside.

The three `delete`/`update` families together (2,976 conflicts) are the ones
docs/prior-art.md §8.2.3 identifies as recoverable with a "covering" check —
confirm the modified descendants survive elsewhere in the deleting revision
before declaring a conflict. That is the highest-value resolve-rate work
remaining.

### Path taken

| path | count | share |
|---|---:|---:|
| `semantic` | 20,448 | 79.65% |
| `fast` | 4,926 | 19.19% |
| `fallback` | 297 | 1.16% |

Fallbacks: `invariant_failed` 252 (225 of them the new duplicate check),
`parse_error` 44, `timeout` 1. No panics, no aborts and no unwritten outputs
across 69,622 invocations.

---

## 8. Latency

| population | n | p50 | p90 | p99 | max | mean |
|---|---:|---:|---:|---:|---:|---:|
| all invocations | 25,671 | 24.1 | 92.6 | 304.4 | 5,019.9 | 43.5 |
| fast path | 4,926 | 13.1 | 32.1 | 89.9 | 229.6 | 18.1 |
| semantic path | 20,448 | 28.2 | 106.1 | 328.4 | 2,151.5 | 49.2 |

Milliseconds of wall clock **including process start-up**, as an external
observer sees it; the driver's own internal total is a few ms lower. **p99 is
304 ms against a 1,000 ms budget.** The single 5,020 ms observation is one case
hitting the 5 s timeout and falling back, which is the timeout working.

**Hardware caveat.** Measured on the development container: 4 cores, four replays
in parallel, release build, warm page cache, corpus on local disk. A merge driver
on an idle laptop will be faster; one on a loaded CI box sharing four cores with
a build will be slower. The number to carry away is the shape — one `fork`/`exec`
plus one parse on the fast path, and a p99 an order of magnitude inside the
budget on the semantic path — not the third digit.

---

## 9. Contaminated cases, reported separately

4,598 conflicted cases carry a resolution containing lines present in no input:
the human did unrelated work while merging. They are in no rate above.

| | value |
|---|---:|
| cases | 4,598 |
| resolved | 1,862 (40.50%) |
| correct (AST-equal) | 806 (43.29% of resolved) |
| incorrect | 1,056 (56.71% of resolved) |

The incorrect rate is 12 points worse than on the clean-denominator population,
which is exactly what a contamination filter is for: on these cases the human's
file provably contains work our merge could not have produced, so grading against
it measures the human's extra commit as much as it measures our merge.

---

## 10. Constant sweep — a null result

`MatchConfig::base_to_side`, the permissive base↔ours and base↔theirs profile,
was swept over `min_height ∈ {1,2,3}` × `min_dice ∈ {0.2,0.4,0.5,0.6}` ×
`max_size ∈ {100,200}` — 24 configurations, including the ASE'14 paper's
`(2, 0.5, 100)`. Each ran over the same seeded, repository-proportional
subsample of **1,500 gradeable cases** (`--subset sample:1500:20260728`), with
the semantic check off, driven through `sm merge --merge-config`. Selection
criterion as specified: minimise incorrect-resolve rate first, break ties on
resolve rate.

| min_height | min_dice | max_size | resolved | correct | incorrect | incorrect % of resolved | resolve % |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 3 | 0.2 | 200 | 732 | 387 | 345 | 47.13% | 48.80% |
| 3 | 0.2 | 100 | 705 | 372 | 333 | 47.23% | 47.00% |
| 3 | 0.4 | 200 | 703 | 370 | 333 | 47.37% | 46.87% |
| 3 | 0.4 | 100 | 671 | 353 | 318 | 47.39% | 44.73% |
| 3 | 0.6 | 200 | 641 | 332 | 309 | 48.21% | 42.73% |
| 3 | 0.5 | 200 | 680 | 352 | 328 | 48.24% | 45.33% |
| 1 | 0.5 | 100 | 787 | 407 | 380 | 48.28% | 52.47% |
| 1 | 0.4 | 100 | 809 | 418 | 391 | 48.33% **(shipped default)** | 53.93% |
| 1 | 0.6 | 100 | 757 | 391 | 366 | 48.35% | 50.47% |
| 1 | 0.5 | 200 | 793 | 409 | 384 | 48.42% | 52.87% |
| 1 | 0.6 | 200 | 770 | 397 | 373 | 48.44% | 51.33% |
| 3 | 0.6 | 100 | 611 | 315 | 296 | 48.45% | 40.73% |
| 1 | 0.4 | 200 | 812 | 418 | 394 | 48.52% | 54.13% |
| 3 | 0.5 | 100 | 647 | 333 | 314 | 48.53% | 43.13% |
| 2 | 0.6 | 100 | 651 | 334 | 317 | 48.69% | 43.40% |
| 2 | 0.2 | 100 | 766 | 392 | 374 | 48.83% | 51.07% |
| 2 | 0.2 | 200 | 786 | 402 | 384 | 48.85% | 52.40% |
| 1 | 0.2 | 100 | 822 | 420 | 402 | 48.91% | 54.80% |
| 2 | 0.4 | 200 | 760 | 388 | 372 | 48.95% | 50.67% |
| 1 | 0.2 | 200 | 825 | 421 | 404 | 48.97% | 55.00% |
| 2 | 0.4 | 100 | 740 | 377 | 363 | 49.05% | 49.33% |
| 2 | 0.5 | 200 | 738 | 375 | 363 | 49.19% | 49.20% |
| 2 | 0.5 | 100 | 715 | 361 | 354 | 49.51% *(ASE'14 paper)* | 47.67% |
| 2 | 0.6 | 200 | 702 | 351 | 351 | 50.00% | 46.80% |

**The surface is flat and the shipped defaults are kept.** Incorrect-resolve
spans 47.13%–50.00% across all 24 configurations — a 2.9-point range — while the
resolve rate spans 40.73%–55.00%. The nominal winner, `(3, 0.2, 200)`, beats the
shipped `(1, 0.4, 100)` by 1.2 points of incorrect rate, which on n ≈ 750 is
under one standard error (two-proportion z = 0.47, p ≈ 0.64), and pays for it
with five points of resolve rate.

The paired comparison says the same thing more sharply. On the same 1,500 cases,
`(3, 0.2, 200)` is wrong on 11 cases the default gets right and right on 57 the
default gets wrong — but it also **declines 77 more cases**. Nearly all of its
apparent advantage is refusing to answer, not answering better.

**Conclusion: the matcher constants do not control correctness on this corpus.**
They trade resolve rate against absolute wrong-merge count at an almost constant
ratio of ~0.48 wrong per resolve. Whatever produces §3's incorrect resolutions
lives in the merge and emit layers, not in the matcher's thresholds. The defaults
from docs/prior-art.md §2.2 stand, unchanged.

---

## 11. The M6 semantic check, scanned over the corpus

`sm merge --semantic=report` re-resolves every reference in a candidate merged
tree and reports references the merge broke or captured (SPEC §7, M6). The replay
ran it on every clean semantic-path merge.

| | value |
|---|---:|
| clean semantic merges checked | 10,226 |
| merges with ≥ 1 finding | **869 (8.50%)** |
| `broken_reference` findings | 12,753 |
| `captured_reference` findings | 796 |

The check does **not** run on the fast path — no tree is built there — or on a
merge that already has conflicts in it.

### 11.1 The false-positive audit

A finding rate of 8.5% is not a result until somebody has read the findings. 40
findings were sampled across repositories and kinds with a seeded hash, the merge
was regenerated for each, and all four files were read.

| verdict | n | share |
|---|---:|---:|
| **False positive — wildcard-import model** | 29 | 72.5% |
| **False positive — benign rebinding after a refactor** | 3 | 7.5% |
| **False positive / unresolved — resolver artefact** | 2 | 5.0% |
| **True positive — the merged program really is broken** | **6** | **15.0%** |

**Audited precision ≈ 15%.** On n = 40 the 95% Wilson interval is roughly
6%–30%, so the honest statement is "somewhere between one finding in three and
one in sixteen is real", and the point estimate is one in seven.

**Recommendation: the check stays in `--semantic=report`, which is the shipped
default.** At this precision `--semantic=conflict` would block roughly six merges
for every one it should.

### 11.2 The false-positive taxonomy

**Wildcard imports (29 of 40).** `sm-bind` models `import java.util.*;` as
binding nothing — deliberately, because it cannot see the package. A very common
Java refactor replaces a list of single-type imports with a wildcard, or the
reverse. The other branch's code still uses the simple name; in the merged file
that name is now supplied only by the wildcard, which our scope tree does not
model, so the reference reads as broken while the program compiles fine.

**This one simplification is measurable and fixable.** Suppressing a
`BrokenReference` whose origin declaration is a single-type import of `pkg.Name`
when the merged program contains `import pkg.*;` would remove **29 of the 29**
wildcard cases in this sample and **none of the six true positives** — not one
confirmed hit is in a file with a covering wildcard. It is the single
highest-value change available to M6 and it is the top follow-up.

**Benign rebinding (3 of 40).** `apache/cassandra`'s
`handleFinalizeProposeMessage` is the shape: our branch refactored
`(InetAddressAndPort from, FinalizePropose propose)` into `(Message<…> message)`
plus a local `InetAddressAndPort from = message.from();`. Their branch's body
still says `from`. The report is *literally* accurate — the name binds to a
different declaration — but the new binding holds the same value, so nothing is
broken. A `CapturedReference` whose new declaration is initialised from the old
one is a shape worth suppressing.

**Resolver artefact (2 of 40).** Two findings could not be reproduced as a real
break by reading the merged file. Counted as false positives rather than
excluded, because an unexplained finding is not evidence of anything.

### 11.3 Real hits, written up

These are cases where we predicted a break, and the strongest validation
available is that **the human's committed resolution had to fix the same thing**.

**1 — `apache/ignite`, merge `2c480b4fedc4`, `GridDhtPartitionTopologyImpl.java`.
The canonical case, in the wild.** Their branch renamed the type
`GridDhtPartitionMap2` back to `GridDhtPartitionMap`, import and all. Our branch
left eleven uses of `GridDhtPartitionMap2` untouched. The two edits are on
different lines, the merge is textually clean, and the merged file imports
`GridDhtPartitionMap` while using `GridDhtPartitionMap2` eleven times — it does
not compile. **The human's committed resolution contains zero occurrences of
`GridDhtPartitionMap2`:** they had to complete the rename by hand. This is
SPEC §1's opening example, found in real history, caught before the file was
written.

**2 — `apache/dubbo`, merge `eb62f48a3f5e`, `ConsulServiceDiscovery.java`.**
Their branch removed `import java.util.concurrent.ConcurrentHashMap;` while
tidying imports; our branch's field initialiser still says
`new ConcurrentHashMap<>()`. The merged file uses the name with no import. **The
human's resolution keeps the import.** Here the syntactic merge is also at fault
— we dropped an import that was still needed — so the semantic check caught our
own bug on the way out.

**3 — `elastic/elasticsearch`, merge `f6458344cebf`,
`MetadataIndexTemplateService.java`. A rename of a parameter, not a type.** Our
branch renamed a method parameter `metadata` to `projectMetadata`; their branch
edited the body. The merged result is

```java
public static Settings resolveSettings(final ProjectMetadata projectMetadata, final String templateName) {
    return resolveSettings(metadata, templateName, Map.of());   // `metadata` no longer exists
}
```

Nothing about this is visible to a line merge, and nothing about it is visible to
a purely syntactic structural merge either: both edits are individually valid and
they do not overlap.

**4 — `apache/cassandra`, merge `6564fe39ac55`, `ShadowRoundTest.java`.** Our
branch removed `import org.apache.cassandra.net.MessagingService;`; their branch
added ten uses of `MessagingService`. Merged file: ten uses, no import. The
human's resolution carries the import.

**5 — `apache/geode`, merge `6a45ca0cc8cd`.** Our branch deleted the helper
`assertNotAuthorized`; their branch added thirteen calls to it. The merged file
calls a method that does not exist. The human resolved it by removing the calls —
a different fix from ours, and a confirmation that something needed fixing.

Five of the six confirmed hits are an import or declaration removal against a new
use on the other branch, which is precisely the "build conflict" family the
literature describes (docs/prior-art.md §9.2). One is a parameter rename inside a
single method, squarely inside a single-file analysis's reach.

### 11.4 Scope

A single-file merge driver sees one file. The canonical rename-in-A / call-in-B
case is frequently **cross-file**, and cross-file is exactly where Bucond and
IntelliMerge operate; those instances are invisible here by construction. What is
measured above is the within-file subset, at corpus scale, which is a measurement
nobody appears to have published.

---

## 12. Comparison with published numbers

**Read the denominators before reading the numbers.** Ours is conditioned on *git
having conflicted on this file*, which is the hardest population there is — the
control arm confirms it: `git merge-file` resolves **0 of 16,109**. Spork's and
jdime's headline figures are over **all file merges in a merge scenario**, the
overwhelming majority of which git merges cleanly and which we would take the
fast path on. The two are not comparable as printed.

| source | population | number |
|---|---|---|
| Spork replication (n = 1,740 file merges, 119 projects) | all file merges | 86.2% success / 11.8% conflict / 2.0% fail |
| jdime, same replication | all file merges | 85.6% / 13.0% / 1.3% |
| Mori & Hashimoto, ASE 2025 (n = 43,774, 76 projects) | ground-truth-free | `d3j` reports zero incorrect |
| Mergiraf | — | no published evaluation exists |
| **`semantic-merge`, this report** | **git conflicted (16,238 cases)** | **52.34% resolve / 47.66% conflict / 0% fail** |
| **`semantic-merge`, this report** | ground-truth-free | 99.99% parsable, 99.99% universal |

Placing us on Spork's denominator needs a number this corpus does not have — how
many *non*-conflicted file merges each merge commit contained — because the miner
extracts the conflicted paths plus a bounded clean sample, not every path. A
rough floor: over the 4,835 clean cases plus 16,238 conflicted ones we produce a
conflict-free result for 4,796 + 8,499 = 13,295 of 21,073, i.e. 63.1%, on a
population deliberately enriched for conflicts by a factor this corpus cannot
measure. Quoting that against 86.2% would be meaningless in both directions.

Two comparisons that *are* like-for-like:

* **Against `git merge-file` on identical inputs**, the control arm in this
  replay: 8,499 conflict-free results to git's 0, at a wrong-merge rate whose
  audited estimate is a few percent of those 8,499, and with a regression rate
  against git of exactly zero.
* **On the ground-truth-free criteria** (Mori & Hashimoto, docs/prior-art.md
  §8.3.3): 99.99% parsable and 99.99% universal on clean results — one case out
  of 8,499 fails each, and it is the same case.

A direct Mergiraf benchmark on this corpus is the obvious missing table and is
future work; it is installable, fast, and has no published numbers of its own.

---

## 13. Limitations

**The human resolution is ground truth for what was *committed*, not for what was
*correct*** (SPEC §6.2). §3.5 and §4 show this is not a formality: a third of the
sampled "incorrect" resolutions are cases where our output applied an edit from a
branch that the human deliberately discarded. It is the best available oracle at
this scale and it is not a good one. The ground-truth-free criteria and the
semantic scan exist partly because of this.

**Single-file scope.** The driver merges one file with no classpath, no build and
no view of the rest of the repository. Cross-file renames are invisible (§11.4),
and so is any conflict whose two halves live in different files.

**Mining bias.** The corpus is repositories that merge locally rather than squash
or rebase, that record `--no-ff` merges, and whose conflicts survive in history.
Squash-merging projects contribute nothing. `git merge-tree` replays each merge
*today*, with today's merge-ort, which is not necessarily what the developer's
git did at the time.

**Per-repository cap.** Eleven repositories hit the miner's 1,500-case cap, so
their rows describe the newest slice of their history rather than all of it, and
the pooled numbers weight them by the cap rather than by their true size. Every
headline is therefore also given as an unweighted per-repository mean (§3.6).

**Concentration.** 35.7% of the incorrect population comes from ten merge
commits, and one repository's copyright-header change accounts for 890 of them
(§3.6). Rates over a corpus like this are not independent samples.

**This machine.** The latency numbers describe a 4-core container running four
replays in parallel (§8).

**Java only.** The evaluation corpus is Java. **TypeScript support is
demonstrated by the test suite, not corpus-evaluated** — the `Language` trait,
the grammar wiring, the scope model and the merge configuration all have
TypeScript coverage in `crates/*/tests`, but no TypeScript repository has been
mined and no TypeScript merge has been replayed. Mining TypeScript history is
future work, and until it is done no claim about TypeScript merge quality at
scale should be read into this document.

**The audits are small.** Thirty incorrect resolutions and forty semantic
findings. Both intervals are stated where they are used and both are wide.

---

## 14. Provenance

| | |
|---|---|
| binary | `target/release/sm` (`sm 0.0.0`) |
| corpus | `corpus/`, schema version 1, 62 repositories with cases |
| subset | all 34,811 cases |
| arms | `sm`, and `--line-merge-only` as the control |
| jobs | 4 |
| semantic mode | `report` |
| invocations | 69,622 |
| wall clock | 690 s |
| replay schema | version 1 (`crates/sm-eval/src/replay.rs`) |

The full replay log, the retained outputs for every incorrect and diverged case
and the 24 sweep runs are not committed — they are 1.6 GB. `docs/evaluation.json`
carries the metrics and the 40-case incorrect-resolution gallery; everything else
is regenerated by the two commands at the top of this file.
