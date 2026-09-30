# HANDOFF — 2026-10-01 impl/v0 (~/Projects/rollcall), end of session 3 (usage limit)

Goal (author's `/goal`): done when the final **Opus** skeptic gate reports spec §12.1 #1–#18 MET. Spec **rev 13**, plan **rev 7**. Run mechanics: `docs/superpowers/plans/tasks/v0/{ORIENTATION,WORKER-RULES,RISKS,COUNTS}.md`. Model roles unchanged (Sonnet `plan-surgeon-surgeon` implementers with `model: "sonnet"`; Opus breaker; Opus final skeptic). Push policy: plan Q1 (push after each lane merge) — followed all session.

## Done (verified, committed, pushed; origin/impl/v0 = `ac470a0`)
- Merged this session: P07b, P07c, P09, P10, P15b, P15c, P18b, P18c, P20, P21, P27a, P27b, P27c, P28, P29, P30, P31, P36, P37, P38. Last full gate at `ac470a0`: fmt/clippy clean, **641 passed / 0 failed / 4 ignored** (gate14.log).
- Spec rev 13 (P15b U-10..U-15; author chose a killable per-repo worker process for U-13): design gate (Fable) PASS on round 3; rounds recorded in `docs/superpowers/specs/2026-09-29-rollcall-redteam.md`. Probes: `scripts/probe-gix-p15b/`, `scripts/probe-gix-p15c/`.
- gix audit: UNCOVERED 0 (P14, P15a, P15b, P15c).

## In progress (agents launched at ~00:10, NOT integrated; check their worktrees under `.claude/worktrees/`)
- **P32** — FAILED, parked on branch `worktree-agent-a23c39f96a255487d` (WIP commit). 8/13 pass; 5 fail on ONE product bug: `crates/rollcall-git/src/classify.rs` `worktree_dir` reads only the first line of `worktrees/<n>/gitdir`, so the fixture worktree `$F/shell-wt\nnewline` becomes `$F` → `untrusted worktree path`. Fix: read the whole file, strip one trailing `\n`, take `.parent()`; add a unit test with an embedded newline. Then re-run `cargo test -p rollcall-cli --test it --locked -- discovery:: no_agent::` in that worktree and merge.
- **P33** (sessions.rs), **P34** (mcp.rs + tests/it/mcp.rs), **P35** (hostile/privacy/store.rs — first real run of the memory monitor), **P39** (shell.rs): running when the session ended; results unknown. Each worktree holds uncommitted edits; inspect `git -C <wt> status`, re-run its test command, merge if green.

## Next
1. Fix `worktree_dir` (above), finish/merge P32–P35, P39; full gate + COUNTS + push after each.
2. Lane E: P40 perf (CI job), P41 release pipeline (orchestrator-owned, `dist`), P42 README + demo. #16–#18 need CI runs on origin.
3. Opus breaker on the integrated change (§12.1 as spec), then Opus skeptic; `gate-record-audit` before each round; loop until #1–#18 MET.

## Gotchas
- Worktrees from agent isolation start at stale `19bfdee`; workers ff to impl/v0 themselves. HANDOFF.md hook rewrites: restore before committing; skip-worktree was unset for this write.
- `block-dangerous-git.sh` blocks `branch -D`: use `-d` after merging.
- macOS ignores `XDG_CACHE_HOME`: every manual rollcall run needs a scratch `HOME` (WORKER-RULES).
- P36 §8.1 deviations and P31 `render_session` no-email-scrub are logged in RISKS for the final gate.
