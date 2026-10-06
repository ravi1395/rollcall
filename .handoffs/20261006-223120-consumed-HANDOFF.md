# HANDOFF — 2026-10-06 impl/v0 (~/Projects/rollcall), end of session 11

**Status: ACTIVE.** Supersedes the session 10 handoff (byte copies: `.handoffs/20261005-230515-consumed-HANDOFF.md` at ingest, `.handoffs/20261006-101619-superseded-HANDOFF.md` at end). Spec rev 16: `docs/superpowers/specs/2026-09-29-rollcall-design.md`; fixed criteria: `docs/superpowers/plans/tasks/v1/ACCEPTANCE.md`; workflow: `.claude/workflows/rollcall-v1.js` (its prompts read the three author/autonomy sections at the bottom of this file, so keep those headings).

**Session 12 directive: run all the remaining work end to end in one go, not one step.** Sequence:
1. **S11 follow-ups** (author decisions 2026-10-06, section at the bottom; all four approved, including their R1 exceptions). First write their fixed criteria into ACCEPTANCE.md as a new section `## S11 Follow-ups` (IDs `S11-n`, observable, each able to fail), and record the three R1 exceptions in the plan's pinned list (`docs/superpowers/plans/2026-09-29-rollcall-v0-plan.md`: the `config.rs: … Ctx {…}`, `output.rs: status(…)` and `WorktreeState {…}` entries, annotated like the `WalkOptions { … ignore … }` one) and in spec §3.1/§8.2. Then implement A–D, run the breaker (`Agent({subagent_type: "breaker", model: "opus"})`), guard, fix, gate, commit, and run the skeptic (`skeptical-reviewer`, `model: "fable"`) until accepted. Use direct Agent calls, not the workflow (see Gotchas).
2. **One main-loop skeptic over `eeb6aeb..HEAD`** against all nine ACCEPTANCE.md items plus S11. It must check **I5c-6 and I5e-6 on the real binary**. Both were left `unverified` because they need `--top` / `--dirty`, which I5d added after them. Each item's reviewer passed everything else.
3. Full R2 gate, then push `impl/v0` (already authorized; 27+ unpushed commits).
4. If CI goes green, open the PR `impl/v0` → `main` (never merge it, never enable auto-merge). If billing still blocks CI, report that and don't wait.

Stop only on: R1 hits beyond the three approved below, AMBIGUOUS breaker findings, a fix loop that makes no progress, or any need to edit `.claude/workflows/rollcall-v1.js`.

## Done (verified)
- **I3 config `ignore` paths: accepted.** Committed `272f87b`, `f676104` (breaker tests plus fixes: realpath-as-written in `scan::canonical_lenient`; `doctor` recounts from visible rows via `scan::visible_diagnostics`), `bac6556`, `aa10506`, `3da937a`. Fable skeptic: I3-1..12, R1 and the gate all met; gate 852 passed / 0 failed / 4 ignored at `aa10506`. Mutation reds were quoted for the doctor recount (`left: 6 right: 2`), skipped_dirs (`left: 0 right: 1`) and the scan-time drop (`left: (Some(4), "a")`).
- **I5a HTML entities: accepted** (workflow run `wf_27be9aad-870`): `fd6509a`, `863293f`. Follow-up `84db9ee` (merged in `bb76630`): decoded prompt text goes through §5.0 again, and the `rows_only` test helpers assert the risk line instead of stripping any `at risk:` line.
- **I5d filters, I5b colour/width, I4 tap rename: accepted** (run `wf_ceb91383-b01`): `681f96d`, `135aded`, `fc7ea65`, `6ff96ee`, `5273324`. No release run (I4 only changed `dist-workspace.toml` and regenerated `release.yml`).
- **I5c risk line and I5e prune hint: implemented, every criterion met except the deferred ones above** (`81e73d6`, `5fe28d7`, `a2737ce`, `2535db0`). I5c-6 and I5e-6 are unverified only because of the ordering, not because of a defect.
- The final workflow skeptic (I5d, I5b, I4) failed exactly one check: the **R1 pinned signature** change to `output::status` (it gained `filters`, `render`). Its fix is follow-up A below.

