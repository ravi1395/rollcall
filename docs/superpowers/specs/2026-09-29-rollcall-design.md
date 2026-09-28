# rollcall — every repo, every worktree, and the agent session that left it dirty

**Status:** Revision 2 — rewritten after red-team round 1 (`2026-09-29-rollcall-redteam.md`). Awaiting red-team round 2, then author review.
**Date:** 2026-09-29
**Name:** `rollcall` — takes attendance of every repo and agent session. crates.io, Homebrew, npm and PyPI names free on 2026-09-29; only GitHub code collision is `withtally/rollcall` (unrelated). A short alias is offered via `rollcall init <shell>`, never as a shipped binary.

## 1. Brief

### Pitch

> **rollcall: every repo, every worktree, and the Claude Code or Codex session that left it dirty — one screen, one key to resume.**

### Problem

People running AI coding agents across many repos and worktrees lose track of:

- **Repo state** — dirty trees, unpushed commits, branches without upstreams, stale or deleted worktrees.
- **Agent context** — which Claude Code / Codex session last touched which worktree, what it was doing, how to get back into it.

### Differentiator (revised after prior-art search)

Claude Code's `/resume` picker already lists sessions across projects with their branch, and `codex resume --all` does the same for Codex. Multi-repo git dashboards (gita, worktrunk, mani, …) already show git state. **Nothing joins the two**: rollcall shows Claude **and** Codex session history attached to each worktree's git attention state (unpushed / dirty / stale), sorted by what needs attention, with one key to resume the session in the right directory.

The README must include a "How is this different from `claude --resume`, `codex resume`, worktrunk, agent-deck, ccmanager, gita?" section (prior art table in `2026-09-29-rollcall-redteam.md`).

### Generic by default

- Point rollcall at **any folder**; it finds every git repo under it. No config, no assumed layout.
- Agent sessions are an enrichment. With no agent logs, rollcall is a complete multi-repo git dashboard and shows no agent UI. This mode is supported but is **not** the pitch.

### Audience

People running Claude Code and/or Codex across many repos and worktrees. Developers without agents are a side benefit.

### What makes it fun (author goals)

Delightful TUI, a hard technical core (safe discovery, incremental parsing of ~1.4 GB of logs, caching), Rust, a real user base.

### Grounding data (author's machine, measured 2026-09-29)

- `~/Projects`: 17 repos + 29 linked worktrees (incl. `.claude/worktrees/`, `.worktrees/`); 6 worktrees prunable (dirs deleted); external worktrees under `/private/tmp` and `~/.codex/worktrees`; 2.2 M filesystem entries, depth 21, 525 symlinks.
- Claude Code: 1,979 top-level session logs + 469 subagent logs; Codex: 1,403 rollout files + 1,448 rows in `state_5.sqlite` `threads`. Total log bytes 1.35 GB (1.54 GB incl. subagents); largest file 45 MB; longest line 5.2 MB.
- Logs contain secrets (mostly inside tool output) and terminal escape sequences; see §9.

### Decided

| Decision | Choice |
|---|---|
| Language | Rust |
| Interfaces | TUI + CLI (`status --json`) + stdio MCP server, one shared core |
| Root | Positional `[PATH]`, default = current directory (MCP: `--root` required) |
| Discovery | Any git repo at any depth under the root, with safety guards (§4) |
| Agent sessions | Optional; v0 sources Claude Code + Codex behind `SessionSource` |
| Scope | Full scope in v0 (MCP, Homebrew, GIF included); estimated 3–5 weeks solo |
| Pitch | Agent-first |
| Architecture | Approach A: scan on demand + incremental cache (no daemon) |
| VCS | Git only |
| Subprocesses | None during scan — no `git` shell-outs (§4.4) |
| Obsidian / notes | Out of scope (roadmap) |

### Assumptions

- Claude Code and Codex log/database formats are internal and change between versions (Claude docs say so explicitly). Every source must detect unrecognised shapes, degrade to partial data, and report it in `doctor`.

## 2. Approaches considered

- **A. Scan on demand + cache (chosen).** Zero-config, reads existing history on first run. Freshness = last scan.
- **B. Daemon + live index.** Live TUI but install/lifecycle friction. Roadmap, behind the `Index` trait.
- **C. Agent hooks write a ledger.** Exact but per-agent setup, no history. Roadmap as an optional extra source.

## 3. Components, commands, definitions

### 3.1 Commands

Positional `PATH` is the root for every command except `mcp`, which takes `--root` because MCP servers are registered as fixed argument lists.

