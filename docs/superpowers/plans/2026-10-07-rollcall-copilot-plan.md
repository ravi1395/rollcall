# Plan: Copilot CLI session source  (5 phases, branch/worktree: `impl/copilot` from `origin/main`)

**Status: DRAFT, not approved.** Author asked for Copilot CLI (not VS Code Copilot Chat, not the cloud agent) on 2026-10-07. Nothing below runs until the author approves this plan and answers the open questions; the v1 autonomy grant (2026-10-04) does not cover it.

- Research (sources, confidence levels, unknowns): `docs/research/2026-10-07-copilot-cli-sessions.md`. Format facts below cite its sections as R§n.
- Contract: spec `docs/superpowers/specs/2026-09-29-rollcall-design.md` rev 16 → rev 17 (Phase 0). Rules: `CLAUDE.md` R1–R4.
- Baseline: `origin/main` `474cf23` gate → 973 passed / 0 failed / 4 ignored (session 13).
- **No real Copilot session exists on this machine** (`~/.copilot` has `config.json ide logs mcp-config.json`, no `session-state/`). Every format-dependent criterion is marked *(assumption)*: its fixtures are rebuilt from GitHub's SDK event types plus a third-party 1.0.68 capture (R§3). Q6 asks how to get a real sample.

## What the code already gives us (verified this session)

