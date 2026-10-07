# HANDOFF — 2026-10-08 impl/copilot (~/Projects/rollcall), end of session 15 (spec stage done)

**Status: ACTIVE.** Supersedes session 14 (byte copy: `.handoffs/20261007-231419-superseded-HANDOFF.md`; v1 author decisions live in `.handoffs/20261006-235942-superseded-HANDOFF.md`).

**Next: author reviews spec rev 17, then the implement stage.** Spec rev 17 + `docs/superpowers/plans/tasks/copilot/ACCEPTANCE.md` are committed (`cad3ec1` draft, `f0c7ddb` review fixes); the Opus skeptic passed S1–S6 on the committed text. Branch `impl/copilot` exists (from `origin/main` `474cf23` + the session 13–15 docs commits). Plan (DRAFT): `docs/superpowers/plans/2026-10-07-rollcall-copilot-plan.md`. Research: `docs/research/2026-10-07-copilot-cli-sessions.md`. Workflow: `.claude/workflows/rollcall-copilot.js`.

1. **Author answers recorded** (session 15 interview, 2026-10-07): Q1–Q7 and R1 are filled in the section `## Author answers 2026-10-07 (Copilot plan)` at the bottom, committed. The workflow preflight parses that exact heading; it also stops on a dirty tree (outside `.claude/` and `graphify-out/`). Q1=(a) means P4 is not run; Q6 means P5 waits for the author's throwaway Copilot session.
1b. **Copilot CLI installed** (author, 2026-10-07): Homebrew cask `copilot-cli` → `/opt/homebrew/bin/copilot`, `GitHub Copilot CLI 1.0.93.` (Session 15 trap: the Homebrew *formula* `copilot` is AWS Copilot, 1.34.1, the same binary name; it had taken the symlink, so it was uninstalled and the cask reinstalled. Check with `copilot --version`.) As of session 15 `~/.copilot` still has no `session-state/` (`config.json ide logs mcp-config.json`), so no real session exists yet. The research's fixture shapes come from a 1.0.68 capture; 1.0.93 is close, but expect some drift. Key-set probe (re-run it after any Copilot upgrade) so spec rev 17 states confirmed shapes instead of *(assumption)*. It prints key names and event types only, never values or prompt text:
   ```bash
   for f in ~/.copilot/session-state/*/events.jsonl; do jq -cR 'fromjson? | [.type, (if (.data|type)=="object" then (.data|keys) else (.data|type) end)]' "$f"; done | sort | uniq -c
   for f in ~/.copilot/session-state/*/events.jsonl; do jq -cR 'fromjson? | keys' "$f"; done | sort | uniq -c
   ls ~/.copilot/session-state/*/ | sort | uniq -c
   ```
   Record the output in this handoff and pass it to the spec stage (it covers research R§9 unknowns 1, 2, 5 and 6). P5 still runs after implementation for the `rollcall doctor` counts.
   **Probe run, session 15** (2 real sessions, GitHub Copilot CLI 1.0.93; types, key names and non-prompt enum values only):
   - Store: `session-state/<uuid>/` holds `events.jsonl`, `workspace.yaml`, `checkpoints/`, `files/`, `research/`, `rewind-file-snapshots/`. Dir name == `session.start.data.sessionId`. No flat `*.jsonl`, no `history-session-state/`. A hidden sibling dir `.session-operation-locks/` holds `<uuid>.lock` files (not `inuse.<pid>.lock` inside the session dir, as the research had it). **Discovery must skip dot-dirs without counting `non_uuid`**, or every user gets a permanent `non_uuid: 1`.
   - Lines: 22 per file, 0 malformed, last byte `\n`. Envelope keys always `data,id,parentId,timestamp,type`; `timestamp` is an RFC 3339 UTC string with ms (`9999-99-99T99:99:99.999Z`). No `agentId`/`ephemeral` seen (no sub-agent ran).
   - `session.start.data`: `alreadyInUse,context,contextTier,copilotVersion,producer,remoteSteerable,sessionId,startTime,version` (`version` 1, `producer` "copilot-agent", `copilotVersion` "1.0.93"); `context` = `branch,cwd,gitRoot` (branch a string).
   - `user.message.data`: `content` (string), `delivery` ("idle"), `interactionId,messageId,parentAgentTaskId,supportedNativeDocumentMimeTypes,transformedContent,turnId`. **`parentAgentTaskId` is a non-empty 36-char id on a real human prompt: it is not a sub-agent marker.** Q4 exclusion keys on the envelope `agentId` only. No `isAutopilotContinuation` key seen (no autopilot ran).
   - Other types seen: `assistant.message/turn_start/turn_end`, `model.*` (8 kinds), `session.error` (`errorType` "mcp"), `session.info` (`infoType` "authentication"/"model"), `session.model_change`, `session.shutdown` (`shutdownType` "routine"), `session.usage_checkpoint`, `system.message`. `model.*` and `session.usage_checkpoint` are not in the plan's known-type list; they would count `unknown_type` on every session, so the spec should add them.
   - `workspace.yaml` top-level keys: `id cwd git_root branch client_name name user_named summary_count fork_count created_at updated_at remote_steerable mc_task_id mc_session_id mc_last_event_id` (Q1=(a), so unused).
   **Probe re-run, session 15, after the author resumed a session and used `/cd`** (3 sessions: 22, 29 and 3 lines; 0 malformed; all end in `\n`):
   - **Resume appends to the same `events.jsonl`:** `session.shutdown` (line 22) → `session.resume` (23) → `session.context_changed` ×2 (26, 27) → `session.shutdown` (29). One log can hold several `shutdown`s; `ended_at` = max `timestamp` still holds. `session.resume.data` = `alreadyInUse,context,contextTier,eventCount,eventsFileSizeBytes,reasoningEffort,resumeTime,selectedModel,sessionLimits`; its `context` = `branch,cwd,gitRoot`.
   - **`/cd` pending/settled pair confirmed:** `session.context_changed{cwd, pendingGitContext:true}` (no branch key), then `session.context_changed{branch,cwd,gitRoot}` (branch a string), both with a cwd different from the start. No `ephemeral` key on either.
   - **Zero-prompt session:** the 3-line session has `session.start` and `session.model_change` but no `user.message`. The spec must say whether a session with no human prompt is listed (follow the existing Claude/Codex §5.0 behaviour).
   - Locks: `.session-operation-locks/` holds 3 `<uuid>.lock` files, one per session, including the shut-down ones. Lock presence is not liveness (Q2 is out of scope anyway). No `inuse.*` files anywhere.
   - **Still *(assumption)*:** sub-agent envelope `agentId`, `isAutopilotContinuation`, rewind truncation.
   - **Exact on-disk `.type` strings** (session 16 re-check, type names only): `"session.context_changed"` ×2, `"session.resume"` ×1, `"session.shutdown"` ×3. The event name `session.context_changed` (with the `session.` prefix) is confirmed, not an assumption; earlier shorthand in this file wrote it without the prefix.
