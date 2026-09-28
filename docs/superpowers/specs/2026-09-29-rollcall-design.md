# rollcall — "where was I" across repos and agent sessions

**Status:** Design approved in brainstorming (Sections 1–4 + generic-discovery revision). Awaiting written-spec review.
**Date:** 2026-09-29
**Name:** `rollcall` — takes attendance of every repo and agent session. crates.io and Homebrew names verified free on 2026-09-29. A short alias is offered as an opt-in shell snippet, never as a shipped binary.

## 1. Brief

### Problem

Developers who work across many repos — especially those running AI coding agents (Claude Code, Codex) — lose track of where they were:

- **Repo sprawl** — dirty trees, unpushed commits, branches without upstreams, stale worktrees.
- **Agent context lost** — which session touched which repo/branch, what it was doing.
- **Notes ↔ code gap** — deferred to a later adapter (see Roadmap).

Existing tools (claude-mem, `.remember`, `/handoff`, `/repo-debt-report`, graphify) are **per-project**. None gives one cross-repo view. That cross-repo view is the differentiator.

### Generic by default

- Point `rollcall` at **any folder**; it finds **every git repo** under it. No config, no assumed layout.
- Agent sessions are an **optional enrichment**. With no agent logs on the machine, `rollcall` is a complete multi-repo git dashboard and shows no agent UI.

### Grounding data (author's machine, 2026-09-28)

- 15 git repos + 14 worktrees under `~/Projects`.
- 1,972 Claude Code session logs (`~/.claude/projects/*/*.jsonl`); records carry `cwd` and `gitBranch`.
- 1,403 Codex session logs (`~/.codex/sessions/**.jsonl`); `session_meta` record carries `cwd`.
- No local Cursor session logs found.

### Audience

Developers working across several repos; AI-agent users get the extra session view. Obsidian is **not** required.

### What makes it fun (author goals)

Delightful UX (demo-able TUI), a hard technical core (repo discovery, incremental parsing, indexing), a new stack (Rust), and a fast first ship.

### Decided

| Decision | Choice |
|---|---|
| Language | Rust |
| Interfaces | TUI for humans + MCP/CLI for agents, one shared core |
| Root | `rollcall [path]`, default = current directory |
| Discovery | Any git repo at any depth under the root |
| Agent sessions | Optional; v0 sources Claude Code + Codex behind a `SessionSource` trait |
| Obsidian / notes | Deferred to a later adapter |
| Architecture | **Approach A: scan on demand + incremental cache** (no daemon) |
| VCS | Git only in v0 |

### Assumptions

- Agent session log formats are undocumented and can change without notice; parsers must fail soft.

## 2. Approaches considered

- **A. Scan on demand + cache (chosen).** Zero-config, reads existing history on first run, weekend-sized v0. Freshness equals last scan.
- **B. Daemon + live index.** Live TUI, but install/lifecycle friction and ~2–3× v0 scope. Planned as v1 upgrade behind the `Index` trait.
- **C. Agent hooks write a ledger.** Exact but needs per-agent setup and misses history. Possible future optional source.

## 3. Scope and components

### Commands

All commands take an optional `[path]` root; default is the current directory.

- `rollcall [path]` — TUI.
- `rollcall status [path] [--json]` — table for scripts/agents.
- `rollcall scan [path]` — explicit rescan.
- `rollcall doctor [path]` — discovery and parser diagnostics.
- `rollcall mcp [--root <path>]` — stdio MCP server; root defaults to the server process's cwd. Tools `where_was_i(repo?)`, `list_stale()`.

Optional config (`$XDG_CONFIG_HOME/rollcall/config.toml`) only for: saved extra roots, extra skip patterns, `redact_prompts`, stale threshold. Never required.

### Per repo → branch/worktree

- Branch, ahead/behind upstream, dirty file count, unpushed commits, no-upstream flag.
- Stale worktrees: branch merged into default, or no commits in N days (default 14).
- Last agent session on that repo+branch (only when a session source has data): tool, time ago, title or first prompt, last user prompt, message count.
- Attention sort: unpushed > dirty > stale > quiet.

### Workspace crates

