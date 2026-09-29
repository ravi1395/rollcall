# HANDOFF — 2026-09-29 main (~/Projects/rollcall)

**Next session's job:** write the implementation plan for rollcall v0 from the design spec. No code exists yet.

## Done (verified)

- Design spec, revision 5 — `docs/superpowers/specs/2026-09-29-rollcall-design.md` — committed `e071b71`. Evidence: `grep -nE 'TBD|TODO|PENDING'` on the spec → no matches.
- Red-team history, rounds 1–4 — `docs/superpowers/specs/2026-09-29-rollcall-redteam.md` — committed `e071b71`. Final verdicts:
  - Security: Fable, stands with fixes; fixes applied verbatim in `e071b71`, **not re-verified**.
  - Testability: orchestrator self-check (two Fable attempts stalled), stands after `5e2b428`.
  - Feasibility + product: stands with fixes at round 2; fixes landed in revision 3 (`6efcfd8`), not re-run.
- Stale hook-written handoff archived → `.handoffs/20260929-223745-HANDOFF.md` (bytes verified with `cmp`).

## In progress

- **Spec approval.** The user reviewed an ELI5 summary and asked to hand the implementation plan to the next session. They did not write an explicit "spec approved".
  - Exact next step: confirm approval with the user in one line (or treat the handoff request as approval, per their global CLAUDE.md "proceed on reversible work"), then invoke `superpowers:writing-plans` on the spec.
- Repo state at handoff (`git status --short` before this file was written):
  ```
  ?? HANDOFF.md
  ```
  That untracked file was the stale hook-generated handoff, now archived. This `HANDOFF.md` and `.handoffs/` are committed together with this note.

## Decisions + why

These are recorded in the spec's §1 "Decided" table; not repeated there are these:

- **Name `rollcall`** (was placeholder `ww`). Free on crates.io, Homebrew, npm, PyPI as of 2026-09-29.
- **Full scope in v0** (MCP, Homebrew, vhs GIF, perf gate), user's explicit choice over a slimmer cut. Estimate is 5–7 weeks. Do not re-propose cutting scope.
- **Agent-first pitch**, user's choice. Generic discovery and no-agent mode stay as features.
- **5-crate workspace** kept, with the dependency direction and interfaces in spec §3.4.
- Chosen on reviewer advice and surfaced to the user, no objection raised:
  - MCP returns only genuine titles by default; prompts need `--prompts`.
  - `R` execs the agent directly; `rc` is opt-in.
- **Codex primary source** is `~/.codex/state_<N>.sqlite` `threads` (read-only), with rollout JSONL as fallback. Reason: measured to carry title, branch, and cwd.
- **No subprocesses during scan; hardened gix open** (spec §4.4). Verified against the gitoxide checkout at `~/Projects/gitoxide` @ `5708de4` (gix 0.86).

## Test state

No code, no tests. The plan must produce them. Acceptance criteria: spec §12.1 (18 gating, CI-only, fixture-based) and §12.2 (author smoke, non-gating).

## Gotchas

- **A session hook auto-writes `HANDOFF.md`** in this repo, with "(inferred, unverified)" content. It may overwrite this file. If this file looks different from the above, recover it with `git show HEAD:HANDOFF.md`.
- **Fable subagents stalled** (10-minute watchdog, no output) 3 times late in the session. Give reviewers a tool-call budget and focused scope; retry once, then self-check and say so.
- **Pinned facts rest on real data from this machine.** Log shapes, counts, and gix line refs are recorded in the red-team doc. Re-verify against `gix >= 0.88` when pinning. The spec's §4.4 maintenance rule requires re-auditing gix uncapped read sites on any version bump.
- **Claude log dirs start with `-`.** Use `./` prefixes or `find` when scripting over `~/.claude/projects/*`.
- **Never print prompt text from real logs** in outputs. They contain secrets (counts are in the red-team doc).
- The parent `~/Projects/CLAUDE.md` describes `sutra` as the active project. It is not relevant here, and this repo has no CLAUDE.md yet.

## Suggested skills

- `superpowers:writing-plans`: brainstorming's required next step once the spec is approved.
- `/phases`: phase the plan with acceptance criteria. Map every §12.1 criterion to a phase, and order the fixture manifest plus hand-written oracles (§10.1) **before** the code they test.
- `/executor-grade-plan-audit`: run before chunking.
- `/pchunker` then `/pexec`: parallel waves.
  - Shared interfaces (spec §3.4 types and traits in `rollcall-core`) must land first and serially.
  - Crates depend only on core, so `rollcall-git`, `rollcall-sessions`, and `rollcall-store` can then run in parallel.
- `plan-run-artifacts`: run ledgers. `gate-record-audit`: before each skeptic round.
- Breaker (fable) + skeptical-reviewer (fable) at the end, per global CLAUDE.md.

## Not started

- Implementation plan (spec → phases → task files).
- Repo scaffolding: Cargo workspace, CI, `schema/`, `tests/fixtures/MANIFEST.toml`, repo CLAUDE.md.
- Everything in spec §12.1.
- No GitHub remote exists. Nothing has been pushed; creating or pushing a repo needs user authorization.