2. ~~Spec stage~~ **done** (see Done). The workflow now also has `{stage: "spec-review", since: "<commit before the draft>"}` to re-review a committed draft.
3. **Author review of spec rev 17** (`git diff 00cf969 f0c7ddb -- docs/superpowers CLAUDE.md`: spec §1/§3.1/§3.4/§5.3 (new)/§7/§8.1/§10/§12, `ACCEPTANCE.md` P1–P3 (34 criteria), CLAUDE.md Safety line). Decide these open items from the skeptic (non-blocking for S1–S6; each is a small spec edit or "leave it"):
   - a. **Sub-agent dirs:** if a sub-agent ever writes its own UUID dir under `session-state/`, §5.3 has no rule that keeps it out of session rows (marked *assumption*, no P2 criterion). Add a rule now, or wait for P5 data?
   - b. **Detached HEAD:** Claude maps branch `"HEAD"` to `null`; §5.3 does not say what happens to Copilot's value. Same rule?
   - c. **Non-RFC-3339 `timestamp`:** unspecified (skip the line? count it?).
   - d. §5.3 line ~381 says "`files` and `records` as for §5.1", but §5.1 defines neither; define them inline (claude.rs:307, :340-346 semantics).
   - e. ACCEPTANCE P3-3's "unit test on EnvPaths" cannot use `std::env::set_var` (R3); reword it to the child-process pattern (`rollcall-store` `fs.rs` `run_child`) or rely on its integration half.
   - f. Cosmetic: CLAUDE.md line 3 still says "Claude/Codex sessions"; §12.1 has no Copilot gating item (Copilot gating lives in ACCEPTANCE.md); P2-3/P2-5/P2-7 tag the `agentId` assumption inconsistently.
   - g. §12.2 now asks the author for a smoke `rollcall resume` on a real Copilot session (Q6 only covered doctor counts): OK?
   If any edit is wanted: make it (or have the main loop make it), commit, then `{stage: "spec-review", since: "485dfe6"}` to re-gate. Then record the approval verbatim under the answers section as `- Spec rev 17 approval: …`.