| Command | Purpose |
|---|---|
| `rollcall [PATH]` | TUI. If stderr is not a TTY, behaves as `rollcall status PATH`. |
| `rollcall status [PATH] [--json]` | Table (plain) or JSON (§7). |
| `rollcall scan [PATH]` | Explicit rescan; prints a one-line summary. |
| `rollcall doctor [PATH] [--json]` | Discovery + source diagnostics (§6.4). |
| `rollcall mcp --root PATH [--root PATH …] [--prompts]` | stdio MCP server (§8.3). Roots may instead come from config `roots`. |
| `rollcall init <bash\|zsh\|fish>` | Prints the `rc` shell function used for resume (§8.1). |
| `rollcall cache clear` | Deletes the cache directory. |

Shared flags (TUI, `status`, `scan`, `doctor`): `--submodules` (show submodules), `--follow-symlinks`, `--yes` (allow `/` or `$HOME` as root), `--no-scan` (use cache as-is).

Exit codes: `0` success; `1` completed with per-repo errors or a partial walk; `2` usage or config error; `3` root refused (§4.3).

`NO_COLOR` is honoured. Colours are the 16 named ANSI colours only (no RGB), so light and dark terminal themes both work.

### 3.2 Config (optional)

`$XDG_CONFIG_HOME/rollcall/config.toml`, default `~/.config/rollcall/config.toml` on macOS and Linux. Never required.

| Key | Default | Meaning |
|---|---|---|
| `roots` | `[]` | Saved roots; used by `mcp` when no `--root`. Saved roots count as confirmed for §4.3. |
| `skip_dirs` | `[]` | Extra directory names to skip (added to built-ins). |
| `stale_days` | `14` | Inactivity threshold for `stale_reason = "inactive"`. |
| `redact_prompts` | `false` | Hide `title` / `last_prompt` everywhere. |
| `walk_budget_entries` | `500000` | Directory entries visited before the walk stops (§4.3). |
| `walk_budget_secs` | `20` | Wall-clock walk budget. |

Unknown keys → warning on stderr. Invalid TOML or wrong types → exit 2.

### 3.3 Definitions

All git data is read in-process with `gix` (§4.4).

- **Default branch** of a repo: target of `refs/remotes/origin/HEAD`; else local branch `main`; else local `master`; else none (`default_branch: null`). (`init.defaultBranch` is not consulted: it describes new repos, not this one.)
- **Upstream** of a branch: `branch.<name>.remote` + `branch.<name>.merge` from repo config, resolved to a remote-tracking ref. None if unset or the ref is missing.
- **ahead / behind**: commits reachable from the branch tip and not from upstream (ahead), and vice versa (behind), computed with `rev_walk().with_hidden()`. `null` when there is no upstream.
- **unpushed**: with upstream → `ahead`. Without upstream but with ≥ 1 remote → count of commits on the branch not reachable from any `refs/remotes/*`. Without any remote → `null` (local-only repo; not flagged).
- **dirty_count**: tracked paths that differ from HEAD (staged or unstaged; modified, added, deleted, renamed, type-changed, conflicted). Untracked files are **not** included.
- **untracked_count**: untracked, non-ignored files (gix status with `UntrackedFiles::Files`). Ignore rules include user-level `core.excludesFile`.
- **last_commit_at**: committer time of the worktree's HEAD commit.
- **merged**: worktree is a linked (non-main) worktree on a branch other than the default branch, and its tip is an ancestor of the default branch tip. Squash-merged branches are **not** detected in v0 (documented limitation).
- **missing**: a linked worktree registered in `.git/worktrees/*` whose directory no longer exists (gix `Proxy::is_prunable`).
- **stale_reason** (linked worktrees only; main worktree is never stale): `"missing"` if missing; else `"merged"` if merged; else `"inactive"` if `last_commit_at` is older than `stale_days` **and** dirty_count = 0 **and** untracked_count = 0 **and** unpushed ∈ {0, null}; else `null`.
- **Worktree attention** (integer): `4` error; `3` unpushed > 0; `2` dirty_count > 0 or untracked_count > 0; `1` stale_reason ≠ null; `0` otherwise.
- **Repo attention** = max over its worktrees (and its own `error` → 4).
- **Last activity** of a repo = max over its worktrees of `last_commit_at` and matched sessions' `ended_at`.
- **Sort order** of repos: attention desc, then last activity desc, then path asc.
- **quiet Nd**: a repo with attention 0 shows `quiet Nd`, N = whole days since last activity.

### 3.4 Workspace crates and dependency direction

