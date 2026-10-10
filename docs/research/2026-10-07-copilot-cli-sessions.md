# GitHub Copilot CLI: on-disk session persistence (research for rollcall)

Researched 2026-10-07 from public sources only. Current CLI at research time: **1.0.92 (2026-10-05)**.
No Copilot software was installed or run, and no local `~/.copilot` was read. Public files were fetched with curl/WebFetch into this scratchpad and inspected with python3.

Confidence labels:
- **docs**: stated in docs.github.com.
- **source**: GitHub-authored source (the `github/copilot-sdk` generated event types, the `github/copilot-cli` changelog), or a GitHub issue.
- **3p-observed**: a third party's first-hand capture (mainly empathic/toolpath at CLI 1.0.67/1.0.68).
- **inferred**: my own reasoning from the above.

Primary sources used:
- Config dir reference: https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference
- Command reference: https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference
- Session data concepts: https://docs.github.com/en/copilot/concepts/agents/copilot-cli/chronicle (and `/concepts/security-governance-and-network-settings/session-data`)
- Rewind: https://docs.github.com/en/copilot/concepts/agents/copilot-cli/cancel-and-roll-back
- CLI best practices: https://docs.github.com/en/enterprise-cloud@latest/copilot/how-tos/copilot-cli/cli-best-practices
- Hooks reference: https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-hooks-reference
- SDK generated event types, authored by GitHub: https://raw.githubusercontent.com/github/copilot-sdk/main/nodejs/src/generated/session-events.ts
- SDK types (`SessionMetadata`, `SessionContext`, `baseDirectory`): https://raw.githubusercontent.com/github/copilot-sdk/main/nodejs/src/types.ts
- CLI changelog: https://raw.githubusercontent.com/github/copilot-cli/main/changelog.md
- Issues: https://github.com/github/copilot-cli/issues/3551 (formalize events.jsonl; v1.0.54, 2026-05-28, no maintainer reply), #2012 (U+2028 broke per-line JSON.parse)
- Third-party corroboration: empathic/toolpath format docs (https://github.com/empathic/toolpath/tree/main/docs/agents/formats/copilot-cli) and its public fixture `crates/toolpath-copilot/tests/fixtures/real-session.jsonl` (CLI 1.0.68); docs.rs/toolpath-copilot; Narnia (https://www.devleader.ca/projects/narnia/process-diagnostics); gfarb/copilot-session-manager; Shmuelie.Copilot PowerShell module; skills.sh dzianisv/github-copilot-sessions.

---

## 1. Root directory

| Item | Finding | Confidence |
|---|---|---|
| Default root | `~/.copilot` (`$HOME/.copilot`), the same on macOS and Linux. Windows uses `$HOME\.copilot`. | **docs**: config-dir ref: "By default, this directory is `~/.copilot`". The command ref env table says `COPILOT_HOME`: "Override the configuration and state directory. Default: `$HOME/.copilot`." |
| `COPILOT_HOME` | Replaces the **entire** root, so sessions live at `$COPILOT_HOME/session-state/`. | **docs**: "COPILOT_HOME replaces the entire ~/.copilot path." The SDK's `baseDirectory` option "Sets the COPILOT_HOME environment variable on the spawned runtime. When not set, the runtime defaults to ~/.copilot." (**source**) |
| `--config-dir=DIR` | Deprecated flag that does the same job. Setting it per invocation means a session's store might not be under `$COPILOT_HOME` at all. | **docs**: "This option is deprecated. Use `COPILOT_HOME` instead." |
| XDG | **Not honored for state.** The docs say: "Previous XDG-based configuration locations are migrated to `~/.copilot` at startup when `COPILOT_HOME` isn't set." Changelog 0.0.421 (2026-03-03): "Use consistent ~/.copilot/pkg path for auto-update instead of XDG_STATE_HOME". A blog (inventivehq) claims `$XDG_CONFIG_HOME/copilot/`. That describes old builds and has been superseded. | **docs** + **source** |
| Cache dir (not session data) | macOS `~/Library/Caches/copilot`. Linux `$XDG_CACHE_HOME/copilot` or `~/.cache/copilot`. Override with `COPILOT_CACHE_HOME`. `COPILOT_HOME` does not affect it. | **docs** |
| Per-OS | macOS and Linux differ only in the cache dir. Session paths are identical. | **docs** |

**Rollcall resolution rule (inferred):** use `$COPILOT_HOME` if it is set and non-empty, otherwise `$HOME/.copilot`. Skip XDG. As an optional secondary root, also check `history-session-state/` (see §2).

## 2. Session store layout

```
$COPILOT_HOME/                      (default ~/.copilot)
  session-state/
    <session-id>/                   # dir name == session id
      events.jsonl                  # event log, source of truth          [docs]
      workspace.yaml                # session metadata                     [docs: "workspace.yaml: metadata"; fields 3p-observed]
      plan.md                       # if a plan was made                   [docs]
      checkpoints/                  # compaction history                   [docs]
      files/                        # session artifacts                    [docs]
      session.db                    # per-session SQLite                   [3p-observed 1.0.67]
      rewind-snapshots/             # file snapshots for rewind            [3p-observed 1.0.67]
      research/                     #                                      [3p-observed 1.0.67]
      inuse.<pid>.lock              # present while a process holds it     [3p-observed; see §4]
  session-store.db (+ -wal, -shm)   # global SQLite index                  [docs; WAL 3p-observed]
  history-session-state/            # legacy, pre-0.0.342                  [source]
  logs/process-{timestamp}-{pid}.log                                       [docs]
  command-history-state(.json)      # docs say dir, 3p observed a .json file
```

- **docs** (config-dir ref, `session-state/`): "Contains session history data, organized by session ID in subdirectories. Each session directory stores an event log (`events.jsonl`) and workspace artifacts (plans, checkpoints, tracked files). This data enables session resume (`--resume` or `--continue`)."
- **docs** (best practices): `~/.copilot/session-state/{session-id}/` contains `events.jsonl`, `workspace.yaml`, `plan.md`, `checkpoints/`, `files/`.
- **Session ID format.** CLI sessions use UUIDs (**3p-observed**). SDK clients can choose their own IDs; the SDK docs' example is `session-state/user-123-task-456/` (**docs**, SDK session-persistence page). **Rollcall must not assume the directory name is a UUID.**

### Layout history

| Version / date | Change | Source |
|---|---|---|
| ≤ 0.0.341 | Sessions in `~/.copilot/history-session-state/` (older format). | changelog |
| **0.0.342, 2025-10-15** | "New session logging format… New sessions are stored in `~/.copilot/session-state`. Legacy sessions are stored in `~/.copilot/history-session-state` -- these will be migrated to the new format & location as you resume them from `copilot --resume`." | **source** (changelog) |
| 0.0.422, 2026-03-05 | "Session usage metrics… are now persisted to events.jsonl after each session ends" (`session.shutdown` payload). | **source** |
| 1.0.7, 2026-03-17 | "Session resume no longer fails with 'Session file is corrupted' for sessions created before 1.0.6". | **source**; that a format change happened around 1.0.6 is **inferred** |
| 1.0.10 / 1.0.79 / 1.0.81 | Multiple concurrent sessions per process (experimental in 1.0.10, Sessions sidebar in 1.0.76/1.0.79). Interrupted-session restore arrived in 1.0.81 and is opt-in via `COPILOT_ENABLE_INTERRUPTED_SESSION_RESTORE=1`. | **source** / **docs** |

I found no primary source showing whether the earliest `session-state/` builds used flat `<id>.jsonl` files before moving to `<id>/events.jsonl`. **Unknown.** Rollcall should tolerate both, or treat flat files as legacy and skip them.

## 3. File format: `events.jsonl`

It is genuine JSONL: one `JSON.parse` per line (**source**: issue #2012, where raw U+2028/U+2029 broke resume, fixed in 1.0.41 per the changelog "Session files containing Unicode line separator characters load correctly"). The schema is **officially undocumented** (issue #3551 asks GitHub to formalize it). However, GitHub's own SDK ships generated TypeScript types for every event, and the toolpath 1.0.68 fixture matches those types field-for-field. Treat the SDK types as the de facto schema (**source**, strong).

### Envelope (every line), **source** + **3p-observed**

```
{ "type": "<dotted.name>", "data": {...}, "id": "<uuid v4>",
  "timestamp": "<ISO 8601, ms, Z>", "parentId": "<prev event id>|null",
  "agentId"?: "<sub-agent instance>",    // absent for main agent
  "ephemeral"?: bool }                   // "When true, the event is transient and not persisted to the session event log on disk"
```

`parentId` is documented as "ID of the chronologically preceding event in the session, forming a linked chain. Null for the first event." In the fixture the chain was strictly linear.

### Field locations rollcall needs

| Need | Where | Confidence |
|---|---|---|
| Session ID | Directory name. Also `session.start.data.sessionId` and `workspace.yaml: id`. | source / 3p-observed |
| Event-format schema version | `session.start.data.version` (int; `1` in the 1.0.68 fixture). CLI version is in `data.copilotVersion`. `data.producer` = `"copilot-agent"`. | source / 3p-observed |
| Start time | `session.start.data.startTime` (ISO). `session.shutdown.data.sessionStartTime` is **epoch ms**. | source |
| Per-event time | Envelope `timestamp`. | source |
| **cwd at start** | `session.start.data.context.cwd`. The `context` object is `WorkingDirectoryContext {cwd, gitRoot?, repository? ("owner/name"), repositoryHost?, hostType? ("github"\|"ado"), branch?, headCommit?, baseCommit?, pendingGitContext?}`. Outside a git repo only `cwd` is present (fixture: `{"cwd":"/tmp/…"}`). | source / 3p-observed |
| **cwd on resume** | `session.resume.data.context` has the same shape, plus `resumeTime`, `eventCount`, `alreadyInUse?`, `sessionWasActive?`. | source |
| **cwd on `/cd`** | `session.context_changed`, whose `data` **is** a `WorkingDirectoryContext`. Its `ephemeral?` is optional, not forced true, so it **is persisted**. On a directory change the CLI first emits a preliminary event with `pendingGitContext: true`, then "A settled follow-up event (enriched with git context, or cwd-only for a non-repository) is always emitted afterward". **Use the latest non-pending `context_changed`, falling back to `resume`, then `start`.** | source (persistence by schema; not observed on disk) |
| Git repo / branch | Same `context` (`gitRoot`, `repository`, `branch`, `headCommit`). Changelog 1.0.52: "`copilot --continue`… refreshes the saved branch and git context". | source |
| Worktrees | `copilot --worktree` creates or reuses `<repo>.worktrees/<name>` and starts the session there, so `cwd`/`gitRoot` point into the worktree (inferred). | docs (flag) |
| Model | `session.start.data.selectedModel?`, then `session.model_change.data.newModel` (fixture: `"auto"` right after start), and `assistant.message.data.model` per message. | source / 3p-observed |
| User prompt | `user.message.data.content` (raw text). `transformedContent` is the model-facing wrapped version. Also `attachments`, `interactionId`, `source?`, `isAutopilotContinuation?`. | source / 3p-observed |
| Assistant text | `assistant.message.data.content`, plus `messageId`, `turnId`, `model`, `toolRequests[] {toolCallId,name,arguments,…}`, `reasoningText?`, `outputTokens?`. | source / 3p-observed |
| Turn boundaries | `assistant.turn_start {turnId, interactionId}` and `assistant.turn_end {turnId}`. Both are persisted. A `turnId` is one model-call iteration. **`interactionId` groups all iterations of one user prompt**; it also appears on `user.message`. | 3p-observed |
| Tool calls | `tool.execution_start {toolCallId, toolName, arguments, turnId, …}` and `tool.execution_complete {toolCallId, success, result{content,detailedContent}, …}`. | source / 3p-observed |
| Task done | `session.task_complete {summary?, success?, outcome?, reason?}`. | source |
| Abort | `abort {reason: "user_initiated"\|"remote_command"\|"user_abort"\|"autopilot_credit_limit"}`. | source (not yet observed on disk) |
| Errors | `session.error {errorType, message, statusCode?, …}`. | source |
| Session end | `session.shutdown {shutdownType: "routine"\|"error", errorReason?, sessionStartTime(ms), currentModel?, codeChanges, modelMetrics, eventsFileSizeBytes?, …}`. Not written on a crash or kill (inferred). | source / 3p-observed |
| **Title** | **Not available from events.jsonl.** `session.title_changed {title}` is declared `ephemeral: true` and therefore **never persisted**. Read `workspace.yaml: name` (`user_named: bool` says whether the user set it) or `session-store.db` `sessions.summary`. | source + 3p-observed |
| **Idle / busy** | `session.idle {aborted?, mode?}` is also **ephemeral and never persisted**. See §4 for the inference rule. | source |

Other persisted events seen or declared: `system.message` (the large system prompt, about 56 KB), `subagent.started/completed` (these carry the envelope `agentId`), `session.compaction_start/complete`, `session.mode_changed`, `session.plan_changed`, `session.handoff`, `session.truncation`, `hook.start/end`, `skill.invoked`, `permission.*`. Rollcall should ignore unknown types.

The full ephemeral list (73 types, never on disk) includes `session.title_changed`, `session.idle`, `assistant.idle`, `assistant.*_delta`, `assistant.usage`, `session.usage_info`, `session.snapshot_rewind`, `tool.execution_progress`, `tool.shell_output`, `session.todos_changed`, and `user_input.*`. Extracted from session-events.ts.

### `workspace.yaml` (3p-observed at 1.0.67; no official schema)

The redacted real sample from toolpath `session-state.md`:
```yaml
id: <session-uuid>
cwd: /path/to/repo
git_root: /path/to/repo
repository: owner/repo
host_type: github
branch: main
client_name: github/cli
name: List Directory Contents   # auto-generated unless user_named
user_named: false
summary_count: 0
created_at: 2026-07-01T14:28:54.677Z
updated_at: 2026-07-01T14:31:30.280Z
```
The Shmuelie PowerShell module also reads `cwd, created_at, updated_at, summary, name, branch, repository`, so `summary` can appear too. There is no commit field. **This is the cheapest single read for a status board** (cwd, repo, branch, title, last update), but it is undocumented. The SDK's `SessionMetadata {sessionId, startTime, modifiedTime, summary?, isRemote, context?{workingDirectory, gitRoot?, repository?, branch?}}` is GitHub's official projection of the same data (**source**). Its `context` is documented as "from session creation".

## 4. Index / metadata / liveness

- **`session-store.db`** (**docs**): "A SQLite database used by the CLI for cross-session data such as checkpoint indexing and search." It is rebuildable with `/chronicle reindex` "from the files under `~/.copilot/session-state/`", which means it is a **derived index, not the source of truth**. WAL mode is 3p-observed. The reported tables (single third-party source, column names paraphrased) are `sessions(id, cwd, repository, branch, summary, created_at, updated_at, host_type)`, `turns(session_id, turn_index, user_message, assistant_response, timestamp)`, `checkpoints`, `session_files`, `session_refs`, and `search_index` (FTS5). If read at all, open it with `?mode=ro` and query defensively. **inferred / 3p**
- **Per-session `session.db`**: 3p-observed only. Contents unknown.
- **Live lock: `inuse.<pid>.lock`** inside `session-state/<id>/`. Three independent third parties report it: Narnia ("Copilot writes an `inuse.<pid>.lock` marker in each active session-state folder… A single Copilot runtime can own more than one session-state folder. Additional locks can represent provisional state created while resuming, a background agent, or an in-process subtask."), gfarb/copilot-session-manager ("Refuses if `inuse.*.lock` is present"), and toolpath. GitHub does not document it. **3p-observed.**
- **Stale locks happen** (**source**, changelog): 1.0.40 "Resuming a session no longer falsely reports it as in use after a previous CLI process exited unexpectedly". 1.0.22 "Warn when resuming a session that is already in use by another CLI or application". 1.0.35 "Session selector shows… idle/in-use status". The `alreadyInUse` field on `session.start`/`session.resume` records this.
- **Status inference rule for rollcall (inferred):**
  1. **live**: some `inuse.<pid>.lock` exists **and** that PID is alive (`kill(pid, 0)`, ideally checking that the process is `copilot`/node; PID reuse is a risk).
  2. If live, the session is **idle/waiting** when the last persisted event is `assistant.turn_end`, `session.task_complete`, or `abort` **and** no new line has arrived for a quiet window of a few seconds. Otherwise it is **working**. The quiet window matters because a `turnId` is one model-call iteration, not one user turn: in the fixture, one prompt (one `interactionId`) spanned turnIds 0–4, with about 2 ms between each `turn_end` and the next `turn_start`. Envelope `timestamp` and file mtime give last activity.
  3. **ended**: no live lock and `session.shutdown` is present (`shutdownType` gives routine or error).
  4. **crashed/interrupted**: no live lock, no `shutdown`, and a stale lock file or none at all.
- No pid file at the root level. `ide/` holds "lock files and state for IDE integrations" (**docs**), which is unrelated to session liveness.

## 5. How `--resume` / `--continue` find sessions (**docs**, command ref)

- `--continue`: "Resume the most recent session in the current working directory, falling back to the globally most recent session." Changelog 1.0.35 adds "prefers resuming sessions from the current working directory". This implies a per-session saved cwd, which is the `workspace.yaml cwd` / session-store `sessions.cwd` (inferred).
- `-r, --resume[=VALUE]`: matches a session ID, ID prefix, or session name. "Name matching is exact and case-insensitive; falls back to the auto-generated summary when no explicit name matches."
- `--session-id ID`: exact match only. "a new session is created only when the value is a valid UUID."
- `-n/--name`, `/rename`, `/session rename` set the name (stored in `workspace.yaml: name`, inferred).
- 1.0.52: "Sessions now resume in their saved working directory; pass -C <dir> to override." 1.0.65: "/cd now persists the working directory so resuming a session returns to it". **Together these mean the saved cwd is updated after `/cd`.** The likely store is `workspace.yaml` (`cwd`, `updated_at`) and/or the persisted `session.context_changed` (inferred).
- The canonical store is `session-state/` (docs: it "enables session resume"). `session-store.db` is the index the picker and search use. Legacy `history-session-state/` sessions migrate on resume.

## 6. Plaintext and redaction

- Prompts and responses are stored **in plaintext**. `user.message.data.content` holds the raw prompt, and `system.message` holds the full system prompt (**3p-observed**; docs: session data "encompasses prompts and responses, tools used, and changes made to files").
- I found **no documented redaction of session-state content.** Secret filtering is mentioned only for debug logs (1.0.87: "Secrets exported in the launching shell are no longer written to debug logs"), `/settings` output (1.0.72), and published commits/PRs (1.0.51). Tool arguments and results, such as `bash` command output, are stored verbatim (3p-observed).
- Auth tokens are separate: they go in the keychain, falling back to plaintext `config.json` only with `storeTokenPlaintext` (**docs**).
- Sessions are synced to the user's GitHub account by default (**docs**). That is irrelevant to reading, but relevant to privacy notes.
- **Rollcall implication:** per the repo's Safety rule, never print `content`, `transformedContent`, `arguments`, or `result` fields. Read only envelope fields, `type`, `context`, timestamps, and `workspace.yaml` metadata.

## 7. Write behavior

- **Append-only in normal operation.** Third parties (toolpath, #3551) describe it as "append-only", and it is consistent with `parentId` chaining. **3p / source**
- **Exception: rewind truncates history.** The docs (cancel-and-roll-back) say: "Session history is truncated. The conversation is rewound to the selected point. All messages and tool calls that occurred after that point are removed from the session." The schema's `session.snapshot_rewind {upToEventId, eventIds?, eventsRemoved}` describes the removal (that event is itself ephemeral). **I could not determine whether the file is physically truncated or rewritten, or whether the truncation is only logical.** The legacy migration on resume also rewrites.
- **Torn tail lines are possible.** Changelog 1.0.57: "Session resume works correctly after a crash that left partial data in the session log."
- **fsync / atomic-rename:** no public evidence either way. Unknown.
- **Tail-reader recommendations (inferred):** remember `(dev, inode, size, offset)`. If size < offset or the inode changed, re-read from 0. Buffer and ignore a trailing line with no `\n`. Treat any line that fails JSON parsing as skippable. Note that `workspace.yaml` is rewritten in place (its `updated_at` changes), so read it whole, and tolerate a partial or empty read during a write.

---

## Example lines (shape only; prompt/content text omitted)

These are verbatim from the public toolpath fixture (CLI 1.0.68, a scratch dir, not a git repo):
```json
{"type":"session.start","data":{"sessionId":"7a80f0ee-0c9f-4e1a-acbd-d655a5721cfe","version":1,"producer":"copilot-agent","copilotVersion":"1.0.68","startTime":"2026-07-02T15:28:15.745Z","contextTier":null,"context":{"cwd":"/tmp/elicit-scratch"},"alreadyInUse":false,"remoteSteerable":false},"id":"dd2ec0bc-…","timestamp":"2026-07-02T15:28:15.754Z","parentId":null}
{"type":"session.model_change","data":{"newModel":"auto","reasoningEffort":null,"contextTier":null},"id":"047003db-…","timestamp":"2026-07-02T15:28:17.004Z","parentId":"dd2ec0bc-…"}
{"type":"assistant.turn_start","data":{"turnId":"0","interactionId":"affa141a-…"},"id":"ad47593b-…","timestamp":"2026-07-02T15:28:18.443Z","parentId":"b9605a34-…"}
{"type":"assistant.turn_end","data":{"turnId":"0"},"id":"03560526-…","timestamp":"2026-07-02T15:28:22.520Z","parentId":"3bbfb7e5-…"}
```
The following skeletons are built from the SDK types and fixture key sets. `…` marks redacted or elided values:
```json
{"type":"session.start","data":{…,"context":{"cwd":"/repo","gitRoot":"/repo","repository":"owner/repo","hostType":"github","repositoryHost":"github.com","branch":"main","headCommit":"…","baseCommit":"…"}},…}
{"type":"session.context_changed","data":{"cwd":"/repo/sub","pendingGitContext":true},…}
{"type":"session.context_changed","data":{"cwd":"/repo/sub","gitRoot":"/repo","repository":"owner/repo","branch":"main"},…}
{"type":"session.resume","data":{"resumeTime":"…","eventCount":123,"context":{…},"alreadyInUse":false},…}
{"type":"user.message","data":{"content":"<REDACTED>","transformedContent":"<REDACTED>","attachments":[],"interactionId":"…","delivery":…},…}
{"type":"assistant.message","data":{"messageId":"…","turnId":"0","model":"…","content":"<REDACTED>","toolRequests":[{"toolCallId":"…","name":"bash","arguments":{…}}],"outputTokens":42},…}
{"type":"tool.execution_start","data":{"toolCallId":"…","toolName":"bash","arguments":{…},"turnId":"0"},…}
{"type":"tool.execution_complete","data":{"toolCallId":"…","success":true,"result":{"content":"<REDACTED>","detailedContent":"…"},"turnId":"0"},…}
{"type":"session.task_complete","data":{"summary":"<REDACTED>","success":true},…}
{"type":"abort","data":{"reason":"user_initiated"},…}
{"type":"subagent.started","agentId":"…","data":{"toolCallId":"…","agentName":"…","agentDisplayName":"…","model":"…"},…}
{"type":"session.shutdown","data":{"shutdownType":"routine","sessionStartTime":…,"currentModel":"…","eventsFileSizeBytes":…,"codeChanges":{…},"modelMetrics":{…},"totalApiDurationMs":…},…}
```

## Unknowns that need a real sample file

1. Is `session.context_changed` actually on disk after `/cd`, and does `workspace.yaml` `cwd`/`git_root`/`branch`/`updated_at` get rewritten at the same time? (Both are inferred from the schema and changelog.)
2. Does rewind physically truncate `events.jsonl` (inode or size change) or append a marker? Same question for compaction.
3. `inuse.<pid>.lock`: exact name, whether it has content, when it is created and removed, and whether `/restart` or the multi-session sidebar uses one lock per session or one per process.
4. Per-session `session.db` contents. Exact `session-store.db` table and column names (the only source is a paraphrase).
5. Whether `workspace.yaml` always exists (for example, for a session with zero prompts) and whether `summary` vs `name` both appear.
6. Whether sub-agent events interleave in the parent `events.jsonl` (the fixture shows only `subagent.started/completed` markers with `agentId`) or land elsewhere.
7. Whether flat `session-state/<id>.jsonl` files ever existed between 0.0.342 and about 1.0.6, and the format of the legacy `history-session-state/` files.
8. fsync and atomic-write behavior for `events.jsonl` and `workspace.yaml`.
9. How the "stale session reaper" (changelog 1.0.12) works, and whether Copilot ever deletes session-state directories on its own (`/session cleanup|prune` exist as manual commands).
