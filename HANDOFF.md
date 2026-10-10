# HANDOFF — 2026-10-09 impl/copilot (~/Projects/rollcall), end of session 16 (implementation done)

**Status: ACTIVE.** Supersedes session 15 (byte copy: `.handoffs/20261009-120750-consumed-HANDOFF.md`; Copilot probe results, step 1b, live there).

## Next step
1. **Review and merge the PR** `impl/copilot` → `main` (`gh pr view impl/copilot --web`). CI is billing-blocked, so its checks will not run.
2. Optional (skeptic notes, not acceptance failures): re-add a one-line `:60 → :59` pin to `copilot::tests::the_envelope_timestamp_is_strict_rfc_3339` (the ignored probe was deleted per answer (l), so a jiff bump that stops clamping has no test); `copilot.rs:299-302` counts a log that turned into a FIFO/symlink as `vanished` while still adding to `files` (same as `claude.rs`).

## Done (verified)
- **Implement stage** (workflow `rollcall-copilot` `{stage:"implement", specApproved:true}`, run `wf_b1032c95-609`, 32 agents, ~113 min): P1 accepted (r2), P2 accepted (r1), P3 accepted (r1). Commits `c760836 762c9fe 4edc032 93d58e4 2966977 33a669e 0fcc758`. Its final skeptic failed only P1-12/P3-11 (6 ignored vs 4: two AMBIGUOUS breaker tests left `#[ignore]`d).
- **Author answers (h)–(l)** (below) applied: `32ab23a` (strict RFC 3339 Copilot timestamps, non-empty-string `agentId`, store-root tests for all three sources; spec §5.0/§5.3 + Status, ACCEPTANCE P2-19(e)/P2-21/P3-12). Breaker r3 (Fable) on `32ab23a`: 3 BROKEN (fraction > 9 digits rejected; offset hour 24/25 accepted) fixed in `df310da`, tests kept in `crates/rollcall-sessions/tests/breaker_copilot_r3.rs`.
- **Final gate (Fable skeptic) PASS** on `d943c95..df310da`: P1-1..12, P2-1..21, P3-1..12 all met; mutation probes on the offset-hour bound, fraction cut and `has_agent` each turned a named test red; Cargo files unchanged; only R1-approved structural edits.
- **R2 gate** (main loop, `df310da`): `cargo fmt --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked` → exit 0, 1077 passed / 0 failed / 4 ignored.
- **§12.2 author smoke passed** (2026-10-10): in a Terminal panel tab, `PATH=<repo>/target/debug:$PATH`, `eval "$(rollcall init zsh)"`, `rc ~/Projects/scrath`; the TUI showed `scrath/main` with last session `copilot b254628…` (the `/cd` session; 2 older), `R` resumed it in `copilot`, author confirmed "it resumed". Note: `rc` runs `command rollcall`, and `~/.cargo/bin/rollcall` is an older claude/codex-only build; until it is reinstalled (`cargo install --path crates/rollcall-cli --locked --force`) it shows `quiet —` for Copilot-only worktrees. `R` resumes the selected worktree's last session only.
- **Pushed** `impl/copilot` to `origin` by the author (2026-10-09), upstream `origin/impl/copilot`.
- **P5 real data** (main loop; real `~/.copilot` read-only via `COPILOT_HOME`, scratch HOME/XDG/CLAUDE/CODEX dirs, counts only, no prompt text): copilot doctor `{mode jsonl, files 3, records 54, sessions 3, malformed 0, unknown_type 0, oversize 0, non_uuid 0, excluded_children 0}` (records = 22+29+3 lines), identical after `df310da`; `/cd` session via `status --json` on its git root: found 1, cwd == post-`/cd` settled cwd **true**, branch equal **true**, resume plan present, `title_kind` prompt, `message_count` 1.

## In progress
- Nothing. `git status --short` after this handoff's commit: `?? graphify-out/` only (never commit it).

