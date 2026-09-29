# Plan: rollcall v0  (37 phases in 5 waves, branch/worktree: `impl/v0` at `../rollcall-v0`)

**Spec:** `docs/superpowers/specs/2026-09-29-rollcall-design.md` (revision 6). The spec is the contract; this plan references its sections (`§n`) instead of restating them. Where the plan and spec disagree, the spec wins and the plan is the defect.

**Deviation from the `/phases` house format:** the house format asks for 3–5 phases. This is a 5–7 week greenfield project with 18 gating criteria and a psurgeon cap of 2 substantive files per task, so it has 37 phases grouped into 5 waves. Every other house rule holds (criteria first, ≤ 2 substantive files, tree green after every phase, open questions asked).

**Explorer pass:** not applicable. The repo holds only docs; there is no code and no `graphify-out/`. Every source path below is `(new)`. External facts were checked on 2026-09-29: toolchain `rustc 1.97.1`; crates.io latest `gix 0.88.0`, `rusqlite 0.40.2`, `ratatui 0.30.2`, `crossterm 0.29.0`, `jiff 0.2.37`, `clap 4.6.7`; the gitoxide checkout at `5708de4` (gix 0.86) depends non-optionally on `jiff` (gix-date), `libc` (gix-features), `crossbeam-channel` (gix-fs) — P01 re-confirms these for 0.88 with `cargo tree -i`; GitHub-hosted runner labels (see spec revision 6).

**Setup (once, before P01):** `git worktree add ../rollcall-v0 -b impl/v0` from `main`. All phases run there. One commit per phase, message `P<nn>: <title>`. Nothing is pushed without authorization (Q1).

---

## Execution rules (apply to every phase)

### R1. Shared files are orchestrator-owned after P01

`Cargo.toml` (root and every crate), `Cargo.lock`, every `lib.rs`/`main.rs` `mod` list, and `crates/rollcall-cli/tests/it/main.rs` are created complete in P01. No later phase edits them. If a worker needs a new dependency, feature, or module, it stops and reports; the orchestrator makes the edit serially between waves. This is what makes the parallel waves file-disjoint.

### R2. Test-first inside each phase

Each phase writes its tests, runs them red, then implements. Unit tests live in the same file as the code (`#[cfg(test)] mod tests`). Gating integration tests live in the single integration-test crate `crates/rollcall-cli/tests/it/` and compare against hand-written oracles committed in Wave B before any code they judge.

### R3. Every check must be able to fail

- Any filtered `cargo test` run must report `> 0` tests run; a filter that matches nothing is a failed check. Pass counts are read from the full line `test result: ok. N passed; 0 failed`. One positional filter per cargo invocation; several filters go after `--`.
- Tests that need an optional tool (`fish`, `zsh`, `hyperfine`, `vhs`) skip locally when the tool is absent, and **fail** instead of skipping when `ROLLCALL_REQUIRE_TOOLS=1` (set in CI).
- Every phase's verification includes `cargo clippy --all-targets --locked -- -D warnings` and `cargo test --locked` for the whole workspace, green.

### R4. Ponytail (lazy senior dev) — how, never what

Source: the ponytail ruleset (github.com/dietrichgebert/ponytail, `.agents/rules/ponytail.md`), adapted into the repo `CLAUDE.md` in P01 so every worker session gets it.

- **Applies to how code is written:** before writing code, stop at the first rung that holds: (1) not needed, (2) already in this repo, (3) Rust std, (4) platform feature, (5) a dependency already in `Cargo.lock`, (6) one line, (7) minimum code. No unrequested abstractions, no boilerplate, fewest files, boring over clever. Deliberate corner-cuts get a `// ponytail: <ceiling>, <upgrade path>` comment.
- **Does not apply to what is built:** v0 scope, the 5-crate layout, the `Index` / `SessionSource` traits, the fixture manifest, hand-written oracles, `insta`, `expectrl`, `jsonschema`, and every §12.1 criterion are decided in the spec. Ponytail never cuts them. It is also never lazy about input validation at trust boundaries, §4.4 hardening, §9 privacy, or error handling that prevents data loss.

### R5. Dependency ladder (recorded; P01 declares exactly this set)

| Need | Choice | Rung | Notes |
|---|---|---|---|
| git | `gix 0.88`, `default-features = false`, explicit features | 7 | Feature list fixed in P01 by compiling the APIs §4.4/§3.3 name; `cargo tree -i gix-transport` must be empty. |
| timestamps | `jiff` | 5 | Already a gix dependency. `Timestamp = jiff::Timestamp`. |
| threads, channels | `std::thread::scope`, `std::sync::mpsc` | 3 | No `rayon`, no direct `crossbeam`. |
| scan lock | `std::fs::File::try_lock` | 3 | Stable since Rust 1.89 → `rust-version = "1.89"`. No `fs2`/`fd-lock`. |
| UUID check | hand-written `is_uuid(&str)` (36 chars, hyphens at 8/13/18/23, hex elsewhere) | 6 | No `uuid` crate; ids stay `String` newtype `Uuid`. |
| `O_NOFOLLOW`, `O_NONBLOCK` constants | `libc` constants only | 5 | Already a gix dependency. Constants are safe; no `libc` function calls. |
| SQLite (Codex DB + cache) | `rusqlite` with `bundled` | 7 | One crate for both uses. |
| JSON / TOML / CLI | `serde`, `serde_json`, `toml`, `clap` (derive) | 7 | |
| fingerprints | `xxhash-rust` (`xxh3` feature only) | 7 | Tiny, no deps. |
| scrubbing | `regex` | 7 | |
| truncation | `unicode-segmentation` | 7 | Named by §3.3. |
| TUI | `ratatui`, `crossterm` | 7 | |
| `$EDITOR` split | `shell-words` | 7 | Edge-case-correct; hand-rolled splitting is the flimsier option. |
| clipboard | `arboard` | 7 | Gated on Q3. |
| cache dir | `directories` | 7 | Gated on Q3. |
| lock file name | `sha2` | 7 | Gated on Q3. |
| MCP | hand-written JSON-RPC 2.0 over stdio (`serde_json`) | 7 | ~150 lines for `initialize`, `tools/list`, `tools/call`, `ping`. No `rmcp` / `tokio`. |
| dev: temp dirs | `tempfile` | 7 | |
| dev: oracles | `jsonschema`, `insta`, `expectrl`, `criterion` | 7 | Named by §10. `criterion` gated on Q3. |
| fixture repos | real `git` CLI, resolved to an absolute path at harness start | 4 | Independent of rollcall's logic, as §10.1 requires. |

### R6. Safe-std mapping for every syscall the spec names (`unsafe_code = "forbid"`)

