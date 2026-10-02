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
2. **Homebrew tap** `ravi1395/homebrew-tap`: assumed name, unconfirmed (formula `rollcall-cli` confirmed by the author 2026-10-02).

## Approved, not started (author said yes 2026-10-02, session 7)
3. **Align plain `status` columns like `ls`** — closes a spec gap: §8.2 says "columns as in the TUI", but `plain_status` joins cells with two spaces, so columns drift with name length (`crates/rollcall-cli/src/output.rs` `repo_line` / `worktree_line`).
   - Each row → 3 cells. Repo: `name` | attention badges | last activity. Worktree: `"  " + label` | state badges | last session (agent age "title").
   - Two passes in `plain_status`: build all cells, pad cols 1–2 to the widest entry in this output, trim trailing spaces. Cap col 1 at ~40 cols; a longer cell (e.g. `/private/tmp/...` ext worktree paths) overflows on its own row instead of widening every row.
   - Width via `ratatui::text::Span::width()` (unicode-width, already a dep) — no `Cargo.toml` edit (R1). Only `output.rs` changes.
   - Tests first (R3): a red test that every row's third column starts at the same display column, plus the cap. The existing `plain_status_is_one_line_per_repo_then_one_indented_line_per_worktree` must stay green unchanged.
   - Target shape:
     ```
     agentrec                          ●16 ↑1 6 stale no-upstream  4d
       codex/integrate-agentrec        ●3                          claude 4d "Skills improviser"
     CRM                               ●264 2 stale no-upstream    4d
       main                            ✓ clean                     claude 4d "Remaining work after merge"
     ```

## Candidates (from a real `rollcall status ~/Projects` run, session 7; not approved — each needs its own design)
- **Junk in `.git/refs`:** a Finder `.DS_Store` (> 4096 bytes) made sutra `untrusted git dir` and hid the whole repo. Tolerate/ignore non-ref names like `.DS_Store`. Touches §4.4 security, so it needs a spec revision.
- **One repo error → exit 1:** the `.DS_Store` case makes the whole `status` look failed. Consider exit 0 with a warning, or a distinct code.
- **Risk summary line first:** e.g. "at risk: CRM ●264 no-upstream; agentrec ●16".
- **Not-mine repos:** upstream clones (uv ●1148, gitoxide ●1928) swamp the list. Config `ignore`, or rank down repos with no commits by the user's email.
- **Title length:** truncate to terminal width on a tty (agentic-os-lab's Codex automation title wraps several lines).
- **HTML entities:** gemma-cli's Codex title prints `&lt;command-name&gt;`; decode before display.
- **Colour in plain `status` on a tty:** reuse the TUI palette.
- **Filters:** `--dirty`, `--stale`, `--top N`.
- **Missing worktrees:** hint `git worktree prune` (stay read-only).
- **Prompt-derived titles in `status`:** by design (§8.1), but MCP hides them by default; maybe `status` should match.

## Optional (skeptic, non-blocking)
- `lock_when_free` narrows, but does not close, the fork race before the released-control call (microsecond window).
- Full-F MCP reply has 0 `recent_sessions` (F carries no session corpus); session rendering is covered by the F_clean `data_world` tests.
- `scripts/probe-gix-p15b/sample.py` still prints `first>320MiB` (README labels it rev-13).

## Gotchas
- The global `~/.claude/hooks/write-handoff.sh` now skips repos with `git config handoff.auto false`; this clone has it set (local git config, not committed). A fresh clone needs it set again.
- Same as session 5 archive: stage explicit paths; scratch `HOME` for manual runs; hostile `scan_h*` with `--release` locally.