## Decisions + why
- Fixes after the workflow ran in the main loop (one Sonnet worker, Fable breaker + skeptic), not a workflow resume: the implement stage had finished and would only replay cached agents.
- Answer (c) (timestamp "like Claude") is superseded by (h) for strictness; the Claude reader still uses jiff `parse`.
- jiff 0.2.37 has no strict RFC 3339 parser, so `copilot.rs` `is_rfc3339` checks the shape and `nanos` cuts fraction digits past the ninth before jiff parses.
- P3-12(d) (case-differing ids) skips on case-insensitive APFS; verified twice on a scratch case-sensitive APFS image (3 passed each time). Linux still never run.

## Test state
- `df310da` → 1077 passed / 0 failed / 4 ignored (by design: `materialize_perf`, `materialize_demo`, `fs.rs::modes_child`, `fs.rs::lock_child`).
- CI billing-blocked (every job 0 steps).

## Gotchas
- Installed `~/.cargo/bin/rollcall` predates Copilot support; reinstall after merge.
- Local `main` is stale (`f7ada95`); use `origin/main`.
- Homebrew formula `copilot` is AWS Copilot (same binary name); GitHub Copilot CLI is the cask `copilot-cli`.
- Never print prompt text from `~/.copilot`; never read `~/.config/github-copilot/apps.json` (tokens).
- The bash safety check blocks `rm -rf "$VAR/home"`-style paths; use fresh scratch dirs.
- Haiku `xhigh` worked for P1 in the workflow.

## Suggested skills
- `finishing-a-development-branch` / PR flow after push approval; `verify-ledger` for the §12.2 smoke; `ci-divergence-triage` once CI runs.

## Not started
- Copilot plan P4 (not run, Q1=(a)).
- Author actions: fix CI billing and re-run CI on `main`; create `ravi1395/homebrew-rollcall`, add `HOMEBREW_TAP_TOKEN`, tag a release; consider pinning `ubuntu-24.04` (`ubuntu-latest` → 26 from 2026-10-19).
- Optional carry-overs (non-blocking): `lock_when_free` fork-race window; full-F MCP reply has 0 `recent_sessions`; `scripts/probe-gix-p15b/sample.py` prints `first>320MiB`; non-UTF-8 missing-worktree lock key collides with its twin; macOS cache ignores `XDG_CACHE_HOME`.