| Crate | Responsibility |
|---|---|
| `rollcall-core` | Domain types (`Repo`, `Worktree`, `Session`, `Attention`), `Index` trait, query API. No I/O. |
| `rollcall-git` | Repo discovery + status via `gix`, parallel via `rayon`. |
| `rollcall-sessions` | `SessionSource` trait; `claude` and `codex` impls; offset-based JSONL reads; `cwd`+`gitBranch` → worktree match. Adding an agent = one new impl, no core changes. |
| `rollcall-store` | SQLite (`rusqlite`) cache implementing `Index`; migrations. One cache file per root: `$XDG_CACHE_HOME/rollcall/<root-hash>.sqlite`. |
| `rollcall-cli` | `clap` commands, `ratatui` TUI, `rmcp` MCP server — all over the same core query API. |

### Out of v0 (roadmap)

Daemon/watcher (B), LLM session summaries, Obsidian adapter, Cursor source, hook source (C), decision-journal integration, non-git VCS (jj/hg), Windows support.

## 4. Data flow and error handling

### Scan pipeline

Runs on `rollcall scan`, and implicitly before `status` / TUI / MCP queries when the cache is older than 30 s.

1. **Discover repos** — parallel walk of the root, any depth:
   - `.git` **directory** → normal repo.
   - `.git` **file** → linked worktree or submodule (resolved via its `gitdir:` pointer). Submodules are grouped under their parent repo and hidden unless `--submodules`.
   - `HEAD` + `objects/` + `refs/` with no work tree → **bare repo**; listed with branch/ref info only.
   - **Nested repos** are each reported separately; an inner repo is never folded into the outer one.
   - The walk continues **into** a discovered repo's working tree (to find nested repos) but not into its `.git` dir.
   - Symlinks are **not followed** by default (`--follow-symlinks` opts in, with a visited-inode set to break loops).
   - Skips: a built-in directory-name list (`node_modules`, `target`, `.venv`, `venv`, `dist`, `build`, `.cache`) extensible via config. `.gitignore` is **not** used for discovery, so repos cloned into ignored folders are still found. No max-depth cap.
   - Unreadable directories are counted and reported by `rollcall doctor`; the walk continues.
   - Worktrees listed by `git worktree list` whose path is **outside** the root are shown, marked `external`.
2. **Git status per worktree** via `gix`. A failing repo gets an `Error` state row with the message; the scan continues.
3. **Sessions** (skipped entirely when no source finds logs): per source, list log files and compare stored `(path, inode, size, offset)`.
   - Size grew → read from stored offset.
   - Inode changed or size shrank → re-read from 0.
   - Partial trailing line → stop before it; do not advance offset.
4. **Match session → worktree:** longest-prefix match of session `cwd` against known worktree paths (after canonicalising both). Branch from record `gitBranch`, else the worktree's current branch. No match → `orphaned` (sessions outside the root are orphans by design and are not shown in the main view).
5. **Store:** one SQLite transaction per scan. Queries read committed state only; a crash mid-scan leaves the previous state intact.

### Format drift

- Unknown record types and malformed lines are skipped and counted.
- Each source stores `parser_version`; bumping it forces re-parse from offset 0.
- `rollcall doctor` reports per source: files, records, skipped lines, orphans — a format change shows as a spike, not silent bad data.

### Privacy

- Read-only, fully local, no network.
- Prompt text truncated in output (default 200 chars). `redact_prompts = true` hides prompts entirely — matters because MCP hands text to whatever model is connected.

### Performance targets

- Cold scan: ~30 worktrees + ~3.4k logs in < 5 s.
- Warm scan: < 300 ms.
- Verified by a `criterion` benchmark on a synthetic corpus plus a local-only run on real data.

## 5. TUI / MCP UX

### TUI (`ratatui`)

```
 rollcall ─ ~/Projects ─ 15 repos · 29 worktrees · scanned 4s ago           [r]escan
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
 j/k move · enter expand · / filter · tab pane · R resume session · c copy cd · o open · ? help
```