| Crate | Depends on | Responsibility |
|---|---|---|
| `rollcall-core` | — | Domain types (§7 shapes), `Index` trait, pure functions: attention, stale, sort, session→worktree matching, text sanitising and secret scrubbing. No I/O. Contains an in-memory `Index` for tests. |
| `rollcall-git` | core | Discovery walk (§4) and per-worktree git state via `gix`. |
| `rollcall-sessions` | core | `SessionSource` trait; `claude` and `codex` sources (§5). Emits sessions with `cwd`; knows nothing about worktrees. |
| `rollcall-store` | core | SQLite cache implementing `Index` (§6). |
| `rollcall-cli` | all | `clap` commands, scan orchestration (git → sessions → core matching → store), `ratatui` TUI, `rmcp` MCP server, shell init. Integration tests live in `rollcall-cli/tests/`. |

`rollcall-git`, `rollcall-sessions`, and `rollcall-store` never depend on each other.

## 4. Discovery and git state

### 4.1 Walk

Parallel walk of the root, any depth, visiting directories only.

- **`.git` directory** (by `lstat`; a symlink counts only with `--follow-symlinks`) → a repo whose working tree is the parent directory.
- **`.git` regular file** → read at most 4 KiB; must contain `gitdir: <path>` (relative paths resolve against the file's directory). The resolved target must be an existing directory, else the entry is recorded as an error row.
  - Target path contains a `…/.git/worktrees/<name>` component → **linked worktree** of the repo owning that `.git`.
  - Target contains `…/.git/modules/…` → **submodule** of the enclosing repo.
  - Otherwise → opened as a repo; `doctor` notes the unusual layout.
- **Old-style submodule** (a `.git` directory at a path listed in the enclosing repo's `.gitmodules`) → submodule, not nested repo.
- **Bare repo**: a directory not named `.git` containing a regular file `HEAD` and directories `objects/` and `refs/`, which `gix` opens as bare. Listed with branch and ref information only (no worktree status). Test fixtures inside other repos (e.g. `*/tests/fixtures/*.git`) will appear; this is expected and documented.
- **Nested repos**: after finding a repo, the walk continues into its working tree (never into its `.git` directory). A repo found inside another repo's working tree is reported as its own row with `kind: "nested"`; it is never folded into the outer repo.
- **Submodules** are grouped under their parent repo and omitted from output unless `--submodules`.
- **Linked worktrees** of each discovered repo are enumerated from its `.git/worktrees/*` via `gix` (`Repository::worktrees()`), not only from the walk. Two external cases:
  - **A — worktree outside root, main repo inside:** listed under its repo with `external: true`.
  - **B — worktree inside root, main repo outside:** its repo is shown as a row with `kind: "external_main"`; all its worktrees are listed, with `external: true` on those outside the root.

### 4.2 Skips

- Built-in directory names skipped anywhere: `node_modules`, `target`, `.venv`, `venv`, `dist`, `build`, `.cache`, `.Trash`, `__pycache__`, plus config `skip_dirs`.
- Built-in paths skipped: `$HOME/Library` (macOS: contains iCloud `Mobile Documents`, `CloudStorage` placeholders, TCC-gated data).
- `.gitignore` is **not** used for discovery, so repos cloned into ignored folders are still found.

### 4.3 Guards

- **Single filesystem:** never cross into a directory whose `st_dev` differs from the root's (skips `/Volumes`, network mounts, `/proc`, `/sys`, `/dev`, FUSE mounts).
- **Refused roots:** `/` or exactly `$HOME` → exit 3 with a message, unless `--yes` or the root is in config `roots`. MCP applies the same rule to `--root`.
- **Budget:** the walk stops after `walk_budget_entries` entries or `walk_budget_secs` seconds, marks the scan `partial: true`, and exits 1; `doctor` reports where it stopped.
- **Symlinks** are not followed by default. With `--follow-symlinks`, a visited `(dev, inode)` set prevents loops.
- **Unreadable directories** are counted and skipped.
- **Paths** are canonicalised with `realpath` where they exist; paths that no longer exist (missing worktrees, gone session cwds) use lexical normalisation.

### 4.4 Untrusted repositories

rollcall must be safe to run over folders of cloned or unpacked repos that may be hostile.

- **No subprocesses** during scan: no `git`, no filter drivers, no hooks, no fsmonitor, nothing from repo config is ever executed.
- `gix` is opened with system and user (global) config trusted, and repo-local config read only for non-executing keys (refs, remotes, branch tracking, ignore settings). Filter drivers, `core.fsmonitor`, hooks, and `includeIf` targets outside the repo are disabled.
- Consequence: repos that rely on clean/smudge filters (e.g. Git LFS, custom eol filters) may show files as dirty that `git status` would show clean. Documented limitation.
- `gix` errors on one repo produce an `error` string on that row; the scan continues.

## 5. Session sources

A `SessionSource` yields `Session` records (§7) plus diagnostics. Sources must be **read-only**: they never write to, lock, or modify agent files or databases.

Common rules:

- Session ids must match the UUID regex `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`; anything else is skipped and counted.
- JSONL is read with a streaming reader. Lines longer than **1 MiB** are skipped and counted without being fully buffered. Malformed JSON lines and unknown record types are skipped and counted.
- A session's `cwd` is the **latest** cwd it recorded (a session can change directory mid-way).
- Subagent / child sessions are **excluded** from session rows in v0 and counted in `doctor`.
- **Human prompt** = text typed by the user, never tool output, system/meta injections, or command wrappers (rules per source below). Text whose first non-whitespace character is `<` is treated as an injection and excluded (a known false negative for prompts that genuinely start with `<`).

### 5.1 Claude Code

- **Root:** `$CLAUDE_CONFIG_DIR/projects` if set, else `~/.claude/projects`.
- **Files:** `projects/*/*.jsonl` (one session per file). Subagent logs `projects/*/<sessionId>/subagents/*.jsonl` are excluded and counted.
- **Record fields used:** `type`, `sessionId`, `timestamp`, `cwd`, `gitBranch`, `isMeta`, `isSidechain`, `message.content`, `customTitle`, `aiTitle`, `lastPrompt`.
- **id:** `sessionId` of the first record carrying one; fallback the filename stem.
- **cwd:** latest record with `cwd`.
- **branch:** latest record with `gitBranch`. The literal value `"HEAD"` means detached → `branch: null`. Missing → `null`.
- **started_at / ended_at:** min / max `timestamp` over records that have one.
- **Human prompt:** record `type == "user"`, `isMeta` not true, `isSidechain` not true, and `message.content` is either a string or an array whose blocks are all `type == "text"` (any `tool_result` block disqualifies the record); text passes the common `<` rule.
- **title:** latest `custom-title.customTitle`; else latest `ai-title.aiTitle`; else first human prompt; else `null`.
- **last_prompt:** latest `last-prompt.lastPrompt` if present and non-empty; else latest human prompt; else `null`.
- **message_count:** number of human prompts.

### 5.2 Codex

- **Root:** `$CODEX_HOME` if set, else `~/.codex`.
- **Primary: thread database.** Open the highest-numbered `state_<N>.sqlite` whose `threads` table has columns `id, rollout_path, created_at, updated_at, cwd, title, git_branch, archived, first_user_message`, read-only (`mode=ro`), never writing. The database uses WAL; read-only opens must tolerate a concurrent Codex writer (retry with `busy_timeout`).
  - One session per `threads` row whose `id` is not a `thread_spawn_edges.child_thread_id` (children excluded and counted). Archived threads are included.
  - `cwd` ← `threads.cwd`; `branch` ← `threads.git_branch` (empty → `null`); `started_at` ← `created_at`, `ended_at` ← `updated_at` (Unix seconds); `title` ← `threads.title` (empty → `first_user_message`, empty → `null`).
  - `last_prompt` and `message_count` come from the rollout file at `rollout_path` (incremental, §6.2), if it exists; otherwise `null` / `0`.
- **Fallback: rollouts only** (no recognised database). Files `sessions/**/rollout-*.jsonl` and `archived_sessions/*.jsonl`. `id`, `started_at` from the first `session_meta.payload` (`id`, `timestamp`); `cwd` from the latest `turn_context.payload.cwd`, else `session_meta.payload.cwd`; `branch` from `session_meta.payload.git.branch` if present, else `null`; files whose `session_meta.payload.parent_thread_id` is set are children (excluded). A file with a second `session_meta` of a different id is attributed to the first id. `title` = first human prompt.
- **Human prompt (rollouts):** `event_msg` with `payload.type == "user_message"` → `payload.message`; otherwise `response_item` with `payload.type == "message"` and `payload.role == "user"` → concatenated `input_text` blocks, excluding text that passes the `<` rule or starts with `# AGENTS.md`.
- `doctor` reports which mode was used and the database version found.

## 6. Store, freshness, concurrency

### 6.1 Location and permissions

- Cache directory from the `directories` crate: macOS `~/Library/Caches/rollcall`, Linux `$XDG_CACHE_HOME/rollcall` (default `~/.cache/rollcall`).
- Directory mode `0700`; `cache.sqlite` and its `-wal` / `-shm` files `0600`, set explicitly regardless of umask.
- One database for all roots. Repos and worktrees are keyed by canonical path; a root's view is the set of rows under it. Sessions are keyed by `(source, id)` and shared across roots.

### 6.2 What is stored

- Git state per worktree and the fields in §7.
- Per log file: `(dev, inode)` as key, plus `path`, `size`, `mtime`, `offset`, `parser_version`.
- **No free text from logs is stored.** Titles and prompts are stored as pointers (`log path, byte offset, length`, or Codex `thread id`) and resolved when rendering. Clearing or deleting the cache removes nothing from agent logs and adds no copy of them.

### 6.3 Incremental reads

For each log file, compare the stored `(dev, inode)` entry:

- New file → read from 0.
- Same inode, size grew → read from stored `offset`.
- Size shrank, or mtime changed with the same size → re-read from 0.
- Same inode seen at a new path (e.g. Codex moving to `archived_sessions`) → update `path`, keep `offset`.
- Partial trailing line (no `\n`) → stop before it; do not advance `offset`.
- Source `parser_version` differs from stored → re-read from 0.

### 6.4 Freshness and concurrency

- Commands scan implicitly before answering when `scanned_at` for the root is older than **30 s** (unless `--no-scan`).
- SQLite in WAL mode with `busy_timeout = 5000 ms`. Each scan writes in **one transaction**; readers only see committed scans. A crash mid-scan leaves the previous state intact.
- A per-root advisory lock (`flock` on `<cache>/scan-<hash>.lock`, hash = SHA-256 of the canonical root path) allows one scanner at a time. Other processes serve the current cache and report `stale: true`.
- MCP never blocks longer than **2 s** on a scan: it starts a scan, and if the scan doesn't finish in time it answers from the cache with `stale: true`.
- Schema version is `PRAGMA user_version`. On mismatch the cache is deleted and rebuilt (it is a cache; no migrations).

### 6.5 Session → worktree matching

- Canonicalise the session `cwd` (§4.3) and pick the worktree whose canonical path is the longest **component-wise** prefix of it.
- A matched session attaches to that worktree regardless of branch; its own `branch` is shown with it.
- Unmatched sessions are classified: `outside_root` (cwd not under the root), `path_gone` (cwd no longer exists), `non_repo` (cwd under the root, exists, but in no worktree).
- A worktree's `last_session` is its matched session with the greatest `ended_at`.

### 6.6 doctor

Reports, per root: entries walked, repos by kind, worktrees (external, missing), submodules, unreadable dirs, skipped dirs, whether the budget stopped the walk; per source: mode, files, records, skipped lines (malformed / unknown type / oversize), sessions, excluded children, matched, and unmatched by class.

## 7. Output schema (`status --json`, MCP)

Timestamps are RFC 3339 UTC strings. `schema_version` is bumped on any incompatible change.

```
StatusDoc {
  schema_version: 1,
  root: string,                 // canonical absolute path
  scanned_at: string,
  stale: bool,                  // served from cache while another scan runs
  partial: bool,                // walk budget hit
  repos: Repo[]                 // sorted per §3.3
}

Repo {
  name: string,                 // basename of the working tree (or of the bare dir)
  path: string,
  kind: "normal" | "nested" | "bare" | "external_main",
  default_branch: string | null,
  has_remote: bool,
  attention: 0 | 1 | 2 | 3 | 4,
  last_activity_at: string | null,
  error: string | null,
  worktrees: Worktree[],        // main worktree first, then linked by path; empty for bare
  submodules?: Repo[]           // present only with --submodules
}

Worktree {
  path: string,
  is_main: bool,
  external: bool,
  branch: string | null,        // null when detached
  detached: bool,
  head: string | null,          // commit sha
  last_commit_at: string | null,
  upstream: string | null,      // e.g. "origin/main"
  ahead: int | null,
  behind: int | null,
  unpushed: int | null,
  dirty_count: int,
  untracked_count: int,
  stale_reason: "missing" | "merged" | "inactive" | null,
  attention: 0 | 1 | 2 | 3 | 4,
  error: string | null,
  last_session: Session | null
}

Session {
  source: "claude" | "codex",
  id: string,                   // validated UUID
  branch: string | null,
  cwd: string,
  started_at: string,
  ended_at: string,
  title: string | null,         // sanitised, scrubbed, ≤ 200 chars; null when redacted
  last_prompt: string | null,   // same rules
  message_count: int,
  resume: string                // exact shell line, §8.1
}
```

For a `missing` worktree, git fields are `null` / `0` and `attention` is 1.

## 8. UX

### 8.1 TUI

```
 rollcall ─ ~/Projects ─ 17 repos · 29 worktrees · scanned 4s ago             [r]escan
┌ Repos (by attention) ──────────────┐┌ CRM ───────────────────────────────────────────┐
│▶ CRM            ↑3 ●2  2 stale wt  ││ Worktrees                                       │
│  sutra          ●5     1 no-upstr  ││  main                         ✓ clean  ↑0       │
│  agentrec       ↑1                 ││  ui-refine/css-consolidation  ●2       ↑3       │
│  games                 quiet 9d    ││  chore/post-pr1-housekeeping  merged → stale    │
│  uv                    quiet 21d   ││                                                  │
│                                    ││ Agent sessions (this repo)                       │
│                                    ││  claude  2h   ui-refine  "consolidate dark-mode…"│
│                                    ││  codex   1d   main       "fix flaky e2e in…"     │
│                                    ││  claude  3d   chore/…    "post-PR1 cleanup"      │
└────────────────────────────────────┘└──────────────────────────────────────────────────┘
 j/k move · enter expand · / filter · tab pane · R resume · c copy cd · o open · r rescan · q quit
```

- The TUI draws on **stderr**; stdout is reserved for the resume line.
- Keys: `j`/`k` or arrows move; `enter` expands/collapses a repo; `tab` switches pane; `/` filters repos by name substring (Esc clears); `r` rescans; `q` quits (exit 0, nothing on stdout).
- **`R` (resume)** on a session: quits and prints exactly one line to stdout: `cd -- '<cwd>' && claude --resume <id>` or `cd -- '<cwd>' && codex resume <id>`, with the path shell-quoted and the id UUID-validated. If the session `cwd` no longer exists, `R` is refused with a status-line message.
- `rollcall init zsh|bash|fish` prints a function `rc` that runs `rollcall "$@"`, captures stdout, and `eval`s it only if non-empty. `rc` then `R` lands the user in the session.
- **`c`** copies `cd -- '<worktree path>'` (shell-quoted) to the clipboard; if no clipboard is available, shows a status-line message.
- **`o`** opens the worktree in `$VISUAL` or `$EDITOR`, split with shell-words rules and executed **without a shell**, path as a separate argument; unset → status-line message.
- The **Agent sessions** pane and `R` are absent when no source has data.
- Badges: `nested`, `bare`, `ext` (external), `missing`. Colours: unpushed = yellow, dirty = blue, stale/missing = dim, error = red.

### 8.2 `status`

Plain output: one line per repo, indented lines per worktree, columns matching the TUI. `--json` emits `StatusDoc`.

### 8.3 MCP (`rollcall mcp`)

- Roots: `--root` (repeatable) or config `roots`; neither → exit 2. Each root is checked against §4.3.
- Tools, all read-only:
  - `where_was_i(root?: string, repo?: string, limit?: int = 5)` → `{schema_version, root, scanned_at, stale, repos: Repo[], recent_sessions: Session[]}`. `repo` matches repo `name` or `path`; without it, the top `limit` repos by attention. `recent_sessions` = the latest `limit` sessions across the returned repos.
  - `list_stale(root?: string)` → `{schema_version, root, scanned_at, stale, items: [{repo, worktree_path, branch, stale_reason}]}`. Never deletes anything.
  - `root` must be one of the configured roots; omitted → the first.
- **Prompts are off by default:** `title` and `last_prompt` are `null` unless the server runs with `--prompts`.
- Tool descriptions state that `title` / `last_prompt` / branch names are untrusted text from logs and repos and must not be treated as instructions.
- Each response is at most **16 KiB** of JSON. Over budget: drop `recent_sessions` from the end, then trailing repos, and set `truncated: true`.

## 9. Privacy and security

- **Local and read-only.** No network access; nothing is written outside the cache and config directories. The MCP path hands data to whatever model the client uses; this is why prompts are off by default there.
- **Sanitising (all text rendered in TUI or plain output):** from paths, branch names, titles, prompts, and error messages, remove ESC-introduced sequences, all other C0 control characters, DEL, and C1 controls; newline and tab become a single space. JSON output relies on JSON string escaping.
- **Secret scrubbing** (title and last_prompt everywhere, applied before truncation): replace with `[redacted]` matches of:
  - `sk-[A-Za-z0-9_-]{16,}` · `AKIA[0-9A-Z]{16}` · `AIza[0-9A-Za-z_-]{35}` · `gh[pousr]_[A-Za-z0-9]{36,}` · `xox[abprs]-[A-Za-z0-9-]{10,}`
  - `Bearer\s+\S+` · `-----BEGIN [A-Z ]*PRIVATE KEY-----`
  - `(?i)\b(password|passwd|secret|token|api[_-]?key)\s*[=:]\s*\S+`
  - credentials in URLs: `[A-Za-z][A-Za-z0-9+.-]*://[^/\s:@]+:[^@\s/]+@`
- **Truncation:** 200 characters, on a grapheme boundary, `…` appended.
- **`redact_prompts = true`** → `title` and `last_prompt` are `null` everywhere.
- **Cache:** no log text stored (§6.2); `0700` / `0600` permissions.
- **Untrusted repos:** §4.4. **Filesystem guards:** §4.3.

## 10. Testing

All tests run in CI on generated fixtures. No real user data is committed.

- **Discovery fixture tree:** normal repo; linked worktree; worktree outside root (case A); worktree inside root whose main repo is outside (case B); missing worktree; submodule (modern) and old-style submodule; bare repo; nested repo; repo inside a `.gitignore`d folder; repo under `node_modules/` and `target/`; symlink loop; unreadable dir (test skipped with a logged reason when running as root); a second filesystem is not required (single-filesystem rule unit-tested on `st_dev` values).
- **Git status:** dirty (staged, unstaged), untracked, ahead/behind, no upstream with remote, no remote, detached HEAD, merged linked worktree, inactive linked worktree (commit dates set in the past), no default branch, corrupted repo (→ `error`, others unaffected).
- **Untrusted repo:** fixture with `filter.x.clean`, `core.fsmonitor`, and a hook each pointing at a script that creates a marker file; a fake `git` first on `PATH` does the same. After scan, no marker exists.
- **Guards:** root `/` and `$HOME` (via `HOME` override) → exit 3 without `--yes`; walk budget hit → `partial: true`, exit 1.
- **Claude source fixtures** (synthetic, shapes per §5.1): human prompts vs `tool_result` records (last user record is a `tool_result` and must not become `last_prompt`); `isMeta`; `<command-name>` wrapper; `custom-title` and `ai-title` precedence; `last-prompt`; multi-cwd session; `gitBranch: "HEAD"`; subagent log (excluded); non-UUID session id (skipped); oversize line (skipped, counted); malformed line.
- **Codex source fixtures:** a `state_5.sqlite` with `threads` + `thread_spawn_edges` (child excluded; archived included; empty title → first_user_message); matching rollouts for `last_prompt`; database absent → rollout fallback; rollout moved to `archived_sessions` (offset kept); `parent_thread_id` child; second `session_meta`; injected-context user messages excluded.
- **Incremental reads:** append 3 lines + a partial line → 3 new records, offset before the partial; truncate → re-read; same-size rewrite with new mtime → re-read; `parser_version` bump → re-read.
- **Matching:** fixed fixture of 20 sessions: 16 matchable in root, 1 `non_repo`, 1 `path_gone`, 2 `outside_root`.
- **Privacy:** scrub patterns each replaced; truncation at 200 on a multi-byte grapheme; `redact_prompts`; MCP without `--prompts` returns null text; cache files have modes `0700` / `0600` under umask `022`; cache contains none of the fixture prompt strings (byte search).
- **Sanitising:** branch name and prompt containing `\x1b[2J` and `\x9b` render without them in plain output and TUI snapshots.
- **Shell safety:** worktree path with space, `$`, backtick, `;`, `'` → `R` line and `c` text round-trip through `sh -c 'printf %s "$0"'` unchanged; non-UUID id never printed.
- **Store:** idempotent rescan (two scans → identical `status --json` apart from `scanned_at`); crash mid-scan (transaction dropped) → prior state; two concurrent scans → one takes the lock, the other reports `stale: true`; `user_version` mismatch → rebuild.
- **TUI:** `ratatui` `TestBackend` 100×30 + `insta` snapshots, with and without sessions.
- **MCP:** spawn `rollcall mcp --root <fixture>` over stdio; call both tools; validate against §7; response ≤ 16 KiB on a 200-session fixture; `truncated` set when over.
- **Bench:** `criterion` micro-benchmarks for JSONL parsing and matching; `hyperfine` end-to-end on the CI corpus (§12).

## 11. Distribution and release

- **CI (GitHub Actions, macOS + Linux):** `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`, `cargo audit`. Actions pinned by commit SHA. `Cargo.lock` committed.
- **Releases:** `cargo-dist` → GitHub Release binaries with SHA-256 checksums and GitHub artifact attestations, Homebrew tap, shell installer; publish to crates.io.
- **License:** dual MIT / Apache-2.0.
- **README:** pitch, "how is this different" section, install, `rc` shell setup, MCP registration for Claude Code and Codex (with `--root`), privacy notes, demo GIF recorded with `vhs` against a **synthetic demo root and synthetic logs** (no real repo names or prompts).
- Windows: not supported in v0.

## 12. v0 acceptance criteria

### 12.1 Gating (checked in CI on generated fixtures; `F` = fixture root)

1. **Discovery:** `rollcall status F --json | jq -S '[.repos[] | {path, kind, worktrees: [.worktrees[] | {path, external, stale_reason}]}]'` equals `tests/expected/discovery.json`; submodules appear only with `--submodules`; the run finishes in < 10 s with the symlink loop present, in default and `--follow-symlinks` modes.
2. **Git status:** for every fixture worktree, `branch, detached, upstream, ahead, behind, unpushed, dirty_count, untracked_count, stale_reason, attention` equal `tests/expected/status.json`; the corrupted repo has `error != null` and all other rows are present.
3. **Sort:** repo order in `status --json` equals the order in `tests/expected/order.json` (built to exercise every attention level and both tie-breaks).
4. **Claude source:** fixture sessions produce exactly the `title`, `last_prompt`, `branch`, `cwd`, `message_count` values in `tests/expected/claude.json`; subagent, non-UUID, oversize and malformed cases are counted in `doctor --json`.
5. **Codex source:** same, for both database mode and rollout-fallback mode, against `tests/expected/codex_db.json` and `tests/expected/codex_rollout.json`.
6. **Matching:** the §10 matching fixture yields 16 worktrees with the expected `last_session.id`, and `doctor --json` reports `non_repo: 1, path_gone: 1, outside_root: 2`.
7. **Incremental reads:** each §10 incremental case produces the expected record counts and offsets.
8. **Untrusted repo:** no marker file exists after `rollcall scan F` on the hostile fixture.
9. **Guards:** `/` and `$HOME` roots exit 3 without `--yes`; budget fixture → `partial: true`, exit 1.
10. **Privacy:** every scrub pattern is redacted; `last_prompt` ≤ 200 characters; `redact_prompts = true` → all `title` / `last_prompt` null; cache modes `0700` / `0600`; no fixture prompt string appears in the cache files.
11. **Sanitising & shell safety:** the §10 sanitising and shell-safety tests pass.
12. **Store:** idempotent rescan, crash-safety, concurrent-scan, and schema-rebuild tests pass.
13. **No-agent mode:** `HOME=$(mktemp -d) rollcall status F --json` exits 0 with every `last_session: null`; the TUI snapshot contains no "Agent sessions" text.
14. **MCP:** stdio integration test passes both tools; responses validate against §7; ≤ 16 KiB; `title` / `last_prompt` null without `--prompts`; `rollcall mcp` with no root exits 2.
15. **Performance** (CI corpus: 50 repos incl. 20 linked worktrees, 5,000 synthetic logs totalling 500 MB; `ubuntu-latest`; `hyperfine -w 2 -r 10`): cold `rollcall scan` median < 5 s; warm `rollcall status --json` (no log changes, git state re-read) median < 1 s.
16. **Colours:** TUI snapshots use only named ANSI colours; with `NO_COLOR=1` they contain no colour styling.
17. **Build & release:** CI jobs in §11 pass; `dist plan` and `dist build` succeed for macOS (arm64, x86_64) and Linux (x86_64, arm64) in CI; `HOME=$(mktemp -d) cargo install --path crates/rollcall-cli --locked && rollcall --version` succeeds on Linux CI.
18. **Demo:** `vhs demo.tape` renders the README GIF from the synthetic demo root in CI.

### 12.2 Author smoke (non-gating; recorded in the handoff, not judged by the skeptic)

- `rollcall status ~/Projects --json` lists the repos and worktrees `find ~/Projects -name .git` finds outside skipped dirs; dirty / ahead / behind match `git status` on 5 sampled worktrees.
- 100 % of Claude and Codex sessions whose cwd lies inside a discovered worktree are matched; the rest appear in `doctor` under the right class.
- Cold scan < 5 s and warm < 500 ms on the author's machine.
- `rc` then `R` resumes a real Claude session and a real Codex session.
- Claude Code with `rollcall mcp --root ~/Projects` registered answers "where was I in CRM?" with CRM's worktrees and last session.
- `brew install` from the published tap works on a clean macOS user account.

## 13. Roadmap (not v0)

Daemon / file watcher (Approach B); hook source (Approach C); squash-merge detection; LLM session summaries; Obsidian / notes adapter; Cursor and other agent sources; decision-journal integration; non-git VCS (jj, hg); Windows.