- `SourceKind` (`crates/rollcall-core/src/lib.rs:69`) is a closed enum with serde `snake_case`; matched exhaustively in `crates/rollcall-cli/src/resume.rs:70,113`, `output.rs:59,714`, `tui.rs:454`, `crates/rollcall-store/src/lib.rs:288`.
- `Backing::LogFile { dev, inode, path }`, `TextRef::LogLine`, `SourceMode::Jsonl` fit `events.jsonl` with no new type.
- `crates/rollcall-sessions/src/jsonl.rs` has `open_log` (O_NOFOLLOW|O_NONBLOCK, fstat), `plan_read`, `head_fp`/`tail_fp`, `resolve_log_ref`, `file_exists_same`. `claude.rs` is the closest model (one JSONL per session, a dev/inode path index for `resolve`).
- Sources are built in two places: `scan.rs:395` `scan_sources` and `output.rs:~296` (the resolver list), both `[Box<dyn SessionSource>; 2]`.
- Env: `config.rs:42` `EnvPaths` (`codex_home: path("CODEX_HOME")`); test sandbox `crates/rollcall-cli/tests/it/support.rs:591-630` sets `CLAUDE_CONFIG_DIR`/`CODEX_HOME`.
- Schemas: `schema/status.json:77` and `schema/doctor.json:66` `"source": { "enum": ["claude", "codex"] }`. `schema/where_was_i.json` has no literal `source` (check whether it `$ref`s status's Session in P1).
- No YAML crate in `Cargo.lock`.

## R1 for this plan

Phase 1 owns the only structural edits: the `SourceKind::Copilot` variant (pinned interface), `pub mod copilot;` in `crates/rollcall-sessions/src/lib.rs` (`mod` list), and `EnvPaths.copilot_home` (P3, pinned struct). **Each needs author approval in the Phase 0 review.** No `Cargo.toml`/`Cargo.lock` edit in any phase; if Q1=(b) or Q2 needs a crate, stop and report.

## Phase 0 — Spec rev 17 + fixed acceptance criteria (docs only)
**Description:** Write the contract before code: a new §5.3 Copilot CLI source and the enum/env/resume deltas, plus a fixed criteria file the breaker and skeptic use. Author review gate at the end.
**Files:** `docs/superpowers/specs/2026-09-29-rollcall-design.md` (edit), `docs/superpowers/plans/tasks/copilot/ACCEPTANCE.md` (new), `CLAUDE.md` (mech: Safety line adds `~/.copilot`)
**Changes:**
- §5.3 Copilot CLI: root `$COPILOT_HOME` if set and non-empty, else `~/.copilot`; no XDG (R§1). Files `session-state/<id>/events.jsonl`. Worked records = the R§3 skeletons (shapes only, no prompt text). Field rules from P2 below.
- §3.4/§7: `source` enum gains `"copilot"`; schema_version per Q7. §8.1: resume = `copilot --resume <id>` (R§5). §10.1: test env adds `COPILOT_HOME`. §4.4: audit note if Q2 adds a lock-file probe (`symlink_metadata` only, never read).
- Each *(assumption)* from R§9 "Unknowns" is listed in the spec as such.
**Acceptance criteria:**
- [ ] Spec `**Status:**` line (line 3) opens with "Revision 17"; §5.3 exists and states root, files, id, cwd, branch, started/ended, human prompt, title, last_prompt, message_count, children and incremental rules.
- [ ] Every open question below is either answered in the spec (with the author's answer quoted in `HANDOFF.md`) or its phase stays marked gated.
- [ ] `ACCEPTANCE.md` lists P1–P3 criteria verbatim from this plan, plus the Conventions block from `tasks/v1/ACCEPTANCE.md` extended with `COPILOT_HOME`.
- [ ] `grep -n 'copilot' CLAUDE.md` shows the Safety line covers `~/.copilot`.
**Expected test outputs:** none (no executable change); `git diff --stat` touches only the three files. Breaker skipped (docs only).

## Phase 1 — `SourceKind::Copilot` end-to-end, inert
**Description:** Add the variant and every match arm, the schema enums and an empty `Copilot` source, without registering it in a scan. The tree is green and output is unchanged for users (dark launch).
**Files:** `crates/rollcall-core/src/lib.rs` (edit), `crates/rollcall-sessions/src/lib.rs` (edit), `crates/rollcall-sessions/src/copilot.rs` (new: `scan` returns `SourceMode::Absent` diagnostics, `resolve` returns `None`); (mech) `crates/rollcall-cli/src/resume.rs`, `crates/rollcall-cli/src/output.rs`, `crates/rollcall-cli/src/tui.rs`, `crates/rollcall-store/src/lib.rs`, `schema/status.json`, `schema/doctor.json`, `schema/examples/*` (new valid/invalid examples)
**Changes:**
- core: `SourceKind::Copilot`; the serde name test table at `lib.rs:999` gains `(Copilot, "copilot")`.
- arms: resume `("copilot", "--resume")`, resume-file field `"copilot"`, output hint `"copilot --resume"`, label `"copilot"`, store `source_name` `"copilot"`.
- schemas: enum gains `"copilot"`.
**Acceptance criteria:**
- [ ] `serde_json::to_value(SourceKind::Copilot)` == `"copilot"` and round-trips (unit test in core).
- [ ] `resume::plan` for a Copilot target with a fake `copilot` on `PATH` yields `Exec { args: ["--resume", <id>], cwd }` (unit test next to `resume.rs:455-463`); the resume file holds `<cwd>\0copilot\0<id>\0`.
- [ ] A `StatusDoc` and a `DoctorDoc` with `source: "copilot"` validate against the schemas; `"copilt"` is rejected (new `schema/examples` pair, picked up by the existing example-validation test).
- [ ] Store round-trip: a Copilot session written and read back via `SqliteIndex` is equal (unit test in `rollcall-store`); no `user_version` bump (old caches simply have no copilot rows).
- [ ] TUI and plain-status row with a Copilot session render within the existing column budget (unit/snapshot test with a `copilot` session; `"copilot"` is one char longer than `"claude"`).
- [ ] `no_agent.rs:56` still asserts `[("claude","absent"),("codex","absent")]` (Copilot not registered yet).
- [ ] R1: `git diff --quiet origin/main -- Cargo.lock '*Cargo.toml'` exits 0.
**Expected test outputs:** R2 gate → 973 + N passed (N ≈ 6 new), 0 failed, 4 ignored.

## Phase 2 — `copilot.rs` parser and incremental reads (dark)
**Description:** Implement §5.3 extraction over `events.jsonl` with the existing `jsonl.rs` helpers, unit-tested on hand-written fixtures. Still not registered in `scan_sources`.
**Files:** `crates/rollcall-sessions/src/copilot.rs` (edit), `crates/rollcall-sessions/src/lib.rs` (edit: `Copilot::scan/resolve` delegate)
**Changes / rules** (each *(assumption)* until a real sample confirms, R§3):
- Discovery: entries of `<root>/session-state/` that are real directories (not symlinks) holding a regular `events.jsonl`. Dir name is the session id and must match the §5.0 UUID regex, else skipped and counted `non_uuid` [Q5]. `history-session-state/` per [Q3].
- Envelope: `{type, data, id, timestamp, parentId, agentId?, ephemeral?}`. Lines with an envelope `agentId` are sub-agent events: not human prompts, never move `cwd` [Q4].
- `cwd`/`branch`: latest of `session.start.data.context`, `session.resume.data.context`, `session.context_changed.data` where `pendingGitContext` is not `true`; `branch` from the same object, missing → `null`.
- `started_at`/`ended_at`: min/max envelope `timestamp`.
- Human prompt: `user.message` without `agentId` [and per Q4 without `isAutopilotContinuation: true`], `data.content` string through the §5.0 filter. `message_count` counts them; `last_prompt` = latest.
- Title: per [Q1]. If Q1=(a): first human prompt, `TitleKind::Prompt`. If Q1=(b): Phase 4.
- Known types: `session.*`, `user.*`, `assistant.*`, `tool.*`, `subagent.*`, `hook.*`, `skill.*`, `permission.*`, `system.*`, `abort`; anything else counted `unknown_type`; malformed and > 1 MiB lines per §5.0.
- Incremental: `jsonl::plan_read`; rewind that shrinks the file (R§7) is caught by size < offset or `tail_fp` → full re-read.
- Never read or log `content`, `transformedContent`, `arguments`, `result` except `user.message.data.content` through `TextRef` (CLAUDE.md Safety).
**Acceptance criteria:**
- [ ] Fixture start(cwd `/A`, branch `main`) → session cwd `/A`, branch `main`; + `context_changed{cwd:/B, pendingGitContext:true}` → still `/A`; + settled `context_changed{cwd:/B, branch:"dev"}` → `/B`, `dev`; + `session.resume{context:{cwd:/C}}` → `/C`, branch `null`.
- [ ] `user.message` with `agentId` set is not counted; two plain `user.message` lines → `message_count` 2, `last_prompt` resolves to the second, title (Q1=a) resolves to the first.
- [ ] A `user.message` whose content starts with `<` contributes no prompt (§5.0).
- [ ] Dir `not-a-uuid/events.jsonl` → no session, `skipped.non_uuid` 1. A symlinked session dir and a symlinked `events.jsonl` → no session.
- [ ] Incremental equivalence: for every split point of a 12-line fixture, scan(prefix) then scan(full) yields the same `Session` as scan(full) from empty state (property-style loop like the Codex rev-14 test).
- [ ] Truncation: after scan, truncate the file to its first 3 lines → rescan rebuilds from 0 and `message_count` drops; resolving the old `last_prompt` ref returns `None`.
- [ ] Partial trailing line (no `\n`) is not consumed; after the line completes, the next scan picks it up.
- [ ] A line `{"type":"made.up","data":{}}` increments `unknown_type`; a 1 MiB + 1 line increments `oversize`; `{` increments `malformed`.
- [ ] No test output, note or error string contains the fixture prompt marker `PROMPT-MARKER` except via `resolve` (grep assertion over `SourceScan.notes` and `Debug` of diagnostics).
**Expected test outputs:** `cargo test -p rollcall-sessions --locked copilot` → ≥ 10 tests, 0 failed; R2 gate green.

## Phase 3 — Register the source and wire `COPILOT_HOME`
**Description:** Turn the source on: env var, both source lists, doctor, MCP text, the test sandbox, and one end-to-end status test.
**Files:** `crates/rollcall-cli/src/config.rs` (edit: `copilot_home: path("COPILOT_HOME")`), `crates/rollcall-cli/src/scan.rs` (edit: array of 3, default `$HOME/.copilot`), `crates/rollcall-cli/tests/it/sessions.rs` (edit: Copilot world + tests); (mech) `crates/rollcall-cli/src/output.rs` (resolver list), `crates/rollcall-cli/src/mcp.rs` (description text), `crates/rollcall-cli/tests/it/support.rs` (sandbox `COPILOT_HOME`), `crates/rollcall-cli/tests/it/no_agent.rs`, `tests/fixtures/MANIFEST.toml`
**Acceptance criteria:**
- [ ] `rollcall doctor --json <empty root>` in the sandbox lists sources `[claude, codex, copilot]`, copilot `mode: "absent"` (`no_agent.rs` updated).
- [ ] With `COPILOT_HOME=<fixture>` holding one session whose cwd is worktree `w` under the root: `rollcall status <root> --json` shows `w.last_session.source == "copilot"`, the fixture id, `title_kind == "prompt"` (Q1=a); `doctor` shows copilot `mode: "jsonl"`, `sessions: 1`, `matched: 1`.
- [ ] `COPILOT_HOME` empty string is treated as unset (falls back to `$HOME/.copilot`), same as `CODEX_HOME`.
- [ ] Sandbox: every test command sets `COPILOT_HOME` to a scratch dir (grep `support.rs`); no test reads the real `~/.copilot`.
- [ ] `rollcall resume` on the Copilot session with `ROLLCALL_RESUME_FILE` writes `…\0copilot\0<id>\0`.
- [ ] Existing Claude/Codex integration tests pass unchanged except the doctor source lists.
**Expected test outputs:** R2 gate → baseline + P1 + P2 + ≥ 4 new, 0 failed, 4 ignored.

## Phase 4 — [gated on Q1, Q2] Title from `workspace.yaml` and/or liveness
**Description:** Only if the author picks them. Written per option so the plan is complete.
**Files:** if Q1=(b): `crates/rollcall-core/src/lib.rs` (new `TextRef` variant that re-reads one key of `session-state/<id>/workspace.yaml` at resolve time, like `CodexThread` re-reads a row; R1 exception), `crates/rollcall-sessions/src/copilot.rs`. If Q2=yes: spec-first design, its own plan (the `Session` model has no live/busy field today).
**Acceptance criteria:**
- [ ] If Q1=(b): a fixture `workspace.yaml` with `name: Fix login` and `user_named: true` → title `Fix login`, kind `custom`; `user_named: false` → kind `ai`; missing or unparseable file → falls back to the first prompt; the parser is ≤ 1 top-level `key: scalar` line reader (no YAML crate), and a quoted or multi-line `name` → fallback, never a partial string.
- [ ] If Q1≠(b) and Q2≠yes: phase not run.
**Expected test outputs:** per chosen option, `cargo test -p rollcall-sessions copilot_title` ≥ 4 pass.

## Phase 5 — [gated on Q6] Real-data check
**Description:** Fixtures are reconstructed; the claims rest on real data shape, so verify on a real `~/.copilot` before calling it done (verify-ledger).
**Acceptance criteria:**
- [ ] With a scratch cache and `COPILOT_HOME` = the real store (read-only), `rollcall doctor --json` → copilot `mode: "jsonl"`, `skipped.malformed == 0`, `sessions ≥ 1`, and `rollcall status --json | jq '[.. | .last_session? | select(.source=="copilot")] | length'` ≥ 1. Only counts are printed; never titles or prompts.
- [ ] Each research unknown (R§9 items 1, 2, 5, 6) is resolved in the spec as confirmed or changed, from the real file's key sets (`jq 'keys'`, no values).
**Expected test outputs:** commands above, counts recorded in `HANDOFF.md`.

## Edge cases considered
- **Missing/invalid input:** no `~/.copilot` → `absent`; no `session-state/` → `absent`; empty `events.jsonl` → no session, not an error; dir without `events.jsonl` → ignored (not counted: may be a session being created).
- **Hostile FS:** symlinked session dir or log → skipped (O_NOFOLLOW + dir type from `symlink_metadata`); FIFO named `events.jsonl` → `open_log` refuses (O_NONBLOCK, fstat regular).
- **Concurrency:** Copilot appends while we read → read only up to size at open (§5.0); partial tail not consumed; rewind/truncate → full re-read; `workspace.yaml` rewritten in place (Q1=b) → a torn read falls back.
- **Boundaries:** non-UUID ids (SDK clients) per Q5; 1 MiB line cap; `context_changed` pending/settled pair; session outside any root → `outside_root` (existing §6.5).
- **Privacy:** prompts are plaintext in Copilot logs (R§6); only `TextRef` pointers are stored (§6.2); tests use a marker string and assert it never leaks.
- **Version drift:** format changed at 0.0.342 and ~1.0.6 (R§2); `session.start.data.version` is recorded nowhere yet. P2 counts unknown types rather than failing, so a format change degrades to counts in `doctor`.

## Open questions (answer before implementation)
1. **Title.** `session.title_changed` is never written to disk (R§3). Options: (a) first human prompt only (`prompt` kind; zero new machinery); (b) read `workspace.yaml: name` at resolve time via a new `TextRef` variant (undocumented file, rewritten in place, R1 exception in core, Phase 4); (c) read `session-store.db` `sessions.summary` (column names from one third-party paraphrase only).
2. **Liveness.** `inuse.<pid>.lock` shows a running session (third-party only, stale locks happen, R§4). rollcall shows no live/busy state for Claude or Codex today. In scope now, later, or never?
3. **Legacy stores.** `history-session-state/` (pre-0.0.342) and any flat `session-state/<id>.jsonl`: skip silently, skip and count in `doctor`, or parse?
4. **Sub-agent and autopilot.** Exclude envelope-`agentId` events (and `subagent.started` → `excluded_children`)? Exclude `user.message` with `isAutopilotContinuation: true` from prompts and `message_count`?
5. **Non-UUID ids.** SDK clients may name sessions freely (R§2). Keep the §5.0 rule (skip + count `non_uuid`), or allow any safe dir name for Copilot?
6. **Real sample.** No Copilot CLI sessions exist here. Options: you install Copilot CLI and run one throwaway session (then Phase 5 runs on it); or ship on fixtures and leave Phase 5 open in the verify ledger.
7. **`schema_version`.** Adding `"copilot"` to the `source` enums breaks a consumer that validates strictly. Bump `StatusDoc`/`DoctorDoc`/`where_was_i` `schema_version` 1 → 2, or treat an added enum value as compatible (spec note)?