## In progress
- Nothing half-edited. `git status --short` → `?? graphify-out/` only (untracked; never commit). HEAD `5273324` plus this handoff commit; `git rev-list --count origin/impl/v0..HEAD` was 27 before this commit.

## Decisions + why (session 11)
- **A side worktree for main-loop fixes while a workflow runs.** The workflow's commit agent stages every changed path in the main checkout, so main-loop edits made while it runs end up in its commits. The `s11-followups` branch was merged between workflow runs (`bb76630`, conflicts resolved in `rows_only`).
- **I5c-6 / I5e-6 deferred, not fixed.** Adding `--top` / `--dirty` inside I5c/I5e would have pre-empted I5d. The I5d code trims `doc.repos` before `plain_status`, and that is what the two criteria rely on. Verify them on the real binary in step 2.
- Permission prompts: the global `ask` rules for `rm`/`truncate` caused most prompts. The classifier blocks Claude from editing `~/.claude/settings.json`, so a proposed file was staged in the session 11 scratchpad for the author to apply (backup at `~/.claude/settings.json.bak-20261005-231034`). Whether it was applied is unknown; check with `grep -c '"Bash(rm \*)"' ~/.claude/settings.json` (0 means applied).

## Test state
- `cargo fmt --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked` at `5273324` → exit 0; the `test result:` lines sum to **947 passed / 0 failed / 5 ignored**. The 5th ignored test is `breaker_i5b.rs` `breaker_width_is_that_of_the_stdout_terminal_not_the_controlling_one`; follow-up B makes it a regression test (remove its `#[ignore]`).
- R1 so far: `git diff --quiet eeb6aeb..HEAD -- Cargo.lock '*Cargo.toml'` → exit 0 (follow-up B will change this, as approved).
- CI: none since `960cb0a` (billing); nothing pushed in session 11.

## Gotchas
- **The workflow can't run the follow-ups.** `rollcall-v1.js` filters items through a fixed `ORDER` list (I2…I4), so an `S11` section is dropped, and editing the script is blocked (self-modification). Drive S11 with direct Agent calls: Sonnet implementer (`model: "sonnet"`), then the breaker, then Fable as skeptic.
- **Workflow-reported AMBIGUOUS items arrive in its result, not as stops.** Read `ambiguous_for_author` and each item's `optional` after every run. The workflow continues past them, but the directive says to stop and ask.
- **Don't edit the main checkout while a workflow runs** (see Decisions).
- Python `re.sub` replacement strings turn `\n` into real newlines. Session 11 broke a Rust char literal that way; use plain `str.replace`.
- Carried over: breaker stalls (resume with an unchanged script); hostile `scan_h*` tests are slow in debug builds (long cargo timeouts); concurrent sessions may edit this branch (stage explicit paths); `git config handoff.auto false`; use a scratch `HOME` for manual runs; never print prompt text from `~/.claude` / `~/.codex`.
- Optional skeptic notes (non-blocking): the `StatusFilters` doc line sits above `ColorMode` in `cli.rs`, and `StatusFilters` has no doc of its own; the I5b-1..4 and I5d-6 tests use private fixtures, not `$F` (a skeptic re-ran them against `$F` and they held); new test files are in `crates/rollcall-cli/tests/*.rs`, not `tests/it/`.

## Suggested skills
- `gate-record-audit` before each skeptic round, `probe-hygiene` before mutation probes, `repo-debt-report` before push, `verify-ledger` before any merge-ready claim.

## Not started
- S11 follow-ups A–D, the range skeptic, push, CI, PR (directive above).
- **Open author questions (not blocking; ask once and record the answers):** (1) `check_root`'s refused-root suggestions (`root.rs` `Peek::admits`) can name an ignored repo. §4.3 says the peek applies the §4.2 skips, so the fix is to mirror the walk's `.chain(ignore)`, or add a spec note. (2) A linked worktree under an ignored dir whose main repo is outside the root is never discovered (the walk doesn't descend). §4.2's I3-12 assumption "stays listed" can't hold there; add a spec line.
- Tap prerequisites (create `ravi1395/homebrew-rollcall`, the `HOMEBREW_TAP_TOKEN` secret) and any release run are author actions, later.
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
