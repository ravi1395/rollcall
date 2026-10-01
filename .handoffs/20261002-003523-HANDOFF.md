# HANDOFF — 2026-10-01 impl/v0 (~/Projects/rollcall), end of session 5

**Status: ACTIVE.** Previous handoffs are in `.handoffs/`.

The v0 run is complete: the final Opus skeptic gate (round 2, at `db6581d`) reported spec §12.1 #1–#18 all MET (`GATE: PASS`). Spec is now **revision 14**. What remains is loose ends, release prerequisites, and merging to `main`. Run mechanics and history: `docs/superpowers/plans/tasks/v0/{ORIENTATION,WORKER-RULES,RISKS,COUNTS}.md`.

## Done (verified)
- Lane D lanes P32–P35, P39 merged; Lane E P40 (perf CI job), P41 (cargo-dist, `dist-workspace.toml`, SHA-pinned `release.yml`), P42 (README, `demo.tape`, `demo.gif`, CI `demo` job) — committed, pushed. Evidence: CI run 36933875396 on `9abb2a1`, 13/13 jobs success.
- Breaker pass (`crates/rollcall-cli/tests/breaker_v0.rs`): 4 spec violations fixed (refusal/stderr sanitising, MCP wait overflow, Codex title kind mismatch, Claude duplicate session id) + test-side scan-lock flake fix — committed, pushed (`ca6c895` and later).
- Skeptic round 1 failed #8 (hostile scan peak RSS over 512 MB); author chose spec rev 14: worker RSS cap 256 MiB (`1aec2cf`). Round 2 PASS: 41/41 skeptic local runs under 512 MB (max 495,927,296 bytes), CI printed 301.0 MiB (ubuntu) / 280.1 MiB (macos-14).
- Breaker ambiguities resolved by the author (spec rev 14 clarifications, `26e345c`): `R` with `ROLLCALL_RESUME_FILE` and `Y` refuse a session whose cwd is gone (`cee9038`); Codex rollout incremental read equals full re-read (already true; differential test `8b124e8`).
- Local gate at `26e345c`: `cargo fmt --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked --no-fail-fast` → exit 0, 765 passed / 0 failed / 4 ignored. Pushed: `origin/impl/v0` = `5f3b435`; CI run 36938392591 on `5f3b435`: 13/13 jobs success, hostile scan printed peak RSS 283.0 MiB and 357.2 MiB on the two runners. This handoff edit is one later local commit, not pushed.

