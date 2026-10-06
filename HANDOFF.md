# HANDOFF — 2026-10-06 impl/v0 (~/Projects/rollcall), end of session 12

**Status: ACTIVE.** Supersedes the session 11 handoff (byte copy at ingest: `.handoffs/20261006-223120-consumed-HANDOFF.md`). Spec rev 16: `docs/superpowers/specs/2026-09-29-rollcall-design.md`; fixed criteria: `docs/superpowers/plans/tasks/v1/ACCEPTANCE.md` (items I2, I1, I3, I5a, I5c, I5e, I5d, I5b, I4, S11 Follow-ups); workflow: `.claude/workflows/rollcall-v1.js` (its prompts read the author/autonomy sections at the bottom of this file, so keep those headings).

**Next session: one step is left, and it waits on the author.** CI on `impl/v0` cannot run until GitHub billing is fixed. Once the author says billing is fixed, re-run CI on `1147a53` or later (`gh run rerun <id>` or push), and if it goes green open the PR `impl/v0` → `main` (already authorized; never merge it, never enable auto-merge). Don't open the PR while CI is billing-blocked (author decision of session 10).

## Done (verified)
- **S11 follow-ups A–D: accepted.** Commits: `1209b48` (criteria, spec notes, plan R1 annotations), `af84933` (feat), `8c237b1` (breaker fix: the locked set is keyed per (repo path, worktree path)), `c860290` (breaker tests `crates/rollcall-cli/tests/breaker_s11.rs`), `2e70a2d` (repeat scope per the author's answers), `f2c11a5` (docs). Fable skeptic at `f2c11a5` found S11-1..17, 11a and 11b all met, and re-ran two mutations itself: S11-6 (width reverted to crossterm `terminal::size()` → `widest line 60 columns on a 40-column stdout (controlling terminal 60)`) and S11-14 (dropping the locked filter → red).
- **Range gate `eeb6aeb..f2c11a5`: GATE PASS** (Fable). Every ACCEPTANCE.md item and the Conventions section are met. **I5c-6 and I5e-6 are verified on the real binary**: `--top 2` gives `at risk: r1 ↑2`, `--top 1` gives no risk line, and `--dirty` hides the clean repo's hint. I4-7 ran `dist generate` 0.33.0 on a scratch clone and printed nothing. R1 over the range: only the approved rustix line plus its one lock line; no `mod` list changes.
- **Open questions (1) and (2) from session 11: answered and done.** Committed `1147a53`. The refused-root peek (`root.rs` `peek`) now chains `WalkOptions.ignore` into its path skips; new unit test `root::tests::ignored_paths_are_not_suggested`, red before the fix and green after. Spec §4.2 and §4.3 have notes. Single-file fix with a test, so it went direct, with no breaker or skeptic pass.
- **Pushed:** `origin/impl/v0` = `1147a53`; `git rev-list --count origin/impl/v0..HEAD` → 0.

## In progress
- Nothing. `git status --short` → `?? graphify-out/` only (untracked; never commit).

## Decisions + why (session 12)
- **S11-3 exception.** The old I5e probe `breaker_probe_locked_missing_worktree_is_counted` recorded behaviour (AMBIGUOUS) that author decision D reversed. It was inverted and renamed `…_is_not_counted`, and S11-3 now names that edit. Decision D is what forces this change; no criterion was weakened.
- **Locked set per repo.** The breaker showed that two repos can register a linked worktree at the same path, and a global path set let one repo's lock hide the other's hint. The set is now keyed by `(Repo.path, Worktree.path)`, built in `Builder::repo`. `Repo`, `Worktree` and `StatusDoc` stay unchanged (R1).
- **Repeat scope by per-argument `overrides_with`, not `args_override_self`.** `args_override_self` on the `Status` variant also covered `--json` and the flattened shared flags. Per the author, only `--dirty`, `--stale`, `--top` and `--color` take the last value.

## Test state
- `cargo fmt --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked` at `1147a53` → exit 0; the `test result:` lines sum to **973 passed / 0 failed / 4 ignored**. The ignored tests are `materialize_perf`, `materialize_demo`, `fs.rs::modes_child` and `fs.rs::lock_child`, all by design.
- CI run `37540606022` (`f2c11a5`): every job `failure` with 0 steps. Annotation: "The job was not started because recent account payments have failed or your spending limit needs to be increased." No code signal; Linux (`tcgetwinsize` under the ubuntu runner) is still untested.

## Gotchas
- **CI is billing-blocked, not red.** Check annotations with `gh api repos/ravi1395/rollcall/check-runs/<job id>/annotations` before debugging any failure.
- The workflow can't run S11-style sections: `ORDER` in `rollcall-v1.js` is fixed, and editing the script is blocked. Use direct Agent calls.
- The permission-prompt fix is applied (author, 2026-10-06 23:47): `grep -c '"Bash(rm \*)"' ~/.claude/settings.json` → 0. Backup: `~/.claude/settings.json.bak-20261005-231034`.
- Carried over: Python `re.sub` replacement strings turn `\n` into real newlines (use `str.replace`); hostile `scan_h*` tests are slow in debug; concurrent sessions may edit this branch (stage explicit paths); `git config handoff.auto false`; use a scratch `HOME` for manual runs; never print prompt text from `~/.claude` / `~/.codex`.
- Skeptic observations (non-blocking): a missing worktree with a non-UTF-8 path shares its lossy lock key with a twin (no test; §4.4 says such paths fail as `git error: worker failed` anyway). On macOS the cache follows `$HOME/Library/Caches`, not `XDG_CACHE_HOME`.

## Suggested skills
- `verify-ledger` before any merge-ready claim, `repo-debt-report` before the PR, `ci-divergence-triage` if CI runs and goes red.

## Not started
- PR `impl/v0` → `main`: waits on green CI, which waits on billing.
- Tap prerequisites (create `ravi1395/homebrew-rollcall`, add the `HOMEBREW_TAP_TOKEN` secret) and any release run: author actions, later.
- Carried over, optional: the `lock_when_free` fork-race window; the full-F MCP reply has 0 `recent_sessions`; `scripts/probe-gix-p15b/sample.py` prints `first>320MiB`.

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

## Author decisions 2026-10-05 (session 10)
- I3: approve the R1 exception adding `ignore: Vec<PathBuf>` to `WalkOptions`; the walk never descends into an ignored dir (I3-9 stays as written).
- I1: accept `git error: …` for a HEAD symref to a dot-named loose ref (bounded, no read).
- I3 breaker AMBIGUOUS (session 11): (A) `doctor` from the cache recounts from the visible rows after an `ignore` is added (filter on read); (B) ignored dirs still count in `walk.skipped_dirs`; (C) a session whose cwd is in an ignored repo nested inside a non-ignored repo goes to the outer repo under the §6 longest-prefix rule (follows from the rule; no test exercises it). Accepted, no change.
- I5a/I5c breaker AMBIGUOUS (session 11, 2026-10-06): keep `&#X…;` decoding and the 12-byte reference cap; accept that entity-encoded secrets are unscrubbed outside plain status; re-run the §5.0 filter on decoded prompt text; print repo names as written in the risk line (no escaping). Recorded in spec §8.2.
- Session 10 confirmed the launch: run the full `implement` workflow end to end, push `impl/v0`, no PR while CI is billing-blocked.

## Author decisions 2026-10-06 (session 11; approved, R1 exceptions included)
- **A. Filters and render mode move into `Ctx`.** `Ctx` gains `status_filters: StatusFilters` and `render: Render` (R1 exception), and `output::status` returns to its pinned signature `status(ctx, &ScanOutcome, root, json: bool, out: &mut dyn Write) -> ExitCode`. Today it takes `filters: &StatusFilters, render: Render` (`output.rs` `pub fn status`); callers are `cli.rs` and output tests that pass `&StatusFilters::default()`.
- **B. Width comes from stdout's own terminal, not the controlling one.** Add `rustix` to `crates/rollcall-cli/Cargo.toml` with the `termios` feature (R1 exception; rustix 1.1.5 is already in Cargo.lock via crossterm, so no new package, only a dependency line). Read the width with `rustix::termios::tcgetwinsize(std::io::stdout())`; no `unsafe` (workspace `unsafe_code = "forbid"`). Spec §8.2 currently names crossterm `terminal::size()`; update it. The ignored breaker test above becomes a regression test.
- **C. Lenient flags.** A `--top N` above `usize::MAX` means all repos (`0`, non-integers and negatives stay exit 2, per I5d-5). A repeated filter flag takes the last value, not exit 2 (clap `args_override_self` or equivalent). Update spec §3.1/§8.2.
- **D. Locked missing worktrees are left out of the prune hint.** `WorktreeState` gains `locked: bool` with `#[serde(default)]` so old caches load (R1 exception, plan `WorktreeState {…}` entry). It is set when `<admin_dir>/locked` exists, checked with `symlink_metadata` only, never read (in `rollcall-git` `state::missing_worktree_state`). It is not in the JSON doc. A repo whose missing worktrees are all locked gets no hint; otherwise the count excludes locked ones. Re-check the §4.4 audit note for the new admin-dir probe.
- **Accepted, spec notes only:** a repo named `hint:` prints a row that starts like a hint line; an `error:` badge with an empty message prints `ESC[31merror:ESC[0m ` (trailing space outside the colour).

- **Session 12 answers (2026-10-06):** only `--dirty`, `--stale`, `--top` and `--color` take their last value when repeated; any other repeated flag (`--json`, `--yes`, `--no-scan`, `--submodules`, `--follow-symlinks`) stays exit 2 in `status`, the piped form and `scan` alike. Every occurrence is validated (`--top 0 --top 2` exits 2). Refused-root suggestions skip config `ignore` paths (done, `1147a53`). A linked worktree under an ignored dir whose main repo is outside the root is not shown (spec §4.2 note only).
