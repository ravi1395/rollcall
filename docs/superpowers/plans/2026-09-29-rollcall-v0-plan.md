# Plan: rollcall v0 — revision 6  (47 phases in 9 lanes, branch: `impl/v0` in the main checkout; lane worktrees from agent isolation)

**Spec:** `docs/superpowers/specs/2026-09-29-rollcall-design.md` (revision 12). The spec is the contract; this plan cites sections (`§n`) instead of restating them. Where plan and spec disagree, the spec wins and the plan is the defect.

**Revision 6** (2026-09-30, implementation run) follows spec revision 12 (P15a audit U-4 .. U-9, in-process guards chosen by the author): new phases P07b (manifest + checker for the revision 12 hostile fixtures) and P18b (pre-open and open additions, since P18/P19 are merged); P15c, P20, P21, P27b and P35 gain revision 12 items. R1 still holds: every revision 12 change lands in files those phases already own; P07b is the one sanctioned edit of `scripts/check-manifest.py` after P07.

**Revision 5** records the author's answers to Q1–Q3 (§ Open questions). Revision 4 folded in plan red-team rounds 1–3 (Opus reviewers; findings and dispositions in `2026-09-29-rollcall-v0-plan-redteam.md`). Spec-level defects went to spec revisions 7–9; revision 10 records the author's Q3 answers. Phases P15 and P27 are split into lettered parts, so numbering stays stable.

**Deviation from the `/phases` house format:** 45 phases, not 3–5. This is a 5–7 week greenfield project with 18 gating criteria and a 2-substantive-file cap per worker. Every other house rule holds.

**Explorer pass:** not applicable — the repo holds only docs; every source path below is new. External facts are re-measurable with `scripts/probe-plan-facts.sh`.

**Setup (once):** `git checkout -b impl/v0` from `main` **in the main checkout** (not `../rollcall-v0`): agent-tool worktree isolation forks from the session checkout's HEAD, so `impl/v0` must be checked out there for lane worktrees to branch from it. Workers leave edits uncommitted; the orchestrator commits one commit per phase, message `P<nn>: <title>`. `impl/v0` is pushed to the private GitHub repo (`origin`) after each lane merge (Q1).

---

## Execution rules

### R1. Frozen shared files

P01 creates, complete: every `Cargo.toml` (all dependencies, features, dev-dependencies, `[[bench]]`), `Cargo.lock`, every crate's module list (`lib.rs` / `main.rs` `mod` lines), and `crates/rollcall-cli/tests/it/main.rs` with its module list. P04 writes every cross-crate public signature (§ Interfaces). After that, no phase edits a manifest, a module list, or a pinned signature. A worker that needs one stops and reports; the orchestrator decides the text. If the file belongs to an open lane, that lane's worker applies it at its next phase boundary; otherwise the orchestrator commits it on `impl/v0` and open lanes rebase only at phase-commit boundaries, with their worktree clean.

### R2. Lanes run in isolated worktrees; the orchestrator integrates

- A **lane** is a chain of phases on one crate or one file set. Lanes — and parallel sub-chains inside a lane (A: 02 ∥ 03; B1: 05→06 ∥ 07→08 ∥ 09; C1: 14→15a→15b→15c ∥ 16→17) — each get their own worktree branched from the current `impl/v0` (`plan-surgeon-surgeon` agents, or `isolation: "worktree"`), so a peer's half-finished edit never breaks another worker's compile.
- Lanes are file-disjoint by construction (lane table), so merges are conflict-free. After each merge the orchestrator runs the full gate on `impl/v0`: `cargo fmt --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`.
- A lane that depends on another lane's output starts from (or rebases onto) `impl/v0` after that output is merged. "After P11" means after the P11 commit is merged, not the end of lane B2.

### R3. Test-first; every check must be able to fail