## In progress
- Nothing mid-edit. Exact next step: work the loose ends below (start with #2/#3, the test-only ones); every `git push` needs the author's approval.
- state: `git status --short` at write time:
  ```
   M HANDOFF.md
  ?? .handoffs/20261001-235936-HANDOFF.md
  ```
  (this file and its archive; committed together with this handoff).

## Decisions + why
- Worker RSS kill cap 320 → 256 MiB (spec rev 14) — author's choice; no in-spec lever worked because `/bin/ps` takes 22–34 ms sampling a child growing at GiB/s (worker probe). Rejected: raising the 512 MB bound, Linux `/proc` sampling, 192 MiB.
- Hostile `scan_h*` tests run on the release binary in CI (`ci.yml` `test` job, `--nocapture` prints RSS) — the bounds are release figures.
- `repository` added to `crates/rollcall-cli/Cargo.toml` (dist requires it) — orchestrator edit beyond R1's `[profile.dist]` allowance; Cargo.lock unchanged.
- Tap `ravi1395/homebrew-tap`, formula `rollcall-cli` — assumed names, not confirmed by the author.

## Test state
`cargo test --workspace --locked --no-fail-fast` at `26e345c` → 765 passed / 0 failed / 4 ignored (gate20). Release hostile set: `cargo test --release -p rollcall-cli --test it --locked -- hostile::scan_h --nocapture` → 15 passed.

## Gotchas
- A hook rewrites `HANDOFF.md` repeatedly during a session (seen many times this session). Restore with `git checkout HANDOFF.md` before every commit; stage explicit paths only. Finding and fixing that hook is a loose end below.
- `block-dangerous-git.sh` blocks `git branch -D` and any command whose text has `--force` after `push` (even inside quotes): use `branch -d`, write such strings to a file.
- macOS ignores `XDG_CACHE_HOME`: every manual rollcall run needs a scratch `HOME`.
- Hostile `scan_h*` tests in **debug** can time out under machine load (seen locally while other cargo jobs ran); CI skips them in debug. Run them with `--release` locally.
- Agent-isolation worktrees start at stale `19bfdee`; workers must `git merge --ff-only impl/v0` first.
- CI's stable toolchain floats (Rust 1.99 deprecated `fetch_update`); MSRV is 1.89, so newer std APIs fail locally under clippy's MSRV lint.

## Loose ends (not started)
1. **Push** the local commit that records this handoff's CI result (ask the author first).
2. **Scan-lock `.expect` in integration tests**: `crates/rollcall-cli/tests/it/store.rs` and `it/mcp.rs` take the lock with `try_scan_lock(..).expect(..)` — same fork/fd-inheritance race fixed for unit tests in `37dcfc5`; switch to a bounded wait.
3. **MCP gate world**: `it/mcp.rs` builds the MCP world from `F_clean`; spec §10.2 says `--root $F`. Add a full-F MCP test (the round 1 skeptic's uncommitted probe found all replies schema-valid and ≤ 16 KiB).
4. **Stale 320 MiB in probe tooling**: `scripts/probe-gix-p15b/README.md` and `sample.py` (rev 13 probe) still report "over the 320 MiB cap"; update to 256 or mark as a rev 13 historical probe.
5. **#8 margin is thin**: worst local peak 495,927,296 bytes (≈ 3% under 512 MB) on the author's Mac; slower/loaded hardware or two simultaneous pack bombs untested. Options if it bites: Linux `/proc/<pid>/statm` sampling (no spawn), cap lower again (author's call).
6. **Release prerequisites (never run)**: create the `ravi1395/homebrew-tap` repo, add the `HOMEBREW_TAP_TOKEN` secret, then a first tag runs `release.yml` (attestations + Homebrew publish untested). crates.io trusted publishing is a manual checklist item. README install lines point at that tap/release, which don't exist yet.
7. **README absolute claim**: "Reading a repo never runs its hooks, clean/smudge filters, or diff and merge drivers" rests on the hostile fixture set; consider wording it as tested behaviour.
8. **The HANDOFF.md-rewriting hook**: find it (`~/.claude/settings.json` hooks / plugins) and stop it writing into repos, or point it elsewhere.
9. **Stray dirs** `.claude/worktrees/agent-a46bd3d90c7d35a28`, `agent-ab242545324fe5b8d`, `agent-acfce1bcf56d4b92b`: not registered git worktrees (another session's); inspect and remove if empty.
10. **Outside-project leftover**: `/private/tmp/scan.rs.orig` (a worker's pristine copy of scan.rs) — deleting it needs the author's approval (outside the project).
11. **Author's global auto-mode settings change** (only `git push` and deletes outside the project need approval): auto mode refused to let Claude edit `~/.claude/settings.json`. The author must apply it by hand; the jq filter was in session 5's scratchpad (`automode.jq`), which may be gone — re-derive: `permissions.ask` = git push only, `autoMode.classifyAllShell = true`, `autoMode.soft_deny` = {git push; deletes outside the project folder}, back up first. `block-dangerous-git.sh` would still block `branch -D` / `reset --hard`.
12. **§12.2 author smoke** (non-gating): not run. Then open the PR `impl/v0` → `main` (or merge) — author's call.

## Suggested skills
- `/handoff ingest` at start; `verify-ledger` before any release-ready claim; `ci-divergence-triage` if CI and local disagree; `gate-record-audit` before committing record text; `repo-debt-report` before merging to `main`.
