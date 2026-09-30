# HANDOFF — 2026-09-30 impl/v0 (~/Projects/rollcall)

> CONSUMED 2026-09-30 21:19 by session 2 ingest; archived as `.handoffs/20260930-211905-HANDOFF.md`. Still the latest state until replaced.

Goal (author's `/goal`): run is done when the final Opus skeptic gate reports every spec §12.1 criterion 1–18 MET. Run mechanics, model roles, gotchas: `docs/superpowers/plans/tasks/v0/ORIENTATION.md`; worker brief rules: `WORKER-RULES.md`; risks: `RISKS.md`; counts: `COUNTS.md` (same dir).

## Done (verified, merged on impl/v0, pushed except the last 2 docs commits)
- P01–P07, P11–P14, P16–P19, P22–P26 merged. Evidence: full gate at `5c085ea` → fmt/clippy clean, `377 passed / 0 failed / 2 ignored` (core 105, git 111, sessions 135, store 16, cli 10); after P19 merge `cargo test -p rollcall-git` → 136 passed.
- Spec rev 11 (+ amendment) closes P14's U-1..U-3 (commits `4c014f2`, `ff981d8`).
- CI on impl/v0: `b25517a` green (run 36648100233); earlier red run 36647871827 was a Linux inode-reuse test flaw, fixed by `b25517a`.
- impl/v0 HEAD `d988d6c` (docs only since the push) — not pushed.

## In progress (committed on worktree branches, NOT merged)
- **P08** `worktree-agent-a894a4cb37238495d` @ `6284996` — passed (check-manifest `sessions` ok, `git` ok: 74 repos; F_repos 27, F_worktrees 36; claude 43, codex_db 10, codex_rollout 10). Next: merge, re-run both checker modes.
- **P15a** `worktree-agent-ab039db1395e5371d` @ `f56578c` — BLOCKED: 6 UNCOVERED (U-4..U-9) in `docs/audits/gix-0.88-read-sites.md`: U-4 index TREE/UNTR recursive decode → process stack-overflow abort (reproduced); U-5 dirwalk `is_git` reads nested `.git/HEAD` etc. unbounded (FIFO hangs); U-6 tracked 100755 entry over symlink/FIFO followed + built-in filters read_to_end; U-7 `.gitignore`/`.gitattributes` index blobs 40–70× memory; U-8 `sharedindex.*` not pre-opened; U-9 rename similarity ~1 GiB/pair via repo `core.bigFileThreshold`. **Needs a spec rev 12 decision** — some options (process isolation) conflict with §4.4 "no subprocesses"; ask the author if a fix changes architecture.
- **P20** `worktree-agent-a1908ee2d13fad1d7` @ `ba59068` — WIP, agent died (machine slept): state.rs 1058 lines, 36 tests, 5 `unimplemented!`, never run. Next: re-dispatch P20 on that branch (after rev 12 settles U-4..U-9 constraints on status calls).

## Decisions + why (details in RISKS.md)
- impl/v0 in main checkout; isolated worktrees fork from stale `19bfdee` → fast-forward each (`git -C <wt> restore HANDOFF.md; git -C <wt> merge --ff-only impl/v0`).
- Per-worktree `CARGO_TARGET_DIR` (APFS clone of main `target/`): shared dir made test binaries collide.
- claimd ignored via local untracked `.claims/config.json` (`lint.ignore=["**"]`, in `.git/info/exclude`) — author chose this 2026-09-30.
- Rev 11 amendment: pseudo-refs other than HEAD capped 1 MiB (4 KiB rejected real FETCH_HEAD/COMMIT_EDITMSG).
- §5.0 prompt filter applies to Codex `user_message` too; FIFO `.git` → `bad gitdir:` row; enclosing repo worktree is `untrusted worktree path` by rule (d) literally; hostile branch is `$(true>marker-rc)` (git rejects the spec's space).
- R1 edit: rollcall-core dev-dep `serde_json`.

## Test state
`cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked` → last full run 377/0/2 at `5c085ea`; not re-run after P19 merge + test fix (git crate alone: 136 passed). `touch crates/*/src/*.rs` first (stale-mtime builds).

## Gotchas
- Session hook rewrites HANDOFF.md on push/stop; stage explicit paths, never `commit -a`. Worktrees carry it dirty too → `git restore HANDOFF.md` before merges.
- `block-dangerous-git.sh` blocks `reset --hard`, `branch -D`, `restore .`; use `git worktree remove --force` + `branch -d`.
- Test fixtures contain fake secrets (`AKIAIOSFODNN7EXAMPLE`, `ghp_FIXTURE…`) — GitHub push protection may block the P08 push.
- P07 worker ran `rm -rf /private/tmp/claude-501/-Users-ravichandrasekhar` (maybe another session's scratch) — tell the author.
- Never `cd` into a worktree in the orchestrator shell (moves session cwd).

## Suggested skills
`/handoff ingest`, `gate-record-audit` before gates, `probe-hygiene` before mutation probes, `plan-run-artifacts` (COUNTS rows per merge).

## Not started
- Spec rev 12 for U-4..U-9 → then P15b, P15c, P20 (finish), P21.
- P09, P10 (oracles; P09 must include f-shell: +1 repo, +7 worktrees; enclosing row untrusted), P27a–P28 (harness must model P08's extra keys), P29–P39, P40–P42 (CI-bound), breaker + Opus skeptic final gate.
- Full gate + push of impl/v0 after merging P08.