| Spec need | Safe API |
|---|---|
| `open(O_NOFOLLOW \| O_NONBLOCK)` | `OpenOptions::custom_flags(libc::O_NOFOLLOW \| libc::O_NONBLOCK)` (`std::os::unix::fs::OpenOptionsExt`) |
| `O_CLOEXEC` | std default |
| `fstat` regular-file / owner / links / mode | `File::metadata()` + `MetadataExt::{dev, ino, uid, nlink, mode}`; `file_type().is_file()` |
| `lstat` | `std::fs::symlink_metadata` |
| `SF_DATALESS` (macOS) | `std::os::macos::fs::MetadataExt::st_flags() & SF_DATALESS`, with `const SF_DATALESS: u32 = 0x4000_0000` under `cfg(target_os = "macos")` |
| effective uid | see the rule below the table |
| `pread` | `FileExt::read_exact_at` |
| truncate via fd | `File::set_len(0)` |
| `chmod 0700/0600` regardless of umask | `DirBuilderExt::mode(0o700)` then `set_permissions`; files `OpenOptionsExt::mode(0o600)` then `set_permissions` |
| `flock` | `File::try_lock` |
| `chdir` + `execv` (TUI `R`) | `Command::new(abs_path).args(..).current_dir(cwd).exec()` (`CommandExt::exec`, no shell, no PATH search because the path is absolute) |
| realpath | `std::fs::canonicalize` |

**Effective uid rule:** the resume-file owner check compares the file's `uid()` with the `uid()` of a file rollcall itself creates in its own cache directory (created by P25 with mode `0700`). No `geteuid` call is needed, no `unsafe`, no new crate.

---

## Wave A — scaffold and core types (serial)

## Phase 01 — Workspace scaffold
**Description:** Create the full 5-crate workspace, every module file as an empty stub, all dependencies, and the repo `CLAUDE.md`. After this phase no other phase edits a manifest or a `mod` list (R1).
**Files:** `Cargo.toml` (new), `CLAUDE.md` (new); mech: `crates/rollcall-{core,git,sessions,store}/Cargo.toml` + `src/lib.rs`, `crates/rollcall-cli/Cargo.toml` + `src/main.rs`, every stub module named in this plan, `crates/rollcall-cli/tests/it/main.rs` + stub modules, `.gitignore`, `LICENSE-MIT`, `LICENSE-APACHE`, `Cargo.lock` (generated).
**Changes:**
- Root `Cargo.toml`: `resolver = "3"`, `rust-version = "1.89"`, `edition = "2024"`, `[workspace.lints.rust] unsafe_code = "forbid"`, `[workspace.dependencies]` = R5 set; every crate `[lints] workspace = true`.
- Crate deps follow §3.4: `git`, `sessions`, `store` depend on `core` only; `cli` on all four.
- `rollcall --version` / `--help` via clap; all other subcommands parse and exit 2 with `not implemented` until their phase.
- `CLAUDE.md`: spec + plan pointers, R1–R6 in brief, the ponytail ladder paraphrased with attribution, the "never print prompt text from real logs" rule, the §4.4 gix re-audit rule.
**Acceptance criteria:**
- [ ] `cargo build --workspace --locked` succeeds.
- [ ] `cargo tree -p rollcall-git -e normal | grep -E 'rollcall-(sessions|store)'` and the same for `sessions`→`git|store` and `store`→`git|sessions` print nothing.
- [ ] `cargo tree -i gix-transport --workspace` exits non-zero ("package not found"); same for `reqwest`, `hyper`, `curl`, `ureq`.
- [ ] Every crate `Cargo.toml` contains `[lints] workspace = true`; adding `unsafe {}` to any crate fails the build (checked once by hand, reverted, noted in the commit body).
- [ ] `cargo run -q -p rollcall-cli -- --version` prints `rollcall 0.1.0`.
**Expected test outputs:** `cargo test --workspace --locked` → `0 failed` (no tests yet); `cargo clippy --all-targets --locked -- -D warnings` → no output, exit 0.

## Phase 02 — CI and supply-chain baseline
**Description:** CI jobs from §11, pinned by SHA, least-privilege. Cannot run until a GitHub remote exists (Q1); local equivalents are the gate until then.
**Files:** `.github/workflows/ci.yml` (new), `deny.toml` (new); mech: `.github/dependabot.yml`.
**Changes:** jobs `fmt`, `clippy`, `test` (matrix `ubuntu-latest`, `macos-14`; installs `zsh`, `fish`, `jq`; sets `ROLLCALL_REQUIRE_TOOLS=1`), `deny`, `audit` (on push + weekly `schedule`); `permissions: contents: read` at top level; every `uses:` pinned to a 40-hex SHA with a version comment. `deny.toml` bans `reqwest`, `hyper`, `curl`, `gix-transport`, `ureq`; license allow-list compatible with MIT/Apache-2.0.
**Acceptance criteria:**
- [ ] `grep -E 'uses: [^@]+@[0-9a-f]{40}' .github/workflows/ci.yml` matches every `uses:` line (count equal to `grep -c 'uses:'`).
- [ ] `cargo deny check` passes (run locally if Q2 allows installing it, else first CI run).
- [ ] Temporarily adding `ureq` to a crate makes `cargo deny check bans` fail (done once, reverted, noted in commit body).
**Expected test outputs:** `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`.

## Phase 03 — Core domain types, traits, in-memory Index
**Description:** Every type §3.4 names, including those the spec leaves to the plan, plus the §7 output documents. Everything downstream codes against these.
**Files:** `crates/rollcall-core/src/lib.rs` (new), `crates/rollcall-core/src/mem.rs` (new).
**Changes:**
- Types from §3.4 verbatim: `SourceKind`, `TextRef`, `TextField`, `TitleKind`, `Session`, `FileState`, `SourceState`, `SourceScan`, trait `Index`.
- Types §3.4 leaves open, defined here:
  - `Timestamp = jiff::Timestamp`; `Uuid(String)` with `Uuid::parse(&str) -> Option<Uuid>` (R5).
  - `Backing { LogFile { dev, inode, path }, CodexThread { db_path, thread_id: Uuid } }`.
  - `RepoKind { Normal, Nested, Bare, ExternalMain }`; `StaleReason { Missing, Merged, Inactive }`.
  - `WorktreeState` (git fields of §7 `Worktree`: path, is_main, branch, detached, head, last_commit_at, upstream, ahead, behind, unpushed, dirty_count, untracked_count, missing, merged, error).
  - `RepoState { default_branch, has_remote, error, worktrees: Vec<WorktreeState>, submodules: Vec<RepoState> }`.
  - `WalkOptions { follow_symlinks, submodules, skip_dirs, budget_entries, budget_secs, home, yes }`; `GitOptions { repo_budget: Duration, excludes_file: Option<PathBuf> }`.
  - `DiscoveredRepo { git_dir, workdir: Option<PathBuf>, kind: RepoKind, contains_root, is_submodule_of: Option<usize>, linked: Vec<LinkedWorktree>, error: Option<String> }`, `LinkedWorktree { path, admin_dir, missing }`, `Discovery { root, repos, walk: WalkDiag, partial }`.
  - `WorktreeKey { canonical_path, hidden, missing }` (adds `missing`, needed by §6.5); `SessionLocation { cwd_canonical, cwd_exists, under_root }`; `MatchResult { Matched(usize), OutsideRoot, PathGone, NonRepo }` (one per session, index into the key slice).
  - `RepoRow { path, name, kind, contains_root, default_branch, has_remote, error, worktrees: Vec<WorktreeState>, submodules: Vec<RepoRow> }`.
  - `ScanDiagnostics` = the `DoctorDoc` body minus `schema_version`/`root`; `SourceDiagnostics` = one `sources[]` entry.
  - `ScanResult { scanned_at, partial, repos: Vec<RepoRow>, sources: Vec<(SourceKind, SourceScan)>, matches: Vec<(SourceKind, Uuid, Option<PathBuf>)>, diagnostics: ScanDiagnostics }`.
  - §7 output types with `serde::Serialize`: `StatusDoc`, `Repo`, `Worktree`, `SessionOut`, `WhereWasIDoc`, `ListStaleDoc`, `DoctorDoc`. Field names and enum spellings exactly as §7.