- Each phase writes its tests, runs them red, then implements. Unit tests live in the file they test (`#[cfg(test)] mod tests`). Gating tests live in `crates/rollcall-cli/tests/it/` and compare against oracles committed before the code they judge.
- A worker's own verification: `cargo clippy -p <crate> --all-targets --locked -- -D warnings` plus its phase's test command. Pass counts are read from the full line `test result: ok. N passed; 0 failed`; a filtered run reporting `0 passed` is a failure. One positional filter per cargo invocation; several go after `--`.
- Tests needing an optional tool (`fish`, `zsh`, `hyperfine`, `vhs`, GNU `time`) skip locally when it is absent and **fail** when `ROLLCALL_REQUIRE_TOOLS=1` (set in CI).
- Every test that runs rollcall or could block (FIFOs, hang probes) runs under a watchdog: a spawned child is polled with `try_wait` against a deadline, killed and failed on expiry; in-process probes run on a thread joined with `recv_timeout`.
- Wall-clock assertions only where the spec states a time bound (§12.1 #1, #8, #17, §8.3's 2 s MCP wait); elsewhere assert counters.
- Snapshot tests run with `INSTA_UPDATE=no`, and the gate also fails if any `*.snap.new` exists. Snapshots are created once with `INSTA_UPDATE=always`, and each `.snap` is reviewed against §8.1 by the orchestrator before commit — rollcall's own output is not its own oracle.
- Never `std::env::set_var` (it is `unsafe` in edition 2024 and `unsafe_code` is forbidden): environment reaches code only through `EnvPaths`, and tests vary it by building an `EnvPaths` or by `Command::env` on a child.
- Stub parameters are `_`-prefixed until their phase fills the body; `tests/it/main.rs` carries `#![allow(dead_code)]` for the whole test crate.

### R4. Ponytail — how, never what

Adapted from the ponytail ruleset (github.com/dietrichgebert/ponytail; plugin skill `ponytail:ponytail`). Before writing code, stop at the first rung that holds: (1) not needed, (2) already in this repo, (3) Rust std, (4) platform feature, (5) a crate already in `Cargo.lock`, (6) one line, (7) minimum code. No unrequested abstractions, no scaffolding for later, fewest files, boring over clever; deliberate corner-cuts get `// ponytail: <ceiling>, <upgrade path>`.

It governs **how**. It never cuts **what** the spec decides: v0 scope, the 5-crate layout, `Index` / `SessionSource`, the fixture manifest, hand-written oracles, `insta` / `expectrl` / `jsonschema`, every §12.1 criterion, §4.4 hardening, §9 privacy.

### R5. Dependencies (P01 declares exactly this; default features trimmed)

| Crate | Normal deps | Dev deps |
|---|---|---|
| `rollcall-core` | `serde` (`derive`), `jiff` (`serde`), `regex`, `unicode-segmentation` | `serde_json` (R1 orchestrator edit at P03: §7 key-set tests need a JSON value type) |
| `rollcall-git` | core, `gix` (`default-features = false`, `features = ["sha1", "status", "revision"]`; **not** `interrupt`), `libc` (constants only); feature `test-hooks = []` (enables the P21 delay seam outside `cfg(test)`) | `tempfile` |
| `rollcall-sessions` | core, `serde`, `serde_json`, `rusqlite` (`bundled`), `gix-hash` (`sha1`), `libc` | `tempfile` |
| `rollcall-store` | core, `serde_json`, `rusqlite` (`bundled`), `gix-hash` (`sha1`) | `tempfile` |
| `rollcall-cli` | all four (dev-dependency on `rollcall-git` with `features = ["test-hooks"]`), `clap` (`derive`), `serde`, `serde_json`, `toml`, `ratatui` (`default-features = false`, `features = ["crossterm"]`), `shell-words`, `libc`, `arboard` (`default-features = false`) | `tempfile`, `rusqlite`, `toml`, `serde`, `insta`, `expectrl`, `jsonschema` (`default-features = false`) |

Ladder notes: `jiff`, `libc`, `tempfile`, `gix-hash` are already pulled in by gix (rung 5). Fingerprints (§6.3 "e.g. xxh3-128") use the first 16 bytes of SHA-1 from `gix-hash` — no `xxhash-rust`. The scan-lock file name is the hex SHA-1 of the canonical root (spec rev 10) and the cache path is computed directly (no `directories`). Terminal I/O uses `ratatui::crossterm` and `ratatui::init` / `restore` (panic-safe restore included) — no direct `crossterm`. Threads: `std::thread::scope` for the walk; `std::thread::spawn` (detached) for per-repo workers, because §4.4 abandons timed-out threads; `std::sync::mpsc` for channels. Scan lock: `File::try_lock` (stable since Rust 1.89 → `rust-version = "1.89"`). UUID check: hand-written. MCP: hand-written JSON-RPC 2.0 over stdio, no `rmcp` / `tokio`. Fixture repos: the real `git` binary resolved to an absolute path; the `big_object` zlib stream via `python3 -c 'import zlib …'` in the generator.

Measured 2026-09-29 in a scratch resolve (gix 0.88 with `sha1`): with `jsonschema` trimmed, `cargo tree -i reqwest` and `-i hyper` match nothing; `gix-transport` is present with only its empty `default` feature (spec §11, revision 7).

### R6. Safe-std map for spec syscalls (`unsafe_code = "forbid"`)

| Need | Safe API |
|---|---|
| `open(O_NOFOLLOW \| O_NONBLOCK)` | `OpenOptionsExt::custom_flags(libc::O_NOFOLLOW \| libc::O_NONBLOCK)`; `O_CLOEXEC` is std's default |
| fstat regular / uid / nlink / mode | `File::metadata()`, `MetadataExt::{dev, ino, uid, nlink, mode}` |
| lstat | `std::fs::symlink_metadata` |
| `SF_DATALESS` | `std::os::macos::fs::MetadataExt::st_flags() & 0x4000_0000` (value from the macOS SDK `sys/stat.h`), behind a pure `is_dataless(flags)` predicate |
| pread | `FileExt::read_exact_at` |
| truncate via fd | `File::set_len(0)` |
| explicit modes | `DirBuilderExt::mode(0o700)` + `set_permissions`; DB pre-created with `OpenOptionsExt::mode(0o600)` + `set_permissions` — SQLite gives `-wal`/`-shm` the main file's mode (measured under `umask 000`) |
| flock | `File::try_lock` |
| chdir + execv | `Command::new(abs_path).args(..).current_dir(cwd).exec()` |
| realpath | `std::fs::canonicalize` |
| effective uid | `uid()` of a file created fresh (`create_new`) in the cache dir on each call, then removed |
| environment | read once into `EnvPaths` in `run`; never `set_var` |

---

## Interfaces (pinned by P03/P04; bodies `unimplemented!()` until their phase)

Names below are fixed; executors may add private helpers but not change these. Every `lib.rs` entry point is a one-line delegation to a `pub fn` in its owning module, written by P04 (e.g. git `check_root` → `root::check_root`, sessions `impl SessionSource for Claude` → `claude::scan` / `claude::resolve`, core `match_sessions` → `matching::match_sessions`); later phases fill module bodies only and never touch `lib.rs` — except rollcall-git `lib.rs` (P21 owns the `discover` / `repo_state` bodies) and rollcall-store `lib.rs` (P26 owns `SqliteIndex`). P04's delegation targets: core `matching.rs`; git `root.rs`, `open.rs`; sessions `claude.rs`, `codex_rollout.rs` (Codex dispatch).

**`rollcall-core`** (P03):
- `type Timestamp = jiff::Timestamp; type Error = Box<dyn std::error::Error + Send + Sync>; type Result<T> = std::result::Result<T, Error>;`
- `struct Uuid(String)` with `Uuid::parse(&str) -> Option<Uuid>` and `as_str()`.
- §3.4 verbatim: `SourceKind`, `TextRef`, `TextField`, `TitleKind`, `Session`, `FileState`, `SourceState`, `SourceScan`, `trait Index` (`sessions_for(&self, root: &Path, worktree: &Path)`, spec rev 7; `scanned_submodules(&self, root: &Path) -> Option<bool>`, spec rev 9), `fn match_sessions`.
- `Backing { LogFile { dev, inode, path }, CodexThread { db_path, thread_id: Uuid } }`; `RepoKind { Normal, Nested, Bare, ExternalMain }`; `StaleReason { Missing, Merged, Inactive }`.
- `WorktreeState { path, is_main, branch, detached, head, last_commit_at, upstream, ahead, behind, unpushed, dirty_count, untracked_count, missing, merged, error }` (optional counts `Option<u32>`, others `u32`).
- `RepoState { default_branch, has_remote, error, worktrees: Vec<WorktreeState>, pending: Option<Arc<AtomicBool>> }` — `pending` is set only for a timed-out repo; the abandoned worker stores `true` when it finishes (serde-skipped).
- `RepoRow { path, name, kind, contains_root, state: RepoState, submodules: Vec<RepoRow> }`.
- `WalkOptions { home, confirmed: bool, follow_symlinks, submodules, skip_dirs: Vec<String>, budget_entries: u64, budget: Duration }`; `GitOptions { root, home, skip_paths: Vec<PathBuf>, repo_budget: Duration, excludes_file: Option<PathBuf> }` — trust rule (d)'s "under a skipped path" uses §4.2's path skips only; name skips (`node_modules`, …) prune discovery and are not a trust boundary.
- `DiscoveredRepo { git_dir, workdir: Option<PathBuf>, kind, contains_root, submodule_of: Option<usize>, linked: Vec<LinkedWorktree>, error: Option<String> }`; `LinkedWorktree { path, admin_dir, missing, error: Option<String> }`.
- `WalkDiag` = the `DoctorDoc.walk` object; `Discovery { repos, walk: WalkDiag, partial, notes: Vec<String> }`.
- `WorktreeKey { canonical_path, hidden, missing }`; `SessionLocation { cwd_canonical, cwd_exists, under_root }`; `MatchResult { Matched(usize), OutsideRoot, PathGone, NonRepo }`.
- `SourceDiagnostics` = one `DoctorDoc.sources[]` entry; sources leave `matched` / `unmatched` at 0 and P30 fills them after matching. `SourceScan` carries `notes: Vec<String>` (spec rev 8). `type ScanDiagnostics = DoctorDoc`.
- `ScanResult { scanned_at, submodules: bool, partial, repos: Vec<RepoRow>, sources: Vec<SourceScan>, matches: Vec<(SourceKind, Uuid, Option<PathBuf>)>, diagnostics: DoctorDoc }`.
- §7 documents with `Serialize + Deserialize`: `StatusDoc`, `Repo`, `Worktree`, `SessionOut`, `WhereWasIDoc`, `ListStaleDoc`, `DoctorDoc` (incl. revision 7 `notes`). Timestamps serialise via `jiff::Timestamp` `Display` (`Z` suffix, fraction only when non-zero — measured with jiff 0.2.37).
- `pub struct MemIndex` (plain `pub`, not `cfg(test)`) implementing `Index`.

**`rollcall-git`** (`lib.rs`): `check_root(root: &Path, opts: &WalkOptions) -> RootCheck` (`Allowed` | `Refused { suggestions: Vec<PathBuf> }`); `discover(root: &Path, opts: &WalkOptions) -> Discovery`; `repo_state(repo: &DiscoveredRepo, opts: &GitOptions) -> RepoState`; `user_excludes_file(home: &Path, xdg_config_home: Option<&Path>) -> Option<PathBuf>`.

**`rollcall-sessions`** (`lib.rs`): `trait SessionSource` (§3.4 verbatim); `Claude::new(projects_dir: PathBuf)`; `Codex::new(codex_home: PathBuf)`; both `impl SessionSource`; `const PARSER_VERSION: u32`.

**`rollcall-store`**: `SqliteIndex::open(cache_dir: &Path) -> Result<SqliteIndex>` + `impl Index`; `cache_dir(home: &Path, xdg_cache_home: Option<&Path>) -> PathBuf`; `ensure_cache_dir(&Path) -> Result<()>`; `try_scan_lock(cache_dir: &Path, root: &Path) -> Result<Option<ScanLock>>`; `clear(cache_dir: &Path) -> Result<()>`; `effective_uid(cache_dir: &Path) -> Result<u32>`.

**`rollcall-cli`** (library + one-line `main.rs`: `fn main() -> ExitCode { rollcall_cli::run(std::env::args_os()) }`):
- `cli.rs`: clap types and `run` dispatch for every §3.1 command, written complete in P04 against the entry points below.
- `config.rs`: `EnvPaths { home, xdg_config_home, xdg_cache_home, claude_config_dir, codex_home, path, visual, editor, no_color: bool, resume_file, rollcall_now }` (read once in `run`); `Ctx { config: Config, now: Timestamp, env: EnvPaths, cache_dir: PathBuf }`; `load(args, env) -> Result<Ctx, UsageError>`.
- `scan.rs`: `scan(ctx: &Ctx, root: &Path, mode: ScanMode) -> ScanOutcome { index: SqliteIndex, stale: bool, exit: ExitCode }` with `ScanMode::{Fresh, NoScan, WaitAtMost(Duration)}`. `WaitAtMost(d)`: if the lock is free, run the scan on a spawned thread and wait up to `d` on a channel; on timeout return a second `SqliteIndex::open` with `stale = true` while the detached thread commits later. If the lock is held elsewhere, poll up to `d`, then answer stale.
- `output.rs`: `status(ctx, &ScanOutcome, root, json: bool, out: &mut dyn Write) -> ExitCode`; `doctor(…)`; `scan_summary(…)`; `build_status(ctx, index, root, stale, policy: TextPolicy) -> StatusDoc`; `render_session(&Session, &dyn SessionSource, TextPolicy) -> SessionOut`; `ResumeTarget { source, id, cwd }`; `display_line(&ResumeTarget) -> String` (the §8.1 `Y` line; lives here so every session renderer has it from P31).
- `mcp.rs`: `serve(ctx, roots: Vec<PathBuf>, prompts: bool, input: impl BufRead, output: impl Write) -> ExitCode`.
- `tui.rs`: `draw(frame: &mut Frame, model: &Model)`; `Model { doc: StatusDoc, selected, expanded, pane, filter, status_message: Option<String>, show_help, no_color: bool, now: Timestamp, scanning: Option<usize> }` (`scanning` = repos found so far during a first scan) (pinned by P04 so P37 needs no `tui.rs` edit).
- `app.rs`: `run(ctx, root) -> ExitCode` (terminal, event loop, background scan); `handle_key(&mut Model, &EnvPaths, KeyEvent) -> Action` where `Action::{None, Quit, Rescan, Copy(String), Spawn { program, args }, Resume(ResumeTarget)}` (the unit-testable seam; `run` performs the action); `apply_copy(&mut Model, text: &str, write: impl FnOnce(&str) -> Result<(), String>)` sets the status message on failure.
- `resume.rs`: `resume(target: &ResumeTarget, env: &EnvPaths, uid: &dyn Fn() -> Result<u32>) -> Result<ExitCode, String>` (execs on the direct path, so it returns only on refusal or with exit 10 after the file protocol; `uid` is called only in the resume-file branch — production passes `|| rollcall_store::effective_uid(&ctx.cache_dir)`, tests pass a constant); `find_agent(path_var: &OsStr, name: &str) -> Option<PathBuf>`; `init_script(shell: Shell) -> &'static str`.

---

## Fixture manifest schema (`tests/fixtures/MANIFEST.toml`; written by P07/P08, parsed by P27a–P28)

- `[meta]`: `now` (RFC 3339; the `ROLLCALL_NOW` of every gating test), `author`, `committer` (`{ name, email }`).
- `[[repo]]`: `id` (unique); `root` (`F` | `EXT` | `H` | `budget` | `home` | `perf` | `demo`); `path` (relative to its root); `kind` (`normal` | `bare` | `nested` | `submodule` | `submodule_old` | `special`); `special` (when `kind = "special"`: `fifo_git`, `symlink_fanout`, `symlink_out_of_root`, `unreadable_dir`, `corrupted`, `bad_gitdir`, `untrusted_path`, `drivers`, `submodule_lfs`, `worktree_escape`, `commondir_escape`, `symlinked_files`, `worktree_config_symlink`, `big_object`, `hang_probe`, `hang_probe_worktree_config`, `enclosing_root`, `tree_bomb`, `tree_bomb_long`); `in_clean` (false ⇒ excluded from `F_clean`); `target` (for `symlinked_files`: which file is the 4 GiB symlink); `covers` (array of §10.2 bullet names — a real key, not a comment); `parent` (submodule / nested); `commits` (`[{ branch, message, date, files = { path = content } }]`); `branches`; `head` (branch name or `detached:<i>`, `i` indexing this repo's `commits` array); `remotes` (`[{ name, target_repo_id }]`); `upstreams` (`{ branch = "remote/branch" }`); `origin_head`; `worktrees` (`[{ path, branch, missing, external }]`); `state` (`{ staged, unstaged, untracked, ignored }` path lists) and `edit` (`{ path = replacement }` for unstaged edits, same byte length as the committed content); `config` (repo-local key → value).
- `[home]`: `files` (`{ relpath = content }`) for the sandboxed `HOME` (`.gitconfig` with `core.excludesFile`; the refused-root suggestion repos) and `expected_suggestions`.
- `[[claude]]`: `id`, `project_dir`, `file`, `corpus` (`matching` | `claude_rules` | `codex_rules` | `incremental` | `privacy` | `sanitise` | `shell`; the `[mcp_200]` and `[demo]` tables generate their own sessions and have no corpus), `lines` (JSONL strings), `steps` (`[{ op = "append" | "truncate" | "replace" | "symlink_swap" | "delete" | "move", lines?, to?, then_scan = true, expect = { records, offset, sessions, vanished } }]` — the §12.1 #7 oracle lives here), `expect` (`{ title, title_kind, last_prompt, branch, cwd, message_count, started_at, ended_at }`, or `excluded = "<reason>"`), `worktree` (repo id + worktree path it must match) or `unmatched = "<class>"`, `covers`.
- `[[codex_thread]]` (DB rows incl. `thread_spawn_edges`; `rollout_id` links to its `[[codex_rollout]]`) and `[[codex_rollout]]`: same `corpus` / `expect` / `worktree` / `covers` / `steps` keys.
- `[[big_log]]`: `{ source, bytes = 67_108_864, single_line = true }` for P33's RSS check.
- `[expect_counts]`: hand-written counts the oracle phases check against: `F_repos`, `F_worktrees`, and `<source>_sessions` for `claude`, `codex_db`, `codex_rollout`, each counting that source's sessions in corpora `matching`, `<source>_rules` (`claude_rules` or `codex_rules`) and `incremental` — exactly the rows of the P10 oracle file of the same name.
- Path tokens in any manifest value: `$F $EXT $H $HOME $CODEX_HOME $CLAUDE_PROJECTS` and `$FIFO:<name>` (a FIFO the harness creates); the builder substitutes them before writing. `special = "corrupted"` is pinned as `core.repositoryformatversion = 7` (passes pre-open checks, fails gix open with `open failed: `, fails `git fsck`); `special = "tree_bomb"` / `"tree_bomb_long"` per spec revs 8–9.
- Which corpora each gate run materialises: P32 `matching` (for #3 `order.json`); P33 `matching` + `claude_rules` + `codex_rules` + `incremental` + `[[big_log]]`; P34 `mcp_200` + `matching`; P35 `privacy` + `sanitise`; P39 `shell`.
- `[matching]`: `matched = 16`, `non_repo = 1`, `path_gone = 1`, `outside_root = 2` — counted over `corpus = "matching"` only.
- `[perf]`: `repos = 50`, `linked_worktrees = 20`, `logs = 5000`, `bytes = 500_000_000`. `[mcp_200]`: `sessions = 200`. `[demo]`: repos and synthetic sessions for the vhs demo.

Per-session fields (§12.1 #4, #5) are observed by calling `Claude` / `Codex` `scan` and `output::render_session` in-process through the cli library — the code path `status` uses — because `StatusDoc` shows only each worktree's `last_session`.

---

## Lane table

| Lane | Phases (serial inside) | Starts after |
|---|---|---|
| A — scaffold | 01 → 03 → 04; 02 in parallel with 03 | — |
| B1 — oracles | 05 → 06; 07 → 08 → 07b; 09 after 07; 10 after 08 and 09 | A |
| B2 — core functions | 11 → 12 → 13 | A |
| C1 — git | 14 → 15a → 15b → 15c (audit docs) ∥ 16 → 17; 18 after 15c and 17 → 19 → 18b → 20 → 21 | A |
| C2 — sessions | 22 → 23 → 24 → 25 | A + P11 |
| C3 — store | 26 | A |
| C4 — generator | 27a → 27b → 28 | A + B1 (incl. 07b) |
| D — cli (one crate, serial) | 29 → 30 → … → 39 | B2, C1–C4 merged |
| E — release | 40 → 42 (both edit `ci.yml`); 41 orchestrator-owned | D |

---

## Lane A — scaffold

## Phase 01 — Workspace scaffold
**Description:** The workspace skeleton with every manifest, dependency, feature, module list and test-module list final (R1).
**Files:** `Cargo.toml` (new), `CLAUDE.md` (new); mech: five crate `Cargo.toml`s; every module as an empty stub — core `lib.rs mem.rs rank.rs matching.rs text.rs`; git `lib.rs walk.rs classify.rs root.rs open.rs trust.rs state.rs testgit.rs` (`testgit` under `#[cfg(test)]`); sessions `lib.rs jsonl.rs claude.rs codex_db.rs codex_rollout.rs`; store `lib.rs fs.rs`; cli `lib.rs main.rs cli.rs config.rs scan.rs output.rs mcp.rs tui.rs app.rs resume.rs`; `tests/it/main.rs` (`#![allow(dead_code)]`) + stubs `support.rs`, `build_git.rs`, `build_sessions.rs`, `selftest.rs`, `materialize.rs`, `discovery.rs`, `no_agent.rs`, `sessions.rs`, `mcp.rs`, `hostile.rs`, `privacy.rs`, `store.rs`, `shell.rs`; `.gitignore`, `LICENSE-MIT`, `LICENSE-APACHE`, `Cargo.lock`.
**Changes:** cli `[[bin]] name = "rollcall"`; `cli.rs` handles `--version` only (P04 completes it); test paths to `schema/`, `tests/` resolve from `env!("CARGO_MANIFEST_DIR")/../..`. Workspace `resolver = "3"`, `edition = "2024"`, `rust-version = "1.89"`, `license = "MIT OR Apache-2.0"`, `[workspace.lints.rust] unsafe_code = "forbid"`, every crate `[lints] workspace = true`; R5 exactly; every library module declared `pub mod` so items consumed by later phases are not dead code. `CLAUDE.md`: pointers to spec and plan, R1–R4 in one line each, the ponytail ladder with attribution, and two safety rules — never print prompt text from real logs; re-audit gix read and spawn sites on any gix bump (§4.4).
**Acceptance criteria:**
- [ ] `cargo build --workspace --locked` and `cargo clippy --workspace --all-targets --locked -- -D warnings` succeed.
- [ ] `cargo tree -p rollcall-git -e normal` shows neither `rollcall-sessions` nor `rollcall-store`; same pairwise for the other two leaf crates.
- [ ] `cargo tree -e features -i gix-transport` lists only `gix-transport feature "default"`; `cargo tree -i reqwest`, `-i hyper`, `-i curl`, `-i ureq` each exit 101; `cargo tree -e features -i gix | grep -c 'feature "interrupt"'` → 0.
- [ ] `for f in crates/*/Cargo.toml; do grep -A1 '^\[lints\]' "$f" | grep -q 'workspace = true' || echo "$f"; done` prints nothing.
- [ ] `cargo run -q -p rollcall-cli -- --version` prints `rollcall 0.1.0`.
**Expected test outputs:** `cargo test --workspace --locked` → every `test result:` line `ok. 0 passed; 0 failed`.

## Phase 02 — CI and supply-chain baseline *(parallel with P03)*
**Files:** `.github/workflows/ci.yml` (new), `deny.toml` (new); mech: `.github/dependabot.yml`.
**Changes:** jobs `fmt`, `clippy`, `test` (matrix `ubuntu-latest`, `macos-14`; apt `zsh fish jq time`, brew `fish`; `ROLLCALL_REQUIRE_TOOLS=1`; after tests `test -z "$(find . -name '*.snap.new')"`), `deny`, `audit` (push + weekly `schedule`). Top-level `permissions: contents: read`; every `uses:` pinned to a 40-hex SHA with a version comment. `deny.toml`: crate bans `reqwest`, `hyper`, `curl`, `ureq`; `[[bans.features]]` for `gix-transport` denying `blocking-client`, `async-client`, `http-client-curl`, `http-client-reqwest` (spec §11, revision 7); licence allow-list compatible with MIT/Apache-2.0. Dependabot ecosystems `cargo` and `github-actions`.
**Acceptance criteria:**
- [ ] `grep -cE 'uses: [^@ ]+@[0-9a-f]{40}' .github/workflows/ci.yml` equals `grep -c 'uses:' .github/workflows/ci.yml`, and that count is ≥ 5.
- [ ] `grep -cE 'package-ecosystem: "(cargo|github-actions)"' .github/dependabot.yml` → 2.
- [ ] `cargo deny check` passes locally, and a scratch copy with `gix-transport/blocking-client` enabled fails `cargo deny check bans` (proves the feature ban works).
**Expected test outputs:** `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`.

## Phase 03 — Core types and in-memory Index
**Files:** `crates/rollcall-core/src/lib.rs`, `crates/rollcall-core/src/mem.rs` (bodies; module list untouched).
**Changes:** every core item in § Interfaces.
**Acceptance criteria:**
- [ ] A fully populated `StatusDoc` and `DoctorDoc` serialise with exactly the §7 key set at every level (hand-listed in the test) and round-trip through `Deserialize`.
- [ ] `TitleKind`, `StaleReason`, `RepoKind`, `SourceKind` serialise to the §7 strings.
- [ ] `MemIndex` stores the `--submodules` flag per root scan and reports it back.
- [ ] `Uuid::parse` accepts upper- and lower-case ids; rejects 35 / 37 chars, a moved hyphen, a non-hex char.
- [ ] `MemIndex`: after scans of roots A and B matching one session to different worktrees, `sessions_for(A, wA)` and `sessions_for(B, wB)` return it and `sessions_for(A, wB)` does not; `scanned_at`, `last_diagnostics`, `source_state` round-trip.
**Expected test outputs:** `cargo test -p rollcall-core --locked` → ≥ 5 passed, 0 failed.

## Phase 04 — Cross-crate interface stubs
**Description:** Transcribe § Interfaces into the other four crates with `unimplemented!()` bodies, and write the `cli.rs` dispatch complete (including §3.1's non-TTY `rollcall [PATH]` → `status` rule, decided by a pure `fn dispatch_target(stdout_is_tty: bool, …)`), so later phases fill bodies only.
**Files:** `crates/rollcall-sessions/src/lib.rs`, `crates/rollcall-cli/src/cli.rs`; mech: delegations and stubs in every other crate's `lib.rs` and module files (core `matching.rs`, git `root.rs open.rs`, sessions `claude.rs`, store `lib.rs fs.rs`, cli `lib.rs config.rs scan.rs output.rs mcp.rs tui.rs app.rs resume.rs`), all stub parameters `_`-prefixed.
**Acceptance criteria:**
- [ ] Workspace builds; clippy clean.
- [ ] `rollcall --help` lists `status`, `scan`, `doctor`, `mcp`, `init`, `cache`; `rollcall status --json --submodules --follow-symlinks --yes --no-scan . --help` exits 0.
- [ ] `dispatch_target(false, no subcommand)` → `Status`; `(true, …)` → `Tui`.
- [ ] `rollcall status --frobnicate` exits 2 (a bare word is a `[PATH]`, §3.1).
**Expected test outputs:** `cargo test -p rollcall-cli --lib --locked -- cli::` → ≥ 3 passed.

---

## Lane B1 — hand-written oracles (committed before any code they judge)

## Phase 05 — Schemas: status, doctor
**Files:** `schema/status.json`, `schema/doctor.json`; mech: `schema/examples/status.valid.json`, `status.invalid-extra-key.json`, `status.invalid-attention.json`, `status.invalid-title-kind.json`, `doctor.valid.json`.
**Changes:** JSON Schema 2020-12 with `$id`s `rollcall:status.json` / `rollcall:doctor.json`; every cross-file `$ref` is absolute (`rollcall:status.json#/$defs/…`) — relative refs fail against the opaque `rollcall:` base (reviewer-measured with jsonschema 0.58.2); each `*.invalid-*.json` is `*.valid.json` with exactly one mutation; `status.json` holds `$defs` `Repo`, `Worktree`, `Session`; `additionalProperties: false`; enums and nullability per §7; `title_kind` null iff `title` null (`if`/`then`); `DoctorDoc.notes` required.
**Acceptance criteria:**
- [ ] The example files exist; their validation runs in P28's self-test (Python's stdlib has no JSON Schema validator, so there is no phase-local validation).
**Expected test outputs:** `jq empty schema/*.json schema/examples/*.json` → exit 0.

## Phase 06 — Schemas: where_was_i, list_stale *(after P05)*
**Files:** `schema/where_was_i.json`, `schema/list_stale.json` (referencing `rollcall:status.json#/$defs/…`); mech: matching `schema/examples/*`.
**Acceptance criteria:** as P05 for `WhereWasIDoc` and `ListStaleDoc` (`truncated` required).
**Expected test outputs:** `jq empty schema/*.json schema/examples/*.json` → exit 0.

## Phase 07 — Manifest, part 1: git fixtures
**Files:** `tests/fixtures/MANIFEST.toml` (new), `scripts/check-manifest.py` (new; stdlib `tomllib`; modes `git`, `sessions`, `expect <key>` — written complete here so P08–P10 never edit it).
**Changes:** `[meta]`, `[home]`, every `[[repo]]` for roots `F`, `EXT`, `H`, `budget`, `home`, per § Fixture manifest schema and spec revisions 7–8 §10.2 (fan-out symlinks, `config.worktree` variants, same-size unstaged edits, `diff=x`, `big_object` as the HEAD root tree, `tree_bomb`, `tree_bomb_long`, `corrupted` as format version 7); `[expect_counts]` for git roots. The schema summary is repeated as a header comment.
**Acceptance criteria:**
- [ ] `python3 scripts/check-manifest.py git`: every §10.2 "Discovery", "Git state", "Deadline", "Hostile repos" bullet (hard-coded list in the script) appears in some `covers`; every `special` value is in the enum; the `in_clean = false` set equals the §10.1 exclusion list.
**Expected test outputs:** `python3 scripts/check-manifest.py git` → `ok: <n> repos, all bullets covered`.

## Phase 08 — Manifest, part 2: sessions, perf, demo *(after P07)*
**Files:** `tests/fixtures/MANIFEST.toml` (edit).
**Changes:** `[[claude]]`, `[[codex_thread]]`, `[[codex_rollout]]` (each with `corpus`; incremental cases as `steps`), `[[big_log]]`, `[matching]`, `[perf]`, `[mcp_200]`, `[demo]`, session `[expect_counts]`; one prompt per §9 scrub pattern; truncation and sanitising payloads; shell-hostile worktree paths and branch `$(touch marker-rc)`.
**Acceptance criteria:**
- [ ] `python3 scripts/check-manifest.py sessions`: every §10.2 "Claude fixtures", "Codex fixtures", "Incremental reads", "Pointer resolution", "Matching", "Privacy", "Sanitising", "Shell safety" bullet covered; matched / unmatched counts equal `[matching]` and sum to 20; ≥ 16 sessions in corpus `privacy` carry prompt text.
**Expected test outputs:** `python3 scripts/check-manifest.py sessions` → `ok`.

## Phase 07b — Manifest, part 3: revision 12 hostile fixtures *(after P08)*
**Files:** `tests/fixtures/MANIFEST.toml` (edit), `scripts/check-manifest.py` (edit: bullet list and `special` enum only).
**Changes:** add `"hostile: index-bombs"`, `"hostile: index-deep-ok"`, `"hostile: nested-probe"`, `"hostile: exec-type"`, `"hostile: big-tracked"`, `"hostile: pattern-blob-bomb"`, `"hostile: rename-bomb"` to the checker's hard-coded list first (checker red), then the `[[repo]]` entries under root `H` per spec §10.2 revision 12 (six `index-bombs` repos, two `index-deep-ok`, both `exec-type`, both `pattern-blob-bomb`), with `special` values added to the enum and the header comment; `in_clean = false` for all of them.
**Acceptance criteria:**
- [ ] With only the checker edit applied, `python3 scripts/check-manifest.py git` exits non-zero naming the seven new bullets; with the manifest entries, it prints `ok`.
- [ ] `expect F_repos` / `expect F_worktrees` unchanged (all new repos are under `H`).
**Expected test outputs:** `python3 scripts/check-manifest.py git` → `ok: <n> repos, all bullets covered`; `python3 scripts/check-manifest.py sessions` → `ok`.

## Phase 09 — Oracles: discovery, status *(after P07)*
**Files:** `tests/expected/discovery.json`, `tests/expected/status.json`.
**Changes:** hand-derived from the manifest and spec, never by running rollcall. `discovery.json` has exactly the array shape of the §12.1 #1 jq projection; `status.json` is an array of `{ path, branch, detached, upstream, ahead, behind, unpushed, dirty_count, untracked_count, stale_reason, attention, error }`. Paths as `$F/…`, `$EXT/…`; errors as their §4.1 prefix; optional per-row `_why` (the harness strips `_`-keys).
**Acceptance criteria:**
- [ ] `jq length` of each equals `python3 scripts/check-manifest.py expect F_repos` / `expect F_worktrees` (hand-written in P07, not computed).
**Expected test outputs:** counts equal.

## Phase 10 — Oracles: order, sessions, matching *(after P08 and P09)*
**Files:** `tests/expected/claude.json`, `tests/expected/codex_db.json`; mech: `tests/expected/codex_rollout.json`, `tests/expected/matching.json`, `tests/expected/order.json`.
**Changes:** per-session rendered fields (sanitised → scrubbed → truncated, §9); `matching.json` = per-worktree `last_session.id` + `older_sessions`, plus `unmatched` counts; `order.json` = repo order with `attention` and `last_activity_at` (which includes matched sessions' `ended_at`, §3.3) for the #3 run with the manifest's agent dirs populated.
**Acceptance criteria:**
- [ ] Each session file's `jq length` equals the matching `check-manifest.py expect <source>_sessions`.
- [ ] `matching.json`: unmatched sum 4, matched 16, ≥ 1 worktree with `older_sessions ≥ 1` (checked by `jq`).
**Expected test outputs:** counts equal.

---

## Lane B2 — core pure functions (serial; one crate)

## Phase 11 — Text pipeline
**Files:** `crates/rollcall-core/src/text.rs`.
**Changes:** `filter_prompt_block`, `sanitise`, `scrub(text, redact_emails)` (regexes in `LazyLock`), `truncate200`, `render_text` (truncate ∘ scrub ∘ sanitise), `grapheme_len`, `shell_quote(&Path)` (single quotes, `'` → `'\''`, bytes via `OsStrExt`).
**Acceptance criteria:**
- [ ] Each §9 scrub pattern: one positive and one near-miss negative.
- [ ] Sanitise removes each listed code point / range endpoint and ESC sequences; `\n` / `\t` → space.
- [ ] Truncation: 201 → 200 incl. `…`; 200 unchanged; 200 ending in `…` unchanged; a ZWJ family emoji is never split.
- [ ] Prompt filter: `<`-first block dropped; `# AGENTS.md` dropped; cut at `<system-reminder>` and at `<task-notification>`; whitespace-only → none.
- [ ] `shell_quote`: for each §10.2 shell-hostile path, `sh -c "printf '%s' <quoted>"` bytes equal the path.
**Expected test outputs:** `cargo test -p rollcall-core --locked -- text::` → ≥ 30 passed.

## Phase 12 — Attention, stale, sort, quiet
**Files:** `crates/rollcall-core/src/rank.rs`.
**Acceptance criteria:**
- [ ] Every §3.3 stale branch, incl. one negative per `inactive` condition; a main worktree is never stale.
- [ ] Attention precedence, one case each; repo attention = max incl. repo error → 4.
- [ ] Sort tie-breaks (activity null last, then path ascending).
- [ ] `quiet`: 23h59m → `quiet 0d`; 24h → `quiet 1d`; null → `quiet —`.
**Expected test outputs:** `cargo test -p rollcall-core --locked -- rank::` → ≥ 12 passed.

## Phase 13 — Session matching
**Files:** `crates/rollcall-core/src/matching.rs`.
**Acceptance criteria:**
- [ ] `/a/b` is not a prefix of `/a/bc`; `/a/b/c` matches `/a/b` over `/a`.
- [ ] A hidden submodule key is skipped (session falls to the parent); a missing worktree matches only an equal cwd.
- [ ] `PathGone`, `OutsideRoot`, `NonRepo` each produced.
**Expected test outputs:** `cargo test -p rollcall-core --locked -- matching::` → ≥ 8 passed.

---

## Lane C1 — `rollcall-git`

## Phase 14 — gix 0.88 audit, part 1: config, refs, open
**Files:** `docs/audits/gix-0.88-read-sites.md` (new).
**Changes:** crate list derived mechanically: every `gix*` package in `cargo tree -e normal -p rollcall-git --prefix none | sort -u`, recorded at the top of the doc and split between parts. Part 1 covers `gix`, `gix-config`, `gix-ref`, `gix-discover`, `gix-path`, `gix-sec`, `gix-fs` (for the locked versions): every `fs::read*`, `read_to_end`, `read_to_string`, `io::copy`, `BufReader::lines`, and `File::open` + unbounded read reachable from `open_opts` and reference iteration → `crate/path:fn` anchor, what it reads, which §4.4 check covers it. Pre-listed accepted exception: §4.4's "Documented limitation" (subdirectory `.gitignore` / `.gitattributes`). Known site to confirm: `config.worktree` (revision 7).
**Acceptance criteria:**
- [ ] Every site is "covered", "accepted exception", or "UNCOVERED". Any UNCOVERED → stop, report, spec revision (§4.4 is never weakened to fit).
- [ ] Anchors are `file:fn`, not line numbers; a reviewer re-locates 3 at random.
**Expected test outputs:** n/a (document); UNCOVERED count stated at the top.

## Phase 15a — gix audit, part 2a: index, status, worktree reads *(after P14)*
**Files:** `docs/audits/gix-0.88-read-sites.md` (edit).
**Changes:** same method for `gix-index`, `gix-status`, `gix-dir`, `gix-glob` (the actual reader behind `.gitignore`, `info/exclude`, `excludesFile`, `info/attributes`), `gix-attributes`, `gix-ignore`, `gix-diff` (blob pipeline reads and resource-cache growth × `Rewrites.limit`, reachable with HEAD↔index rename tracking; check `large_file_threshold`), `gix-filter` (incl. built-in `working-tree-encoding` buffering), `gix-object`, reachable from `tree_index_status` and `index_worktree_status`.
**Acceptance criteria:** as P14.
**Expected test outputs:** n/a (document).

## Phase 15b — gix audit, part 2b: object store and history *(after P15a)*
**Files:** `docs/audits/gix-0.88-read-sites.md` (edit).
**Changes:** same method for `gix-odb`, `gix-pack`, `gix-commitgraph`, `gix-revwalk`, `gix-traverse`, `gix-shallow`, and any remaining crate from the mechanical list, reachable from `rev_walk` and object lookup.
**Acceptance criteria:** as P14; every crate in the mechanical list is assigned to P14, P15a or P15b.
**Expected test outputs:** n/a (document).

## Phase 15c — gix audit, part 2c: spawn sites and API confirmation *(after P15b)*
**Files:** `docs/audits/gix-0.88-read-sites.md` (edit).
**Changes:** revision 6: also confirms at 0.88 `CompareBlobs` (custom impl allowed by `index_worktree_status`), dirwalk `recurse_repositories` / `classify_untracked_bare_repositories`, `Options::thread_limit`, `core.bigFileThreshold` as a config override. Every process-spawn site (`gix-command`, `gix-filter` drivers, credential helpers) with why it is unreachable after §4.4 stripping; memory-growth paths without an interrupt (e.g. `index_from_tree`, spec rev 8's HEAD tree bound). Confirms at 0.88: `open::Options::isolated`, `config_snapshot_mut`, `remove_section_by_id`, `sections_and_ids`, `tree_index_status`, `index_worktree_status(…, submodule, …, should_interrupt, …)`, `SubmoduleStatus` returning `Ok(None)`, `with_hidden`, `use_commit_graph`, `core.commitGraph`, `gitoxide.objects.allocLimit`.
**Acceptance criteria:** as P14; every spawn site is "unreachable because …" or UNCOVERED.
**Expected test outputs:** n/a (document).

## Phase 16 — Discovery walk and classification
**Files:** `crates/rollcall-git/src/walk.rs`, `crates/rollcall-git/src/classify.rs`; mech: `testgit.rs` (shared test helper: absolute real `git`, fixed dates).
**Changes:** §4.1–§4.3 minus root refusal. `std::thread::scope` workers over a `Mutex<Vec<PathBuf>>` stack + `Condvar` + in-flight count; the entry budget is claimed with `fetch_add` before each dirent is read, so the stop is exact; `lstat` only; same-device; `is_dataless`; skips; symlink policy with a visited `(dev, inode)` set; `notes` for unusual layouts.
**Acceptance criteria:**
- [ ] Each §4.1 kind classified; `node_modules/x/.git` and `target/x/.git` skipped; `$HOME/Library` skipped only when the root is outside it (`cfg(target_os = "macos")`; the Linux Trash path under `cfg(target_os = "linux")`).
- [ ] Symlink fan-out (two sibling links to the root, depth 3) with `follow_symlinks`: `walk.entries` ≤ 2 × the count without the links; without the flag, links are not followed.
- [ ] `budget_entries = 10` → `partial` and `walk.entries == 10`.
- [ ] FIFO named `.git`: a writer thread blocked in `open(O_WRONLY)` on it is still blocked after the walk (never opened for reading); the test then opens it for reading itself to release the writer and joins the thread.
- [ ] `$HOME/.git` is not reported as enclosing; a root inside a repo yields `contains_root`.
- [ ] `is_dataless(0x4000_0000)` true; `is_dataless(0)` false.
**Expected test outputs:** `cargo test -p rollcall-git --locked -- walk:: classify::` → ≥ 16 passed.

## Phase 17 — Root refusal and suggestions
**Files:** `crates/rollcall-git/src/root.rs`.
**Acceptance criteria:**
- [ ] A test `HOME` with 7 repos at depth 1–3 → `Refused` with the hand-listed set (depth 3 excluded; `Documents/x` excluded on macOS only; cap 5; path order).
- [ ] `confirmed = true` → `Allowed`.
- [ ] Root `/` → `Refused`, and the peek's entry counter stays within the 2-level bound (no full walk).
**Expected test outputs:** `cargo test -p rollcall-git --locked -- root::` → ≥ 5 passed.

## Phase 18 — Pre-open checks *(after P15c and P17)*
**Files:** `crates/rollcall-git/src/open.rs`.
**Changes:** the §4.4 pre-open list (revision 7, incl. `config.worktree` in the git dir and each `worktrees/<name>/`), refs/ 10,000-entry stop, alternates chain ≤ 8 hops; `user_excludes_file` (`~/.gitconfig`, then `$XDG_CONFIG_HOME/git/config`, same checks, 1 MiB cap, no includes).
**Acceptance criteria:**
- [ ] One test per listed file: symlink → `untrusted git dir`; over cap (sparse `set_len`) → untrusted; FIFO → untrusted without blocking (watchdog); absent → ok.
- [ ] 10,001 loose refs → untrusted; alternates 8 hops ok, 9 → untrusted.
- [ ] `user_excludes_file`: read from `~/.gitconfig`; falls back to XDG; `include.path` not followed; oversized `.gitconfig` → `None` plus a note.
**Expected test outputs:** `cargo test -p rollcall-git --locked -- open::` → ≥ 24 passed.

## Phase 19 — Hardened open and trust rule
**Files:** `crates/rollcall-git/src/trust.rs`.
**Changes:** `gix::open_opts` with `open::Options::isolated()`; overrides `core.commitGraph=false`, `gitoxide.objects.allocLimit=64MiB`, `core.excludesFile`; stripping via `sections_and_ids` + `remove_section_by_id` for every `filter` / `diff` / `merge` section and the five keys, then `commit`; trust rule (a)–(d) using `GitOptions { root, home, skip_paths }`.
**Acceptance criteria:**
- [ ] After open, the snapshot has zero `filter` / `diff` / `merge` sections (two subsections of each seeded) and none of the five keys.
- [ ] **gix-level positive control:** on a `drivers` repo (same-size edit, `* filter=x diff=x`), gix opened with default options running `index_worktree_status` creates `marker-filter-x`; the same call on rollcall's hardened `Repository` does not.
- [ ] Trust (a) `core.worktree=$HOME`, (b) `commondir` redirect, (c) `gitdir` mismatch, (d) workdir = `$HOME` / an ancestor of the root / under a skip path → `untrusted worktree path`.
- [ ] Production open leaves `git_dir_trust` unset (never forced to `Full`); forcing `Trust::Reduced` yields a repo whose resolved config excludes repo-local values.
**Expected test outputs:** `cargo test -p rollcall-git --locked -- trust::` → ≥ 12 passed.

## Phase 18b — Pre-open and open, revision 12 *(after P19)*
**Files:** `crates/rollcall-git/src/open.rs`, `crates/rollcall-git/src/trust.rs`.
**Changes:** spec §4.4 revision 12: `sharedindex.*` on the pre-open list; pattern-file caps 256 KiB (`info/exclude`, `info/attributes`, worktree-root `.gitignore` / `.gitattributes`, the user's excludes file); index validation (streamed iterative parse of each `index` and linked `sharedindex.*`, versions 2/3/4, `TREE` / `UNTR` depth ≤ 1,024, `REUC` ≤ 1 MiB, no chained `link`); override `core.bigFileThreshold=1m` at open.
**Acceptance criteria:**
- [ ] Hand-built index bytes (valid checksum) each → `untrusted git dir`: `TREE` depth 1,025; `UNTR` depth 1,025; `REUC` 2 MiB; entry past EOF; name > 4,096 bytes; `link` → FIFO `sharedindex` (watchdog, no block); `link` → symlinked `sharedindex`; chained `link`. Depth exactly 1,024 in a v2 and a v4 index → ok. A real-git `git update-index --split-index` repo → ok. A real-git index with `TREE`, `REUC` (from a conflicted merge) and `UNTR` (`core.untrackedCache=true`) → ok.
- [ ] Each pattern file at 256 KiB → ok, 256 KiB + 1 → untrusted; a `sharedindex.x` symlink → untrusted.
- [ ] After open, the resolved `core.bigFileThreshold` is 1 MiB when the repo-local config sets `0` and when it sets `1g`.
**Expected test outputs:** `cargo test -p rollcall-git --locked -- open:: trust::` → ≥ 20 more passed than before this phase.

## Phase 20 — Per-worktree git state
**Files:** `crates/rollcall-git/src/state.rs`.
**Changes:** spec rev 9 HEAD tree bound first (iterative walk via `gix::traverse::tree::depthfirst` with a counting `Visit` that cancels, deadline every 1,024 entries; caps 1,000,000 entries / 128 MiB path bytes / depth 4,096 → `git error: HEAD tree too large`; the walked tree id is passed on); then §3.3 fields: default branch, upstream, ahead / behind / unpushed (saturating at 10,000; deadline check every 256 commits; `cfg(test)` walked-commit counter), `dirty_count` (spec rev 8: union by destination path of `tree_index_status` with `TrackRenames::Given(Rewrites { copies: None, percentage: Some(0.5), limit: 1000, ..Default::default() })` and `index_worktree_status` with rewrites `None`, rollcall `SubmoduleStatus` → `Ok(None)`), `untracked_count` (files), `last_commit_at`, `merged`, `detached`, `head`, missing-worktree branch. Revision 6 (spec §4.4 revision 12): status call shape (`thread_limit: Some(1)`, dirwalk `recurse_repositories(true)`, rollcall `CompareBlobs` with the 64 MiB cap over `FastEq`), the §3.3 revision 12 `untracked_count` rule in the delegate, and the pre-status checks (a) 100755 over non-regular → `git error: unsafe tracked file`, (b) pattern blobs → `git error: pattern files too large`.
**Acceptance criteria:**
- [ ] Each §10.2 "Git state" case asserts a hard-coded expected value (non-zero where the case intends it) **and** equality with `git status --porcelain=v2 --branch --untracked-files=all` / `git rev-list --count` on the same tempdir repo.
- [ ] Staged-only change → `dirty_count = 1`; staged plus a further unstaged edit of the same path → 1; `git mv` rename → 1; `git mv` plus an unstaged edit of the destination → 1; unstaged `mv a b` → `dirty_count = 1`, `untracked_count = 1` (matches git).
- [ ] Tree bomb (3 nested levels × 200 entries) and long-name bomb (999 × 1,000 entries, 500-byte names) → `git error: HEAD tree too large`; each stops at its cap (entries, resp. path bytes) before `tree_index_status` is called; a 5,000-deep chain stops at depth 4,096 without stack overflow.
- [ ] Forced `Trust::Reduced` → `upstream` / `ahead` / `behind` null; a corrupted commit-graph chain does not change `ahead` / `behind`.
- [ ] 10,001 commits (`git fast-import`) → `ahead = 10000` and walked-commit counter ≤ 10,001.
- [ ] Squash-merged branch → `merged = false`.
- [ ] Revision 12: untracked `sub/.git/HEAD` FIFO → `untracked_count` counts `sub` once, returns before the watchdog (no git oracle: real git blocks on that FIFO); an untracked nested repo with a regular `HEAD`, and a submodule's worktree files, are counted as `git status --porcelain=v2 -uall` counts them; 100755 entry over a symlink and over a FIFO → `git error: unsafe tracked file` (watchdog); a 300 MiB sparse tracked file under `core.autocrlf=true` + `* ident` → `dirty_count = 1` with a `#[cfg(test)]` read counter in rollcall's `CompareBlobs` at 0 (RSS is gated in P35); a 300 KiB subdirectory `.gitattributes` blob, and eight 200 KiB skip-worktree `.gitignore` blobs → `git error: pattern files too large`; two 60 MiB one-byte-line blobs, staged `git mv` + edit → `dirty_count = 2` in < 10 s.
**Expected test outputs:** `cargo test -p rollcall-git --locked -- state::` → ≥ 24 passed.

## Phase 21 — `discover` / `repo_state` entry points and deadline
**Files:** `crates/rollcall-git/src/lib.rs` (bodies only).
**Changes:** `repo_state` spawns a detached worker per repo; the timer starts before pre-open checks; a per-repo `AtomicBool` interrupt is set at the deadline; the worker checks the deadline at stage boundaries and a late result is discarded; `RepoState.pending` for abandoned workers; a sleep seam around the state call under `#[cfg(any(test, feature = "test-hooks"))]` (`GitOptions` gains no field; the seam reads a `static` set by tests); submodule rows via the same hardened open. Revision 6: the worker is spawned with `std::thread::Builder::stack_size(16 MiB)` (spec §4.4 index validation).
**Acceptance criteria:**
- [ ] `repo_budget = 1 µs` on 3 tempdir repos (normal, bare, linked) → every row `timed out`.
- [ ] Sleep hook 200 ms, budget 50 ms → row `timed out`; `pending` is false at return and true after the worker finishes (awaited via a test-only channel).
- [ ] A `core.repositoryformatversion = 7` repo → its row starts `open failed: ` with `worktrees: []`; siblings unaffected.
- [ ] Revision 6: an index whose `TREE` chain is exactly 1,024 levels decodes through `repo_state` without aborting the test process.
**Expected test outputs:** `cargo test -p rollcall-git --locked` → ≥ 80 passed, 0 failed.

---

## Lane C2 — `rollcall-sessions` (after P11)

## Phase 22 — Log I/O and incremental reads
**Files:** `crates/rollcall-sessions/src/jsonl.rs`.
**Changes:** `open_log` (`O_NOFOLLOW | O_NONBLOCK`, fstat regular); a capped line reader (skips lines > 1 MiB without buffering them, stops before a partial trailing line, never reads past size-at-open, `cfg(test)` high-water buffer counter); `head_fp` / `tail_fp` (first 16 bytes of SHA-1 via `gix-hash`; `try_finalize`'s collision `Err` → treat as changed, re-read from 0); `plan_read(prior: Option<&FileState>, file: &File, meta: &Metadata) -> ReadFrom` (reads fingerprints from the fd); `resolve_log_ref`.
**Acceptance criteria:**
- [ ] Append 3 lines + a partial → 3 records; offset before the partial.
- [ ] Re-read from 0 on truncate; on truncate-then-regrow (`tail_fp`); on head change; on older `parser_version`. Incremental on the same `(dev, inode)` at a new path.
- [ ] A 2 MiB line between small lines → both small lines parsed, `oversize = 1`, high-water ≤ 1 MiB + 8 KiB.
- [ ] FIFO or symlink at a log path → rejected without blocking (watchdog).
- [ ] Resolution → `None` for a new inode, a file shrunk below `size_at_scan`, a symlink swapped in.
- [ ] Vanished file → session dropped, `vanished += 1`.
**Expected test outputs:** `cargo test -p rollcall-sessions --locked -- jsonl::` → ≥ 15 passed.

## Phase 23 — Claude source
**Files:** `crates/rollcall-sessions/src/claude.rs`.
**Acceptance criteria:**
- [ ] One test per §10.2 "Claude fixtures" bullet, using the §5.1 worked records.
- [ ] Title precedence; an appended `custom-title` upgrades a prompt title while the first-prompt ref is kept.
- [ ] Subagent files excluded and counted; non-UUID ids skipped and counted; `projects_dir` honoured.
- [ ] `resolve` is stable after unrelated appends.
**Expected test outputs:** `cargo test -p rollcall-sessions --locked -- claude::` → ≥ 16 passed.

## Phase 24 — Codex thread database
**Files:** `crates/rollcall-sessions/src/codex_db.rs`.
**Acceptance criteria:**
- [ ] Opened `mode=ro`, never `immutable=1`: with a second connection **held open** after an uncheckpointed WAL write, the reader sees the row.
- [ ] Highest `state_<N>.sqlite` with the required columns wins; one lacking a column is skipped.
- [ ] Each of the three child-exclusion rules excludes one thread; counts reported.
- [ ] Title kinds: equal, `…`-prefix, `...`-prefix, > 200 graphemes → `Prompt`; unrelated → `Thread`; empty → filtered `first_user_message`.
- [ ] `rollout_path` outside `CODEX_HOME` → `bad_rollout_path`.
- [ ] DB bytes and mtime unchanged after a scan.
- [ ] `resolve(CodexThread)` after the row's title changes → the new title; row deleted → session dropped, `vanished += 1`.
**Expected test outputs:** `cargo test -p rollcall-sessions --locked -- codex_db::` → ≥ 14 passed.

## Phase 25 — Codex rollouts and source dispatch
**Files:** `crates/rollcall-sessions/src/codex_rollout.rs`; mech: the `Codex` dispatch body (in `codex_rollout.rs`, delegated from `lib.rs` by P04) choosing db / rollouts / absent and filling `SourceScan.notes`.
**Acceptance criteria:**
- [ ] `user_message` events preferred; `response_item` only in files without them; `[injection, user]` blocks → the user text only.
- [ ] A second `session_meta` is ignored; `parent_thread_id` → child.
- [ ] DB absent → `mode = "rollouts"` plus a note; present → `db` with `db_version`.
- [ ] Rollout moved to `archived_sessions/` → read incrementally (records-read counter shows no re-read).
**Expected test outputs:** `cargo test -p rollcall-sessions --locked` → ≥ 49 passed, 0 failed.

---

## Lane C3 — `rollcall-store`

## Phase 26 — SQLite Index, cache dir, lock
**Files:** `crates/rollcall-store/src/lib.rs`, `crates/rollcall-store/src/fs.rs`.
**Changes:** §6.1–§6.2 schema — repos/worktrees by canonical path, root-membership table, sessions by `(source, id)` with `TextRef`s only, per-root matches (revision 7), file states, per-root `DoctorDoc` JSON; one transaction per `commit_scan`; `user_version` rebuild; DB pre-created `0600`, dir `0700`; `try_scan_lock` on `scan-<hash>.lock`; `effective_uid`; `clear`.
**Acceptance criteria:**
- [ ] Parity with `MemIndex` on a shared script (all six methods), incl. the P03 root-keyed matching case.
- [ ] A panic injected between inserts (`cfg(test)` hook) leaves the prior scan intact.
- [ ] `user_version` mismatch → rebuilt empty, no error.
- [ ] Under `umask 000` (an `#[ignore]` child test run via `sh -c 'umask 000; exec <test-bin> --ignored --exact fs::tests::modes_child'`), with a connection open after a write: dir `0700`; DB, `-wal`, `-shm` `0600`.
- [ ] A second `try_scan_lock` from another process → `None`; another root → `Some`; released on drop.
**Expected test outputs:** `cargo test -p rollcall-store --locked` → ≥ 10 passed.

---

## Lane C4 — fixture generator (after B1)

## Phase 27a — Harness
**Files:** `crates/rollcall-cli/tests/it/support.rs`, `crates/rollcall-cli/tests/it/selftest.rs`.
**Changes:** manifest parser (serde structs per § Fixture manifest schema); env sandbox (`HOME`, `XDG_*`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `ROLLCALL_NOW = meta.now`, `PATH` with absolute git); `run_rollcall` with watchdog; `expected(name)` with `$F` / `$EXT` substitution after canonicalising both sides, and `_`-key stripping; `assert_error_prefix`; `validate(schema, doc)` (jsonschema with the `rollcall:` `$id` resources registered); `peak_rss` (GNU `-v` kbytes vs macOS `-l` bytes, normalised to bytes); `require_tool`; read-only roots `F`, `F_clean`, `H` cached in a `OnceLock` per test binary, `fresh(root)` for tests that mutate. Manifest path tokens (§ Fixture manifest schema) are substituted by the builders using helpers defined here.
**Acceptance criteria:**
- [ ] `selftest::watchdog`: `run_rollcall` on a `sleep 30` stub fails within deadline + 1 s.
- [ ] `selftest::peak_rss`: a child allocating and touching 64 MiB reports ≥ 64 MiB on both OSes.
- [ ] `selftest::manifest_parses`: the full `MANIFEST.toml` deserialises with no unknown keys (`deny_unknown_fields`).
**Expected test outputs:** `cargo test -p rollcall-cli --test it --locked -- selftest::` → ≥ 3 passed.

## Phase 27b — Git fixture builder *(after P27a)*
**Files:** `crates/rollcall-cli/tests/it/build_git.rs`; mech: `selftest.rs` addition.
**Changes:** every `[[repo]]` kind and special; `big_object` zlib via `python3`; revision 6: the P07b specials, incl. index files written byte by byte with a SHA-1 trailer (versions 2 and 4) and sparse files via `set_len`.
**Acceptance criteria:**
- [ ] `selftest::f_clean_fsck`: `git fsck` (under watchdog) passes on every `F_clean` repo and fails on the `corrupted` repo.
- [ ] `selftest::build_all_git`: `F`, `H`, `EXT`, `budget`, `home` all build (exercises every special, keeping builders live).
**Expected test outputs:** `cargo test -p rollcall-cli --test it --locked -- selftest::` → ≥ 5 passed.

## Phase 28 — Session fixture builder, materialize, schema self-test *(after P27b)*
**Files:** `crates/rollcall-cli/tests/it/build_sessions.rs`, `crates/rollcall-cli/tests/it/materialize.rs`; mech: `selftest.rs` additions.
**Changes:** Claude project dirs; `[[big_log]]`; Codex `state_5.sqlite` (+ `thread_spawn_edges`) via `rusqlite`; rollouts; `mcp_200`; perf logs; demo. `materialize.rs`: `#[ignore]` tests `materialize_perf` / `materialize_demo` writing a named root to `$ROLLCALL_MATERIALIZE_DIR` — the CI entry point for P40 / P42.
**Acceptance criteria:**
- [ ] `selftest::schemas`: each `schema/examples/*.valid.json` validates and each `*.invalid-*.json` fails.
- [ ] `selftest::build_all_sessions`: every corpus builds; the Codex DB's columns equal the §5.2 required list (catches P24/P28 drift early).
- [ ] `ROLLCALL_MATERIALIZE_DIR=$d cargo test -p rollcall-cli --test it --locked -- --ignored materialize_demo` creates `$d/demo` with ≥ 1 repo and ≥ 1 session file.
**Expected test outputs:** `cargo test -p rollcall-cli --test it --locked -- selftest::` → ≥ 7 passed.

---

## Lane D — `rollcall-cli` (serial; after B2 and C1–C4 are merged)

## Phase 29 — Config and context
**Files:** `crates/rollcall-cli/src/config.rs`.
**Acceptance criteria:**
- [ ] Unknown key → stderr warning, run continues; invalid TOML, wrong type, `repo_budget_secs` ≤ 0 / `nan` / `inf` / `1e30`, or a non-RFC 3339 `ROLLCALL_NOW` → exit 2.
- [ ] Flags override config; config `roots` count as confirmed (§4.3).
- [ ] Config path `$XDG_CONFIG_HOME/rollcall/config.toml`, default `~/.config/rollcall/config.toml` on both OSes.
**Expected test outputs:** `cargo test -p rollcall-cli --lib --locked -- config::` → ≥ 8 passed.

## Phase 30 — Scan orchestration
**Files:** `crates/rollcall-cli/src/scan.rs`.
**Changes:** refusal → lock → `discover` → `repo_state` (≤ `available_parallelism` in flight) → sources → canonicalise cwds → `match_sessions` per root → `commit_scan`; `scanned_at = ctx.now` at scan start; `NoScan` / lock held → cache with `stale`; `repo_timeouts` / `repo_timeouts_abandoned` from `pending`; fills each source's `matched` / `unmatched`; stores the `--submodules` flag (`ScanResult.submodules`, read back via `scanned_submodules`); `WaitAtMost` per § Interfaces; exit codes (§3.1).
**Acceptance criteria (asserted on `ScanOutcome` / `Index`):**
- [ ] Lock held elsewhere → `stale = true`, index unchanged; no cache → `scanned_at` none and exit 1.
- [ ] Any error row or `partial` → exit 1; clean → 0; refused → 3.
- [ ] Unreadable Codex DB → Claude and git results still committed; `DoctorDoc.notes` carries the reason.
- [ ] Two scans at the same `ROLLCALL_NOW` commit identical rows.
- [ ] `NoScan` with a different `--submodules` flag than the stored scan → `stale = true`.
- [ ] `WaitAtMost(50 ms)` with a scan made slow by the P21 sleep seam (via the `test-hooks` dev-dependency feature) → returns stale within 50 ms + slack; the detached scan later commits.
**Expected test outputs:** `cargo test -p rollcall-cli --lib --locked -- scan::` → ≥ 6 passed.

## Phase 31 — Output: status, scan, doctor, cache clear
**Files:** `crates/rollcall-cli/src/output.rs`.
**Changes:** `build_status` / `render_session` (resolve → `render_text`; `redact_prompts`; `resume` via `display_line`, or null when the cwd is gone); `display_line`; plain table (§8.2) sanitising paths, branches, errors; JSON exact; `scan_summary`; `doctor`; `cache clear`. (Non-TTY `rollcall` → `status` is `cli.rs` dispatch, written in P04; gated in P32.)
**Acceptance criteria:**
- [ ] `status --json` on a tempdir root validates against `schema/status.json`; `doctor --json` against `schema/doctor.json` (the tests in `output.rs` build their own `jsonschema` validator; `tests/it` helpers aren't reachable from unit tests).
- [ ] `scan` stderr matches `^scanned .+: \d+ repos, \d+ worktrees, \d+ sessions in [0-9.]+s \(\d+ errors\)$`.
- [ ] Each sanitising payload (`\x1b[2J`, `\x9b`, U+202E, U+200B) in a branch: absent from plain output, present (escaped) in JSON; in a prompt: absent from both.
- [ ] `--no-scan` with a cache → `stale: true`; `cache clear` removes the dir; a session whose cwd was deleted → `resume: null`.
- [ ] `display_line` for each §10.2 shell-hostile cwd round-trips through `sh -c` (as P11's `shell_quote`).
- [ ] A `doctor` note containing `\x1b[2J` is absent from plain output (spec rev 8 §9).
**Expected test outputs:** `cargo test -p rollcall-cli --lib --locked -- output::` → ≥ 10 passed.

## Phase 32 — Gates: discovery, git state, order, guards, no-agent (§12.1 #1, #2, #3, #9, #13 non-TUI)
**Files:** `crates/rollcall-cli/tests/it/discovery.rs`, `crates/rollcall-cli/tests/it/no_agent.rs`.
**Acceptance criteria:** §12.1 #1, #2 (incl. the revision 7 `repo_budget_secs = 0.000001` run on `F_clean`), #3, #9, #13 as written, against the P09 / P10 oracles; `status --json` and `doctor --json` on `F` validate against `schema/`; after the deadline run `doctor --json` has `repo_timeouts ≥ 1`; #1's `< 10 s` on the fan-out fixture in both symlink modes; `rollcall $F | cat` equals `rollcall status $F` (non-TTY).
**Expected test outputs:** `cargo test -p rollcall-cli --test it --locked -- discovery:: no_agent::` → ≥ 10 passed.

## Phase 33 — Gates: sources, matching, incremental (§12.1 #4, #5, #6, #7)
**Files:** `crates/rollcall-cli/tests/it/sessions.rs`.
**Acceptance criteria:** #4 / #5 per session via `Claude` / `Codex` `scan` + `output::render_session` against the P10 oracles, and source counts via `doctor --json`; #6 via `status --json` + `doctor`; #7 via two CLI scans with edits between, offsets read from the cache through `SqliteIndex::source_state`, with the P22 unit cases cited for inode reuse and `parser_version`; a 64 MiB single-line log → `rollcall scan` peak RSS < 48 MB.
**Expected test outputs:** `cargo test -p rollcall-cli --test it --locked -- sessions::` → ≥ 14 passed.

## Phase 34 — MCP server (§12.1 #14; #10's MCP part)
**Files:** `crates/rollcall-cli/src/mcp.rs`, `crates/rollcall-cli/tests/it/mcp.rs`.
**Acceptance criteria:**
- [ ] Both tools validate against `schema/`; the `mcp_200` response is ≤ 16 KiB with `truncated: true`, dropping `recent_sessions` from the end before trailing repos.
- [ ] `repo` matches by name and by path and returns all matches; default `limit` 5 by attention; `recent_sessions` = latest `limit` across returned repos.
- [ ] External worktrees and their sessions absent; omitted `root` → first configured root; unconfigured `root` → JSON-RPC error; no root → exit 2; config `roots` used without `--root`; refused `--root` → exit 3.
- [ ] Without `--prompts`: every `last_prompt` null; prompt-kind titles have `title` and `title_kind` null. With `--prompts`: both present. `redact_prompts` overrides.
- [ ] Empty cache → the first call waits and returns repos; cache present and lock held → answers within 2 s (+ 1 s slack) with `stale: true`.
- [ ] Tool descriptions contain the §8.3 untrusted-data sentence.
**Expected test outputs:** `cargo test -p rollcall-cli --test it --locked -- mcp::` → ≥ 12 passed.

## Phase 35 — Gates: hostile, privacy, store (§12.1 #8, #10, #12)
**Files:** `crates/rollcall-cli/tests/it/hostile.rs`, `crates/rollcall-cli/tests/it/privacy.rs`; mech: `store.rs`.
**Acceptance criteria:** #8 per spec revision 7 — positive controls run first on their own `fresh(H)` builds and fail the test if markers don't appear; `rollcall scan` on a pristine `H` under watchdog: < 10 s, peak RSS < 512 MB, no `marker-*`, the §10.2 error prefixes incl. the `config.worktree` repos, `tree-bomb` and `tree-bomb-long`, and every revision 12 assertion (`index-bombs`, `index-deep-ok`, `nested-probe`, `exec-type`, `big-tracked`, `pattern-blob-bomb`, `rename-bomb`). #10 (non-MCP parts; MCP is P34), incl. cache modes under `umask 022` via `sh -c`, and cache files searched for distinctive substrings of each of ≥ 16 prompts. #12: idempotence, lock → `stale: true`, rebuild; crash-safety satisfied by the P26 unit test (cited).
**Expected test outputs:** `cargo test -p rollcall-cli --test it --locked -- hostile:: privacy:: store::` → ≥ 20 passed.

## Phase 36 — TUI rendering (§12.1 #15; #13's TUI part)
**Files:** `crates/rollcall-cli/src/tui.rs` (in-file `TestBackend` + `insta` tests; snapshots under `src/snapshots/`).
**Acceptance criteria:**
- [ ] Snapshots at 120×32 from `MemIndex`-built models with fixed synthetic paths and `scanned_at = now` ("scanned 0s ago"): with sessions, without sessions, filter active, help open, stale header, `scanning… <N> repos found`.
- [ ] Every cell `fg` / `bg` ∈ {`Reset`, the 16 named colours}; with `Model.no_color = true` every style is default.
- [ ] No-sessions model: the buffer contains no `claude`, `codex`, or `Older sessions`, and the key line has no `R` / `Y`.
- [ ] Badge colours: unpushed yellow, dirty blue, stale / missing dim, error red.
- [ ] Sanitising payloads in a branch and a title are absent from the buffer.
**Expected test outputs:** `INSTA_UPDATE=no cargo test -p rollcall-cli --lib --locked -- tui::` → ≥ 10 passed; no `*.snap.new`.

## Phase 37 — TUI app: event loop, keys, clipboard, editor
**Files:** `crates/rollcall-cli/src/app.rs`.
**Changes:** `ratatui::init` / `restore`; first paint from cache, background scan on `std::thread` + `mpsc`; keys per §8.1 through `handle_key`; `c` / `Y` via the configured clipboard (`arboard` for `system`, OSC 52 for `osc52`, §3.2); `o` via `shell-words`, `$VISUAL` before `$EDITOR`; `R` → `resume::resume`.
**Acceptance criteria (through `handle_key` on a `TestBackend` model):**
- [ ] `j` / `k` / arrows, `enter`, `tab`, `/` (case-insensitive substring over repos and branches), `Esc` clears, `r` rescans, `?`, `q` → exit 0.
- [ ] `c` yields `Action::Copy("cd -- '<path>'")`; `Y` yields `Action::Copy(<§8.1 display line>)`; `apply_copy` with a failing writer sets the status message.
- [ ] `o` with `EnvPaths.visual = "code --wait"` returns `Action::Spawn { program: "code", args: ["--wait", <path>] }`; `visual` beats `editor`; both unset → status message.
**Expected test outputs:** `cargo test -p rollcall-cli --lib --locked -- app::` → ≥ 12 passed.

## Phase 38 — Resume and shell init
**Files:** `crates/rollcall-cli/src/resume.rs`.
**Changes:** absolute-only `PATH` search before chdir; `exec`; resume-file protocol per spec revision 7 (`O_WRONLY | O_NOFOLLOW | O_NONBLOCK`, verify on the fd incl. `effective_uid`, then `set_len(0)` and write, exit 10); `init_script` for bash / zsh / fish with no `eval`.
**Acceptance criteria:**
- [ ] Resume file that is a symlink, `0644`, or hard-linked → refused; its bytes and a hard-link partner's bytes unchanged; a FIFO → refused without blocking.
- [ ] `find_agent("relative/dir:/abs/stub", "claude")` with `claude` planted in both → `/abs/stub/claude`.
- [ ] `resume` with a `uid` closure returning another uid → refused; on the direct-exec path the closure is never called.
- [ ] `grep -c eval` over each `init_script` → 0.
**Expected test outputs:** `cargo test -p rollcall-cli --lib --locked -- resume::` → ≥ 10 passed.

## Phase 39 — Gates: shell safety and resume end-to-end (§12.1 #11)
**Files:** `crates/rollcall-cli/tests/it/shell.rs`.
**Acceptance criteria:** §12.1 #11 as written — `c` / `Y` fragments via `sh -c`; `rc` in bash, zsh, fish (R3 tool rule) with branch `$(touch marker-rc)` for `rc status`, `rc status --json`, `rc --help`, and `rc` + `R` → no marker, and `R` lands in the session cwd with the stub's recorded argv; TUI `R` via an `expectrl` PTY with a stub agent recording argv and cwd.
**Expected test outputs:** `cargo test -p rollcall-cli --test it --locked -- shell::` → ≥ 12 passed.

---

## Lane E — performance, release, demo (after D)

## Phase 40 — Performance gate (§12.1 #17)
**Files:** `.github/workflows/ci.yml` (edit: `perf` job).
**Acceptance criteria:** on `ubuntu-latest`, release build, corpus via `materialize_perf`: `hyperfine --prepare 'rollcall cache clear' -r 5 --export-json cold.json 'rollcall scan $C'` median < 5 s; `hyperfine -w 2 -r 10 --export-json warm.json 'rollcall status --json $C'` median < 1 s; a `jq` step fails the job on breach.
**Expected test outputs:** CI `perf` job green with both medians printed.

## Phase 41 — Release pipeline (§12.1 #16) — orchestrator-owned
**Files:** `dist-workspace.toml` (new), `.github/workflows/release.yml` (generated); root `Cargo.toml` `[profile.dist]` if `dist init` adds it (an orchestrator edit under R1).
**Acceptance criteria:** `dist build` green on `macos-14`, `macos-15-intel`, `ubuntu-latest`, `ubuntu-24.04-arm`; installers include Homebrew and shell; checksums and attestations enabled; if the generated workflow is SHA-pinned, `allow-dirty = ["ci"]` is set and `dist plan` passes; `cargo install --path crates/rollcall-cli --locked --root "$d" && "$d/bin/rollcall" --version` passes in a Linux job. crates.io trusted publishing is a manual checklist item, not a gate.
**Expected test outputs:** release dry-run jobs green on all four runners.

## Phase 42 — README and demo (§12.1 #18) *(after P40)*
**Files:** `README.md` (new), `demo.tape` (new); mech: CI `demo` job.
**Acceptance criteria:**
- [ ] README has: pitch; the §1 differentiator table; install (Homebrew, shell installer, cargo); `rollcall init` / `rc`; MCP registration for Claude Code and Codex with `--root` and a first `rollcall scan`; privacy notes; documented limitations — squash merges (§3.3), bare fixtures inside repos (§4.1), other-uid repos' null upstream fields, filter/LFS dirty counts and the subdirectory ignore/attributes limitation (§4.4), the shell staying in its directory after `R` without `rc`, and the agent's own workspace-trust prompts (§8.1). The CI job greps for each.
- [ ] `vhs demo.tape` renders from `materialize_demo` with stub agents; the last frame contains the stub's `resumed <id>` line.
**Expected test outputs:** CI `demo` job green; GIF artifact uploaded.

---

## §12.1 criterion → phase coverage

| # | Criterion | Build → **gate** |
|---|---|---|
| 1 | Discovery | 07, 09, 16, 17, 21 → **32** |
| 2 | Git state + deadline | 07, 09, 18–21 → **32** |
| 3 | Sort and quiet | 10, 12 → **32** |
| 4 | Claude source | 08, 10, 22, 23, 31 → **33** |
| 5 | Codex source | 08, 10, 24, 25, 31 → **33** |
| 6 | Matching | 08, 10, 13, 30 → **33** |
| 7 | Incremental & pointers | 08, 22 → **33** |
| 8 | Hostile repos | 07, 14, 15a–c, 18, 19, 20 → **35** |
| 9 | Guards & first run | 07, 16, 17 → **32** |
| 10 | Privacy | 08, 11, 26, 31 → **35** + **34** (MCP defaults) |
| 11 | Sanitising & shell safety | 08, 11, 31, 36, 37, 38 → **39** (sanitising also in 31, 36) |
| 12 | Store | 26, 30 → **35** |
| 13 | No-agent mode | 30, 31 → **32** + **36** |
| 14 | Schemas & MCP | 05, 06 → **34** + **32** (status / doctor) |
| 15 | TUI | **36** |
| 16 | Build & supply chain | 01, 02 → **41** |
| 17 | Performance | 08, 28 → **40** |
| 18 | Demo | 28 → **42** |

§12.2 author smoke runs after P42, recorded in the run handoff, not gated.

## Edge cases considered

- **Hostile input:** every file opened from a repo or log goes through `O_NOFOLLOW | O_NONBLOCK` + fstat; FIFOs are tested at `.git`, `packed-refs`, `include.path`, `config.worktree`, a log path, and the resume file, each under a watchdog.
- **Concurrency:** two processes on one root (P26, P30); a live Codex WAL DB (P24); abandoned workers outliving the scan (P21, P30); parallel lanes isolated by worktree (R2).
- **Boundaries:** 10,000-commit saturation, 10,000 refs, 200 graphemes, 1 MiB lines, 16 KiB MCP responses, exact walk budget, the `quiet` day edge.
- **Missing input:** no agent dirs (P32), DB absent (P25), no default branch / remote / upstream (P20), missing worktree dir (P16), cwd gone → `resume: null` (P31).
- **Permissions:** unreadable dir (P16), umask 000 / 022 (P26, P35), resume-file owner / mode / links (P38), other-uid repos (P19, unit-level via forced trust).
- **Crash safety:** a mid-commit panic keeps the prior state (P26).

## Open questions — answered 2026-09-29

1. **GitHub remote:** private GitHub repo now; push `impl/v0` after each lane merge.
2. **Local tools:** install `cargo-deny`, `cargo-audit`, `hyperfine`, `fish`, `cargo-dist` locally; R3's skip path remains for machines without them.
3. **Ponytail trims:** (a) drop `directories` — yes; (b) SHA-1 lock name — yes; (c) keep `arboard` (no OSC 52-only); (d) drop `criterion` — yes. Spec revision 10 records (a), (b), (d).
