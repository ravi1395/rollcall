# rollcall — every repo, every worktree, and the agent session that left it dirty

**Status:** Revision 11 — after red-team rounds 1–4, plan red-team rounds 1–3, and the P14 gix read-site audit (`2026-09-29-rollcall-redteam.md`). Awaiting author review.
**Date:** 2026-09-29
**Name:** `rollcall` — takes attendance of every repo and agent session. crates.io, Homebrew, npm and PyPI names free on 2026-09-29; only GitHub code collision is `withtally/rollcall` (unrelated).

## 1. Brief

### Pitch

> **rollcall: every repo, every worktree, and the Claude Code or Codex session that left it dirty — one screen, one key to resume.**

### Problem

People running AI coding agents across many repos and worktrees lose track of:

- **Repo state** — dirty trees, unpushed commits, branches without upstreams, stale or deleted worktrees.
- **Agent context** — which Claude Code / Codex session last touched which worktree, what it was doing, how to get back into it.

### Differentiator

Each piece exists separately; the combination does not:

| Tool | Multi-repo discovery | Per-worktree git state | Claude + Codex history | One-key resume in right dir |
|---|---|---|---|---|
| Claude Code `/resume`, `codex resume --all` | — | — | one agent each | yes |
| worktrunk (8.4k★), gita (1.9k★) | gita only | yes | — | — |
| agent-deck (970★), ccmanager (1.3k★) | yes | partial | live processes only | — |
| agent-sessions (881★), nicknisi/sessions (36★) | — | — | yes | copy / print line |
| xlaude (184★, dormant since 2025-11) | — | — | last prompt per worktree | opens agent |
| **rollcall** | **yes** | **yes** | **yes** | **yes** |

The README must carry this table as its "How is this different?" section (sources and URLs in `2026-09-29-rollcall-redteam.md`).

### Generic by default

- Point rollcall at any folder; it finds every git repo under it. No config, no assumed layout.
- With no agent logs, rollcall is a complete multi-repo git dashboard and shows no agent UI. Supported, but **not** the pitch.

### Audience

People running Claude Code and/or Codex across many repos and worktrees. Developers without agents are a side benefit.

### What makes it fun (author goals)

Delightful TUI, a hard technical core (safe discovery over hostile repos, incremental parsing of ~1.4 GB of logs, caching), Rust, a real user base.

### Grounding data (author's machine, measured 2026-09-29)

- `~/Projects`: 17 repos + 29 linked worktrees; 6 worktrees prunable; external worktrees under `/private/tmp` and `~/.codex/worktrees`; 2.2 M filesystem entries, depth 21, 525 symlinks.
- Claude Code: 2,012 top-level session logs + 469 subagent logs. Codex: 1,448 rows in `state_5.sqlite` `threads` (all `rollout_path`s exist), 1,403 rollouts + 46 archived. 1.35 GB of logs; largest file 45 MB; longest line 5.2 MB.
- Logs contain secrets (mostly inside tool output) and terminal escape sequences (§9).
- On macOS, `/`, `~`, and iCloud directories report the same `st_dev`.

### Decided

| Decision | Choice |
|---|---|
| Language | Rust (gix ≥ 0.88) |
| Interfaces | TUI + CLI (`status --json`) + stdio MCP server, one shared core |
| Root | Positional `[PATH]`, default = current directory; MCP: `--root` required |
| Discovery | Any git repo at any depth under the root, with safety guards (§4) |
| Agent sessions | Optional; Claude Code + Codex behind `SessionSource` |
| Scope | Full scope in v0 (MCP, Homebrew, GIF included); estimate **5–7 weeks** solo (revised from 3–5 after round 2) |
| Pitch | Agent-first |
| Architecture | Approach A: scan on demand + incremental cache (no daemon) |
| Subprocesses | None during scan (§4.4); only the explicit resume action execs the agent |
| VCS | Git only |

### Assumptions

- Claude Code and Codex log/database formats are internal and change between versions. Every source detects unrecognised shapes, degrades to partial data, and reports it in `doctor`.
- APIs named in §4.4 were checked against gitoxide source at `5708de4` (gix 0.86) and the gix 0.88.0 crate source. If any named API differs in the pinned version, the implementer stops and reports; the safety rule is never weakened to fit.

## 2. Approaches considered

- **A. Scan on demand + cache (chosen).** Zero-config, reads existing history on first run.
- **B. Daemon + live index.** Roadmap, behind the `Index` trait.
- **C. Agent hooks write a ledger.** Roadmap, optional extra source.

## 3. Components, commands, definitions

### 3.1 Commands

Positional `PATH` is the root for every command except `mcp`, which takes `--root` because MCP servers are registered as fixed argument lists.

| Command | Purpose |
|---|---|
| `rollcall [PATH]` | TUI. If stdout is not a TTY, behaves as `rollcall status PATH`. |
| `rollcall status [PATH] [--json]` | Table (plain) or `StatusDoc` JSON (§7). |
| `rollcall scan [PATH]` | Scan and print one summary line to stderr: `scanned <root>: <R> repos, <W> worktrees, <S> sessions in <T>s (<E> errors)`. |
| `rollcall doctor [PATH] [--json]` | Diagnostics; `DoctorDoc` JSON (§7). |
| `rollcall mcp --root PATH [--root PATH …] [--prompts]` | stdio MCP server (§8.3). Roots may instead come from config `roots`. |
| `rollcall init <bash\|zsh\|fish>` | Prints the optional `rc` shell function (§8.1). |
| `rollcall cache clear` | Deletes the cache directory. |
| `rollcall --version`, `--help` | Standard. |

Shared flags (TUI, `status`, `scan`, `doctor`, `mcp`): `--submodules`, `--follow-symlinks`. TUI / `status` / `scan` / `doctor` also: `--yes` (allow a refused root, §4.3), `--no-scan` (answer from cache only).

Exit codes: `0` success (and TUI `q`); `1` completed but any `Repo.error` or `Worktree.error` is non-null, the walk was partial, or no scan has ever completed for the root and none could run now (`--no-scan` or lock held: output has `scanned_at: null`, `repos: []`); `2` usage or config error; `3` root refused; `10` resume handed to the `rc` function (§8.1).

Environment: `NO_COLOR` honoured; `ROLLCALL_NOW` (RFC 3339) overrides "now" for every time computation (tests, demos); `CLAUDE_CONFIG_DIR`, `CODEX_HOME` honoured (§5); `ROLLCALL_RESUME_FILE` (§8.1).

Colours: the 16 named ANSI colours only.

### 3.2 Config (optional)

`$XDG_CONFIG_HOME/rollcall/config.toml`, default `~/.config/rollcall/config.toml` on macOS and Linux. Never required. Command-line flags override config.

| Key | Default | Meaning |
|---|---|---|
| `roots` | `[]` | Saved roots; used by `mcp` when no `--root`; count as confirmed for §4.3. |
| `skip_dirs` | `[]` | Extra directory names to skip. |
| `stale_days` | `14` | Threshold for `stale_reason = "inactive"`. |
| `redact_prompts` | `false` | Hide `title` / `last_prompt` everywhere; overrides `--prompts`. |
| `redact_emails` | `false` | Add email addresses to secret scrubbing (§9). |
| `walk_budget_entries` | `500000` | Directory entries (dirents read) before the walk stops. |
| `walk_budget_secs` | `20` | Wall-clock walk budget. |
| `repo_budget_secs` | `5.0` | Per-repo git-state budget in seconds (finite float, > 0, representable as a `Duration`) (§4.4). |
| `clipboard` | `"system"` | `"system"` (arboard) or `"osc52"`. |

Unknown keys → warning on stderr. Invalid TOML or wrong types → exit 2.

### 3.3 Definitions

"Now" is `ROLLCALL_NOW` if set, else the system clock. A scan's `scanned_at` is "now" taken at scan start; elapsed times (the `scan` summary, budgets, deadlines) use a monotonic clock. All git data is read in-process with gix (§4.4).

