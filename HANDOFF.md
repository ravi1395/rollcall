# HANDOFF — 2026-09-29 main (~/Projects/rollcall)

**Next session's job:** run `/executor-grade-plan-audit` on the plan, then `/pchunker`, then start lane A (P01). Q1–Q3 are answered. No code exists yet.

## Done (verified)

- **Implementation plan, revision 5** — `docs/superpowers/plans/2026-09-29-rollcall-v0-plan.md`, committed `d8e2d51`. Revision 5 only records the Q1–Q3 answers. It has 45 phases in 9 lanes (`grep -c '^## Phase'` → 45), a §12.1 coverage table, pinned interfaces, a fixture manifest schema, and the ponytail rules (R4/R5).
- **Spec, revision 10** — `docs/superpowers/specs/2026-09-29-rollcall-design.md`, committed `d8e2d51`. Revisions 6–10 were made this session; 10 records the Q3 trims; the changelog is at the end of `docs/superpowers/specs/2026-09-29-rollcall-redteam.md`.
- **Plan red-team, 3 rounds of Opus 5.5 `skeptical-reviewer` agents (5, then 3, then 2)** — record in `docs/superpowers/plans/2026-09-29-rollcall-v0-plan-redteam.md`, committed `b398e26`.
  - Round 1: two lenses DOES NOT STAND.
  - Round 2: two lenses DOES NOT STAND, one of them a security regression introduced by a rev-7 fix.
  - Round 3: both STANDS WITH FIXES. The fixes were applied as plan rev 4 / spec rev 9.
  - Evidence: each round's verdict table and orchestrator re-verification table are in that doc.
- **Fact probe** — `scripts/probe-plan-facts.sh`, committed `eeb8bc9`. It re-measures the toolchain, crate versions, gix transitive dependencies and the git remote count.
- **Author answers (2026-09-29):** Q1 private GitHub repo; Q2 install tools locally; Q3 (a) drop `directories`, (b) SHA-1 lock name, (d) drop `criterion`, keep `arboard`; claimd stays off. Recorded in plan § Open questions.
- **Private GitHub repo** `https://github.com/ravi1395/rollcall` created; `main` pushed and tracking `origin/main` (`gh repo view` → `visibility: PRIVATE`). Pushed commits `8129b22..d8e2d51`; later commits in this file's history are local until the next push.
- **Local tools installed via Homebrew:** hyperfine 1.20.0, fish 4.9.3, cargo-deny 0.20.2, cargo-audit 0.22.2, cargo-dist 0.33.0 (`--version` output checked).
- **Consumed handoffs archived** under `.handoffs/`, bytes checked with `cmp`: the session-start handoff (`20260929-224036-HANDOFF.md`) and three hook-written ones.

## In progress

- Nothing half-edited. `git status --short` just before writing this file:
  ```
  ?? .handoffs/20260929-232901-hook-HANDOFF.md
  ?? .handoffs/20260929-235303-hook-HANDOFF.md
  ?? HANDOFF.md
  ```
  These three files (two archived hook handoffs and this file) are untracked; commit them or leave them.
- **Plan rev 4's own edits have had no independent review.** They are local: field additions, scoping sentences, counts. `/executor-grade-plan-audit` is the next check.

## Decisions + why

- **Treated the user's request as spec approval.** The user asked for the plan to be drafted. Spec status still says "Awaiting author review".
- **Plan deviates from the `/phases` house format: 45 phases, not 3–5.** Reason: 18 gating criteria and a cap of 2 substantive files per worker.
- **Per-lane isolated worktrees, not a shared tree.** Parallel workers in one tree break each other's compile. Every red-team round confirmed this.
- **Ponytail governs how the plan builds things, never what it builds.** The ponytail plugin is installed (`ponytail:ponytail` 4.10.0). R4 in the plan states where it applies and where it doesn't.
- **Four spec defects found and fixed while planning:**
  - `macos-13` runner retired → `macos-15-intel`.
  - `ROLLCALL_RESUME_FILE` was opened with `O_TRUNC` before its checks → now opened without it and with `O_NONBLOCK`.
  - The `gix-transport` crate ban can't be met (the crate is non-optional) → ban its client features instead.
  - `config.worktree` is read with no size cap → added to the pre-open checks.
- **HEAD tree bound (spec §4.4, rev 9):** at most 1,000,000 entries, 128 MiB of path bytes, and depth 4,096. The caps were chosen, not measured on real repos; §12.2 author smoke is the first real-data check.
- **Claimd not set up here.** Its stop hook fires on every docs-only commit because this repo has no `.claims/config.json`. Setting it up is the user's call.

## Test state

No code and no tests yet. Acceptance: spec §12.1 (18 gating criteria) and §12.2 (author smoke), mapped to phases in the plan's coverage table.

## Gotchas

- **A session hook rewrites `HANDOFF.md`** with "(inferred, unverified)" content, several times per session. If this file looks different, recover it with `git show HEAD:HANDOFF.md` once it is committed, or from `.handoffs/`.
- **claimd stop hook** repeats coverage warnings for `docs/**` on every turn end in this repo. It is harmless and can't be fixed without `claimd init`.
- **Opus reviewers were reliable at 25–40 tool-call budgets:** 10 of 10 finished. Earlier sessions had Fable reviewers stall.
- **Reviewer scratch crates** are in the session scratchpad and are gone next session. Every load-bearing measurement is written up in the red-team docs.
- **Never print prompt text from real logs.**
- **Edition 2024 makes `std::env::set_var` unsafe**, and the workspace forbids `unsafe`. Tests must inject environment through `EnvPaths` or `Command::env` (plan R3).

## Suggested skills

- `/executor-grade-plan-audit` on the plan, before chunking.
- `/pchunker` → `/pexec`. The plan's R2 needs isolated worktrees per lane, so use `plan-surgeon-surgeon` or `isolation: "worktree"`, not shared-tree `psurgeon`.
- `plan-run-artifacts` at run start; `gate-record-audit` before each skeptic round; breaker plus skeptical-reviewer at the end (global CLAUDE.md).

## Not started

- Everything from P01 on: `git worktree add ../rollcall-v0 -b impl/v0`, then lane A. Push `impl/v0` after each lane merge (Q1).
- The first CI run: nothing has run on GitHub Actions yet; P02 writes the workflow.