4. After approval: `args: {stage: "implement", specApproved: true}` (optional `items: ["P1", ...]`). Per phase, serially: tiered implementer tests-first → full gate → commit → Opus `breaker` (if `Has logic: yes`) → guard keeps test code only → tiered fixes → Opus `skeptical-reviewer` (≤ 3 rounds) → next phase; final Opus skeptic over the range. Stops at the first phase not accepted. P4 is not in ACCEPTANCE.md (Q1=a).
5. Main loop afterwards: audit `git log`/`git status`; P5 real-data check on the author's 3 real sessions (`COPILOT_HOME` = real store read-only, scratch cache; counts only, never prompt text), including `skipped.unknown_type == 0` and `skipped.malformed == 0` for copilot, and that the `/cd` session's cwd is the post-`/cd` dir (compare paths by equality, print only true/false); update this handoff; answer AMBIGUOUS breaker items with the author; **ask before pushing** `impl/copilot`.

## Done (verified)
- **Branch `impl/copilot` created** from `origin/main` `474cf23` (`git fetch` confirmed it is current) with `git cherry-pick origin/main..impl/v0` → `818ddef e99869b 3203a8e b8810bb 22bff5d` (the 5 session 13–14 docs commits, clean picks). **Upstream unset** (`switch -c … origin/main` had set it to track `origin/main`, so a bare `git push` would have targeted main); `git rev-parse impl/copilot@{upstream}` → `fatal: no upstream configured`. Push later with `git push -u origin impl/copilot`.
- **Workflow tiers** (author request 2026-10-07, committed with this handoff): `TIER = { P1: haiku/xhigh, P2: sonnet/high, P3: sonnet/high, P4: sonnet/high }` drives the implement agent and every fix (gate, breaker, review). A Haiku phase escalates to Sonnet high from fix round 2. Breaker and skeptic stay Opus; spec drafting stays Opus; mechanical agents (head, gate, commit, guard, answers/criteria loaders) stay Sonnet low. Evidence: wrapped `node --check` → exit 0; stubbed-hook run of `{stage:"implement", items:["P1"]}` reaches `implement:P1` as `haiku/xhigh` (the same run on the first draft failed `Cannot access 'impl' before initialization`: a local `const impl` shadowed the helper, so it is now `implOpts`); tier assertion script (`implOpts('P1')`=haiku/xhigh, `implOpts('P1',2)`=sonnet/high, `implOpts('P2')`/`implOpts('P4',3)`/unknown id = sonnet/high) → `tier ok`.
- **Spec stage run** (workflow `rollcall-copilot` `{stage:"spec"}`, run `wf_d835ff17-520`, 8 agents, ~32 min): Opus draft → skeptic r1 → revise → skeptic r2 (S1 unmet, S2 unverified) → revise → commit `cad3ec1`. **Flaw found:** that loop committed a revision no reviewer had seen. S2 was caused by the handoff's shorthand `context_changed`; the logs record `"session.context_changed"` (re-checked, type names only).
- **Workflow fixed** (`485dfe6`): the spec review loop always ends on a verdict (≤ 3 rounds) and returns `review_passed`; new stage `spec-review` (`args.since` required, validated as a sha). Evidence: stubbed-hook runs of spec (never-pass → `review:spec:3 > commit`; pass r2 → `review:spec:2 > commit`), spec-review (pass r1; never-pass ends `rereview:spec:3 > commit`; missing `since` → error), and implement (`implement:P1:haiku/xhigh`) all as expected.
- **Spec re-review** (`{stage:"spec-review", since:"00cf969"}`, run `wf_599a739b-dc7`, 8 agents, ~23 min): rereview r1 → revise → r2 → revise → **r3 pass, S1–S6 all met** → commit `f0c7ddb`. Diff since `00cf969`: `CLAUDE.md` 1 line, `ACCEPTANCE.md` +79 (new), spec +86/−21. The skeptic's open items are under Next step 3.
- Carried: PR https://github.com/ravi1395/rollcall/pull/6 merged (`4f102a7`); gate on `origin/main` `474cf23` → 973 passed / 0 failed / 4 ignored (session 13).

