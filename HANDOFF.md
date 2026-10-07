# HANDOFF — 2026-10-07 impl/copilot (~/Projects/rollcall), end of session 15

**Status: ACTIVE.** Supersedes session 14 (byte copy: `.handoffs/20261007-231419-superseded-HANDOFF.md`; v1 author decisions live in `.handoffs/20261006-235942-superseded-HANDOFF.md`).

**Next session: run the Copilot CLI source workflow.** Branch `impl/copilot` exists (from `origin/main` `474cf23` + the session 13–15 docs commits). Plan (DRAFT): `docs/superpowers/plans/2026-10-07-rollcall-copilot-plan.md`. Research: `docs/research/2026-10-07-copilot-cli-sessions.md`. Workflow: `.claude/workflows/rollcall-copilot.js`.

1. **Blocker first:** fill the section `## Author answers 2026-10-07 (Copilot plan)` at the bottom of this file with the author's verbatim answers to plan Q1–Q7 and an explicit R1 approval (`SourceKind::Copilot`, `pub mod copilot;`, `EnvPaths.copilot_home`). The workflow preflight parses that exact heading and **stops immediately** if any Q is blank or R1 is not approved. No code before that; the v1 autonomy grant (2026-10-04) does not cover this work. Commit the answers (explicit path) so the tree is clean: preflight also stops on a dirty tree (outside `.claude/` and `graphify-out/`).
2. Confirm `git branch --show-current` = `impl/copilot` (preflight refuses any other branch).
3. Run workflow `rollcall-copilot` with `args: {stage: "spec"}`: Opus drafts spec rev 17 + `docs/superpowers/plans/tasks/copilot/ACCEPTANCE.md` + the CLAUDE.md Safety line, Opus skeptic (≤ 2 rounds), commits, then **stops for author review**.
4. After the author approves the spec: `args: {stage: "implement", specApproved: true}` (optional `items: ["P1", ...]`). Per phase, serially: tiered implementer tests-first → full gate → commit → Opus `breaker` (if `Has logic: yes`) → guard keeps test code only → tiered fixes → Opus `skeptical-reviewer` (≤ 3 rounds) → next phase; final Opus skeptic over the range. Stops at the first phase not accepted.
5. Main loop afterwards: audit `git log`/`git status`, P5 real-data check only if the author supplied a sample (Q6; counts only, never prompt text), update this handoff, answer AMBIGUOUS breaker items with the author, **ask before pushing** `impl/copilot`.

## Done (verified)
- **Branch `impl/copilot` created** from `origin/main` `474cf23` (`git fetch` confirmed it is current) with `git cherry-pick origin/main..impl/v0` → `818ddef e99869b 3203a8e b8810bb 22bff5d` (the 5 session 13–14 docs commits, clean picks). **Upstream unset** (`switch -c … origin/main` had set it to track `origin/main`, so a bare `git push` would have targeted main); `git rev-parse impl/copilot@{upstream}` → `fatal: no upstream configured`. Push later with `git push -u origin impl/copilot`.
- **Workflow tiers** (author request 2026-10-07, committed with this handoff): `TIER = { P1: haiku/xhigh, P2: sonnet/high, P3: sonnet/high, P4: sonnet/high }` drives the implement agent and every fix (gate, breaker, review). A Haiku phase escalates to Sonnet high from fix round 2. Breaker and skeptic stay Opus; spec drafting stays Opus; mechanical agents (head, gate, commit, guard, answers/criteria loaders) stay Sonnet low. Evidence: wrapped `node --check` → exit 0; stubbed-hook run of `{stage:"implement", items:["P1"]}` reaches `implement:P1` as `haiku/xhigh` (the same run on the first draft failed `Cannot access 'impl' before initialization`: a local `const impl` shadowed the helper, so it is now `implOpts`); tier assertion script (`implOpts('P1')`=haiku/xhigh, `implOpts('P1',2)`=sonnet/high, `implOpts('P2')`/`implOpts('P4',3)`/unknown id = sonnet/high) → `tier ok`.
- Carried: PR https://github.com/ravi1395/rollcall/pull/6 merged (`4f102a7`); gate on `origin/main` `474cf23` → 973 passed / 0 failed / 4 ignored (session 13).

