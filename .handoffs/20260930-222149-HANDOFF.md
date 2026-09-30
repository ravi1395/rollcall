# HANDOFF — 2026-09-30 impl/v0 (~/Projects/rollcall), session 2

Goal (author's `/goal`): run is done when the final Opus skeptic gate reports every spec §12.1 criterion 1–18 MET. Run mechanics, model roles, gotchas: `docs/superpowers/plans/tasks/v0/ORIENTATION.md`; worker brief rules: `WORKER-RULES.md`; risks: `RISKS.md`; counts: `COUNTS.md` (same dir). Session 1's handoff: `.handoffs/20260930-211905-HANDOFF.md`.

## Done (verified; committed on impl/v0, NOT pushed — 8+ commits ahead of origin)
- P08 merged (`eb419fc`). `python3 scripts/check-manifest.py git` → `ok: 74 repos, all bullets covered`; `sessions` → `ok`; `expect F_repos` → 27, `expect F_worktrees` → 36.
- Full gate at `eb419fc` (`touch crates/*/src/*.rs` first): fmt clean, clippy clean, `cargo test --workspace --locked` → 402 passed / 0 failed / 2 ignored (core 105, git 136, sessions 135, store 16, cli 10). COUNTS row added.
- P15a merged (`46306d9`); audit U-4..U-9 annotated `covered (rev 12)`, UNCOVERED count 0.
- Spec revision 12 + plan revision 6 (`827cbb2`): author chose in-process guards (no subprocess isolation). Per-finding choices: red-team log section "P15a gix read-site audit → spec revision 12"; the spec text is in §3.3 `untracked_count` and §4.4 (index validation, big-file threshold, status call shape, pre-status checks, pattern-file budget), plus the §10.2 hostile fixtures.

- Rev 12 independent skeptic (Fable `skeptical-reviewer`): round 1 FAIL. The blocker was that wide `TREE`/`UNTR` index extensions could retain about 10× their size; fixed in `8945aac` with size caps and `index.threads=1`. Round 2 **GATE PASS** on all six criteria. Post-pass record fixes are in `2882fd9`. Residual risks are in `RISKS.md` (last bullet). Probes: `scripts/probe-index-bomb` (16 MiB TREE → 178 MiB peak, re-run this session) and `scripts/probe-index-ext-sizes.py`.
- impl/v0 is 12 commits ahead of origin and NOT pushed; waiting on the author.

## In progress
- P20 `worktree-agent-a1908ee2d13fad1d7` @ `ba59068`: WIP, 5 `unimplemented!`, never run. Next: re-dispatch P20 on that branch (ff it to impl/v0 first) with the plan rev 6 P20 items; it runs after P18b.

## Decisions + why
- Rev 12 options, all checked against the locked gix sources: U-9 `core.bigFileThreshold=1m` can't be bypassed because `is_binary=Some(false)` needs a `diff.<name>` driver, and those are stripped. U-6(b) uses a rollcall `CompareBlobs` that returns modified above 64 MiB without reading. U-5 uses `recurse_repositories(true)`, which skips `is_git`. Side effect: gitlink directories get entered, so the delegate drops entries under gitlinks and collapses directories that contain a `.git` entry. That is read from source only; P20's submodule test must confirm it. U-4: gix-index entry decode is private, so rollcall walks the v2/v3/v4 entries itself inside `open.rs` (R1: no new modules).
- The pattern-file caps drop from 1 MiB to 256 KiB (N7 budget). This touches merged P18/P19 code, so it goes in the new P18b.

## Gotchas
- A session hook rewrites HANDOFF.md with an unverified summary. Earlier versions are kept in `.handoffs/*-hook-HANDOFF.md`. Stage explicit paths only.
- Pushing impl/v0 needs the author's OK. The P08 fixtures contain fake secrets (`AKIAIOSFODNN7EXAMPLE`, `ghp_FIXTURE…`), so GitHub push protection may block the push.
- `cd` in a Bash call moves the session cwd; use absolute paths.
- The P07 worker's `rm -rf /private/tmp/claude-501/-Users-ravichandrasekhar` was reported to the author on 2026-09-30.

## Not started (plan rev 6)
- P07b (checker bullets first, red; then MANIFEST entries). P15b → P15c (with the rev 6 confirmation items). P18b → P20 (finish) → P21. P09, P10. P27a–P28 (P27b gets the byte-level index writer). P29–P39, P40–P42, then the breaker and the Opus skeptic final gate.
