# HANDOFF — 2026-10-07 impl/v0 → impl/copilot (~/Projects/rollcall), end of session 14

**Status: ACTIVE.** Supersedes session 13 (byte copy: `.handoffs/20261007-001525-superseded-HANDOFF.md`; v1 author decisions live in `.handoffs/20261006-235942-superseded-HANDOFF.md`).

**Next session: Copilot CLI session source.** Plan (DRAFT, awaiting author approval): `docs/superpowers/plans/2026-10-07-rollcall-copilot-plan.md`. Research: `docs/research/2026-10-07-copilot-cli-sessions.md`.
1. Get the author's answers to the plan's open questions Q1–Q7 and approval of the R1 items (`SourceKind::Copilot`, `pub mod copilot;`, `EnvPaths.copilot_home`). Record them verbatim in the section `## Author answers 2026-10-07 (Copilot plan)` at the bottom of this file: the workflow's preflight reads that exact heading and stops if any Q is unanswered or R1 is not approved. **No code before that.** The v1 autonomy grant (2026-10-04) does not cover this work.
2. Create the branch: `git switch -c impl/copilot origin/main`, then `git cherry-pick 9b3f8c5 d98ec05` plus this handoff's own commit (on `impl/v0`). In session 14, `switch -c` + `cherry-pick` was blocked by the auto-mode classifier, so the author or the next session must run them with approval. The workflow's preflight refuses any other branch.
3. Run workflow **`rollcall-copilot`** (`.claude/workflows/rollcall-copilot.js`; local, `.claude/` is not tracked) with `args: {stage: "spec"}`: Opus drafts spec rev 17 + `docs/superpowers/plans/tasks/copilot/ACCEPTANCE.md` + the CLAUDE.md Safety line, an Opus skeptic reviews (≤ 2 rounds), it commits, then **stop for author review**.
4. After the author approves the spec: run `rollcall-copilot` with `args: {stage: "implement", specApproved: true}` (optional `items: ["P1", ...]`). Per phase, serially: Sonnet implements tests-first → gate → commit → Opus `breaker` (if has_logic) → guard keeps test code only → Sonnet fixes → Opus `skeptical-reviewer` (≤ 3 rounds) → next phase; final Opus skeptic over the range. Models are the author's choice (2026-10-07): Sonnet implementers, Opus breaker and skeptic (overrides the global Fable-skeptic default for this run).
5. Then, in the main loop: audit `git log`/`git status`, P5 real-data check only if the author supplied a sample (Q6; counts only, never prompt text), update this handoff, and **ask before pushing** `impl/copilot`.

## Done (verified)
- **PR https://github.com/ravi1395/rollcall/pull/6 merged** by the author (merge `4f102a7`); Dependabot #2–#5 merged after it; `origin/main` = `474cf23`.
- **Gate on `origin/main` `474cf23`** (session 13, macOS, detached worktree): fmt + clippy + test → exit 0, **973 passed / 0 failed / 4 ignored**.
- **Copilot CLI research** (session 14, public sources only; nothing installed; `~/.copilot` contents not read). Store `$COPILOT_HOME|~/.copilot/session-state/<id>/events.jsonl`; cwd/branch in `session.start/resume.data.context` and `session.context_changed.data`; title and idle state are ephemeral (not on disk); `workspace.yaml` holds `name`. Confidence and unknowns per section in the research doc.
- **Plan written** (phases skill format: P0 spec, P1 enum + arms inert, P2 parser dark, P3 wiring, P4 gated title/liveness, P5 gated real-data check).
- Commit state: session 13 handoff `9b3f8c5` on `impl/v0` (local, unpushed). Session 14 files (this handoff, plan, research, archive) are committed on `impl/v0` (local, unpushed) in the commit after `9b3f8c5`.

## In progress
- Nothing half-edited. `git status --short` before this session's commit: `?? .handoffs/20261007-001525-superseded-HANDOFF.md`, `?? docs/research/`, `?? docs/superpowers/plans/2026-10-07-rollcall-copilot-plan.md`, `?? graphify-out/` (never commit `graphify-out/`).

## Decisions + why
- **Copilot CLI only** (author, 2026-10-07). Not VS Code Copilot Chat (18 local `chatSessions` files exist but were not chosen), not the cloud agent.
- **Plan reuses `Backing::LogFile` / `TextRef::LogLine` / `SourceMode::Jsonl`.** `events.jsonl` is one append-only JSONL per session, the same shape as Claude logs, so no new core types unless Q1=(b).
- **Phase cut keeps each merge green:** adding the enum variant forces every exhaustive match at once (P1, arms tagged mech); the source stays unregistered until P3, so user output is unchanged through P2.

## Test state
- Last gate: `origin/main` `474cf23` → 973 passed / 0 failed / 4 ignored (ignored by design: `materialize_perf`, `materialize_demo`, `fs.rs::modes_child`, `fs.rs::lock_child`). No code changed since.
- CI: still billing-blocked (every job 0 steps; "recent account payments have failed"). Linux untested.

## Gotchas
- **No real Copilot session on this machine** (`~/.copilot` = `config.json ide logs mcp-config.json`). All format criteria are *(assumption)* until Q6 gets a sample. Never print prompt text from `~/.copilot` (plaintext prompts, tool args and output; research §6). `~/.config/github-copilot/apps.json` likely holds tokens: never read it.
- **`"copilot"` is 7 chars**, one more than `"claude"`: P1 checks TUI/plain column budgets.
- **Two source lists** must stay in sync: `scan.rs` `scan_sources` and `output.rs` resolver list.
- **Test sandbox** (`tests/it/support.rs:591-630`) must set `COPILOT_HOME` in P3, or a test could read the real store.
- `ubuntu-latest` → Ubuntu 26 from 2026-10-19 (runner annotation); consider pinning `ubuntu-24.04` (author decision, not asked).
- Carried: CI annotations via `gh api repos/ravi1395/rollcall/check-runs/<job id>/annotations`; scratch `HOME` for manual runs; `git config handoff.auto false`; stage explicit paths; Python `re.sub` replacement turns `\n` into newlines.

## Suggested skills
- `brief`, then the author Q&A; `/executor-grade-plan-audit` before `/pchunker`; `/pexec`; `gate-record-audit` before the skeptic; `verify-ledger` for P5; `ci-divergence-triage` once CI runs.

## Not started
- Everything in the Copilot plan (P0–P5).
- Re-run CI on `main` after the author fixes billing.
- Release: create `ravi1395/homebrew-rollcall`, add `HOMEBREW_TAP_TOKEN`, tag (author actions).
- Optional carry-overs: `lock_when_free` fork-race window; full-F MCP reply has 0 `recent_sessions`; `scripts/probe-gix-p15b/sample.py` prints `first>320MiB`; non-UTF-8 missing-worktree lock key collides with its twin; macOS cache ignores `XDG_CACHE_HOME`.

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