- The **Agent sessions** pane and the `R` key are absent when no session source has data.
- `R` on a session prints the resume command (`claude --resume <id>` / `codex resume <id>`) and exits so the shell picks it up. Exact CLI flags verified during implementation.
- `c` copies `cd <worktree path>` to the clipboard; `o` opens the worktree in `$EDITOR`.
- Semantic colours: unpushed = amber, dirty = blue, stale = dim, error = red. Must work on light and dark terminals.
- Nested and bare repos show a small type badge; external worktrees are marked `ext`.

### `rollcall status`

Plain table matching the TUI; `--json` emits the same stable schema as the MCP tools.

### MCP tools (read-only)

- `where_was_i(repo?: string, limit?: n)` → `{repo, worktrees:[{branch, path, dirty, ahead, behind, upstream, stale_reason?}], sessions:[{tool, id, branch, ended_at, title, last_prompt}]}`. `sessions` is an empty array when no source has data. No `repo` → the repo containing the server's cwd if any, else top 5 repos by attention.
- `list_stale()` → worktrees/branches that are safe cleanup candidates, each with a reason. Never deletes.
- Compact JSON, truncated prompts, redaction respected, output capped at ~4k tokens.

## 6. Testing

- **Discovery:** generated temp tree containing a normal repo, a linked worktree, a worktree whose main repo is outside the root, a submodule, a bare repo, a nested repo, a repo inside a `.gitignore`d folder, a symlink loop, an unreadable dir, and `node_modules` / `target` noise. Asserts the exact discovered set and types.
- **Git status:** temp repos covering dirty, ahead/behind, no-upstream, merged worktree, detached HEAD, and one corrupted repo (→ `Error` row, scan continues).
- **Session parsers:** golden-file tests on **synthetic** JSONL fixtures that mirror real Claude/Codex record shapes (no real prompts committed). Cases: append-from-offset, partial trailing line, unknown record types, inode rotation, truncation, `parser_version` bump.
- **No-agent mode:** `HOME` pointed at an empty dir → no session UI, no errors, `sessions: []` in JSON.
- **Store:** migrations, idempotent rescan (two scans → identical rows), simulated crash mid-scan leaves prior state intact.
- **TUI:** `ratatui` `TestBackend` + `insta` snapshots, with and without session data.
- **MCP:** integration test spawns `rollcall mcp` over stdio, calls both tools, asserts JSON schema and the token cap.
- **Bench:** `criterion` on a synthetic tree (~50 repos, ~5k logs) in CI; local-only script benchmarks real `~/Projects`, `~/.claude`, `~/.codex`.

## 7. Distribution and release

- **CI (GitHub Actions):** `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` on macOS + Linux. Windows deferred (different log paths).
- **Releases:** `cargo-dist` → GitHub Release binaries, Homebrew tap, shell installer; plus crates.io once the name is settled.
- **License:** dual MIT / Apache-2.0.
- **README:** demo GIF recorded with `vhs` against a **synthetic demo root** — no real repo names or prompts.

## 8. v0 acceptance criteria

1. **Discovery is generic:** on the generated test tree (§6 Discovery), `rollcall status --json` reports exactly the expected repos with correct types (normal / worktree / external worktree / submodule-under-parent / bare / nested), terminates on the symlink loop, and excludes `node_modules` / `target` contents.
2. **Real-data spot check:** `rollcall status ~/Projects --json` lists every repo and worktree that `find ~/Projects -name .git` finds outside the skip-list directories (submodules counted under their parent), and dirty / ahead / behind match `git status` on 5 sampled worktrees.
3. **Session matching:** ≥ 95 % of Claude and Codex logs whose `cwd` is under the root match a worktree; the rest are listed by `rollcall doctor` as orphans.
4. **Performance:** cold scan < 5 s and warm scan < 300 ms on the author's `~/Projects` + agent logs.
5. **Resume:** `R` produces a command that resumes a real Claude session and a real Codex session.
6. **MCP:** Claude Code with `rollcall mcp` registered answers "where was I in CRM?" with the correct branches and last session.
7. **Zero-config, no-agent install:** on a clean machine (or clean `HOME`), `brew install` then `rollcall` in any folder runs with no config and no agent logs, shows the git dashboard, and shows no agent UI.