- **Default branch** (reported as a short local name, e.g. `main`): target of `refs/remotes/origin/HEAD` mapped to its local name; else local branch `main`; else `master`; else `null`.
- **Upstream**: `branch.<name>.remote` + `branch.<name>.merge` from repo config, resolved to a remote-tracking ref, reported as `<remote>/<branch>`. `null` if unset, unresolvable, or repo-local config is untrusted (repo owned by another user).
- **ahead / behind**: commits reachable from the branch tip and not from upstream, and vice versa, via `rev_walk().with_hidden()`. Saturate at 10,000. `null` without upstream.
- **unpushed**: with upstream → `ahead`. Without upstream but ≥ 1 remote → commits on the branch not reachable from any `refs/remotes/*` (saturating at 10,000). No remote → `null`.
- **dirty_count**: tracked paths differing from HEAD (staged or unstaged; modified, added, deleted, renamed, type-changed, conflicted), computed as the union by path of HEAD↔index changes (`tree_index_status`) and index↔worktree changes (`index_worktree_status`); a path changed in both counts once. Rename tracking applies to the HEAD↔index side only (set in code, not via config); index↔worktree rename tracking is off, so untracked files are never paired with deletions. A rename is keyed by its destination path, so a staged rename plus an unstaged edit of the destination counts once. Conflicted and intent-to-add paths come from the index↔worktree side only. Excludes untracked.
- **untracked_count**: untracked, non-ignored files. Ignore rules include the user's `core.excludesFile` (§4.4).
- **last_commit_at**: committer time of the worktree HEAD commit.
- **merged**: a linked worktree on a branch other than the default branch whose tip is an ancestor of the default branch tip. Squash merges are not detected (documented).
- **missing**: a linked worktree registered in `.git/worktrees/<name>` whose directory no longer exists. Its `branch` is read from `.git/worktrees/<name>/HEAD` (≤ 4 KiB); other git fields are `null` / `0`.
- **stale_reason** (linked worktrees only): `"missing"` if missing; else `"merged"` if merged; else `"inactive"` if `last_commit_at` is older than `stale_days` and dirty_count = 0 and untracked_count = 0 and unpushed ∈ {0, null}; else `null`.
- **Worktree attention**: `4` error; `3` unpushed > 0; `2` dirty_count > 0 or untracked_count > 0; `1` stale_reason ≠ null; `0` otherwise.
- **Repo attention** = max over its worktrees and its own `error` (→ 4).
- **last_activity_at** (repo) = max over worktrees of `last_commit_at` and matched sessions' `ended_at`; `null` if none.
- **Repo sort**: attention desc, then `last_activity_at` desc with `null` last, then path asc.
- **quiet**: a repo with attention 0 shows `quiet Nd` (whole days since `last_activity_at`) or `quiet —` when null.
- **Truncation**: title and prompt outputs are at most 200 extended grapheme clusters **including** a trailing `…` (cut at a grapheme boundary with `unicode-segmentation`). Text already ≤ 200 graphemes is unchanged, even if it already ends in `…`.

### 3.4 Workspace crates, dependency direction, interfaces

| Crate | Depends on | Responsibility |
|---|---|---|
| `rollcall-core` | — | Domain types, `Index` trait, pure functions: attention, stale, sort, matching, sanitising, scrubbing, truncation. No I/O. In-memory `Index` for tests. |
| `rollcall-git` | core | Discovery walk (§4.1–4.3) and per-worktree git state (§4.4). |
| `rollcall-sessions` | core | `SessionSource` impls for Claude and Codex (§5), including incremental merge and text resolution. |
| `rollcall-store` | core | SQLite `Index` implementation (§6). |
| `rollcall-cli` | all | Commands, scan orchestration, TUI, MCP, shell init. Integration tests in `rollcall-cli/tests/`. |

`rollcall-git`, `rollcall-sessions`, `rollcall-store` never depend on each other. Interfaces (Rust-flavoured; exact generics left to the plan):

```rust
// rollcall-core
enum SourceKind { Claude, Codex }
enum TextRef {
    LogLine { dev: u64, inode: u64, size_at_scan: u64, offset: u64, len: u32, field: TextField },
    CodexThread { thread_id: Uuid, field: TextField },
}
enum TextField { Title, LastPrompt }
enum TitleKind { Custom, Ai, Thread, Prompt }        // provenance, used by MCP (§8.3)
struct Session {
    source: SourceKind, id: Uuid, cwd: PathBuf, branch: Option<String>,
    started_at: Timestamp, ended_at: Timestamp,
    title: Option<(TextRef, TitleKind)>, last_prompt: Option<TextRef>,
    message_count: u32, backing: Backing,             // the file or thread row the session lives in
}
struct FileState { dev: u64, inode: u64, path: PathBuf, size: u64, mtime: Timestamp,
                   offset: u64, head_fp: [u8; 16], tail_fp: [u8; 16], parser_version: u32 }
struct SourceState { files: Vec<FileState>, sessions: Vec<Session> }   // prior scan, this source
struct SourceScan  { files: Vec<FileState>, sessions: Vec<Session>, diag: SourceDiagnostics, notes: Vec<String> }  // notes → DoctorDoc.notes

trait Index {
    fn source_state(&self, s: SourceKind) -> SourceState;
    fn repos_for_root(&self, root: &Path) -> Vec<RepoRow>;      // via the root-membership table (§6.1)
    fn sessions_for(&self, root: &Path, worktree: &Path) -> Vec<Session>;  // sessions matched by this root's last scan, ended_at desc
    fn scanned_at(&self, root: &Path) -> Option<Timestamp>;
    fn scanned_submodules(&self, root: &Path) -> Option<bool>;    // the --submodules flag of that scan (§6.1)
    fn last_diagnostics(&self, root: &Path) -> Option<ScanDiagnostics>;  // for doctor under contention
    fn commit_scan(&mut self, root: &Path, scan: ScanResult) -> Result<()>;   // one transaction
}
fn match_sessions(worktrees: &[WorktreeKey], sessions: &[SessionLocation]) -> Vec<MatchResult>;
// WorktreeKey { canonical_path, hidden: bool, missing: bool }   SessionLocation { cwd_canonical, cwd_exists, under_root }

// rollcall-sessions
trait SessionSource {
    fn kind(&self) -> SourceKind;
    fn scan(&self, prior: &SourceState) -> SourceScan;      // owns incremental merge (§6.3)
    fn resolve(&self, r: &TextRef) -> Option<String>;       // raw text; caller sanitises/scrubs/truncates
}

// rollcall-git
fn discover(root: &Path, opts: &WalkOptions) -> Discovery;          // repos + diagnostics
fn repo_state(repo: &DiscoveredRepo, opts: &GitOptions) -> RepoState; // bounded by repo_budget_secs
```

Scan orchestration (cli): `discover` → `repo_state` per repo (parallel) → each source `scan(index.source_state(..))` → canonicalise session cwds and check existence → `core::match_sessions` → `index.commit_scan`. Rendering (cli): read rows from `Index`, `resolve` text refs, then core sanitise → scrub → truncate → output types.

## 4. Discovery and git state

### 4.1 Walk

Parallel walk of the root, any depth, directories only. Every file opened during discovery is opened with `O_NOFOLLOW | O_NONBLOCK` and `fstat`-checked as a regular file before reading.

