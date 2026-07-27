# PROGRESS

## Current milestone: M0 — Scaffold + CST layer (in progress)

## Session log

### Session 1 (2026-07-27)

**Status:** M0 in progress.

**Environment notes:**
- Repository is `danfirsten/scion`; the workspace lives at the repo root (not in a
  `semantic-merge/` subdirectory as the spec's layout sketch shows — the repo root *is*
  the project root).
- Development branch: `claude/semantic-merge-spec-8d684v`.
- Toolchain: Rust 1.94.1 stable.

**Done:**
- SPEC.md committed at repo root.
- (in progress) Cargo workspace, all crate boundaries, CI, `sm-cst` with arena, trivia
  attachment, `Language` trait, `sm parse`.

**Decisions (and rationale):**
- (running list — see below)

**Known limitations:**
- (running list — see below)

## Decisions

| # | Decision | Rationale |
|---|---|---|
| 1 | Workspace at repo root of `scion`, crates under `crates/` | The repo root is the project root; spec's `semantic-merge/` wrapper directory would just add a level of nesting. |

## Known limitations

- (none recorded yet)

## Next

- Finish M0, verify exit criteria (`sm parse` output + trivia test suite), commit, push.
- Await user confirmation before starting M1 (corpus miner).
