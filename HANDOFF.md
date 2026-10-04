# HANDOFF — 2026-10-03 impl/v0 (~/Projects/rollcall), end of session 8

**Status: ACTIVE.** Previous handoff (session 6, consumed by session 7) archived as `.handoffs/20261003-015106-consumed-HANDOFF.md`.

v0 passed its §12.1 gate in session 5 (spec rev 14). Session 7 shipped the plain `status` column alignment (spec rev 15). Session 8 tightened §8.2 wording. **CI is red** (test bug, Linux only) — fix that first.

## Done (verified)
- Plain `status` column alignment (session 7's "approved, not started" #3) — committed `0afd6d5`, `6f40475`, `f85b896`, `446cf15`, `960cb0a` (leading-whitespace strip, another session). Spec rev 15 §8.2.
- §8.2 names both capped columns (name, badge) and states an over-cap cell never wraps; status line records author confirmation 2026-10-02 — committed `6d6fc42`, pushed. Evidence: `git show --stat 6d6fc42` → 1 file, +2/−2; §9 untouched.
- `origin/impl/v0` = `960cb0a` = local HEAD (0 ahead / 0 behind, `git ls-remote` checked 2026-10-03).

## In progress
- **CI red on `73ac982` and `960cb0a`** (runs 37077392463, 37077707522; last green `f7ada95`, run 36944117371). Only `test (ubuntu-latest)` fails, one test:
  `no_agent::breaker3_binary_blank_names_and_branches_fall_back_and_trailing_spaces_are_not_counted` at `crates/rollcall-cli/tests/it/no_agent.rs:396`:
  ```
  "\t": last cell not at E + 2 of the over-cap path: "/tmp/.tmpyQdUbu/root/                                quiet 27d"
  ```
  Cause: test assumes the blank-name fallback path is > 40 columns. True on macOS (`/var/folders/…/T/.tmpXXXX/root/…`), false on Linux (`/tmp/.tmpXXXX/root/…`, ~22 cols), so there the cell correctly aligns to S3. Product matches §8.2; test is wrong.
  - Exact next step: test-only fix, keep it able to fail. Preferred: build the blank-name repos under a nested directory long enough that every fallback path is > 40 display columns on any OS (assert that length first so the premise can't silently go false). Alternative: assert `at == max(S3, E + 2)`. Then full gate (CLAUDE.md R2) and push; confirm both `test` jobs green.
- state: `git status --short` → (empty) before this file was written.

## Decisions + why
- §8.2 cap covers every padded column (name and badge) — author, 2026-10-02: a long `error: …` badge would otherwise widen every row.
- An over-cap cell shifts only the rest of its own line, never wraps — author, 2026-10-02: plain status stays one line per repo / per worktree.

## Author decisions 2026-10-03 (session 8; each is approved scope, not yet designed or specced)
Asked with today's behaviour and each option's effect; answers below are the author's. Items 1, 2, 4, 5 and the UX batch each need a spec revision (rev 16+) and their own `/phases` plan before code. Details marked *assumption* were in the option text but not separately confirmed — confirm at design time.

1. **Junk under `.git/refs` → skip dot-files.** Today a > 4 KiB Finder `.DS_Store` under `refs/` fails the §4.4 loose-ref cap and the whole repo becomes `error: untrusted git dir` (seen on sutra). Decided: entries under `refs/` whose name starts with `.` are exempt from the size check (git never treats them as refs); symlinks and other oversize entries are still refused. Before specifying: check whether gix's own ref iteration skips or chokes on `.DS_Store` (if it chokes, exempting it from pre-open just moves the error). Touches §4.4 security → re-audit per CLAUDE.md.
2. **Exit code → distinct code 4.** Today any `Repo.error`/`Worktree.error` → exit 1 (§3.1). Decided: completed scan with some error rows → exit 4; exit 1 stays for partial walk / no scan ever. Check the `rc` shell function (§8.1, uses 10) and every test asserting exit 1 for error rows.
3. **Not-mine repos → config `ignore` paths.** Today only `skip_dirs` (directory names) hides anything. Decided: new §3.2 config key `ignore` (paths or globs); ignored repos are neither scanned nor shown. *Assumption:* `~` expansion and glob syntax — settle in design. R1: if this needs a new dependency (glob crate) or `Cargo.toml` edit, stop and report.
4. **Prompt-derived titles → keep showing** in plain `status` and TUI (no change; `redact_prompts` remains the opt-out). Nothing to do.
5. **Plain-status UX batch → all four approved:**
   - decode HTML entities in titles (`&lt;command-name&gt;` → `<command-name>`), decode *before* sanitising;
   - truncate each line to terminal width with `…` on a tty only (piped output unchanged); must keep the §8.2 one-line-per-row guarantee;
   - risk summary line first (e.g. `at risk: CRM ●264 no-upstream; agentrec ●16`) — selection rule to be defined;
   - filters `--dirty`, `--stale`, `--top N` (new §3.1 flags) and TUI-palette colour on a tty (*assumption:* off when piped or `NO_COLOR` set).
   JSON output unchanged for all of these. Not asked / still open: `git worktree prune` hint for missing worktrees.
6. **RSS margin (#8) → leave it.** Act only if CI or a real scan trips the 512 MB hostile-scan bound.
7. **Homebrew tap → `ravi1395/homebrew-rollcall`** (users: `brew install ravi1395/rollcall/rollcall-cli`). Supersedes the assumed `ravi1395/homebrew-tap`, still named at `dist-workspace.toml:25` (`tap = …`) and `.github/workflows/release.yml:304` (cargo-dist output: change the toml, regenerate with `dist generate`, don't hand-edit the workflow). Release prerequisites still unrun: create the tap repo, add the `HOMEBREW_TAP_TOKEN` secret (author actions).
8. **Merge → open PR, author merges.** After CI is green: open PR `impl/v0` → `main`, report the link, do not merge or enable auto-merge.

## Author design answers 2026-10-04 (refine the decisions above; binding for spec rev 16)
- **`ignore` (3):** path entries only, `~` expands, prefix match: a listed dir hides every repo at or under it in status, TUI and MCP. No glob crate, no `Cargo.toml` change.
- **Exit 4 (2):** applies to `rollcall status` and `rollcall scan`. If the walk was also partial or no scan exists, exit 1 wins. TUI `q` stays 0; MCP unaffected.
- **Risk summary (5):** repos with attention 2–3 (dirty/untracked or unpushed); errors excluded. Max 5 names then `+N more`; no line when none.
- **tty (5):** default auto: colour and width truncation only when stdout is a terminal; `NO_COLOR` disables colour. Plus a new flag `--color auto|always|never`. *Assumption:* `--color` controls colour only; truncation still follows the tty test alone.
- **Filters (5):** repo-level. `--dirty` / `--stale` pick repos where any worktree matches and show all of that repo's worktrees. `--top N` keeps the first N repos of the §3.3 sort, applied after filters. *Assumption:* `--dirty --stale` combine as OR; `--dirty` = dirty/untracked or unpushed > 0; `--stale` = any `stale_reason`.
- **Prune hint (new, approved):** when any worktree is `missing`, one footer line in plain status, e.g. `hint: N missing worktrees — run 'git worktree prune' in <repo>`. rollcall never runs it; not in JSON.
- **HTML entities (5), default not asked:** decode `&lt; &gt; &amp; &quot; &#39;` and numeric `&#N;`/`&#xH;` before sanitising, std only.
- **Tap prerequisites (7):** later, not blocking. Next session changes `dist-workspace.toml` and regenerates `release.yml` only; no release run until the author says so.

## Autonomy granted 2026-10-04
- Without asking: fix the CI test, push to `impl/v0`, open the PR `impl/v0` → `main` (never merge it, never enable auto-merge).
- Then write spec revision 16 covering items 1, 2, 3, 5, the prune hint and the tap rename, and **stop for author review**.
- After the author approves the spec: run the full loop (plan → workers → breaker → skeptic, CLAUDE.md R1–R4) and push to `impl/v0` without per-step questions. Ask only on blockers, R1 hits, or ambiguous breaker findings.

**Suggested order for session 9:** fix CI test → push → green CI → open PR → spec rev 16 (stop for review) → after approval, plan + implement.

**Workflow for all of the above:** `.claude/workflows/rollcall-v1.js` (Sonnet implementers, Opus breaker + Opus skeptic, per the author 2026-10-04). Run one stage at a time from the main loop:
- `Workflow({scriptPath: "/Users/ravichandrasekhar/Projects/rollcall/.claude/workflows/rollcall-v1.js", args: {stage: "ci-fix"}})` → then the main loop pushes, checks CI, opens the PR.
- `args: {stage: "spec"}` → drafts spec rev 16 + `docs/superpowers/plans/tasks/v1/ACCEPTANCE.md`, commits locally, **stops for author review**.
- `args: {stage: "implement", specApproved: true}` (optional `items: ["I2", …]`) → serial items I2 → I1 → I3 → I5a → I5c → I5e → I5d → I5b → I4; per item: implement → full gate → commit → breaker → guard → fixes → skeptic (≤ 3 rounds); stops at the first item that is not accepted. Never pushes.
- By name (`Workflow({name: "rollcall-v1", …})`) was not found in session 8 (registry likely loads at session start); `scriptPath` is verified: a dry run with a bad stage threw the expected error after 0 agents.
- Deviations from defaults, chosen deliberately: Opus (not Fable) skeptic at the author's request; one serial lane in the main checkout instead of R2 worktree lanes, because the items share `cli.rs`, `output.rs` and `config.rs`.

## Test state
- Local full gate not re-run this session (docs-only commit). Last known: session 6 at `1877ca6` → 766 passed / 0 failed / 4 ignored.
- CI at `960cb0a`: 113 passed / 1 failed / 2 ignored in `it` on ubuntu; macOS green.

## Gotchas
- Concurrent sessions edit this branch: check `git status` and `git log origin/impl/v0` before editing; stage explicit paths; never commit another session's hunks (session 8 staged its spec edit via `git update-index` to leave a foreign hunk out).
- `git config handoff.auto false` is set locally (not committed); the global write-handoff hook skips this clone.
- Scratch `HOME` for manual runs; hostile `scan_h*` with `--release` locally. Never print prompt text from real `~/.claude` / `~/.codex` logs.
- R1: no `Cargo.toml` / `Cargo.lock` / `mod` list / pinned-signature edits without stopping to report.

## Suggested skills
- `ci-divergence-triage` (CI-only red), `probe-hygiene` before mutation-checking the fixed test, `verify-ledger` before any merge-ready claim, `phases` for any approved candidate.

## Not started
- Everything in "Author decisions 2026-10-03" above. Spec: `docs/superpowers/specs/2026-09-29-rollcall-design.md`; plan: `docs/superpowers/plans/2026-09-29-rollcall-v0-plan.md`.
- Optional (skeptic, non-blocking, unchanged from session 6): `lock_when_free` fork-race window; full-F MCP reply has 0 `recent_sessions`; `scripts/probe-gix-p15b/sample.py` prints `first>320MiB`.