- Timestamp output rule (fills a spec gap, no behavior choice): RFC 3339 UTC via `jiff::Timestamp` `Display` (`Z` suffix, fractional seconds only when non-zero). Oracles in Wave B use this form.
- `mem.rs`: `MemIndex` implementing `Index` over `Vec`s, for unit tests.
**Acceptance criteria:**
- [ ] A unit test serialises one fully populated `StatusDoc` and one `DoctorDoc` and asserts the exact key set at every level equals §7 (hand-listed in the test).
- [ ] `TitleKind`, `StaleReason`, `RepoKind`, `SourceKind` serialise to the §7 lowercase / snake_case strings.
- [ ] `Uuid::parse` accepts upper- and lower-case valid ids and rejects: 35/37 chars, a hyphen moved, a non-hex char (≥ 6 cases).
- [ ] `MemIndex::commit_scan` then `repos_for_root`, `sessions_for` (ended_at desc), `scanned_at`, `last_diagnostics` round-trip.
**Expected test outputs:** `cargo test -p rollcall-core --locked` → ≥ 4 tests, `0 failed`.

---

## Wave B — oracles and core pure functions (two parallel lanes)

Lane B1 (oracles) and lane B2 (core functions) touch disjoint files. Inside lane B1, phases that share a file run serially.

### Lane B1 — hand-written oracles (committed before any code they judge)

