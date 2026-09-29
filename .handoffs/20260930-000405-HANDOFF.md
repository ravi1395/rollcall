# HANDOFF — 2026-09-30 main (~/Projects/rollcall)

**Next session's job:** execute the v0 plan. Opus 5.5 orchestrates, Sonnet 5.5 implements, Opus 5.5 gates (§ Model roles). Start with `/executor-grade-plan-audit` on the plan, then `/pchunker`, then lane A (P01). No code exists yet.

## Model roles (author's instruction for this run)

| Role | Model | How to bind it |
|---|---|---|
| Orchestrator | Opus 5.5 | the main session: plans waves, integrates lanes, runs full-workspace gates, commits, pushes |
| Implementers | Sonnet 5.5 | `Agent` with `subagent_type: "plan-surgeon-surgeon"` (isolated worktree) and **explicit** `model: "sonnet"` on every call |
| Breaker | Opus 5.5 | `subagent_type: "breaker"`, **explicit** `model: "opus"` |
| Final gate (skeptic) | Opus 5.5 | `subagent_type: "skeptical-reviewer"`, **explicit** `model: "opus"` |

Overrides this requires (don't let defaults win):
- **`/pexec` (`~/.claude/commands/pexec.md`) hard-codes `model: fable`** for the breaker (end-of-plan step 1) and the skeptic (final gate step 1). Pass `model: "opus"` instead. The same agents' frontmatter also defaults to `fable`.
- **`/pexec` dispatches `psurgeon` into the shared tree.** Plan rule R2 requires one isolated worktree per lane and per parallel sub-chain. Either dispatch `plan-surgeon-surgeon` (or `psurgeon` with `isolation: "worktree"`), or drive the lanes by hand with pexec's per-wave loop; the orchestrator merges each lane and runs the full gate itself.
- `psurgeon` / `plan-surgeon-surgeon` frontmatter already says `model: sonnet`. Pass `model: "sonnet"` anyway so a frontmatter change can't silently switch the model.
- **Unverified:** that the `sonnet` / `opus` aliases resolve to Sonnet 5.5 / Opus 5.5 (the Agent tool's `model` enum has only aliases). First dispatch: have the agent state its model id in its receipt and check it before trusting the lane.
- This overrides the global CLAUDE.md default (skeptic = Fable 5.1) for this run only.

## Done (verified)

- **Plan revision 5** — `docs/superpowers/plans/2026-09-29-rollcall-v0-plan.md`, committed `d8e2d51`: 45 phases in 9 lanes (`grep -c '^## Phase'` → 45), pinned interfaces, fixture manifest schema, ponytail rules (R4/R5), Q1–Q3 answered in § Open questions.
- **Spec revision 10** — `docs/superpowers/specs/2026-09-29-rollcall-design.md`, committed `d8e2d51`. Changelog for revisions 6–10 at the end of `docs/superpowers/specs/2026-09-29-rollcall-redteam.md`.
- **Plan red-team, 3 rounds (10 Opus 5.5 reviewers, all completed)** — `docs/superpowers/plans/2026-09-29-rollcall-v0-plan-redteam.md`. Final round: both lenses STANDS WITH FIXES; fixes applied in plan rev 4 / spec rev 9.
- **Private GitHub repo** `https://github.com/ravi1395/rollcall` — `gh repo view` → `visibility: PRIVATE`; `main` tracks `origin/main` (`git status -sb` → `## main...origin/main` at `2990e26`, before this handoff's commit).
- **Local tools (Homebrew):** hyperfine 1.20.0, fish 4.9.3, cargo-deny 0.20.2, cargo-audit 0.22.2, cargo-dist 0.33.0 (`--version` output checked).
- **Fact probe** `scripts/probe-plan-facts.sh`, committed `eeb8bc9`.

## In progress

- Nothing half-edited. `git status --short` before writing this file:
  ```
   M HANDOFF.md
  ?? .handoffs/20260930-000028-HANDOFF.md
  ?? .handoffs/20260930-000028-hook-HANDOFF.md
  ```
  `HANDOFF.md` showed as modified because the session hook rewrites it. The hook's copies and the previous committed handoff are archived in `.handoffs/` (`20260930-*`) and committed together with this file.
- **Plan rev 4 / rev 5 edits have had no independent review.** They are local. `/executor-grade-plan-audit` is the next check.

## Decisions + why

- **Author answers (2026-09-29):**
  - Q1: private GitHub remote; push `impl/v0` after each lane merge.
  - Q2: tools installed locally.
  - Q3: drop `directories`; use a SHA-1 lock-file name; drop `criterion`; keep `arboard`.
  - claimd stays off.
- **Per-lane isolated worktrees** — parallel workers in one tree break each other's compile (found in every red-team round).
- **Ponytail governs how, never what** — plan R4.
- **Spec defects fixed while planning** (details in the spec red-team doc):
  - `macos-13` runner retired.
  - Resume file truncated before its checks; now also opened `O_NONBLOCK` so a FIFO can't hang it.
  - The `gix-transport` crate ban was unsatisfiable (now a feature ban).
  - Uncapped `config.worktree` read.
  - HEAD-tree memory bound, needed twice: entries in rev 8, path bytes and depth in rev 9.
- The spec says "Awaiting author review": revisions 6–10 were made without the author reviewing them. The author did answer Q1–Q3.

## Test state

No code and no tests. Acceptance criteria are spec §12.1 (18 gating) and §12.2 (author smoke); the plan's coverage table maps each to phases.

## Gotchas

- **A session hook rewrites `HANDOFF.md` after tool calls** with "(inferred, unverified)" content. Recover this file with `git show HEAD:HANDOFF.md`. When editing it, write and commit in one shell command, or the hook wins the race.
- **claimd stop hook** warns on every turn end for docs-only commits (no `.claims/` here). The author chose to leave it.
- **Opus reviewers were reliable at 25–40 tool-call budgets** (10/10 completed). Give Sonnet implementers the same tool-call budget, scoped to their phase's files.
- **Edition 2024 `std::env::set_var` is `unsafe`** and the workspace forbids `unsafe`: inject the environment via `EnvPaths` or `Command::env` (plan R3).
- **Never print prompt text from real logs** (they contain secrets).
- **The lane dependency "after P11"** means after the P11 commit is merged, not after lane B2 finishes (plan R2).

## Suggested skills

- `/executor-grade-plan-audit` on plan rev 5 first.
- `/pchunker` next. Check that its task files keep the plan's R1 frozen-file rule and the lane table.
- `plan-run-artifacts` at run start: ORIENTATION.md, then a COUNTS.md row per lane merge.
- `/pexec` only with the § Model roles overrides, or drive the lanes manually.
- `gate-record-audit` before each Opus gate round; `probe-hygiene` before any mutation probe.

## Not started

- P01 onward: `git worktree add ../rollcall-v0 -b impl/v0`, then lane A. Push `impl/v0` after each lane merge.
- First GitHub Actions run (P02 writes the workflow).