## In progress
- Nothing half-edited. `git status --short` before this session's commit: ` M .claude/workflows/rollcall-copilot.js`, `?? .handoffs/20261007-231419-superseded-HANDOFF.md`, `?? graphify-out/` (never commit `graphify-out/`). `impl/v0` is untouched (local, its 5 docs commits now live on `impl/copilot`).

## Decisions + why
- **Tier by complexity, fixed in the script, not in ACCEPTANCE.md:** P1 is enum arms + schema enums (plan tags them mech) → Haiku xhigh. P2 (events.jsonl parser, incremental reads), P3 (two source lists, env, test sandbox; a miss reads the real `~/.copilot`) and P4 (resolve-time `TextRef`, hand-rolled reader) → Sonnet high. Escalation covers a Haiku phase that stalls.
- **Edited `rollcall-copilot.js`, no new workflow:** it already had the Opus breaker/skeptic loop, preflight and serial phases; only the implementer model changed.
- **Optional carry-overs left out of the workflow:** all are non-blocking skeptic observations with no criteria; listed under Not started.
- Earlier (session 14): Copilot CLI only; reuse `Backing::LogFile`/`TextRef::LogLine`/`SourceMode::Jsonl`; phase cut keeps each merge green.

## Test state
- No code changed since `origin/main` `474cf23` → 973 passed / 0 failed / 4 ignored (ignored by design: `materialize_perf`, `materialize_demo`, `fs.rs::modes_child`, `fs.rs::lock_child`). Gate not re-run this session (docs/workflow only).
- CI: billing-blocked (every job 0 steps). Linux untested.

## Gotchas
- **Haiku `xhigh` effort is new for this repo's workflows:** if the runtime rejects it for Haiku, change `HAIKU` to `effort: 'high'` (one line).
- No real Copilot session on this machine; format criteria are *(assumption)* until Q6. Never print prompt text from `~/.copilot`; never read `~/.config/github-copilot/apps.json` (tokens).
- `"copilot"` is 7 chars (one more than `"claude"`): P1 checks TUI/plain column budgets.
- Two source lists must stay in sync: `scan.rs` `scan_sources` and the `output.rs` resolver list.
- Test sandbox `crates/rollcall-cli/tests/it/support.rs` must set `COPILOT_HOME` in P3.
- Local `main` is stale (`f7ada95`); always use `origin/main`.
- `ubuntu-latest` → Ubuntu 26 from 2026-10-19; consider pinning `ubuntu-24.04` (author decision, not asked).
- Carried: CI annotations via `gh api repos/ravi1395/rollcall/check-runs/<job id>/annotations`; scratch `HOME` for manual runs (macOS cache ignores `XDG_CACHE_HOME`); `git config handoff.auto false`; stage explicit paths.

## Suggested skills
- `brief`, then the author Q&A; `gate-record-audit` before approving spec rev 17; `verify-ledger` for P5; `ci-divergence-triage` once CI runs.

## Not started
- Copilot plan P0–P5 (all of it).
- Author actions: fix CI billing and re-run CI on `main`; create `ravi1395/homebrew-rollcall`, add `HOMEBREW_TAP_TOKEN`, tag a release.
- Optional carry-overs (non-blocking): `lock_when_free` fork-race window; full-F MCP reply has 0 `recent_sessions`; `scripts/probe-gix-p15b/sample.py` prints `first>320MiB`; non-UTF-8 missing-worktree lock key collides with its twin; macOS cache ignores `XDG_CACHE_HOME`.

## Author answers 2026-10-07 (Copilot plan)
<!-- Filled in by the next session with the author's verbatim answers. The rollcall-copilot workflow preflight parses this section. -->
- Q1 (title):
- Q2 (liveness):
- Q3 (legacy stores):
- Q4 (sub-agent / autopilot):
- Q5 (non-UUID ids):
- Q6 (real sample):
- Q7 (schema_version):
- R1 approval (SourceKind::Copilot, pub mod copilot, EnvPaths.copilot_home):