- **`.git` directory** (by `lstat`; symlink only with `--follow-symlinks`) → repo; working tree = parent.
- **`.git` regular file** → read ≤ 4 KiB; must contain `gitdir: <path>` (relative resolves against the file's directory); target must be an existing directory, else an error row.
  - Target under `<X>/.git/worktrees/<name>` → **linked worktree** of repo X.
  - Target under `<X>/.git/modules/…` → **submodule** of repo X.
  - Otherwise → opened as a repo; `doctor` notes the layout.
- **Old-style submodule** (`.git` directory at a path listed in the enclosing repo's `.gitmodules`) → submodule.
- **Bare repo**: a directory not named `.git` with a regular file `HEAD` and directories `objects/` and `refs/`, which gix opens as bare. Branch/ref info only. Fixtures inside other repos will appear; expected and documented.
- **Nested repos**: the walk continues into a repo's working tree (never its `.git`); an inner repo is its own row, `kind: "nested"`.
- **Submodules**: grouped under the parent; omitted unless `--submodules`.
- **Linked worktrees** are enumerated from each repo's `.git/worktrees/*`.
- **Worktree trust rule** (applies to every worktree: main, linked, enclosing, nested). Git state is computed only if all hold: (a) after the hardened open (§4.4), `repo.workdir()` realpath equals the directory discovery found (so `core.worktree` cannot redirect it); (b) the repo's common dir resolves to the `.git` of the repo that enumerated it (so `commondir` cannot redirect it) — checked **before** open (revision 11): rollcall reads the pre-open-checked `commondir` file itself, resolves it against the git dir and compares realpaths; for a main or bare repo a `commondir` file must be absent or resolve to the git dir itself; (c) for linked worktrees, the `.git` file's `gitdir:` resolves back to the same `.git/worktrees/<name>` entry; (d) the workdir realpath is not `/`, not `$HOME`, not an ancestor of the root, and not under a skipped path (§4.2). Otherwise the worktree is listed with `error: "untrusted worktree path"` and null git fields.
- External cases:
  - **A — worktree outside root, main inside:** listed under its repo, `external: true`.
  - **B — worktree inside root, main outside:** its repo is a row with `kind: "external_main"`; worktrees outside the root carry `external: true`.
- **Enclosing repo**: if the root itself lies inside a repo's working tree, that repo is included as a row with `contains_root: true`. It is found by walking up from the root's parent with `lstat` only, checking each directory for `.git`, and stopping **before** examining `$HOME` or `/` (a `$HOME/.git` is never treated as an enclosing repo).
- **Error strings** are fixed prefixes, compared by prefix in tests: `"open failed: "` (gix could not open the repo), `"bad gitdir: "` (unusable `.git` file), `"untrusted git dir"` (§4.4 pre-open checks), `"untrusted worktree path"`, `"timed out"`, `"git error: "` (any other gix error during state computation). Any repo-level failure (pre-open check, open, or timeout before worktrees were enumerated) is a `Repo` row with `error` set and `worktrees: []`; a worktree-level failure sets `Worktree.error` with git fields `null` / `0`.

### 4.2 Skips

- Directory names anywhere: `node_modules`, `target`, `.venv`, `venv`, `dist`, `build`, `.cache`, `.Trash`, `__pycache__`, plus config `skip_dirs`.
- Paths (compared by realpath): `$HOME/Library` (macOS), `$HOME/.local/share/Trash` (Linux). An explicit root **under** a skipped path is allowed; the skip applies to descendants only when the root is outside it.
- macOS: entries with the `SF_DATALESS` flag (cloud placeholders) are skipped without being opened.
- `.gitignore` is not used for discovery.

### 4.3 Guards

- **Different device:** never descend into a directory whose `st_dev` differs from the root's. `doctor` lists skipped mount points. (On macOS this does not exclude iCloud — the path skip in §4.2 does.)
- **Refused roots:** a root whose `(dev, inode)` equals that of `/` or `$HOME` → exit 3 unless `--yes` or listed in config `roots`. The message lists up to 5 immediate subdirectories of the root that contain a `.git` within 2 levels, as `rollcall <dir>` suggestions. This peek uses `lstat` only, applies the §4.2 skips (and additionally skips `Desktop`, `Documents`, `Downloads`, `Pictures`, `Movies`, `Music` directly under `$HOME` on macOS to avoid privacy prompts), `SF_DATALESS`, and the same-device rule, and stops after 2 s. MCP applies the same refusal to each `--root`.
- **Walk budget:** stop after `walk_budget_entries` dirents or `walk_budget_secs`; `partial: true`; exit 1.
- **Symlinks:** not followed by default. With `--follow-symlinks`, only targets whose realpath is under the root, with a visited `(dev, inode)` set.
- **Unreadable directories** are counted and skipped.
- **Paths** are canonicalised with realpath where they exist; otherwise lexically normalised.

### 4.4 Untrusted repositories

rollcall must be safe on folders of cloned or unpacked repos that may be hostile, including committed bare repos such as `fixtures/foo.git`.

Every repo — including each submodule and each linked worktree — is opened through the **hardened open** below; nothing is opened any other way.

- **No subprocesses** during scan. gix implements neither hooks nor fsmonitor; filter, diff and merge drivers are removed below; submodules are never opened implicitly by gix.
- **Pre-open checks** (before calling gix): in the git dir, `lstat` `config`, `HEAD`, `packed-refs`, `index`, `commondir`, `gitdir`, `shallow`, `info/exclude`, `info/attributes`, `objects/info/alternates`, `objects/info/commit-graphs/commit-graph-chain`, `config.worktree` (also in each `worktrees/<name>/` admin dir), and every entry under `refs/` (stop after 10,000 entries). Revision 11 (P14 audit): the same list also runs on each linked worktree's admin dir (its git dir) and on the resolved common dir; and in each of those dirs every entry whose name consists only of `A`–`Z` and `_` (gix reads such pseudo-ref names through symbolic refs, e.g. `HEAD` holding `ref: EVIL`) must be a regular file (≤ 4 KiB for `HEAD`, ≤ 1 MiB for the others — `FETCH_HEAD`, `COMMIT_EDITMSG`, `MERGE_MSG` legitimately exceed 4 KiB), and every entry under `worktrees/<name>/refs/` is checked like a loose ref (sharing the 10,000-entry stop). In the worktree root (non-bare repos), `lstat` `.gitmodules`, `.gitignore`, `.gitattributes`. Any of these that is a symlink, is not a regular file (or directory, for `refs/` subdirs), or exceeds its cap — `config`, `config.worktree`, `shallow`, `info/*`, `objects/info/alternates`, `commit-graph-chain`, `commondir`, `gitdir`, and the three worktree-root files 1 MiB; `HEAD` and loose refs 4 KiB; `packed-refs` 64 MiB; `index` 256 MiB — makes the repo an `error: "untrusted git dir"` row with no open. Files that don't exist are fine.
- **Alternates chain:** pre-open follows `objects/info/alternates` itself: each hop's `info/alternates` is checked as above (regular file ≤ 1 MiB), at most 8 hops; otherwise `untrusted git dir`.
- **Commit-graph:** config override `core.commitGraph=false`, and rollcall's rev-walk platform disables commit-graph use.
- **Config:** `gix::open_opts` with `Config::isolated()` permissions (no system, user, or environment config; no includes; no git-binary lookup). Repo-local config is still read. The user's `core.excludesFile` is read by rollcall itself (from `~/.gitconfig`, then `$XDG_CONFIG_HOME/git/config`, no includes, same pre-open checks and 1 MiB cap) and passed as a config override.
- **Config stripping:** immediately after opening, through `config_snapshot_mut()`, remove **all** sections named `filter`, `diff`, `merge` (every subsection, from every source — loop over section ids, since one removal call removes one section) and the keys `core.attributesFile`, `core.excludesFile` (revision 11), `core.worktree`, `core.sshCommand`, `core.hooksPath`, `core.fsmonitor`; then set `core.excludesFile` to the user's excludes file found above, or to `/dev/null` when there is none (so gix never reads a repo-local value or its XDG default); then commit the snapshot so the resolved config gix uses is the stripped one.
- **Allocation limit:** `gitoxide.objects.allocLimit` = 64 MiB (via config override at open).
- **Submodules:** status is driven through `Repository::index_worktree_status()` with a rollcall `SubmoduleStatus` implementation that returns `Ok(None)`, so gix never opens a submodule and never reads `.gitmodules` during status (submodule handling "given, ignore all" — `Submodule::Given { ignore: All, check_dirty: false }` in gix 0.86 terms). Submodule rows (shown with `--submodules`) come from opening each submodule's git dir through this same hardened open.
- **Ownership:** repos owned by another uid open at gix's reduced trust; repo-local config is then not used, so `upstream`/`ahead`/`behind` are `null`. Documented.
- **HEAD tree bound:** before `tree_index_status` (whose `index_from_tree` expands the whole HEAD tree into memory, storing every entry's full path, and takes no interrupt flag), rollcall walks the HEAD tree itself through the hardened repo, iteratively (never recursively), counting expanded entries and the sum of their full-path byte lengths and tracking depth, checking the deadline every 1,024 entries. It stops at 1,000,000 entries, 128 MiB of path bytes, or depth 4,096 → `error: "git error: HEAD tree too large"`. Repeated subtrees count every time they are reached, so a small tree bomb hits a cap quickly. The walked tree id is the one passed to `tree_index_status`.
- **Per-repo deadline:** `repo_state` for each repo runs on a worker thread with a deadline of `repo_budget_secs`, measured from before the pre-open checks. gix's interrupt flag is set at the deadline; it stops `index_worktree_status` (including its dirwalk) but not `tree_index_status`. For `tree_index_status`, the HEAD tree bound limits the tree expansion, and rename similarity is limited by `Rewrites { copies: None, percentage: Some(0.5), limit: 1000 }` (at most 1,000 pair checks) and the allocation limit. rollcall's own loops (HEAD tree bound, rev-walk) check the deadline every 1,024 entries / 256 commits. A result that arrives at or after the deadline is discarded. If the thread has not returned at the deadline, the row gets `error: "timed out"`, the thread is abandoned (not joined), and `doctor` counts it in `repo_timeouts_abandoned`.
- **Consequences (documented):** files using clean/smudge filters (e.g. Git LFS, custom eol) may count as dirty where `git status` says clean; repo-local `core.attributesFile` is ignored.
- **Documented limitation:** untracked `.gitignore` / `.gitattributes` files in **subdirectories** of an unpacked (not cloned) hostile tree are read by gix without a size cap; a symlink there to `/dev/zero` can exhaust memory.
- **Maintenance rule:** the pre-open list is a denylist against gix's internal read sites. The round-4 audit at gitoxide `5708de4` missed `config.worktree` (read uncapped when `extensions.worktreeConfig` is set; found by the plan red-team and confirmed in gix 0.88.0 `config/cache/incubate.rs`), so the list is re-audited against the pinned version before implementation. Any gix version bump requires re-auditing the `fs::read` / `read_to_end` / `read_to_string` / `io::copy` / `BufReader::lines` call sites and the process-spawn sites (`gix-command`, filter drivers) reachable from open, `tree_index_status`, `index_worktree_status` and rev-walk before release.
- gix errors on one repo produce an `error` row (§4.1 prefixes); the scan continues.

## 5. Session sources

Sources are read-only on agent data: they never modify agent files or databases (SQLite shared locks and `-shm` updates on WAL databases are expected and allowed).

### 5.0 Common rules

- Session ids must match `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`; otherwise skipped and counted.
- Log files are opened `O_NOFOLLOW | O_NONBLOCK`, `fstat`-checked as regular files, and read only up to the size observed at open.
- JSONL is streamed; lines > 1 MiB are skipped and counted without full buffering; malformed lines and unknown record types are skipped and counted.
- A session's `cwd` is the latest cwd it recorded, except where a source rule says otherwise.
- Subagent / child sessions are excluded from session rows and counted.
- **Prompt text filtering**, applied per text block: drop a block whose first non-whitespace character is `<`, or that starts with `# AGENTS.md`; within a kept block, cut at the first occurrence of `<task-notification>` or `<system-reminder>`; trim; empty → no prompt. Known false negative: prompts genuinely starting with `<`.
- Sessions whose backing file or thread row no longer exists are dropped at the next scan and counted as `vanished`.

### 5.1 Claude Code

- **Root:** `$CLAUDE_CONFIG_DIR/projects` if set, else `~/.claude/projects`.
- **Files:** `projects/*/*.jsonl` (one session per file). `projects/*/<sessionId>/subagents/*.jsonl` excluded and counted.
- **Worked records** (values illustrative; `timestamp` is RFC 3339 with milliseconds and `Z`):

```json
{"type":"user","sessionId":"<uuid>","timestamp":"2026-09-28T18:04:11.532Z","cwd":"/home/u/p/app","gitBranch":"main","isSidechain":false,"message":{"role":"user","content":"fix the flaky test"}}
{"type":"user","sessionId":"<uuid>","timestamp":"2026-09-28T18:04:20.001Z","cwd":"/home/u/p/app","gitBranch":"main","isSidechain":false,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"…"}]}}
{"type":"user","sessionId":"<uuid>","timestamp":"2026-09-28T18:05:00.000Z","cwd":"/home/u/p/app","gitBranch":"HEAD","isMeta":true,"message":{"role":"user","content":[{"type":"text","text":"<command-name>/clear</command-name>"}]}}
{"type":"assistant","sessionId":"<uuid>","timestamp":"2026-09-28T18:04:15.000Z","cwd":"/home/u/p/app","gitBranch":"main","message":{"role":"assistant","content":[{"type":"text","text":"…"}]}}
{"type":"custom-title","sessionId":"<uuid>","customTitle":"Flaky test hunt"}
{"type":"ai-title","sessionId":"<uuid>","aiTitle":"Fix flaky e2e test"}
{"type":"last-prompt","sessionId":"<uuid>","leafUuid":"<uuid>","lastPrompt":"now run it 20 times"}
```

- **id:** `sessionId` of the first record carrying one; fallback the filename stem.
- **cwd:** latest record with `cwd`. **branch:** latest record with `gitBranch`; `"HEAD"` → `null` (detached); missing → `null`.
- **started_at / ended_at:** min / max `timestamp`.
- **Human prompt record:** `type == "user"`, `isMeta` not true, `isSidechain` not true, and `message.content` is a string or an array whose blocks are all `type == "text"`; then the §5.0 filter per block (string content = one block); kept blocks joined with a space.
- **title:** latest `customTitle` (`TitleKind::Custom`); else latest `aiTitle` (`Ai`); else first human prompt (`Prompt`); else `null`.
- **last_prompt:** latest non-empty `lastPrompt` (it may already be truncated with `…`; §3.3 leaves it unchanged); else latest human prompt; else `null`. A session may have `last_prompt` set and `message_count` 0; that is valid.
- **message_count:** number of human prompt records.

### 5.2 Codex

- **Root:** `$CODEX_HOME` if set, else `~/.codex`.
- **Primary — thread database:** highest-numbered `state_<N>.sqlite` whose `threads` table has columns `id, rollout_path, created_at, updated_at, source, cwd, title, git_branch, archived, first_user_message`. Opened with URI `mode=ro` (never `immutable=1`, which can return corrupt results on a live WAL database), `busy_timeout` 2000 ms, short read transactions. If it cannot be opened, fall back to rollouts and report why in `doctor`.
  - **Exclude** (count as children) a thread if its `id` is a `thread_spawn_edges.child_thread_id`, **or** its `source` column parses as JSON containing key `subagent`, **or** its rollout's `session_meta.payload.parent_thread_id` is set.
  - `rollout_path` must canonicalise to a path under the Codex root; otherwise the thread's rollout is ignored (counted).
  - `cwd` ← `threads.cwd` (the database's current cwd for the thread). `branch` ← `threads.git_branch`; if empty, the rollout's `session_meta.payload.git.branch`; else `null`.
  - `started_at` ← `created_at`, `ended_at` ← `updated_at` (Unix seconds; archiving does not change `updated_at`, which is intended).
  - `title` ← `threads.title` when non-empty. Its kind is `Prompt` if the title, trimmed and with a trailing `…`/`...` removed, is a prefix or substring of `first_user_message`, or is longer than 200 graphemes; otherwise `Thread`. Empty title → `first_user_message` after the §5.0 filter (`Prompt`); else `null`. (Measured: of 1,448 real threads, 989 titles equal the first message and 20 are a truncated prefix of it — all `Prompt`.)
  - `last_prompt`, `message_count` ← from the rollout (below), incremental; missing rollout → `null` / `0`.
- **Fallback — rollouts only:** `sessions/**/rollout-*.jsonl` and `archived_sessions/*.jsonl`. `id`, `started_at` from the first `session_meta.payload` (`id`, `timestamp` RFC 3339); `cwd` from latest `turn_context.payload.cwd`, else `session_meta.payload.cwd`; `branch` from `session_meta.payload.git.branch`, else `null`; `ended_at` = max top-level `timestamp`; files whose `session_meta.payload.parent_thread_id` is set are children; a second `session_meta` with a different id is ignored (session belongs to the first). `title` = first human prompt (`Prompt`).
- **Worked rollout records:**

```json
{"timestamp":"2026-09-28T18:04:11.532Z","type":"session_meta","payload":{"id":"<uuid>","timestamp":"2026-09-28T18:04:11.500Z","cwd":"/home/u/p/app","git":{"branch":"main","commit_hash":"…","repository_url":"…"}}}
{"timestamp":"2026-09-28T18:04:12.000Z","type":"turn_context","payload":{"cwd":"/home/u/p/app","model":"…"}}
{"timestamp":"2026-09-28T18:04:12.100Z","type":"event_msg","payload":{"type":"user_message","message":"fix the flaky test"}}
{"timestamp":"2026-09-28T18:04:12.100Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>…</environment_context>"},{"type":"input_text","text":"fix the flaky test"}]}}
```

- **Human prompt (rollouts):** an `event_msg` with `payload.type == "user_message"` → `payload.message`. Only in files with **no** `user_message` events: `response_item` with `payload.type == "message"` and `payload.role == "user"` → `input_text` blocks through the §5.0 filter, kept blocks joined with a space. `message_count` counts human prompts.
- `doctor` reports the mode and database version.

## 6. Store, freshness, concurrency

### 6.1 Location and permissions

- Cache directory (computed directly, no directory crate): macOS `~/Library/Caches/rollcall`, Linux `$XDG_CACHE_HOME/rollcall` (default `~/.cache/rollcall`). Directory `0700`; `cache.sqlite` and its `-wal`/`-shm` files `0600`, set explicitly regardless of umask.
- One database for all roots. Repos/worktrees keyed by canonical path. A **root-membership table** `(root, repo_path, kind, contains_root)` records which repos a root's scan produced — including the enclosing repo, `external_main` repos, and worktrees outside the root — since `kind` and `contains_root` depend on the root. `external` is computed per root from worktree path vs root. Sessions keyed by `(source, id)`, shared across roots; session → worktree matches are stored per root together with that scan's `--submodules` flag, since candidates depend on both (§6.5); an answer from the cache whose flag differs from the current invocation's is `stale: true`. The last scan's diagnostics are stored per root for `doctor`.

### 6.2 What is stored

- Git state per worktree (§7 fields) and file states (`FileState`, §3.4).
- **No free text from logs is stored.** Titles and prompts are `TextRef` pointers. Resolution (`SessionSource::resolve`): open `O_NOFOLLOW | O_NONBLOCK`, `fstat` regular file, require the same `(dev, inode)` and current size ≥ `size_at_scan`, read `len` (≤ 1 MiB) bytes at `offset`, re-parse the JSON line, and re-apply §5 extraction; any mismatch → `null`. Codex thread refs re-read the row by id.
- The MCP server without `--prompts` never resolves `LastPrompt` refs and resolves `Title` refs only per §8.3.

### 6.3 Incremental reads (owned by each `SessionSource::scan`)

For each log file, compare with the prior `FileState` of the same `(dev, inode)`:

- `head_fp` = 128-bit hash (e.g. xxh3-128) of the first line (≤ 4 KiB); `tail_fp` = hash of the 64 bytes ending at `offset`.
- No prior state, `head_fp` differs, `tail_fp` differs, size < offset, or `parser_version` differs → re-read from 0 and rebuild the session.
- Otherwise read from `offset` and **merge** into the prior session: `started_at` = min, `ended_at` = max, `cwd`/`branch`/`last_prompt` = latest, `title` precedence re-evaluated (a new custom/ai title replaces a prompt-derived one; the first-prompt ref is kept from the prior session), `message_count` += new.
- Same `(dev, inode)` at a new path with matching fingerprints (e.g. Codex archiving) → update `path`, continue incrementally.
- Partial trailing line → stop before it; `offset` not advanced past it.

### 6.4 Freshness and concurrency

- Every command except `--no-scan` runs a scan before answering: git state is always re-read; logs are read incrementally (§6.3).
- SQLite WAL, `busy_timeout = 5000 ms`. Each scan commits in one transaction; a crash mid-scan leaves the prior state.
- One scanner per root: `flock` on `<cache>/scan-<hex SHA-1 of the canonical root>.lock` (SHA-1 from `gix-hash`, already a dependency). A process that cannot take the lock answers from the cache.
- **`stale: true`** when the answer did not come from a scan completed by this invocation (lock held elsewhere, `--no-scan`, or MCP budget exceeded).
- **TUI first paint:** if the cache has rows for the root, render them immediately with `stale` shown, then refresh when the scan completes; otherwise show a `scanning… <N> repos found` line until it completes.
- **MCP:** if the cache already has rows for the root, a tool call waits ≤ 2 s for the scan, else answers stale. If it has none, the call waits for the first scan (bounded by `walk_budget_secs` + 30 s).
- `PRAGMA user_version` mismatch → delete and rebuild the cache.

### 6.5 Session → worktree matching

- Candidates: worktrees under the root, excluding hidden submodules (without `--submodules`) and `missing` worktrees whose path is not the session cwd.
- Canonical session cwd → longest component-wise prefix among candidate canonical paths. A session whose cwd equals a missing worktree's path matches it.
- A matched session attaches regardless of branch; its own `branch` is shown.
- Unmatched: `outside_root`, `path_gone` (cwd doesn't exist), `non_repo` (exists under the root, in no worktree).
- A worktree's `last_session` = matched session with greatest `ended_at`.

## 7. Output schemas

Timestamps are RFC 3339 UTC. JSON Schemas for every document below are committed under `schema/` and validated in tests with the `jsonschema` crate. `schema_version` is bumped on incompatible change.

```
StatusDoc {
  schema_version: 1, root: string, scanned_at: string | null,   // null: no scan ever completed
  stale: bool, partial: bool,
  repos: Repo[]                                  // sorted per §3.3
}

Repo {
  name: string,                                  // basename of working tree / bare dir
  path: string,
  kind: "normal" | "nested" | "bare" | "external_main",
  contains_root: bool,
  default_branch: string | null,
  has_remote: bool,
  attention: 0..4,
  last_activity_at: string | null,
  error: string | null,
  worktrees: Worktree[],                         // main first, then linked by path; [] for bare
  submodules?: Repo[]                            // only with --submodules
}

Worktree {
  path: string, is_main: bool, external: bool,
  branch: string | null, detached: bool, head: string | null,
  last_commit_at: string | null,
  upstream: string | null, ahead: int | null, behind: int | null, unpushed: int | null,
  dirty_count: int, untracked_count: int,
  stale_reason: "missing" | "merged" | "inactive" | null,
  attention: 0..4, error: string | null,
  last_session: Session | null,
  older_sessions: int                            // count of other matched sessions
}

Session {
  source: "claude" | "codex", id: string, branch: string | null, cwd: string,
  started_at: string, ended_at: string,
  title: string | null,
  title_kind: "custom" | "ai" | "thread" | "prompt" | null,   // null iff title is null
  last_prompt: string | null,
  message_count: int,
  resume: string | null                          // display line, §8.1; null if cwd gone
}

WhereWasIDoc {
  schema_version: 1, root: string, scanned_at: string | null, stale: bool, partial: bool,
  truncated: bool, repos: Repo[], recent_sessions: Session[]
}

ListStaleDoc {
  schema_version: 1, root: string, scanned_at: string | null, stale: bool, partial: bool,
  truncated: bool,
  items: [{ repo_name: string, repo_path: string, worktree_path: string, branch: string | null,
            stale_reason: "missing" | "merged" | "inactive" }]
}

DoctorDoc {
  schema_version: 1, root: string, scanned_at: string | null,   // from the last stored diagnostics
  walk: { entries: int, budget_hit: bool, stopped_at: string | null,
          unreadable_dirs: int, skipped_dirs: int, skipped_mounts: string[] },
  repos_by_kind: { normal: int, nested: int, bare: int, external_main: int },
  worktrees: { total: int, external: int, missing: int, untrusted_path: int },
  submodules: int, repo_errors: int, untrusted_git_dirs: int,
  repo_timeouts: int, repo_timeouts_abandoned: int,
  notes: string[],                              // e.g. why a source fell back or is absent; unusual repo layouts (§4.1)
  sources: [{
    source: "claude" | "codex",
    mode: "jsonl" | "db" | "rollouts" | "absent",   // absent: source root missing or contains no session files/DB
    db_version: int | null,
    files: int, records: int,
    skipped: { malformed: int, unknown_type: int, oversize: int, non_uuid: int, bad_rollout_path: int },
    sessions: int, excluded_children: int, vanished: int, matched: int,
    unmatched: { outside_root: int, path_gone: int, non_repo: int }
  }]
}
```

## 8. UX

### 8.1 TUI

```
 rollcall ─ ~/Projects ─ 17 repos · 29 worktrees · scanned 4s ago                            [r]escan
┌ Repos ─────────────────────────┐┌ CRM ──────────────────────────────────────────────────────────────┐
│▶ CRM          ↑3 ●2  2 stale   ││ main                          ✓ clean       codex  1d "fix flaky e2e…" │
│  sutra        ●5  no-upstream  ││ ui-refine/css-consolidation   ●2 ↑3         claude 2h "consolidate dar…"│
│  agentrec     ↑1               ││ chore/post-pr1-housekeeping   merged·stale  claude 3d "post-PR1 clean…" │
│  games        quiet 9d         ││ ../tmp/crm-spike         ext  missing                                   │
│  uv           quiet 21d        ││                                                                        │
│                                ││ Older sessions (ui-refine/css-consolidation)                           │
│                                ││  claude 1d  "split tokens file…"   codex 4d  "audit dark mode…"         │
└────────────────────────────────┘└────────────────────────────────────────────────────────────────────┘
 j/k move · enter expand · tab pane · / filter · R resume · Y copy resume · c copy cd · o open · r rescan · ? help · q quit
```

- **Each worktree row shows its `last_session` inline** (agent, age, title). The lower pane lists older sessions of the selected worktree.
- Keys: `j`/`k`/arrows move; `enter` expands/collapses; `tab` switches pane; `/` filters repos and branches by case-insensitive substring (Esc clears); `r` rescans; `?` help; `q` quits (exit 0).
- **`R` (resume, default):** **before** changing directory, resolves the agent binary (`claude` or `codex`) to an absolute path by searching only the **absolute** entries of `PATH` (empty and relative entries are ignored). Then it restores the terminal, `chdir`s to the session cwd, and `execv`s that absolute path with `--resume <id>` (Claude) or `resume <id>` (Codex), argv built from the validated id; no shell. The user's shell stays in its original directory after the agent exits (documented; `rc` changes that). Refused with a status message if the cwd no longer exists or no absolute agent path is found. The README notes that the resumed agent's own workspace-trust prompts still apply inside untrusted repos.
- **`R` with `rc`:** `rollcall init <shell>` prints a function `rc` that creates a private temp file (`mktemp`, mode 0600), runs `ROLLCALL_RESUME_FILE=<file> command rollcall "$@"`, and only if the exit code is 10: reads three NUL-separated fields (`cwd`, `source`, `id`) from the file (bash/zsh `read -r -d ''`, fish `read -z`); checks `source ∈ {claude, codex}` and `id` against the UUID regex; resolves the agent with `command -v` and requires an absolute result (`/*`) **before** `cd`; then runs `cd -- "$cwd" && "$bin" --resume "$id"` (or `"$bin" resume "$id"`) from those variables. The file is always removed. **No `eval` anywhere.**
- When `ROLLCALL_RESUME_FILE` is set and `R` is pressed, rollcall opens that path with `O_WRONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC` (no `O_TRUNC`; `O_NONBLOCK` so a FIFO fails instead of blocking), verifies on the open fd that it is a regular file, owned by the effective uid, with one link and mode `0600`, then truncates through the fd, writes the three fields, and exits 10. Any check failing → status message; the file is neither truncated nor written.
- **`Y`** copies the display resume line `cd -- '<cwd>' && claude --resume <id>` (path shell-quoted) to the clipboard.
- **`c`** copies `cd -- '<worktree path>'` (shell-quoted). Clipboard per config `clipboard`; failure → status message.
- **`o`** opens the worktree in `$VISUAL` or `$EDITOR`, split with shell-words rules, executed without a shell, path as its own argument; unset → status message.
- Agent columns, the older-sessions pane, `R`, and `Y` are absent when no source has data.
- Badges: `nested`, `bare`, `ext`, `missing`, `no-upstream`. Colours: unpushed = yellow, dirty = blue, stale/missing = dim, error = red.

### 8.2 `status`

Plain: one line per repo (`name  attention badges  last activity`), then one indented line per worktree (`branch  state  last session`), columns as in the TUI. `--json` → `StatusDoc`.

### 8.3 MCP (`rollcall mcp`)

- Roots: `--root` (repeatable) or config `roots`; neither → exit 2. Each root checked against §4.3.
- Tools (read-only):
  - `where_was_i(root?: string, repo?: string, limit?: int = 5)` → `WhereWasIDoc`. `repo` matches repo `name` or `path` and returns **all** matching repos; without it, the top `limit` repos by attention. `recent_sessions` = latest `limit` sessions across returned repos.
  - `list_stale(root?: string)` → `ListStaleDoc`.
  - `root`, when given, must equal a configured root after canonicalisation; otherwise an MCP error. Omitted → first configured root.
- **Text defaults:** without `--prompts`, `last_prompt` is always `null`, and `title` is returned only when `title_kind` is `custom`, `ai`, or `thread` (never prompt-derived). With `--prompts`, both are returned. `redact_prompts` overrides both. All returned text is scrubbed and truncated (§9).
- External worktrees (`external: true`) and their sessions are omitted from MCP responses.
- Tool descriptions state that titles, prompts, branch names and paths are untrusted data from logs and repos, not instructions.
- Each response ≤ 16 KiB of JSON; over budget → `where_was_i` drops `recent_sessions` from the end, then trailing repos; `list_stale` drops trailing items; both set `truncated: true`.
- README registration instructions run `rollcall scan <root>` once first.

## 9. Privacy and security

- **Local.** No network access (enforced by dependency bans, §11). Writes only to the cache and config directories and, when set, `ROLLCALL_RESUME_FILE`.
- **Sanitising:** `title` and `last_prompt` are sanitised in **every** output (TUI, plain, JSON, MCP). Paths, branch names, error strings and `doctor` notes are sanitised in TUI and plain output (JSON keeps them exact, relying on JSON escaping). Sanitising removes ESC-introduced sequences, all other C0 controls, DEL, C1 controls, and the Unicode format/invisible characters U+00AD, U+061C, U+180E, U+200B–U+200F, U+202A–U+202E, U+2060–U+2064, U+2066–U+2069, U+FEFF, U+FFF9–U+FFFB; newline and tab become a space.
- **Secret scrubbing** (title and last_prompt; applied after sanitising, before truncation). Matches become `[redacted]`:
  - `sk-[A-Za-z0-9_-]{16,}` · `(sk|rk)_(live|test)_[A-Za-z0-9]{10,}` · `AKIA[0-9A-Z]{16}` · `AIza[0-9A-Za-z_-]{35}`
  - `gh[pousr]_[A-Za-z0-9]{36,}` · `github_pat_[A-Za-z0-9_]{20,}` · `npm_[A-Za-z0-9]{30,}` · `xox[abprs]-[A-Za-z0-9-]{10,}`
  - `eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+` (JWT) · `Bearer\s+\S+` · `-----BEGIN [A-Z ]*PRIVATE KEY-----`
  - `(?i)\b(password|passwd|secret|token|api[_-]?key)\s*[=:]\s*\S+`
  - credentials in URLs: `[A-Za-z][A-Za-z0-9+.-]*://[^/\s:@]+:[^@\s/]+@`
  - with `redact_emails = true`: `[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}`
  - Branch names, paths and errors are sanitised, never scrubbed.
- **Truncation:** §3.3.
- **Cache:** no log text; `0700` / `0600`.
- **Untrusted repos:** §4.4. **Filesystem guards:** §4.1–4.3. **Log reads:** §5.0, §6.2.

## 10. Testing

### 10.1 Fixture manifest and oracles

- `tests/fixtures/MANIFEST.toml` is **hand-written** and committed before the code it tests. It declares every fixture repo, worktree, commit (fixed author, committer and dates), remote, file state, and synthetic session (with its expected derived fields).
- A generator (test-only code in `rollcall-cli/tests/it/support.rs`, independent of rollcall's own logic) builds fixture roots from the manifest in temp directories: `F` (full), `F_clean` (`F` minus every fixture expected to carry an `error` or trigger `partial`: corrupted repo, bad `gitdir`, untrusted git dir, untrusted worktree path, hostile repos, hang probe, unreadable dir), `H` (hostile and hang-probe repos only, §10.2), and `EXT` (outside-root targets).
- CI installs `zsh` and `fish` on Linux runners so shell tests run on both OSes.
- Expected outputs `tests/expected/*.json` are **hand-written** from the manifest and the spec, never generated by running rollcall. Paths are written as `$F/…`, `$EXT/…`; the harness canonicalises both sides before substitution and comparison.
- All tests set `ROLLCALL_NOW` to a fixed instant and `HOME`, `XDG_*`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME` to temp directories.

### 10.2 Cases

- **Discovery:** normal; linked worktree; case A; case B; missing worktree; worktree whose `gitdir` points at `$HOME` (untrusted path); modern and old-style submodule; bare; nested; repo in a `.gitignore`d folder; repos under `node_modules/` and `target/`; symlink loop (a fan-out: two sibling links to an ancestor, which grows exponentially without the visited set); symlink to outside the root; unreadable dir (skipped with a logged reason when euid = 0); root inside a repo (enclosing row); refused-root suggestions.
- **Git state:** dirty (staged, unstaged), untracked, user `core.excludesFile` honoured, ahead/behind, no upstream with remote, no remote, detached HEAD, merged, inactive, no default branch, corrupted repo.
- **Deadline:** a **separate run** on `F_clean` with `repo_budget_secs = 0.000001` (§4.4: the deadline covers the pre-open checks and open; late results are discarded) → every repo row has an error starting `"timed out"`, exit 1.
- **Hostile repos** (root `H`; a temp **global** gitconfig with `filter.lfs.clean`/`filter.lfs.process` and `safe.directory=*`; each driver, hook and fake binary writes a **distinct** marker file `marker-<vector>`):
  - `drivers`: repo-local `filter.x.clean`, `diff.x.command`, `merge.x.driver`, `core.fsmonitor`, `core.hooksPath` + a hook; `.gitattributes` `* filter=x diff=x` and `*.bin filter=lfs`; attributed files modified after checkout with same-size content (so no size short-circuit skips the filter).
  - `submodule`: a repo with a submodule whose own `.gitattributes` names `filter=lfs`, attributed file modified after checkout with same-size content.
  - `worktree-escape`: repo-local `core.worktree=$HOME`, `core.bare=false`.
  - `commondir-escape`: a linked worktree whose `commondir` points at another fixture repo.
  - `symlinked-files`: committed bare `fixtures/foo.git` whose `config` and `packed-refs` are symlinks to a sparse 4 GiB file; separate bare repos whose `shallow`, `objects/info/commit-graphs/commit-graph-chain`, and second-hop `objects/info/alternates` are such symlinks; a non-bare repo whose worktree-root `.gitmodules` is such a symlink; a repo with `extensions.worktreeConfig=true` whose `config.worktree` is such a symlink; a repo whose `HEAD` is `ref: EVIL` with `EVIL` such a symlink (revision 11).
  - `excludes-file` (revision 11): a repo whose repo-local `core.excludesFile` names such a symlink, scanned with no user excludes file; its row has no error (the key is stripped).
  - `big-object`: the HEAD commit's root tree is a loose object whose header claims 1 GiB (the HEAD tree bound and `tree_index_status` must read it).
  - `tree-bomb`: a HEAD tree of three nested levels, each a tree with 200 entries pointing at the same subtree (8,000,000 expanded paths from a few KiB of objects); its row carries `"git error: HEAD tree too large"`. `tree-bomb-long`: one subtree of 1,000 entries with 500-byte names reached from 999 top-level 500-byte names (≈ 1,000,000 entries, ≈ 1 GB of path bytes, under the entry cap); same error.
  - `hang-probe` (separate repo): `include.path` pointing at a harness-created FIFO with no writer, `packed-refs` replaced by a FIFO, and a second repo with `extensions.worktreeConfig=true` and `config.worktree` a FIFO.
  - A fake `git`, `git-lfs`, `claude`, `codex` first on `PATH`, each writing `marker-path-<name>`.
  - **Positive controls**, each on its own separate build of `H` so their markers never touch the tree under test: (1) the **real** git by absolute path (resolved before the fake is installed) running `status` in `drivers` and `submodule` (sandboxed `HOME`, recursive submodule status) creates `marker-filter-x` and `marker-filter-lfs` respectively; (2) gix opened with default (non-hardened) options running `index_worktree_status` on `drivers` creates `marker-filter-x` — proving rollcall's config stripping, not gix's defaults, is what prevents it.
  - **Assertions:** `rollcall scan H` exits within 10 s with peak RSS < 512 MB (`/usr/bin/time -v` on Linux, `-l` on macOS); no `marker-*` file exists; every `symlinked-files` repo and the `hang-probe` row carry `"untrusted git dir"`; `worktree-escape` and `commondir-escape` rows carry `"untrusted worktree path"`; the `tree-bomb` and `tree-bomb-long` rows carry `"git error: HEAD tree too large"`.
- **Claude fixtures:** each §5.1 rule, including `tool_result` as the last user record, `isMeta`, `<command-name>`, injected marker after user text, custom/ai/prompt title precedence, pre-truncated `lastPrompt`, `lastPrompt` with zero human prompts, multi-cwd, `"HEAD"`, subagent file, non-UUID id, oversize line, malformed line.
- **Codex fixtures:** a `state_5.sqlite` with `threads` + `thread_spawn_edges`: spawn-edge child, `source` subagent child, `parent_thread_id` child, archived thread, empty title, title equal to `first_user_message`, empty `git_branch` with rollout branch, `rollout_path` outside `CODEX_HOME`; rollouts with `user_message` events, without them (`response_item` path, `[injection, user]` blocks); database absent → fallback; rollout moved to `archived_sessions`; second `session_meta`.
- **Incremental reads:** append 3 lines + a partial → 3 new records, offset before the partial; truncate → re-read; truncate-then-regrow past offset → re-read (fingerprint); inode reuse and `parser_version` bump are **unit tests** that inject a prior `FileState` (different `head_fp`, or older `parser_version`) through the in-memory `Index`; title upgraded from prompt to custom on append; vanished file → session dropped.
- **Pointer resolution:** log replaced after scan → text `null`; symlink swapped in → `null`.
- **Matching:** fixture of 20 sessions: 16 matchable (distributed over worktrees so that at least one worktree has `older_sessions ≥ 1`, and one session sits in a hidden submodule and matches the parent), 1 `non_repo`, 1 `path_gone`, 2 `outside_root`.
- **Privacy:** each scrub pattern; truncation (multi-byte grapheme, pre-ellipsised input); `redact_prompts`; MCP defaults (no `last_prompt`, `prompt`-kind titles null); cache modes under umask `022`; after asserting ≥ 16 session rows exist, no fixture prompt string appears in any cache file.
- **Sanitising:** branch names and prompts containing `\x1b[2J`, `\x9b`, U+202E, U+200B render without them.
- **Shell safety:** for worktree paths containing space, `$`, backtick, `;`, `'`, and newline: the quoted fragment from `c` passed as `sh -c "printf '%s' <quoted>"` prints exactly the raw path; the `Y` line run as `sh -c "$line"` with a stub agent (absolute path on `PATH`) records exactly the session cwd and argv. `rc` in bash, zsh and fish on a fixture whose branch is `$(touch marker-rc)` running `rc status`, `rc status --json`, `rc --help`, and `rc` + `R` → no `marker-rc`, and the `R` case lands in the session cwd with the stub agent's recorded argv. `R` in the TUI is driven through a PTY harness (e.g. `expectrl`), with a stub agent that records argv and cwd; a relative `PATH` entry containing a planted `claude` is never used. `ROLLCALL_RESUME_FILE` pointing at a symlink, a file with mode `0644`, or a hard-linked file → the file's bytes (and a hard-link partner's) are unchanged.
- **Store:** idempotent rescan (two scans → identical `status --json` for the same `ROLLCALL_NOW`); crash mid-scan (transaction dropped) → prior state; harness holds the scan lock → command answers `stale: true`; `user_version` mismatch → rebuild.
- **TUI:** `ratatui` `TestBackend` 120×32 + `insta` snapshots with and without sessions, with fixed `ROLLCALL_NOW`.
- **MCP:** spawn `rollcall mcp --root $F` over stdio; call both tools; validate against `schema/`; ≤ 16 KiB on a 200-session fixture with `truncated` set when over; `root` not configured → error; no root → exit 2; empty cache → first call waits and returns repos.
- **Bench:** `hyperfine` end-to-end (§12.1 #17); no micro-benchmark harness in v0.

## 11. Distribution and release

- **Build hygiene:** `Cargo.lock` committed; `unsafe_code = "forbid"` in all crates; `gix` with `default-features = false` and an explicit feature list (no transport/network features).
- **CI (GitHub Actions, macOS + Linux, actions pinned by SHA, least-privilege tokens):** `cargo fmt --check`; `cargo clippy --all-targets --locked -- -D warnings`; `cargo test --locked`; `cargo deny check` (licenses, advisories, and `bans` denying the crates `reqwest`, `hyper`, `curl`, `ureq`, and the `gix-transport` features `blocking-client` and `async-client` (every `http-client*` feature implies `blocking-client`) — the `gix-transport` crate itself is a non-optional dependency of gix through `gix-protocol` and compiles with no client features); `cargo audit` on every push and on a weekly schedule. Dependabot for cargo and actions.
- **Releases:** `cargo-dist` → GitHub Release binaries with SHA-256 checksums and GitHub artifact attestations, Homebrew tap, shell installer; crates.io via trusted publishing (OIDC).
- **License:** dual MIT / Apache-2.0.
- **README:** pitch; differentiator table (§1); install; `rollcall init` / `rc` setup; MCP registration for Claude Code and Codex with `--root` and a first `rollcall scan`; privacy notes; demo GIF recorded with `vhs` against a synthetic demo root and synthetic logs, with stub `claude`/`codex` on `PATH`.
- Windows: not supported in v0.

## 12. v0 acceptance criteria

### 12.1 Gating (CI, generated fixtures, fixed `ROLLCALL_NOW`, oracles per §10.1)

1. **Discovery:** `rollcall status $F --json | jq -S '[.repos[] | {path, kind, contains_root, error, worktrees: [.worktrees[] | {path, external, stale_reason, error}]}]'` equals `tests/expected/discovery.json` after path substitution (error strings compared by their §4.1 prefix); submodules only with `--submodules`; finishes < 10 s with the symlink loop, in default and `--follow-symlinks` modes.
2. **Git state:** for every fixture worktree, `branch, detached, upstream, ahead, behind, unpushed, dirty_count, untracked_count, stale_reason, attention, error` equal `tests/expected/status.json` (errors by prefix); the corrupted repo row has `error` starting `"open failed: "` and `worktrees: []`; all other rows present. The separate deadline run (§10.2) marks every row `"timed out"` and exits 1.
3. **Sort and quiet:** repo order and each repo's `attention` / `last_activity_at` equal `tests/expected/order.json`.
4. **Claude source:** per fixture session, `title, title_kind, last_prompt, branch, cwd, message_count, started_at, ended_at` equal `tests/expected/claude.json`; `doctor --json` `.sources[] | select(.source=="claude")` counts equal the manifest.
5. **Codex source:** same for database mode (`tests/expected/codex_db.json`) and fallback mode (`tests/expected/codex_rollout.json`), including child exclusion counts and branch fallback.
6. **Matching:** every worktree's `last_session.id` and `older_sessions` equal `tests/expected/matching.json` (16 matched sessions, at least one worktree with `older_sessions ≥ 1`); `doctor --json` reports `unmatched: {non_repo: 1, path_gone: 1, outside_root: 2}`.
7. **Incremental reads & pointers:** every §10.2 incremental and pointer case yields the expected sessions, counts and offsets.
8. **Hostile repos:** both positive controls create their markers; `rollcall scan H` completes < 10 s with peak RSS < 512 MB, creates no `marker-*` file, and every row §10.2's assertions list carries its error prefix.
9. **Guards & first run:** root `/` exits 3 without `--yes` (suggestions not asserted); a fixture `HOME` exits 3 and lists exactly the expected suggestions; budget fixture → `partial: true`, exit 1; root inside a repo → enclosing repo row with `contains_root: true`.
10. **Privacy:** every scrub pattern redacted; every `title` / `last_prompt` ≤ 200 graphemes; `redact_prompts` → all null; MCP defaults per §8.3; cache modes `0700` / `0600`; with ≥ 16 session rows present, no fixture prompt string in the cache files.
11. **Sanitising & shell safety:** all §10.2 sanitising, shell-safety, `rc`, and `R` stub tests pass.
12. **Store:** idempotence, crash-safety, lock (`stale: true`), and rebuild tests pass.
13. **No-agent mode:** with empty `CLAUDE_CONFIG_DIR` and `CODEX_HOME`, `rollcall status $F_clean --json` exits 0, every `last_session` is null, `doctor` reports both sources `mode: "absent"`, and the TUI snapshot has no agent columns.
14. **Schemas & MCP:** `status`, `doctor`, `where_was_i`, `list_stale` outputs validate against `schema/*.json`; MCP integration test (§10.2) passes; ≤ 16 KiB.
15. **TUI:** snapshots match; every cell's `fg`/`bg` is `Reset` or a named ANSI colour (never `Rgb`/`Indexed`); with `NO_COLOR=1` every cell style is default.
16. **Build & supply chain:** §11 CI jobs pass; `dist build` succeeds on native runners for macOS arm64 (`macos-14`), macOS x86_64 (`macos-15-intel`), Linux x86_64 (`ubuntu-latest`) and Linux arm64 (`ubuntu-24.04-arm`); `d=$(mktemp -d) && cargo install --path crates/rollcall-cli --locked --root "$d" && "$d/bin/rollcall" --version` succeeds on Linux.
17. **Performance** (CI corpus from the generator: 50 repos incl. 20 linked worktrees, 5,000 synthetic logs, 500 MB; `ubuntu-latest`): `hyperfine --prepare 'rollcall cache clear' -r 5 'rollcall scan $C'` median < 5 s (cold); `hyperfine -w 2 -r 10 'rollcall status --json $C'` median < 1 s (warm: git re-read, no log changes).
18. **Demo:** `vhs demo.tape` renders the README GIF in CI from the synthetic demo root with stub agents, ending on a resumed session.

### 12.2 Author smoke (non-gating; recorded in the handoff, not judged by the skeptic)

- `rollcall status ~/Projects --json` lists the repos and worktrees `find ~/Projects -name .git` finds outside skipped dirs; dirty / ahead / behind match `git status` on 5 sampled worktrees.
- `doctor` on real data: unmatched sessions fall into plausible classes; no parser errors beyond `unknown_type`.
- Cold scan < 5 s, warm < 500 ms on the author's machine. (`~/Projects` has ~2.2 M entries before skips; if the default `walk_budget_entries` of 500,000 is hit, record it and tune the default rather than treating it as a pass.)
- `R` resumes a real Claude session and a real Codex session, including one whose cwd is under `.claude/worktrees/`.
- Claude Code with `rollcall mcp --root ~/Projects` answers "where was I in CRM?" with CRM's worktrees and last sessions.
- `brew install` from the published tap on a clean macOS user account, then `rollcall` in `~` (refusal with suggestions) and in `~/Projects`.

## 13. Roadmap (not v0)

Daemon / file watcher (B); hook source (C); squash-merge detection; LLM session summaries; Obsidian / notes adapter; Cursor and other agent sources; decision-journal integration; non-git VCS; Windows.