## Author answers 2026-10-07 (Copilot plan)
<!-- Filled in session 15 (2026-10-07) from the author's interview choices; quoted labels are the options the author picked. The rollcall-copilot workflow preflight parses this section. -->
- Q1 (title): "(a) First prompt" — first human prompt, `TitleKind::Prompt`; zero new machinery. Phase 4 not run.
- Q2 (liveness): "Never / later" — out of scope; matches Claude/Codex (no live state).
- Q3 (legacy stores): "Skip silently" — ignore `history-session-state/` and flat `session-state/<id>.jsonl`; no doctor count.
- Q4 (sub-agent / autopilot): "Exclude both" — envelope-`agentId` events and `user.message` with `isAutopilotContinuation: true` are not human prompts and not in `message_count`; `subagent.started` → `excluded_children`.
- Q5 (non-UUID ids): "Keep §5.0 rule" — skip non-UUID session dirs, count `non_uuid`.
- Q6 (real sample): "I'll install + run one" — author installs Copilot CLI and runs one throwaway session; P5 runs in the main loop on it (read-only, counts only, never prompt text). Sample taken session 15 (3 sessions incl. one resumed with `/cd`, CLI 1.0.93); the probe results under step 1b are binding format facts for spec rev 17: skip dot-dirs such as `.session-operation-locks/` uncounted; `parentAgentTaskId` is not a sub-agent marker (exclude on envelope `agentId` only); add `model.*` and `session.usage_checkpoint` to the known types; `session.start`/`session.resume` `.data.context` and settled `session.context_changed.data` = `branch,cwd,gitRoot` are confirmed, as is the `pendingGitContext:true` pending event; resume appends to the same log after a `session.shutdown`; a session can have zero `user.message` (spec decides per §5.0); `agentId`, `isAutopilotContinuation` and rewind truncation remain *(assumption)*.
- Q7 (schema_version): "Additive = compatible" — keep `schema_version` 1; spec note that new enum values are compatible and consumers must tolerate unknown sources.
- R1 approval (SourceKind::Copilot, pub mod copilot, EnvPaths.copilot_home): "Approve all three" — approved. Any other structural edit still stops and reports.
- Spec rev 17 review decisions (author interview, 2026-10-08; applied in the commit after `fc7c478`):
  - (a) Sub-agent own `session-state/<uuid>/`: "Wait for real data" — stays *assumption*, no rule until P5 or a later probe shows one.
  - (b) Detached HEAD: "Same as Claude" — branch `"HEAD"` → `null` (§5.1); new criterion P2-18.
  - (c) Bad envelope `timestamp`: "Malformed, like Claude" — non-string or non-RFC-3339 → malformed; absent/`null` → line used without a timestamp (`claude.rs:114-117`); new criterion P2-19. *Strictness superseded by (h) 2026-10-09: Copilot timestamps are strict RFC 3339, the Claude reader keeps jiff `parse`.*
  - (d) `files`/`records`: "Define inline, Claude semantics" — §5.3 doctor bullet; new criterion P2-20.
  - (e) P3-3: "Integration test only" — unit-test clause dropped (R3 forbids `set_var`). Note: no existing test covers `CODEX_HOME=""`; P3-3 is the first such test.
  - (f) Cosmetic: "CLAUDE.md line 3" only — now "Claude/Codex/Copilot sessions". Not taken: a §12.1 Copilot gating item; consistent `agentId` (assumption) tags.
  - (g) §12.2 manual `rollcall resume` smoke on a real Copilot session: "Keep it".
- Spec rev 17 approval (2026-10-08, verbatim): "approved, handoff the execution of the whole workflow to the next session please" — approves the spec and ACCEPTANCE.md at `88a464a`; the implement stage may run with `specApproved: true`.
- Implement-stage AMBIGUOUS answers (author interview, 2026-10-09; run `wf_b1032c95-609`):
  - (h) Envelope `timestamp` edge strings: "Strict RFC 3339" — Copilot only (Claude stays on jiff `parse`): `2026-10-07T18:04Z`, `20261007T180400Z`, `2026-10-07T18:04:00Z[UTC]` are malformed; un-ignore `breaker_copilot_ambiguous_non_rfc3339_timestamp_is_malformed`.
  - (i) Envelope `agentId: ""`: "Non-empty only" — only a non-empty string marks sub-agent work; `""` and `null` are human; un-ignore `breaker_copilot_ambiguous_empty_agent_id_is_not_an_agent`.
  - (j) Store-root edges, all three sources: "Specify + test now", each as current behaviour: UUID dirs differing only by case → two ids (id = dir name as written); relative `COPILOT_HOME`/`CODEX_HOME`/`CLAUDE_CONFIG_DIR` → resolved against the process cwd; a symlinked store root (env dir or its `session-state`/`sessions`/`projects` dir) is followed, entries inside stay never-followed.
  - (l) Breaker r3 on `32ab23a` (author 2026-10-09): second `60` → "Accept, clamp" (read as `:59`; the ignored test was deleted); whitespace-only `agentId` → "Keep literal" (sub-agent work). BROKEN items fixed in code: fraction digits past the ninth are cut (were malformed), offset hour > 23 is malformed (was accepted).
  - (k) P1 breaker item (cached Copilot row before P3): resolved by the P1 review fix `4edc032` (fallback lookup); P3 restored the hard `expect` once the list held three sources. Recorded, no action.