## In progress
- Nothing half-edited. `git status --short`: `?? graphify-out/` only (never commit `graphify-out/`). `impl/v0` untouched (local). `impl/copilot` is local and unpushed, with no upstream.

## Decisions + why
- **Tier by complexity, fixed in the script, not in ACCEPTANCE.md:** P1 is enum arms + schema enums (plan tags them mech) → Haiku xhigh. P2 (events.jsonl parser, incremental reads), P3 (two source lists, env, test sandbox; a miss reads the real `~/.copilot`) and P4 (resolve-time `TextRef`, hand-rolled reader) → Sonnet high. Escalation covers a Haiku phase that stalls.
- **Edited `rollcall-copilot.js`, no new workflow:** it already had the Opus breaker/skeptic loop, preflight and serial phases; only the implementer model changed.
- **Spec reviewed against the probe, not just the research:** where they disagree the probe wins (real 1.0.93 data); *assumption* marks remain only on `agentId`, `isAutopilotContinuation`, rewind, sub-agent placement and planner counting choices.
- **Optional carry-overs left out of the workflow:** all are non-blocking skeptic observations with no criteria; listed under Not started.
- Earlier (session 14): Copilot CLI only; reuse `Backing::LogFile`/`TextRef::LogLine`/`SourceMode::Jsonl`; phase cut keeps each merge green.

## Test state
- No code changed since `origin/main` `474cf23` → 973 passed / 0 failed / 4 ignored (ignored by design: `materialize_perf`, `materialize_demo`, `fs.rs::modes_child`, `fs.rs::lock_child`). Gate not re-run this session (docs/workflow only).
- CI: billing-blocked (every job 0 steps). Linux untested.

## Gotchas
- **Haiku `xhigh` effort is new for this repo's workflows:** if the runtime rejects it for Haiku, change `HAIKU` to `effort: 'high'` (one line).
- No real Copilot session on this machine; format criteria are *(assumption)* until Q6. Never print prompt text from `~/.copilot`; never read `~/.config/github-copilot/apps.json` (tokens).
- `"copilot"` is 7 chars (one more than `"claude"`): P1 checks TUI/plain column budgets.
- Two source lists must stay in sync: `scan.rs` `scan_sources` and the `output.rs` resolver list.
- Test sandbox `crates/rollcall-cli/tests/it/support.rs` must set `COPILOT_HOME` in P3.
- Local `main` is stale (`f7ada95`); always use `origin/main`.
- `ubuntu-latest` → Ubuntu 26 from 2026-10-19; consider pinning `ubuntu-24.04` (author decision, not asked).
- Carried: CI annotations via `gh api repos/ravi1395/rollcall/check-runs/<job id>/annotations`; scratch `HOME` for manual runs (macOS cache ignores `XDG_CACHE_HOME`); `git config handoff.auto false`; stage explicit paths.

## Suggested skills
- `brief`, then the author Q&A; `gate-record-audit` before approving spec rev 17; `verify-ledger` for P5; `ci-divergence-triage` once CI runs.

## Not started
- Copilot plan P0–P5 (all of it).
- Author actions: run one throwaway Copilot CLI session (GitHub Copilot CLI 1.0.93 installed; ideally in a scratch git repo, `cd` to a second dir mid-session, then `copilot --resume` it once so `session.resume` and `session.context_changed` get exercised; Q6, unblocks the key-set probe and P5); fix CI billing and re-run CI on `main`; create `ravi1395/homebrew-rollcall`, add `HOMEBREW_TAP_TOKEN`, tag a release.
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
