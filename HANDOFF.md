# HANDOFF — 2026-10-02 impl/v0 (~/Projects/rollcall), end of session 6

**Status: ACTIVE.** Previous handoffs are in `.handoffs/` (session 5's consumed copy: `20261002-003523-HANDOFF.md`).

v0 passed its §12.1 gate in session 5 (spec rev 14). Session 6 closed the code-actionable loose ends; what remains needs the author.

## Done (verified)
- Loose ends #2–#4 — committed `1877ca6`: integration tests take the scan lock via `support::lock_when_free` (bounded wait, panics at `RUN_DEADLINE`); new `mcp::both_tools_on_full_f_are_valid_and_within_the_cap` (`rollcall mcp --root $F`, both tools, schema-valid, < 16 KiB); `scripts/probe-gix-p15b/README.md` marked as a rev-13 probe.
- Evidence: full gate at `1877ca6` → exit 0, 766 passed / 0 failed / 4 ignored (baseline at `5347a38` measured this session: 765 / 0 / 4). Opus skeptic (`skeptical-reviewer`, model opus) → GATE: PASS, AC1–AC5 MET, with its own mutation probes (cap `< 10` → red "3216 bytes"; lock on wrong root → held assert red; deadline branch should_panic ok). Breaker skipped: tests + docs only, no product code.

## In progress
- Nothing mid-edit. Pushed: `origin/impl/v0` = `fb8790b`; CI run 36943101006 on `fb8790b`: all 14 jobs success (both `test` jobs ran the three changed tests `... ok`; hostile scan H peak RSS 267.8 MiB and 296.2 MiB). The commit recording this line is local, not pushed.

## Not started (author decisions)
1. **#8 RSS margin thin** (worst local peak 495,927,296 bytes vs 512 MB): act only if it bites (Linux `/proc` sampling or a lower cap).
2. **HANDOFF-rewriting hook**: `~/.claude/hooks/write-handoff.sh` (global settings, SessionEnd + PostToolUse on `git push`) overwrites this file; disable or scope it per repo.
3. **Homebrew tap** `ravi1395/homebrew-tap`, formula `rollcall-cli`: still assumed names.
4. Merge `impl/v0` → `main`.

## Optional (skeptic, non-blocking)
- `lock_when_free` narrows, but does not close, the fork race before the released-control call (microsecond window).
- Full-F MCP reply has 0 `recent_sessions` (F carries no session corpus); session rendering is covered by the F_clean `data_world` tests.
- `scripts/probe-gix-p15b/sample.py` still prints `first>320MiB` (README labels it rev-13).

## Gotchas
- Same as session 5 archive: restore `HANDOFF.md` (`git checkout HANDOFF.md`) before commits; stage explicit paths; scratch `HOME` for manual runs; hostile `scan_h*` with `--release` locally.
