# HANDOFF — 2026-09-30 impl/v0 (~/Projects/rollcall), end of session 2

Goal (author's `/goal`): the run is done when the final **Opus** skeptic gate reports every spec §12.1 criterion 1–18 MET. Spec rev 12 is the contract: `docs/superpowers/specs/2026-09-29-rollcall-design.md`. Plan rev 6: `docs/superpowers/plans/2026-09-29-rollcall-v0-plan.md`. Run mechanics: `docs/superpowers/plans/tasks/v0/ORIENTATION.md`. Worker rules: `WORKER-RULES.md`; risks: `RISKS.md`; counts: `COUNTS.md` (same dir). Previous handoffs are in `.handoffs/`.

## Model roles (author, restated 2026-09-30; same as ORIENTATION "Run mechanics")
- **Implementers: Sonnet.** `Agent` with `subagent_type: "plan-surgeon-surgeon"` and an explicit `model: "sonnet"`. Parallel lanes also get `isolation: "worktree"`. The first receipt must state its model id.
- **Breaker: Opus.** `breaker` agent with an explicit `model: "opus"`. Its frontmatter defaults to Fable, so the override is required.
- **Gate: Opus.** `skeptical-reviewer` agent with an explicit `model: "opus"`. Its frontmatter defaults to Fable, so the override is required. The rev 12 design gate this session ran on Fable, which is fine for a design review, but the final §12.1 gate must be Opus.
- The orchestrator (Opus, main session) owns dispatch, integration, the full gate, commits and pushes. The author asked implementers to follow `/ponytail:ponytail` (shortest correct code); put that in every brief.

## Done (verified, committed, pushed; origin/impl/v0 = `146e717`)
- P08 merged (`eb419fc`). `python3 scripts/check-manifest.py git` → `ok: 74 repos, all bullets covered`; `sessions` → `ok`.
- Full gate at `eb419fc`: fmt clean, clippy clean, tests 402 passed / 0 failed / 2 ignored.
- P15a merged (`46306d9`). Spec rev 12 and plan rev 6 are in `827cbb2`, `7810b56`, `8945aac` and `2882fd9`. The independent skeptic (Fable) failed round 1 and passed round 2 on all six design criteria. The reasoning is in `docs/superpowers/specs/2026-09-29-rollcall-redteam.md` (the last two sections).
- CI on `146e717` passed (run 36778273565): deny, test (macos-14), test (ubuntu-latest), clippy, audit and fmt all succeeded.

## In progress
- **P20** in worktree `.claude/worktrees/agent-a1908ee2d13fad1d7`, branch `worktree-agent-a1908ee2d13fad1d7`, commit `ba59068`. It is WIP: `state.rs` has 5 `unimplemented!`, has never been run, and predates rev 12. Next step, after P18b merges: `git -C <wt> merge --ff-only impl/v0` (restore that worktree's HANDOFF.md first if it is dirty). Then re-dispatch a Sonnet implementer on that branch with plan P20, including the rev 6 items: status call shape, `CompareBlobs` cap, the §3.3 untracked rule, pre-status checks, and the rev 12 acceptance bullet.

## Next session: order of work (plan rev 6 lane table)
1. **P18b** (`open.rs`, `trust.rs`). It unblocks P20. Can run in parallel with: **P07b** (MANIFEST + checker bullets; checker red first), **P15b** → **P15c** (audit docs; P15c has the rev 6 confirmation items), and **P09** (oracles).
2. **P20** (finish, see above) → **P21**. **P10** after P09.
3. **P27a → P27b → P28** after P07b. P27b writes index files byte by byte with a `python3 hashlib` SHA-1 trailer, since R1 forbids adding a SHA-1 crate.
4. Lane D **P29–P39** (serial, one crate) after B2 and C1–C4 are merged. Then lane E **P40–P42** (P41 is orchestrator-owned; #16–#18 need CI runs on origin).
5. At the end: the **Opus breaker** on the integrated change, with §12.1 as its spec. Then the **Opus skeptic**, which gets the criteria, the diff, the breaker report and the evidence. Loop fixes until every §12.1 criterion is MET. Run `gate-record-audit` before each gate round.
- After each merge: run the full gate (`touch crates/*/src/*.rs` first), add a COUNTS row, then push. The author authorized this session's push only; confirm the push policy with the author (plan R-setup says push after each lane merge, Q1).

## Decisions + why (details in the red-team log and RISKS.md)
- Rev 12 uses in-process guards (author's choice), not subprocess isolation. The per-finding options are listed in the red-team section "P15a gix read-site audit → spec revision 12".
- The index validator lives inside `open.rs`, because R1 forbids new modules. gix-index keeps entry decoding private, so rollcall walks v2/v3/v4 entries itself.
- Overrides at open: `core.bigFileThreshold=1m` and `index.threads=1`. Both close attacker-controlled bypasses; the evidence is in the red-team log.

## Test state
`cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked` → 402 / 0 / 2 at `eb419fc`. The later commits change docs and add out-of-workspace probes only, and CI passed on `146e717`.

## Gotchas
- A session hook rewrites HANDOFF.md with an unverified summary after turns. Stage explicit paths only; hook versions are kept in `.handoffs/*-hook*-HANDOFF.md`.
- `cd` in a Bash call moves the session cwd; use absolute paths and `git -C`.
- Worktrees fork from a stale base. Fast-forward each one to impl/v0 before a worker starts; each worktree gets its own `CARGO_TARGET_DIR`.
- `block-dangerous-git.sh` blocks `reset --hard`, `branch -D` and `restore .`. Use `git worktree remove --force` followed by `branch -d`.
- `scripts/probe-index-bomb/` is a separate crate with a `[workspace]` table. It uses `unsafe getrusage` and is never part of the workspace gates. Build it with a scratch `CARGO_TARGET_DIR`.
- Open rev 12 risks (the last `RISKS.md` bullets):
  - The `UNTR` decoder was never run end to end.
  - The 4 MiB `UNTR` cap hasn't been measured against a real untracked-cache monorepo.
  - The scan-level memory bound holds for `H` only by enumeration.
  - The 16 MiB stack is margin only; no test proves it.

## Suggested skills
`/handoff ingest`, `brief` (vault context), `plan-run-artifacts` (COUNTS rows), `probe-hygiene` (before mutation probes), `gate-record-audit` (before every gate), `ponytail:ponytail` (in implementer briefs).

## Tree at handoff
`git status --short` before this file was written: ` M HANDOFF.md` (the hook's rewrites, archived as `.handoffs/20260930-222149-hook-HANDOFF.md` and `-hook2-`).