## Phase 04 — JSON Schemas: status and doctor
**Description:** Schemas for `StatusDoc` and `DoctorDoc` from §7, hand-written.
**Files:** `schema/status.json` (new), `schema/doctor.json` (new).
**Changes:** JSON Schema 2020-12. `status.json` holds `$defs` `Repo`, `Worktree`, `Session` for reuse. `additionalProperties: false` everywhere; nullability and enums exactly as §7; `attention` integer 0–4; `submodules` optional.
**Acceptance criteria:**
- [ ] Both files parse as JSON and as valid 2020-12 schemas (checked by the P16 harness self-test; until then by `jq empty`).
- [ ] A hand-written minimal valid `StatusDoc` (in P16's self-test) validates; the same document with one extra key, `attention: 5`, or `title_kind: null` while `title` is non-null fails. (`title_kind` null iff `title` null is enforced with `if/then`.)
**Expected test outputs:** `jq empty schema/*.json` → exit 0.

## Phase 05 — JSON Schemas: where_was_i and list_stale
**Description:** MCP document schemas, reusing `status.json#/$defs`.
**Files:** `schema/where_was_i.json` (new), `schema/list_stale.json` (new).
**Acceptance criteria:** same form as P04 for `WhereWasIDoc` and `ListStaleDoc`, including `truncated: bool` required.
**Expected test outputs:** `jq empty schema/*.json` → exit 0.

## Phase 06 — Fixture manifest, part 1: git fixtures
**Description:** The hand-written `MANIFEST.toml` sections for roots `F`, `F_clean`, `H`, `EXT`, the budget fixture, the refused-`HOME` fixture, and the perf corpus shape (§10.1, §10.2, §12.1 #9, #17).
**Files:** `tests/fixtures/MANIFEST.toml` (new).
**Changes:** one table per fixture repo: path, kind, commits (fixed author/committer name, email, dates), branches, remotes, upstream config, worktrees (linked, external, missing), file states (staged, unstaged, untracked, ignored via a manifest-declared `core.excludesFile`), submodules (modern, old-style), bare, nested, symlink loop, symlink out of root, unreadable dir, corrupted repo, bad `gitdir`, untrusted-path worktree, every §10.2 hostile repo and marker name, hang probe, root-inside-repo case; `[refused_home]` with its expected suggestion list; `[budget]` with entry count > the test's `walk_budget_entries`; `[perf]` counts from §12.1 #17; `members` lists for `F_clean` and `H` (§10.1).
**Acceptance criteria:**
- [ ] Every §10.2 "Discovery", "Git state", "Hostile repos" bullet names at least one manifest entry (checked by a comment `# covers: <bullet>` on the entry, grep-able).
- [ ] `F_clean` = `F` minus exactly the entries §10.1 lists (explicit list in the file).
**Expected test outputs:** `python3 -c 'import tomllib,sys;tomllib.load(open("tests/fixtures/MANIFEST.toml","rb"))'` → exit 0.

## Phase 07 — Fixture manifest, part 2: sessions
**Description:** Synthetic Claude and Codex session fixtures with their expected derived fields (§10.1), after P06 (same file).
**Files:** `tests/fixtures/MANIFEST.toml` (edit).
**Changes:** `[[claude]]` entries covering each §10.2 "Claude fixtures" bullet; `[[codex]]` DB rows + rollouts covering each "Codex fixtures" bullet; the 20-session matching set (16 matchable incl. one in a hidden submodule, 1 `non_repo`, 1 `path_gone`, 2 `outside_root`); incremental-read scripts (append 3 + partial, truncate, truncate-regrow, title upgrade, vanish); pointer cases; one prompt per §9 scrub pattern; multi-byte and pre-ellipsised truncation inputs; sanitising payloads (`\x1b[2J`, `\x9b`, U+202E, U+200B) in a branch name and a prompt; shell-hostile worktree paths (space, `$`, backtick, `;`, `'`, newline) and branch `$(touch marker-rc)`; `[mcp_200]` generating 200 sessions.
**Acceptance criteria:**
- [ ] Every §10.2 "Claude fixtures", "Codex fixtures", "Incremental reads", "Pointer resolution", "Matching", "Privacy", "Sanitising", "Shell safety" bullet has a `# covers:` entry.
- [ ] The matching set's counts are stated in the file and add to 20.
**Expected test outputs:** same `tomllib` load → exit 0.

## Phase 08 — Expected outputs: discovery, status, order
**Description:** Hand-written oracles for §12.1 #1, #2, #3, from the manifest and spec only — never by running rollcall.
**Files:** `tests/expected/discovery.json` (new), `tests/expected/status.json` (new); mech: `tests/expected/order.json` (small, same derivation).
**Changes:** paths as `$F/…`, `$EXT/…`; error strings as their §4.1 prefix; `order.json` = repo path order + per-repo `attention`, `last_activity_at` under the fixed `ROLLCALL_NOW` declared in the manifest.
**Acceptance criteria:**
- [ ] Each file lists every repo/worktree the manifest puts in `F` (count stated in a top-level `"_count"` key the harness strips).
- [ ] Each attention and stale value carries a one-line derivation in a sibling `"_why"` key (harness strips `_`-prefixed keys) so a reviewer can re-derive it from §3.3.
**Expected test outputs:** `jq empty tests/expected/*.json` → exit 0.

## Phase 09 — Expected outputs: sessions and matching
**Description:** Oracles for §12.1 #4, #5, #6.
**Files:** `tests/expected/claude.json` (new), `tests/expected/codex_db.json` (new); mech: `tests/expected/codex_rollout.json`, `tests/expected/matching.json`.
**Acceptance criteria:**
- [ ] Every manifest session appears once with `title, title_kind, last_prompt, branch, cwd, message_count, started_at, ended_at` and a `_why` for title kind.
- [ ] `matching.json` has 16 matched sessions, ≥ 1 worktree with `older_sessions ≥ 1`, and `unmatched: {non_repo: 1, path_gone: 1, outside_root: 2}`.
**Expected test outputs:** `jq empty tests/expected/*.json` → exit 0.

### Lane B2 — core pure functions (TDD, unit tests in-file)

## Phase 10 — Attention, stale, sort, quiet
**Description:** §3.3 derived fields as pure functions.
**Files:** `crates/rollcall-core/src/rank.rs` (new content in stub).
**Changes:** `stale_reason(&WorktreeState, is_linked, now, stale_days)`, `worktree_attention`, `repo_attention`, `last_activity_at`, `sort_repos`, `quiet_label(now) -> "quiet Nd" | "quiet —"`.
**Acceptance criteria:**
- [ ] Table test covers every §3.3 branch: missing beats merged; merged only for linked non-default branch; inactive requires all four conditions (one negative case per condition); main worktree never stale.
- [ ] Attention precedence 4 > 3 > 2 > 1 > 0 with one case each; repo attention = max incl. repo error → 4.
- [ ] Sort: attention desc, then activity desc with null last, then path asc (≥ 1 tie-break case each).
- [ ] `quiet` whole-day floor at exactly 24 h boundary (23h59m → `quiet 0d`, 24h → `quiet 1d`).
**Expected test outputs:** `cargo test -p rollcall-core rank:: --locked` → ≥ 12 passed, 0 failed.

## Phase 11 — Session → worktree matching
**Description:** `match_sessions` per §6.5.
**Files:** `crates/rollcall-core/src/matching.rs`.
**Acceptance criteria:**
- [ ] Longest component-wise prefix: `/a/b` does not match `/a/bc`; `/a/b/c` picks `/a/b` over `/a`.
- [ ] Hidden submodule keys are skipped (session falls through to the parent); a missing worktree matches only a session whose cwd equals its path.
- [ ] Classes: `cwd_exists=false` and no key → `PathGone`; `under_root=false` → `OutsideRoot`; exists, under root, no key → `NonRepo`.
- [ ] 10,000 sessions × 100 keys completes in < 500 ms in the default (debug) test profile, guarding an accidental quadratic-per-component blowup.
**Expected test outputs:** `cargo test -p rollcall-core matching:: --locked` → ≥ 8 passed.

## Phase 12 — Text pipeline: prompt filter, sanitise, scrub, truncate, shell quote
**Description:** §5.0 prompt filter, §9 sanitising and scrubbing, §3.3 truncation, and the shell quoting used by `c`/`Y` (§8.1).
**Files:** `crates/rollcall-core/src/text.rs`.
**Changes:** `filter_prompt_block`, `sanitise`, `scrub(text, redact_emails)` (regexes compiled once via `std::sync::LazyLock`), `truncate200`, `render_text = truncate200(scrub(sanitise(x)))`, `shell_quote(&Path) -> String` (single-quote wrap, `'` → `'\''`, bytes preserved via `OsStrExt`).
**Acceptance criteria:**
- [ ] Each §9 scrub pattern: one positive and one near-miss negative (e.g. `sk-` + 15 chars not redacted).
- [ ] Sanitise removes every §9 code point class (one case per listed range endpoint) and maps `\n`/`\t` to space.
- [ ] Truncation: 201 graphemes → 200 incl. `…`; 200 unchanged; a 200-grapheme input ending in `…` unchanged; a family emoji ZWJ sequence is never split.
- [ ] Prompt filter: `<`-first block dropped; `# AGENTS.md` dropped; cut at `<system-reminder>`; whitespace-only → none.
- [ ] `shell_quote` round-trip: for each §10.2 shell-hostile path (space, `$`, backtick, `;`, `'`, newline), `sh -c "printf '%s' <quoted>"` output bytes equal the path (this test spawns `/bin/sh`).
**Expected test outputs:** `cargo test -p rollcall-core text:: --locked` → ≥ 30 passed.

---

## Wave C — crates in parallel (git ‖ sessions ‖ store ‖ fixture generator)

Four lanes, file-disjoint, each depending only on Wave A (+ Wave B oracles for the generator). Inside a lane, phases are serial.

### Lane C1 — `rollcall-git`

## Phase 13 — gix 0.88 read-site re-audit
**Description:** §4.4's maintenance rule: the pre-open denylist was verified at gix 0.86; the pin is 0.88. Re-audit before writing the hardened open.
**Files:** `docs/audits/gix-0.88-read-sites.md` (new).
**Changes:** from the registry source of the exact locked `gix*` versions, list every `fs::read`, `read_to_end`, `read_to_string`, `BufReader::lines`, `std::fs::File::open` + unbounded read reachable from `open_opts`, `index_worktree_status`, `rev_walk`, reference iteration, and config loading; for each: file:line, what it reads, and which §4.4 pre-open check or config override covers it. Also confirm each §4.4 API name exists at 0.88 (`Config::isolated`, `config_snapshot_mut`, `index_worktree_status`, `SubmoduleStatus`, interrupt flag, `rev_walk().with_hidden()`, `gitoxide.objects.allocLimit`, `core.commitGraph` override).
**Acceptance criteria:**
- [ ] Every read site has a "covered by" entry or is listed under "uncovered".
- [ ] If "uncovered" or "API differs" is non-empty: stop, report, and raise a spec revision; the §4.4 rule is never weakened to fit (§1 Assumptions).
**Expected test outputs:** n/a (document); reviewer spot-checks 3 random file:line refs.

## Phase 14 — Discovery walk
**Description:** §4.1 walk and classification, §4.2 skips, §4.3 guards except root refusal.
**Files:** `crates/rollcall-git/src/walk.rs`, `crates/rollcall-git/src/classify.rs`.
**Changes:**
- `walk.rs`: parallel walk with `std::thread::scope`, `available_parallelism()` workers sharing a `Mutex<Vec<PathBuf>>` stack + in-flight counter + `Condvar`; `lstat` only; same-`st_dev`; `SF_DATALESS`; skip names/paths; symlink policy with visited `(dev, inode)`; atomic entry counter and deadline for the budget → `partial`; unreadable-dir count; mount list for `doctor`.
- `classify.rs`: `.git` dir / file (`gitdir:` ≤ 4 KiB, `O_NOFOLLOW|O_NONBLOCK`, fstat regular) / old-style submodule via parent `.gitmodules` (same pre-open cap) / bare heuristic / nested / external cases A and B / linked-worktree enumeration from `.git/worktrees/*` incl. missing (`HEAD` ≤ 4 KiB) / enclosing repo walk-up stopping before `$HOME` and `/`.
**Acceptance criteria:**
- [ ] Unit tests with tempdir trees built by an in-file `git()` helper (absolute path to real git): each §4.1 kind; `node_modules/x/.git` and `target/x/.git` skipped; `$HOME/Library` skipped only when root is outside it.
- [ ] Symlink loop: default and `follow_symlinks` both terminate < 2 s; symlink to outside root not followed in either mode.
- [ ] Budget: `budget_entries = 10` on a 100-dir tree → `partial = true` and walk stops ≤ 10 + workers entries.
- [ ] A FIFO named `.git` is never opened (test creates one; walk completes, no hang, counted as non-repo).
- [ ] `$HOME/.git` exists in the test `HOME` → not reported as enclosing repo.
**Expected test outputs:** `cargo test -p rollcall-git --locked -- walk:: classify::` → ≥ 14 passed.

## Phase 15 — Root refusal and suggestions
**Description:** §4.3 refused roots (`/`, `$HOME` by `(dev, inode)`), `--yes` / config `roots` override, suggestion peek (lstat only, 2 levels, skips incl. the macOS privacy dirs, same-device, 2 s cap, max 5).
**Files:** `crates/rollcall-git/src/root.rs`.
**Acceptance criteria:**
- [ ] Root = test `HOME` → `Refused { suggestions }` equal to a hand-listed set from a tempdir `HOME` containing 7 repos at depth 1–3 (depth-3 excluded, `Documents/x` excluded on macOS, cap 5, sorted by path).
- [ ] `yes = true` or root in `roots` → `Allowed`.
- [ ] Root = `/` → `Refused` without walking (test asserts it returns in < 100 ms).
**Expected test outputs:** `cargo test -p rollcall-git root:: --locked` → ≥ 5 passed.

## Phase 17 — Hardened open and worktree trust rule
**Description:** §4.4 pre-open checks, alternates chain, isolated config, config stripping, alloc limit, commit-graph off, user `core.excludesFile`, ownership; §4.1 worktree trust rule (a)–(d). Depends on P13's audit being clean.
**Files:** `crates/rollcall-git/src/open.rs`.
**Acceptance criteria:**
- [ ] Each pre-open file as a symlink → `untrusted git dir`; each over its cap (sparse file via `set_len`) → `untrusted git dir`; each missing → opens fine. One test per file in the §4.4 list.
- [ ] Alternates: 8 hops ok, 9 → untrusted; a hop's `info/alternates` symlinked → untrusted.
- [ ] After open, the resolved config has zero `filter.*`, `diff.*`, `merge.*` sections even with two subsections each across repo config (asserted by iterating the snapshot), and none of the five stripped keys.
- [ ] `core.worktree=$HOME` → trust rule (a) fails → `untrusted worktree path`; `commondir` to another repo → (b) fails; linked `gitdir` mismatch → (c) fails; workdir = `$HOME` → (d) fails.
- [ ] Repo owned by another uid: covered by a unit test on the reduced-trust branch using gix's trust API directly ; an extra CI-only case chowns a fixture repo with `sudo` on Linux runners if P02's workflow grants it, otherwise the unit test alone stands and the gap is documented.
**Expected test outputs:** `cargo test -p rollcall-git open:: --locked` → ≥ 25 passed.

## Phase 18 — Per-worktree git state
**Description:** §3.3 git fields via gix: default branch, upstream, ahead/behind/unpushed (saturating at 10,000, deadline check every 256 commits), dirty/untracked (via `index_worktree_status` with rollcall's `SubmoduleStatus` returning `Ok(None)`), last_commit_at, merged, detached, head.
**Files:** `crates/rollcall-git/src/state.rs`.
**Acceptance criteria:**
- [ ] For each §10.2 "Git state" bullet, a tempdir repo built with real git asserts the field equals what `git status --porcelain=v2 --branch` / `git rev-list --count` report (git is the oracle here; allowed because it is test-only and independent).
- [ ] Untracked count honours `core.excludesFile` from the sandboxed `HOME`'s `~/.gitconfig` (read by rollcall itself, §4.4) and from `$XDG_CONFIG_HOME/git/config` when `~/.gitconfig` lacks it; an `include.path` in `~/.gitconfig` is not followed.
- [ ] 10,001-commit linear history ahead of upstream → `ahead = 10000` (saturation) — built with `git fast-import` for speed.
- [ ] Squash-merged branch → `merged = false` (documented limitation holds).
**Expected test outputs:** `cargo test -p rollcall-git state:: --locked` → ≥ 16 passed.

## Phase 19 — `repo_state` with deadline, and `discover` entry point
**Description:** Public API of §3.4: `discover(root, &WalkOptions) -> Discovery`, `repo_state(&DiscoveredRepo, &GitOptions) -> RepoState` with per-repo detached worker thread, `mpsc::recv_timeout(repo_budget)`, per-repo interrupt `AtomicBool` set at the deadline, abandoned-thread counter; submodule rows via the same hardened open.
**Files:** `crates/rollcall-git/src/lib.rs` (fill public fns; `mod` list untouched).
**Acceptance criteria:**
- [ ] `repo_budget = 1 ms` on a 3-repo tempdir → every row `error` starts `timed out`.
- [ ] A test-only hook that sleeps 200 ms inside state computation with a 50 ms budget → `timed out` returned in < 150 ms and `abandoned` counter = 1.
- [ ] Error in one repo (corrupted `HEAD`) doesn't affect siblings.
**Expected test outputs:** `cargo test -p rollcall-git --locked` → all C1 tests, ≥ 60 passed, 0 failed.

### Lane C2 — `rollcall-sessions`

## Phase 20 — Log file I/O and incremental reads
**Description:** §5.0 file rules and §6.3 incremental mechanics, source-agnostic.
**Files:** `crates/rollcall-sessions/src/jsonl.rs`.
**Changes:** `open_log(path) -> Result<(File, Metadata)>` (`O_NOFOLLOW|O_NONBLOCK`, fstat regular); capped line iterator over `BufReader` that skips lines > 1 MiB without buffering them (counts `oversize`), stops before a partial trailing line, never reads past size-at-open; `head_fp` (xxh3-128 of first line ≤ 4 KiB), `tail_fp` (64 bytes ending at offset via `read_exact_at`); `plan_read(prior: Option<&FileState>, now_meta) -> ReadFrom(0 | offset)` implementing every §6.3 rule incl. `parser_version`; `resolve_log_ref(TextRef::LogLine)` per §6.2.
**Acceptance criteria:**
- [ ] Append 3 lines + partial → 3 records, offset ends before the partial.
- [ ] Truncate → re-read from 0; truncate then regrow past offset → re-read (tail_fp); different head → re-read; older `parser_version` → re-read; same `(dev, inode)` at new path + matching fps → incremental (path updated).
- [ ] A 2 MiB line between two small lines → both small lines parsed, `oversize = 1`, and the reader's high-water buffer length (a counter the reader exposes under `cfg(test)`) stays ≤ 1 MiB + 8 KiB. (A counting `GlobalAlloc` would need `unsafe`, which is forbidden.)
- [ ] A FIFO and a symlink at a log path → rejected without blocking.
- [ ] Resolution: file replaced (new inode) → `None`; shrunk below `size_at_scan` → `None`; symlink swapped in → `None`.
**Expected test outputs:** `cargo test -p rollcall-sessions jsonl:: --locked` → ≥ 14 passed.

## Phase 21 — Claude source
**Description:** §5.1 extraction + §6.3 merge + `SessionSource` impl.
**Files:** `crates/rollcall-sessions/src/claude.rs`.
**Acceptance criteria:**
- [ ] One test per §10.2 "Claude fixtures" bullet, using the §5.1 worked records as literal JSONL.
- [ ] Title precedence custom > ai > first prompt; incremental append of a `custom-title` upgrades a prompt title while keeping the first-prompt ref.
- [ ] Subagent files excluded and counted; non-UUID skipped and counted; `CLAUDE_CONFIG_DIR` honoured.
- [ ] `resolve` returns the raw extracted text; after appending unrelated lines the ref still resolves identically.
**Expected test outputs:** `cargo test -p rollcall-sessions claude:: --locked` → ≥ 16 passed.

## Phase 22 — Codex source: thread database
**Description:** §5.2 primary mode.
**Files:** `crates/rollcall-sessions/src/codex_db.rs`.
**Acceptance criteria:**
- [ ] DB opened via URI `file:…?mode=ro`, never `immutable=1` (asserted by a test that opens a DB in WAL mode with an uncheckpointed write from a second connection and sees the row).
- [ ] Highest `state_<N>.sqlite` with the required columns wins; one lacking a column is skipped.
- [ ] Three child-exclusion rules each exclude one thread; counts in diagnostics.
- [ ] Title kind: equal → `Prompt`; truncated prefix with `…`/`...` → `Prompt`; > 200 graphemes → `Prompt`; unrelated → `Thread`; empty → filtered `first_user_message` as `Prompt`.
- [ ] `rollout_path` outside `CODEX_HOME` → ignored and counted `bad_rollout_path`.
- [ ] After a scan the DB file's bytes and mtime are unchanged (read-only rule), `-shm` changes allowed.
**Expected test outputs:** `cargo test -p rollcall-sessions codex_db:: --locked` → ≥ 12 passed.

## Phase 23 — Codex source: rollouts (fallback + DB-mode prompts)
**Description:** §5.2 fallback mode and the rollout-derived `last_prompt` / `message_count` in DB mode; archiving move via §6.3 path update; `SessionSource` impl for Codex choosing mode.
**Files:** `crates/rollcall-sessions/src/codex_rollout.rs`, `crates/rollcall-sessions/src/lib.rs` (trait + `Codex` type dispatching DB/rollout; `mod` list untouched).
**Acceptance criteria:**
- [ ] `user_message` events preferred; `response_item` path used only in files with none; `[injection, user]` blocks → only the user text.
- [ ] Second `session_meta` with different id ignored; `parent_thread_id` → child.
- [ ] DB absent → mode `rollouts`, reason recorded; DB present → mode `db` with `db_version = N`.
- [ ] Rollout moved to `archived_sessions/` → same session, incremental (no re-read from 0; asserted via records-read counter).
**Expected test outputs:** `cargo test -p rollcall-sessions --locked` → all C2, ≥ 50 passed.

### Lane C3 — `rollcall-store`

## Phase 24 — SQLite Index
**Description:** §6.1–6.2 schema and `Index` impl: repos/worktrees keyed by canonical path, root-membership table, sessions keyed `(source, id)` storing only `TextRef`s, file states, per-root diagnostics (JSON blob), `commit_scan` in one transaction, `PRAGMA user_version` rebuild.
**Files:** `crates/rollcall-store/src/lib.rs`.
**Acceptance criteria:**
- [ ] Round-trip parity with `MemIndex` on a shared test script (same inputs → equal outputs for all six trait methods).
- [ ] Two roots sharing a repo: each `repos_for_root` returns its own `kind` / `contains_root`.
- [ ] A panic injected between inserts (test hook) leaves the prior scan intact.
- [ ] `user_version` mismatch → DB deleted and rebuilt empty, no error.
- [ ] After committing a scan containing sessions with titles, `grep -c` of every title string over the DB, `-wal`, `-shm` bytes → 0.
**Expected test outputs:** `cargo test -p rollcall-store --locked` → ≥ 10 passed.

## Phase 25 — Cache directory, permissions, scan lock
**Description:** §6.1 location and modes; §6.4 lock.
**Files:** `crates/rollcall-store/src/fs.rs`.
**Changes:** `cache_dir()` (macOS `~/Library/Caches/rollcall`, Linux `$XDG_CACHE_HOME/rollcall` or `~/.cache/rollcall`), created `0700`; DB and `-wal`/`-shm` chmod `0600` after open; `ScanLock::try_acquire(root) -> Option<ScanLock>` on `scan-<hash(canonical root)>.lock` with `File::try_lock`; `uid_probe()` for R6's effective-uid rule; `clear()` for `cache clear`.
**Acceptance criteria:**
- [ ] Dir mode `0700` and the three files `0600` after creation with the test process's umask explicitly widened to `000` for the creating child (`sh -c 'umask 000; exec <test-bin> --exact fs::tests::create_child'`), proving modes are set explicitly rather than inherited. The umask-022 CLI case is gated in P31 (#10).
- [ ] Second `try_acquire` for the same root in another process → `None`; different root → `Some`; lock released on drop.
**Expected test outputs:** `cargo test -p rollcall-store fs:: --locked` → ≥ 5 passed.

### Lane C4 — fixture generator

## Phase 16 — Fixture generator and harness self-test
**Description:** §10.1 generator: builds `F`, `F_clean`, `H`, `EXT`, budget, refused-`HOME`, `mcp_200`, and perf roots from `MANIFEST.toml` in temp dirs, using the real `git` binary and `rusqlite` for Codex DBs. Also the harness helpers every gating test uses.
**Files:** `crates/rollcall-cli/tests/it/support.rs`, `crates/rollcall-cli/tests/it/selftest.rs`.
**Changes:** `build(root: Root) -> Fixture { dir, paths }`; env sandbox (`HOME`, `XDG_*`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `ROLLCALL_NOW`, `PATH` with resolved absolute git); `run_rollcall(args, env) -> Output`; `expected(name) -> Value` with `$F`/`$EXT` substitution after canonicalising both sides and `_`-key stripping; `assert_error_prefix`; `validate(schema, doc)`; `peak_rss(cmd)` (`/usr/bin/time -v` Linux, `-l` macOS); `require_tool(name)` implementing R3.
**Acceptance criteria:**
- [ ] Self-test: building `F` twice yields identical commit ids (fixed dates → deterministic).
- [ ] Self-test: every repo in `F` passes `git fsck` except the manifest's corrupted one.
- [ ] Self-test: every `tests/expected/*.json` validates against its schema where one applies, and the P04 negative docs fail.
- [ ] Self-test: every `# covers:` tag in the manifest names a fixture the generator actually built.
**Expected test outputs:** `cargo test -p rollcall-cli --test it selftest:: --locked` → ≥ 4 passed.

---

## Wave D — CLI, integration gates, TUI, resume, MCP (after Wave C)

Lanes: D1 (P26→P27→P28→P29) is serial and first. After P28, lanes D2 (TUI), D3 (resume/shell), D4 (MCP) run in parallel; the gating test phases P29–P31 run in lane D1.

## Phase 26 — Config and argument surface
**Description:** §3.1 commands and flags, §3.2 config, exit code `2` paths, env (`NO_COLOR`, `ROLLCALL_NOW`).
**Files:** `crates/rollcall-cli/src/config.rs`, `crates/rollcall-cli/src/main.rs`.
**Acceptance criteria:**
- [ ] Unknown config key → stderr warning, run continues; invalid TOML or wrong type → exit 2; flags override config.
- [ ] `repo_budget_secs` ≤ 0 → exit 2.
- [ ] `ROLLCALL_NOW` not RFC 3339 → exit 2.
- [ ] `rollcall` with non-TTY stdout behaves as `status`.
**Expected test outputs:** `cargo test -p rollcall-cli config:: --locked` → ≥ 8 passed.

## Phase 27 — Scan orchestration
**Description:** §3.4 pipeline: refusal check → lock → `discover` → `repo_state` (parallel over `available_parallelism`) → each source `scan(index.source_state(..))` → canonicalise cwds → `match_sessions` → `commit_scan`; `stale`/`--no-scan`/lock-held paths; exit code computation (§3.1).
**Files:** `crates/rollcall-cli/src/scan.rs`.
**Acceptance criteria:**
- [ ] Lock held by another process → answer from cache with `stale: true`; no cache → `scanned_at: null`, `repos: []`, exit 1.
- [ ] Any repo/worktree error or `partial` → exit 1; clean → 0; refused root → 3.
- [ ] One source failing (unreadable Codex DB) → other source and git still reported; `doctor` shows the reason.
**Expected test outputs:** `cargo test -p rollcall-cli scan:: --locked` → ≥ 6 passed.

## Phase 28 — Output: `status`, `scan`, `doctor`
**Description:** Build `StatusDoc` / `DoctorDoc` from `Index` rows: resolve refs, `render_text`, `redact_prompts`, sanitise paths/branches/errors in plain output only (§9), `resume` display line; plain table (§8.2); `scan` summary line (§3.1); `cache clear`.
**Files:** `crates/rollcall-cli/src/output.rs`.
**Acceptance criteria:**
- [ ] `status --json` on a 2-repo tempdir validates against `schema/status.json`.
- [ ] `scan` stderr matches `^scanned .+: \d+ repos, \d+ worktrees, \d+ sessions in [0-9.]+s \(\d+ errors\)$`.
- [ ] Plain output of a branch containing `\x1b[2J` has no ESC byte; JSON output keeps it escaped as `\u001b`.
**Expected test outputs:** `cargo test -p rollcall-cli output:: --locked` → ≥ 6 passed.

## Phase 29 — Gates: discovery, git state, order, guards, no-agent (§12.1 #1, #2, #3, #9, #13)
**Files:** `crates/rollcall-cli/tests/it/discovery.rs`, `crates/rollcall-cli/tests/it/no_agent.rs`.
**Acceptance criteria:** exactly §12.1 #1, #2 (incl. separate `F_clean` run with `repo_budget_secs = 0.001`), #3, #9, #13 as written in the spec, against P08 oracles; #1's `< 10 s` with the symlink loop in both symlink modes; #13's TUI no-agent snapshot deferred to P32 and cross-referenced there.
**Expected test outputs:** `cargo test -p rollcall-cli --test it --locked -- discovery:: no_agent::` → ≥ 9 passed, 0 failed.

## Phase 30 — Gates: sessions, matching, incremental, pointers (§12.1 #4, #5, #6, #7)
**Files:** `crates/rollcall-cli/tests/it/sessions.rs`.
**Acceptance criteria:** §12.1 #4–#7 as written, against P09 oracles; incremental cases driven through two CLI scans with file edits between; inode-reuse and `parser_version` cases stay unit tests (P20) per §10.2.
**Expected test outputs:** `cargo test -p rollcall-cli --test it sessions:: --locked` → ≥ 12 passed.

## Phase 31 — Gates: hostile repos, privacy, store (§12.1 #8, #10, #12)
**Files:** `crates/rollcall-cli/tests/it/hostile.rs`, `crates/rollcall-cli/tests/it/privacy.rs`; mech: `crates/rollcall-cli/tests/it/store.rs` (four short tests reusing helpers).
**Acceptance criteria:**
- [ ] §12.1 #8 as written, incl. both positive controls creating `marker-filter-x` and `marker-filter-lfs` with the real git resolved **before** the fake `git` is installed on `PATH`; the positive controls run first and the test fails if they don't produce markers (proves the vectors are live).
- [ ] §12.1 #10 and #12 as written.
**Expected test outputs:** `cargo test -p rollcall-cli --test it --locked -- hostile:: privacy:: store::` → ≥ 20 passed.

## Phase 32 — TUI
**Description:** §8.1 layout, keys, filter, first paint from cache with `stale`, background scan via `std::thread` + `mpsc`, badges and named colours, no agent UI when no source has data; `c`, `Y`, `o`, clipboard per config. `R` calls into P33's `resume` module (stubbed to a status message until P33 lands in the same wave — P33 owns `resume.rs`).
**Files:** `crates/rollcall-cli/src/tui.rs`, `crates/rollcall-cli/tests/it/tui.rs`.
**Acceptance criteria:**
- [ ] `TestBackend` 120×32 `insta` snapshots: with sessions, without sessions (closes #13's TUI part), filter active, help open.
- [ ] Every cell `fg`/`bg` ∈ {`Reset`, 16 named}; with `NO_COLOR=1` every cell style is default (#15).
- [ ] `o` with `EDITOR="code --wait"` spawns `code` with args `["--wait", <path>]` (stub on `PATH`, records argv); unset → status message.
**Expected test outputs:** `cargo test -p rollcall-cli --test it tui:: --locked` → ≥ 6 passed; `cargo insta test --check` → no pending snapshots.

## Phase 33 — Resume (`R`), resume file, `rc` shell init
**Description:** §8.1 `R` (absolute-only PATH search before chdir, `exec`), `ROLLCALL_RESUME_FILE` protocol (spec revision 6: no `O_TRUNC` before checks), exit 10, `rollcall init <bash|zsh|fish>`.
**Files:** `crates/rollcall-cli/src/resume.rs`, `crates/rollcall-cli/tests/it/shell.rs`.
**Acceptance criteria:** §12.1 #11 as written, incl. PTY-driven `R` via `expectrl` with a stub agent recording argv+cwd; relative `PATH` entry with planted `claude` never used; resume-file symlink / `0644` / hard-link → file bytes and hard-link partner bytes unchanged; `rc` in bash, zsh, fish (R3 tool rule) with branch `$(touch marker-rc)` → no marker; `grep -c eval` over `rollcall init` output for all three shells → 0.
**Expected test outputs:** `cargo test -p rollcall-cli --test it shell:: --locked` → ≥ 14 passed (fish/zsh cases counted only where the tool exists; CI requires them).

## Phase 34 — MCP server
**Description:** §8.3 over hand-written JSON-RPC 2.0 stdio (R5): `initialize`, `notifications/initialized`, `tools/list`, `tools/call` for `where_was_i` and `list_stale`, `ping`; text defaults, `--prompts`, `redact_prompts`, external omission, 16 KiB budget with `truncated`, wait rules (§6.4).
**Files:** `crates/rollcall-cli/src/mcp.rs`, `crates/rollcall-cli/tests/it/mcp.rs`.
**Acceptance criteria:** §12.1 #14 as written (both tools validate against `schema/`, ≤ 16 KiB on `mcp_200` with `truncated: true`, unconfigured `root` → JSON-RPC error, no root → exit 2, empty cache → first call waits and returns repos); without `--prompts` every `last_prompt` is null and no `title_kind: "prompt"` title is returned; tool descriptions contain the §8.3 "untrusted data" sentence.
**Expected test outputs:** `cargo test -p rollcall-cli --test it mcp:: --locked` → ≥ 8 passed.

---

## Wave E — performance, release, demo

## Phase 35 — Performance gate and benches (§12.1 #17)
**Files:** `crates/rollcall-cli/benches/scan.rs` (gated on Q3d), `.github/workflows/ci.yml` (edit: `perf` job on `ubuntu-latest`).
**Acceptance criteria:** perf corpus from the generator (`[perf]`: 50 repos incl. 20 linked worktrees, 5,000 logs, 500 MB); `hyperfine --prepare 'rollcall cache clear' -r 5 --export-json` median < 5 s cold; `hyperfine -w 2 -r 10` `status --json` median < 1 s warm; the job parses the JSON and fails on threshold breach (not just prints).
**Expected test outputs:** CI `perf` job → two medians printed, both under threshold, job green.

## Phase 36 — Release pipeline (§12.1 #16)
**Files:** `dist-workspace.toml` (new, `cargo dist init` output trimmed), `.github/workflows/release.yml` (new, generated then SHA-pinned).
**Acceptance criteria:** §12.1 #16 as written on `macos-14`, `macos-15-intel`, `ubuntu-latest`, `ubuntu-24.04-arm`; `cargo install --path crates/rollcall-cli --locked --root "$d"` on Linux; checksums + attestations + Homebrew tap config present; crates.io trusted publishing configured but not triggered.
**Expected test outputs:** `dist build` job green on all four runners.

## Phase 37 — README and demo (§12.1 #18)
**Files:** `README.md` (new), `demo.tape` (new); mech: CI `demo` job.
**Acceptance criteria:** README has every §11 section incl. the §1 differentiator table verbatim and MCP registration with `--root` + first `rollcall scan`; `vhs demo.tape` in CI renders from the generator's demo root with stub agents and ends on a resumed session (last frame text contains the stub agent's "resumed <id>" line).
**Expected test outputs:** CI `demo` job green, GIF artifact uploaded.

---

## Wave map and dependencies

| Wave | Phases | Parallel? | Depends on |
|---|---|---|---|
| A | 01 → 02 → 03 | serial | — |
| B | lane B1: 04, 05, 06 → 07, 08 → 09 (08 after 06; 09 after 07) · lane B2: 10, 11, 12 | B1 ‖ B2; inside B2 all three parallel (separate files) | A |
| C | C1: 13 → 14 → 15 → 17 → 18 → 19 · C2: 20 → 21 → 22 → 23 · C3: 24 → 25 · C4: 16 | four lanes parallel | C1–C3: A only (may start alongside B). C4: B1. |
| D | 26 → 27 → 28 → {29 → 30 → 31} ‖ 32 ‖ 33 ‖ 34 | after 28, four lanes | C |
| E | 35, 36, 37 | parallel (35 and 37 both append CI jobs → serialise 35 → 37) | D |

Phase numbering is by first draft; P16 sits in lane C4.

## §12.1 criterion → phase coverage

| # | Criterion | Phases (build → gate) |
|---|---|---|
| 1 | Discovery | 06, 08, 14, 15, 19 → **29** |
| 2 | Git state + deadline run | 06, 08, 17, 18, 19 → **29** |
| 3 | Sort and quiet | 08, 10 → **29** |
| 4 | Claude source | 07, 09, 20, 21 → **30** |
| 5 | Codex source (db + fallback) | 07, 09, 22, 23 → **30** |
| 6 | Matching | 07, 09, 11 → **30** |
| 7 | Incremental reads & pointers | 07, 20 → **30** (+ P20 unit cases) |
| 8 | Hostile repos | 06, 13, 17 → **31** |
| 9 | Guards & first run | 06, 14, 15 → **29** |
| 10 | Privacy | 07, 12, 24, 25 → **31** |
| 11 | Sanitising & shell safety | 07, 12, 28 → **33** |
| 12 | Store | 24, 25, 27 → **31** |
| 13 | No-agent mode | 27, 28 → **29** + **32** (TUI part) |
| 14 | Schemas & MCP | 04, 05 → **34** (+ 29–31 validate status/doctor) |
| 15 | TUI | **32** |
| 16 | Build & supply chain | 01, 02 → **36** |
| 17 | Performance | 06 → **35** |
| 18 | Demo | **37** |

§12.2 author smoke runs after P37, recorded in the handoff, not gated.

## Edge cases considered

- **Hostile input:** every file rollcall opens from a repo or log goes through `O_NOFOLLOW|O_NONBLOCK` + fstat (P14, P17, P20); FIFOs are tested at `.git`, `packed-refs`, `include.path`, and a log path.
- **Concurrency:** two rollcall processes on one root (P25 lock, P27 stale path); a live Codex WAL DB (P22); abandoned timed-out threads outliving the scan (P19) — output is produced from completed rows only.
- **Boundaries:** 10,000-commit saturation (P18); 200-grapheme edge (P12); 1 MiB line edge (P20); 16 KiB MCP budget (P34); walk budget (P14); `quiet` day boundary (P10).
- **Missing input:** no agent dirs (P29 #13); DB absent (P23); no default branch, no remote, no upstream (P18); missing worktree dir (P14).
- **Permissions:** unreadable dir (P14), umask 022 (P25), resume file owner/mode/links (P33), other-uid repo (P17, unit-level only — noted).
- **Crash safety:** mid-scan panic leaves prior state (P24).

## Open questions (answer before the gated phases start)

1. **GitHub remote.** §12.1 #16–#18 and the `deny`/`audit` jobs are CI-only; no remote exists and pushing needs your authorization. Options: (a) create a private GitHub repo now and push `impl/v0` after each wave; (b) stay local until Wave E, gating on local equivalents; (c) public repo from the start. Gates: P02's CI criteria, P35–P37.
2. **Local tool installs.** `cargo-deny`, `cargo-audit`, `hyperfine`, `fish`, `cargo-dist` are not installed on this machine. Options: (a) install them locally (Homebrew / `cargo install`, downloads from crates.io and Homebrew); (b) CI-only, and local runs skip those checks with the R3 skip message. Gates: P02, P33 (fish), P35, P36.
3. **Ponytail dependency trims that touch spec wording.** Each is a yes/no; the observable behavior the spec requires is identical either way, so no criterion changes. Until answered, phases use the spec's named crate.
   - (a) Drop `directories`; compute the two cache paths by hand in P25 (same paths as §6.1).
   - (b) Lock filename: `xxh3-128` hex instead of `sha256` (drops `sha2`; `xxhash-rust` is already in the tree).
   - (c) Clipboard: keep `arboard` for `clipboard = "system"`, or make OSC 52 the only mechanism (drops `arboard` and its X11/Wayland dependency tree on Linux; changes §3.2's default and removes one config value — this one **is** a behavior change).
   - (d) `criterion` benches (§10.2): drop them, since the gating perf check (#17) is `hyperfine`; or keep.
