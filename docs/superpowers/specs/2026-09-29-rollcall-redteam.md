# rollcall spec — red-team round 1

**Date:** 2026-09-29
**Target:** `2026-09-29-rollcall-design.md` @ `59d22bc`
**Reviewers:** four independent Fable 5.1 agents, read-only, one lens each.

| Lens | Verdict |
|---|---|
| Technical feasibility (tested against real data) | Stands with fixes |
| Product / differentiation (GitHub + docs prior-art search) | Stands with fixes |
| Consistency / testability / scope | **Does not stand** (for executor handoff) |
| Privacy / security / hostile environments | Stands with fixes |

**Overall:** the idea and architecture hold; the core is feasible on real data. The spec as written does not survive handoff to implementers: record formats, JSON schema, privacy defaults, and gate-checkable acceptance criteria are missing, and v0 scope is ~3–5 weeks, not a weekend.

## Real-data measurements (feasibility + security reviewers)

- Real tree: **17 repos + 29 linked worktrees** (spec said 15 + 14; `.claude/worktrees/` and `.worktrees/` were missed). 6 worktrees are prunable (dirs deleted). External worktrees exist under `/private/tmp` and `~/.codex/worktrees`.
- Logs: 1,979 Claude + 1,403 Codex top-level files, **1.35 GB** (1.54 GB incl. 469 subagent logs); largest file 45 MB; max line 5.2 MB; 33 lines > 1 MiB; 0 malformed lines.
- Claude: `cwd`+`gitBranch` on 79 % of records; one `sessionId` per file (1,978/1,979); 16 files have > 1 `cwd`; `gitBranch` is literal `"HEAD"` when detached (166 files).
- Codex: `session_meta.cwd` in all files; `git.branch` missing in 23 % of metas and is a start-of-session snapshot; 6 files have a second `session_meta`; files move to `~/.codex/archived_sessions/`.
- Codex user text: most `response_item` role=user messages are injected context (`<environment_context>`, AGENTS.md, …); clean `event_msg user_message` only 163 times in 1,403 files.
- Longest-prefix cwd→worktree match: Claude 99.6 %, Codex 95.1 % (misses: non-git folders, deleted/renamed repos).
- `origin/HEAD` unset in 7 of 14 repos → "merged into default" undefined there; squash merges never detected by ancestry.
- Resume: `claude --resume <id>` and `codex resume [SESSION_ID]` confirmed via `--help`.
- gix 0.88: `worktrees()`, `Proxy::is_prunable`, `is_bare`, `submodules`, `status()` with untracked modes exist. No ahead/behind API (build via `rev_walk().with_hidden()`); `is_dirty` ignores untracked files.
- MCP spawn cwd: Claude Desktop spawns stdio servers in its scratch dir; Codex desktop app-server cwd is `/`.
- Sensitive data in logs (file counts, both sources): `sk-`-style keys 70, AKIA 15, `AIza` 15, password= 24, postgres creds 9, mongodb creds 6. In Claude, most secret hits in `user` records are `tool_result` payloads, not human text. ESC (`\u001b`) present in 1,834 Claude and 78 Codex files. 11 Claude `cwd` values contain shell metacharacters.
- Cache perms: umask 022 → a new SQLite file would be 0644; Claude logs are 0600.

## Findings, consolidated

### Blockers — must be in the spec before planning

1. **Session record contracts missing.** Per source: log roots (incl. `CLAUDE_CONFIG_DIR`/`CODEX_HOME`, subagent logs, `archived_sessions`), session id location, multi-`cwd` sessions, branch rules (`HEAD` = detached; Codex fallback), title / human-prompt extraction filters (exclude `tool_result`, meta, slash-command expansions, injected context), `ended_at`, message count, multi-meta files. *(feasibility 2, 3, 7, 9; testability 1; security 1)*
2. **JSON schema undefined.** Need one schema: `{schema_version, root, scanned_at, repos[]}`, field types + nullability, `unpushed`, `message_count`, `stale_reason` enum, per-row `error`. *(testability 3)*
3. **Acceptance criteria not gate-checkable.** Five of seven depend on the author's machine, an LLM's judgement, or a published Homebrew release; ≥95 % has a vacuous/wrong denominator. Rewrite as fixture-based, script-checkable criteria; move real-data checks to a non-gating author smoke list. *(testability 2, 8; feasibility 6)*
4. **Privacy defaults unsafe.** "Last user prompt" as written is mostly tool output, which carries most secrets. Prompts go to a networked model via MCP by default. Cache would be a 0644 plaintext copy of 0600 data. *(security 1–3)*

### Major

5. **Untrusted repos:** no `git` subprocess anywhere; open gix isolated (no filter drivers / fsmonitor / global config); bound `.git` pointer reads (lstat, 4 KiB, dir-only target). *(security 4, 5)*
6. **Output injection:** strip C0/C1/ESC from all rendered text; shell-quote paths; UUID-validate session ids; exec `$EDITOR` without a shell; define `R` mechanism (`cd '<path>' && <resume>` on stdout + opt-in shell function). *(security 6, 7; testability 7)*
7. **Walk guards:** stay on one filesystem; skip `Library`, `.Trash`, cloud-storage dirs, `/proc`-class; refuse `/` and bare `~` without `--yes`; entry/time budget reported by `doctor`. *(security 8; feasibility 10)*
8. **MCP root:** server cwd is a scratch dir or `/` in practice → `--root` required; tools take a `root` param; MCP serves stale cache under a time budget rather than blocking. *(feasibility 1; security 5)*
9. **Undefined terms:** attention ordering + tie-breaks; stale vs quiet contradiction (§3 N=14 vs mockup `quiet 21d`); default branch (origin/HEAD → `init.defaultBranch` → `main`/`master` → `no_default_branch`); merged (ancestry; squash undetected in v0); unpushed; dirty (tracked vs untracked counts); new `missing` worktree state; two distinct "external worktree" cases; exit codes; non-TTY behaviour; `NO_COLOR`; macOS cache dir. *(testability 4–6, 9, 10; feasibility 4, 5, 10)*
10. **Concurrency & bounds:** WAL + `busy_timeout` + scan lockfile; streaming reader with 1 MiB line cap; change detection keyed by (dev, inode) + mtime; overlapping roots share one session table. *(feasibility 9, 11; security 9)*
11. **Differentiator claim is stale.** Claude Code's `/resume` picker already lists sessions across all projects with branch; `codex resume --all` exists. What remains unique: **Claude + Codex history joined to per-worktree git attention state, one screen, one-key resume.** *(product 1)*
12. **Pitch:** the generic git dashboard is a crowded, ~0-star category (gita 1.9k and worktrunk 8.4k own the maintained end). Keep generic discovery and no-agent mode, but lead README/GIF with agent users. *(product 2, 5, 7)*
13. **Scope:** ~20 subsystems ≈ 3–5 weeks solo. 5-crate workspace is premature for v0. *(testability scope; product scope)*

### Minor

- MCP `list_stale` adds little over `git branch --merged`. *(product 4)*
- Performance targets should state byte volume and whether "warm" includes git status. *(feasibility 8)*
- Supply chain: GitHub attestations, `cargo-deny` + `cargo-audit`, `--locked`, actions pinned by SHA. *(security 10)*
- Name `rollcall`: npm/PyPI free; only code collision `withtally/rollcall` (77★, unrelated). Keep. *(product 6)*

## Prior art (product reviewer, stars as of 2026-09-29)

| Project | Stars | Overlap | Gap vs rollcall |
|---|---|---|---|
| Claude Code `/resume` (built-in) | — | cross-project sessions, branch filter, resume | Claude only, no git state |
| `codex resume --all` (built-in) | — | cross-dir picker | Codex only, no git state |
| worktrunk | 8,448 | per-worktree dirty/↑/CI, agent launch | single repo, no history |
| gita | 1,943 | recursive discovery, ahead/behind/dirty | no worktrees/sessions/TUI |
| ccmanager | 1,253 | multi-project, per-worktree ↑↓, 8 agents | live processes only, no history |
| agent-deck | 970 | multi-repo TUI of live sessions + worktrees | no history import, no dirty/unpushed |
| ccresume | 176 | browse/resume Claude sessions | no Codex, no git |

## Reviewer-proposed release cut

- **v0:** single crate; normal + linked + nested discovery with guards; git status (dirty, untracked, ahead/behind, upstream, unpushed, missing, inactive-stale); Claude + Codex parsers behind `SessionSource`; SQLite cache; `status [--json]`, `scan`, `doctor`; TUI (j/k, enter, r, q, R); `NO_COLOR`; CI fmt/clippy/test; `cargo install`.
- **v0.1:** MCP (`where_was_i`, `--root` required, prompts off by default), merged-stale, bare/external repos, `--follow-symlinks`, filter, `c`/`o`, config file, perf gate.
- **v0.2:** cargo-dist + Homebrew, attestations, criterion bench, vhs GIF, crates.io.

Suggested pitch: *"rollcall: every repo, every worktree, and the Claude Code or Codex session that left it dirty — one screen, one key to resume."*

---

# Round 2

**Target:** revised spec @ `0e8cacc`. Same four lenses, Fable 5.1, read-only.

| Lens | Verdict |
|---|---|
| Technical feasibility | Stands with fixes |
| Product | Stands with fixes |
| Testability / executor-readiness | **Does not stand** |
| Security / privacy | **Does not stand** |

Round-1 items mostly FIXED; remaining PARTIALs folded into the list below.

## Blockers

1. **§4.4 trust model is not expressible in gix** (feasibility + security, verified in local gitoxide source). Config trust is per file source, not per key; `include.path` is read before filtering (`/dev/tty` hangs); `status` builds a filter pipeline that spawns drivers from resolved config, including the user's global `filter.lfs.*` when a hostile `.gitattributes` names it; `safe.directory=*` promotes reduced trust to full. Fix: open with env/git-binary/includes off, strip every `filter.*`/`diff.*`/`merge.*` section from the in-memory config, set an allocation limit, pin `gix >= 0.88`; hostile fixture needs a positive control.
2. **`rc` evals stdout** (security). Branch names may contain `$(…)`; `rc status` etc. would execute them. Fix: no `eval`; resume data passed through a file as NUL-separated fields, validated, and run from variables.
3. **`doctor --json` and MCP responses have no schema** (testability).
4. **Test oracles unbuildable/circular** (testability): expected JSON embeds tempdir paths; nothing says who writes oracles. Fix: hand-written fixture manifest + path placeholders.
5. **Incremental reads vs derived fields; pointer resolution unowned** (testability): tail reads cannot recompute title/started_at; no crate reads text back. Fix: explicit `SessionSource` merge + `resolve` interfaces.

## Major

- Git-derived paths (`worktrees/*/gitdir`, `core.worktree`) escape the walk guards; no per-repo time budget (security).
- macOS: `/`, `~`, iCloud share one `st_dev` → single-filesystem guard is a no-op there; refused-root check must use `(dev, inode)` (security, measured).
- Pointer resolution TOCTOU and unbounded reads (`/dev/zero`); `rollout_path` must be under `CODEX_HOME` (security).
- §6.3 misreads on inode reuse and truncate-then-regrow → fingerprint (feasibility).
- Codex: 137 `guardian` subagent threads outside spawn edges; 549 threads lose branch available in rollout `session_meta.git.branch` (feasibility, measured).
- Vanished logs/threads leave dangling sessions (feasibility).
- Warm perf criterion measured the cache; `hyperfine -w 2` makes cold not cold (testability + feasibility).
- Wall-clock leaks into snapshots → `ROLLCALL_NOW` (testability).
- Vacuous/contradictory criteria (#8 no positive control, #10, #13 exit code, #16 assertion) (testability).
- Truncation contradiction; Claude `lastPrompt` already ellipsised (testability + feasibility).
- TUI doesn't render the session↔worktree join inline — the GIF frame (product).
- MCP with prompts off is hollow; first MCP call on empty cache returns nothing (product).
- First-run dead-ends: `$HOME` refused with no guidance, root inside a repo finds nothing, blank screen during cold scan (product).
- `R` without `rc` is a confusing exit → exec the agent directly by default (product).

## Minor

Scrub gaps (`github_pat_`, `npm_`, JWT, Stripe; emails optional); bidi/zero-width chars; per-block prompt filtering and cut at injected markers; SQLite `mode=ro` still takes shared locks — never `immutable`; hidden-submodule matching; `ended_at` choice; MCP root exact match and external-worktree leakage; supply-chain additions (`default-features=false`, `cargo deny bans` on network crates, trusted publishing, `unsafe_code = "forbid"`); prior-art rows (xlaude 184★, agent-sessions 881★, nicknisi/sessions 36★); scope estimate revised to 5–7 weeks.

## Correction to orchestrator note

The claim that the newest Codex rollouts lack `git` and `user_message` came from a size/mtime-ordered sample; ordered by `created_at`, 50/50 newest threads yield a prompt and 22/50 have `session_meta.git`.

---

# Round 3

**Target:** revision 3 @ `6efcfd8`. Two lenses (the two that failed round 2), Fable 5.1, read-only.

| Lens | Verdict |
|---|---|
| Testability / executor-readiness | Stands with fixes |
| Security / privacy | **Does not stand** |

All round-2 blockers fixed at the top level (`rc` eval, trust model, schemas, oracles, interfaces).

## Security (verified against gitoxide `5708de4`)

- **Blocker:** gix `status()` defaults to opening submodules itself with their own unstripped config → hostile submodule `filter=lfs` spawns `git-lfs`.
- **Blocker:** unbounded reads of git-dir files (`config`, loose refs, `packed-refs`, `info/exclude`, `alternates`); symlinks to `/dev/zero` or FIFOs are committable inside a bare fixture → OOM or hang, bypassing alloc limit and interrupt flag.
- Major: `core.worktree`, `commondir`, `$HOME/.git` escape the worktree guard; interrupt flag cannot fire in a blocked syscall and rev-walk has none; `R`/`rc` resolve the agent after `chdir` (relative `PATH` entry → planted binary); 20 Codex titles are truncated prompts classed `Thread`; hostile test partly vacuous (`/dev/tty` gives ENXIO in CI; no submodule/escape/symlink cases).
- Minor: refused-root peek ignores skips/TCC; `ROLLCALL_RESUME_FILE` open flags; fish untested; more invisible Unicode characters.

## Testability

- Majors: store query cannot return enclosing/`external_main`/outside-root rows → membership table; no `Index` method for per-worktree sessions; shell-safety test vacuous; positive control satisfiable by the fake `git`; timed-out repo unsatisfiable in one run; error strings/row shape circular with hand-written oracles; exit-1 rule and `F_clean` membership undefined.
- Minor: `scanned_at` null on empty cache; `title_kind` null rule; matching fixture counts; `/` suggestions environment-dependent; inode reuse untestable on real FS; PTY harness for `R`; zsh/fish on CI; `list_stale` truncation; `absent` meaning; cargo install location; native runners for `dist build`.

All addressed in revision 4.

---

# Round 4

**Target:** revision 4 @ `1cab7cc`. Two lenses, Fable 5.1, read-only. Both first attempts stalled (10-min watchdog, no output); both were retried. The testability retry stalled again; the orchestrator did that check itself.

| Lens | Verdict |
|---|---|
| Security / privacy (Fable, verified against gitoxide `5708de4`) | Stands with fixes |
| Testability (orchestrator self-check, **not** an independent Fable verdict) | Stands after two clarifications (`5e2b428`) |

## Security

- All round-3 findings FIXED except unbounded git-dir reads (PARTIAL).
- §4.4 verified implementable in gix 0.86: `Config::isolated()` still reads repo-local config; `commit()` replaces the resolved config the filter pipeline reads; `gitoxide.objects.allocLimit` exists; `Submodule::Given` returns before opening.
- Majors (same class — uncapped reads that abort the process via allocation failure, which thread abandonment cannot contain): `shallow`; commit-graph chain; alternates chain hops; worktree `.gitmodules` (read unconditionally by the builtin submodule status) and `.gitignore`/`.gitattributes`.
- Fixes applied verbatim in revision 5: pre-open list extended, alternates chain walked by pre-open (≤ 8 hops), `core.commitGraph=false`, custom `SubmoduleStatus` returning `Ok(None)`, worktree-root file checks, documented limitation for subdirectory ignore/attribute files in unpacked trees, and a maintenance rule to re-audit gix read sites on version bumps.

## Testability (self-check)

- Round-3 items resolved. Two outcome-changing gaps fixed: `repo_budget_secs` is a float (the deadline test uses 0.001); repo-level failures of any kind yield `worktrees: []`.

## Residual risk

The pre-open list is a denylist against gix internals; each security round found the next uncapped read site. It is now documented as a maintenance rule, but a future gix change can reopen it.

# Plan-time corrections (revision 6)

**Date:** 2026-09-29, found while writing the implementation plan.

- §12.1 #16 named runner `macos-13`, which GitHub has retired. Current Intel macOS labels are `macos-15-intel` and `macos-26-intel` (GitHub-hosted runner reference, checked 2026-09-29). Spec now says `macos-15-intel`.
- §8.1 opened `ROLLCALL_RESUME_FILE` with `O_TRUNC` before the ownership/link/mode checks, so a hard-linked or `0644` target was truncated even when the checks then refused it. Now: open without `O_TRUNC`, verify on the fd, then truncate through the fd. §10.2 test now asserts the bytes are unchanged, not just "nothing written".

# Plan red-team round 1 → spec revision 7

**Date:** 2026-09-29. **Target:** plan `2026-09-29-rollcall-v0-plan.md` @ `eeb8bc9`. **Reviewers:** five independent Opus 5.5 agents (`skeptical-reviewer`, `model: opus`), read-only, one lens each. Full findings and plan dispositions: `docs/superpowers/plans/2026-09-29-rollcall-v0-plan-redteam.md`.

Spec-level defects, each re-verified by the orchestrator before editing (commands in the plan red-team doc):

- **§11 `gix-transport` ban unsatisfiable.** gix 0.88 depends non-optionally on `gix-protocol` → `gix-transport` (`cargo tree -i gix-transport` on gix with only `sha1`). Only its `default` (empty) feature is enabled. Ban now targets the client features, not the crate.
- **§4.4 missed read site: `config.worktree`.** gix 0.88.0 `config/cache/incubate.rs` loads `config.worktree` with `File::open` + `io::copy` (no cap, follows symlinks, blocks on a FIFO) when `extensions.worktreeConfig` is set. Added to the pre-open list and hostile fixtures; maintenance rule widened to `io::copy` and process-spawn sites.
- **§3.3 `dirty_count` needs HEAD↔index status.** `index_worktree_status` alone misses staged changes; `tree_index_status` exists at 0.88. Union-by-path and rename-counts-once now stated.
- **§8.1 resume file blocks on a FIFO.** Open without `O_NONBLOCK` blocks forever on a reader-less FIFO. Added `O_NONBLOCK`.
- **§7 no field for `doctor` notes.** §5.2 ("report why in `doctor`") and §4.1 ("`doctor` notes the layout") had no `DoctorDoc` field. Added `notes: string[]`.
- **§3.4/§6.1 matches not keyed by root.** A stale answer for root A could show root B's matching. `sessions_for` now takes the root; matches stored per root.
- **§10.2 non-discriminating tests.** A single self-loop symlink ends by ELOOP with or without a visited set → fan-out fixture. Positive controls ran on the tree under test and only exercised real git → separate `H` builds plus a gix-default control. Size-changing edits skip clean filters → same-size edits; `diff=x` attribute added so `diff.x.command` can fire. `big-object` must be on a read path → HEAD root tree. 1 ms deadline could let trivial repos finish → 1 µs budget, deadline covers open, late results discarded.
- **§3.3 `scanned_at`** now defined as "now" at scan start (idempotence and TUI snapshots depend on it).

# Plan red-team round 2 → spec revision 8

**Date:** 2026-09-29. Three Opus reviewers on plan revision 2 / spec revision 7 @ `031051e`. Record: `docs/superpowers/plans/2026-09-29-rollcall-v0-plan-redteam.md` (round 2 section).

- **Revision 7 regression: HEAD tree expansion unbounded.** Counting staged changes via `tree_index_status` expands the whole HEAD tree (`index_from_tree`) with no interrupt parameter (orchestrator read gix 0.88.0 `status/tree_index.rs`). Reviewer-reported measurement: 3 nested 200-entry trees → 8,000,000 paths, ~3 GB peak RSS. §4.4 now bounds the HEAD tree walk (1,000,000 entries, deadline-checked) before calling it; `tree-bomb` hostile fixture added; the "interrupt stops status" sentence corrected.
- **Rename tracking scope.** Index↔worktree rename tracking pairs deletions with untracked files and changes `untracked_count` (reviewer-measured). Now HEAD↔index only, keyed by destination path.
- **Deadline rules** moved from a §10.2 parenthetical into normative §4.4.
- **`SourceScan.notes`** added so session sources can feed `DoctorDoc.notes`; `WorktreeKey.missing` recorded in §3.4.
- **Matches keyed by root and `--submodules`**; a cache answer with a different flag is stale.
- **`doctor` notes sanitised** in plain/TUI output.
- **§11 feature ban** names `blocking-client`, `async-client` (the `http-client*` features imply `blocking-client`).
- **`repo_budget_secs`** must be finite and representable (NaN passed `≤ 0`; `Duration::from_secs_f64` panics on NaN/inf/overflow).
- Wording: positive control (2) is default open + `index_worktree_status`; `big-object` is read by the HEAD tree walk, not rev-walk.

# Plan red-team round 3 → spec revision 9

**Date:** 2026-09-29. Two Opus reviewers on the rev 8 / plan rev 3 diffs @ `cf54ac5`.

- **Rev 8's HEAD tree bound counted entries, not bytes.** `index_from_tree` stores each entry's full path (gix-index-0.56.0 `init.rs` `push_entry`: `path_backing.extend_from_slice`, orchestrator-read). Reviewer-measured: 999,999 entries with 500-byte names from 24 KiB of objects → ~1 GB of path bytes, 564 MB peak RSS (macOS). Bound now also caps cumulative path bytes (128 MiB) and depth (4,096), requires an iterative walk, and a `tree-bomb-long` fixture joins §10.2.
- **Rename similarity is not bounded by the tree walk.** Now stated: `Rewrites { copies: None, percentage: Some(0.5), limit: 1000 }` (gix-diff-0.68.0 defaults, orchestrator-read) plus the allocation limit.
- **`--submodules` flag had no carrier.** `Index::scanned_submodules` added to §3.4.
- Conflicted and intent-to-add paths are counted from the index↔worktree side only (reviewer-read from gix-diff / gix-status sources).

# Revision 10 — author decisions on plan Q3 (2026-09-29)

The author chose three ponytail dependency trims: the cache path is computed directly (no `directories` crate, same paths); the lock file name uses SHA-1 from `gix-hash` instead of SHA-256 (no `sha2`); `criterion` micro-benchmarks dropped (the gating performance check is `hyperfine`). The clipboard keeps `arboard` for `clipboard = "system"`.

# P14 gix read-site audit → spec revision 11

**Date:** 2026-09-30. Implementation run, phase P14 (`docs/audits/gix-0.88-read-sites.md`), Sonnet auditor; orchestrator (Opus) re-read the gix 0.88.0 / gix-ref 0.68.0 sources for each site before revising. All changes tighten §4.4; none weakens it. Made without author review under the run's `/goal`.

- **U-1 `commondir` redirect.** `gix/src/open/repository.rs:open_from_paths` follows `<git_dir>/commondir` before any trust check, then reads `config`, `packed-refs`, `refs/**`, `info/*` from the target unbounded. Trust rule (b) is now evaluated before open from rollcall's own read of the (capped) `commondir` file; the pre-open list also runs on linked admin dirs and the resolved common dir. The `commondir-escape` fixture keeps its `untrusted worktree path` error.
- **U-2 pseudo-refs escape `refs/`.** `gix-ref` `name.rs:is_pseudo_ref` accepts any all-caps name, so `HEAD: ref: EVIL` reads `<git_dir>/EVIL` uncapped. All-caps entries in the git dir, common dir and `worktrees/<name>/` (and `worktrees/<name>/refs/**`) are now pre-open checked as loose refs; `symlinked-files` gains a `HEAD → ref: EVIL` variant.
- **U-3 repo-local `core.excludesFile`.** `gix/src/config/cache/access.rs:assemble_exclude_globals` reads a repo-local `core.excludesFile` (or the XDG default) whenever no override is given. The key is now stripped and always overridden (user file, else `/dev/null`); new `excludes-file` hostile fixture.
- **Rev 11 amendment (P18):** a 4 KiB cap on every all-caps entry would mark ordinary repos untrusted (`FETCH_HEAD`, `COMMIT_EDITMSG`, `MERGE_MSG` exceed it). `HEAD` keeps 4 KiB; other all-caps entries must be regular files ≤ 1 MiB — still bounded, still no symlink/FIFO.

# P15a gix read-site audit → spec revision 12

**Date:** 2026-09-30. Implementation run, phase P15a (`docs/audits/gix-0.88-read-sites.md`, U-4 .. U-9). The author chose in-process guards over process isolation (keeps §4.4 "No subprocesses"). The per-finding options are the orchestrator's picks (Opus), each re-checked against the locked gix 0.88 / gix-dir 0.30 / gix-index 0.56 / gix-status 0.35 / gix-diff 0.68 sources before revising. All changes tighten §4.4; none weakens it.

- **U-4 index extensions.** `gix-index` decodes `TREE` / `UNTR` recursively without a depth bound (stack-overflow abort, reproduced) and `REUC` without an allocation limit; `gix_index::decode` exposes no way to skip extensions and entry decoding is private. Rollcall now parses each index itself in the pre-open pass (streamed; v2/v3/v4 entry walk, `EOIE` not trusted), refusing nesting > 1,024 and `REUC` > 1 MiB; repo workers get a 16 MiB stack so an accepted depth decodes. Fixtures `index-bombs`, positive control `index-deep-ok`.
- **U-5 nested-repo probe.** `gix-dir` `classify.rs:maybe_upgrade_to_repository` returns before `gix_discover::is_git` when `recurse_repositories` is true; `.git` entries classify `Pruned` and are never entered. Side effect found by reading the source (not yet run; P20's submodule test confirms it): gitlink directories then classify as tracked `Directory` and are entered, so the delegate drops entries under gitlinks and collapses directories holding a `.git` entry (`lstat` only), keeping `git status` parity in §3.3. Fixture `nested-probe`.
- **U-6 tracked worktree reads.** (a) 100755 entries over non-regular files: pre-status `lstat` pass, worktree error. (b) filtered `read_to_end`: rollcall `CompareBlobs` reports "modified" above 64 MiB without reading (the trait's `Ok(Some(_))` needs no `ReadData` call); `thread_limit: Some(1)` (C-4). Fixtures `exec-type`, `big-tracked`.
- **U-7 pattern-file blobs.** Pre-status `try_header` on every index `.gitattributes` / `.gitignore`: ≤ 256 KiB each, ≤ 1 MiB total. N7 folded in: pre-open pattern-file caps lowered 1 MiB → 256 KiB so the per-repo pattern budget (≈ 160 MiB) fits §10.2's 512 MB. Fixture `pattern-blob-bomb`.
- **U-8 split index.** `sharedindex.*` added to the pre-open list with the `index` rule and validated like the index; chained `link` refused. Fixtures in `index-bombs`.
- **U-9 rename similarity.** `core.bigFileThreshold=1m` override. Not bypassable: in `gix-diff` `blob/pipeline.rs`, `is_binary = Some(false)` is only reachable through a `diff.<name>` driver, and every `diff` section is stripped; binary pairs never match (`tracker.rs` `SourceOrDestinationIsBinary`), so the fixture's staged rename-with-edit counts 2. Fixture `rename-bomb`.
- **N1, N6 wording.** `open::Options::isolated()`; the subdirectory pattern-file limitation covers tracked files too.
- **Skeptic round 1 (Fable) → revision 12 fixes.** `TREE` / `UNTR` size caps (16 MiB / 4 MiB; gix's allocation limit is per node, `scripts/probe-index-bomb` reproduces 16 MiB `TREE` → 178 MiB peak); `index.threads=1` (gix-index `decode/mod.rs:from_bytes` reads the extension table from `EOIE` on its own thread when more than one thread is configured); pattern-blob count limited to what gix reads (`gix-worktree` `stack/state/mod.rs:id_mappings_from_index`); zero-length names legal with `link`; §12.1 #8 covers the no-error and count assertions; `big-tracked` 1 GiB so the RSS bound falsifies it; limitation covers nested repos and submodule worktrees.
- **Skeptic round 2 (Fable): GATE PASS**, all six criteria met. The one record fix (stale U-4 audit note) is folded in, along with: `UNTR` fixture (viii) spread over blocks, a gix-bump probe re-run rule, and a P20 fixture-size note. Residual risks are in `RISKS.md`.

# P15b gix read-site audit → spec revision 13

**Date:** 2026-09-30. Implementation run, phase P15b (`docs/audits/gix-0.88-read-sites.md`, U-10 .. U-15; five reproduced in a scratch crate against gix 0.88.0 with rollcall's open options, U-15 traced). The author chose a **killable per-repo worker process** for U-13 (over a rollcall pack validator, a patched gix-pack, or a documented limitation). This partly reverses revision 12's "no subprocesses": the rule now covers gix only; the scan itself starts rollcall workers and a fixed `/bin/ps` probe. Per-finding picks for the rest are the orchestrator's, in the revision 12 in-process style:

- **U-10 / U-11 / U-12** (`gitoxide.core.shallowFile`, `gitoxide.core.indexFile`, `gitoxide.objects.replaceRefBase` read repo-local): three Api-source overrides at open pointing each read back at a file the pre-open checks already cover. Section removal was rejected: U-11 and U-12 are consumed during open, before `config_snapshot_mut` exists.
- **U-13** (pack delta chains: no link bound, no cycle check): worker process with (a) a time kill at budget + 1 s, (b) a `/bin/ps` RSS monitor every 10 ms with a 320 MiB kill, (c) `worker failed` for a spawn failure or a malformed result, (d) probe failure kills all live children, with a note. Memory without `setrlimit`: `unsafe_code` is forbidden and macOS does not enforce `RLIMIT_AS`, so the parent polls. One `ps` covers both OSes: `rss` is KiB (measured on macOS: 219,264 KiB for a 200 MiB allocation; procps documents KiB). Cap arithmetic (revised after the design review): the 200 MiB/s first measured came from a debug build; the release build grows at about 3.4 GiB/s (`REF_DELTA` cycle); per 10 ms-sleep tick the review measured at most 42 MiB and the orchestrator's re-run (`sample.py headid refdelta 1.5 s @ 10 ms`, three runs) 69 / 55 / 65 MiB, so the cap was lowered from 384 to 320 MiB: 320 + 70 ≈ 390 MiB < 512 MB (≈ 488 MiB); one `ps` run measured about 2 ms; `/usr/bin/time` rusage folds waited-for descendants' `ru_maxrss` in as a max, so §10.2's gate sees the child (measured on macOS: `/usr/bin/time -l sh -c 'python3 … 300 MiB …; true'` reports 329,449,472 bytes; GNU `time -v` on Linux not measured locally). Growth rate re-measured with `scripts/probe-gix-p15b` (README). Process exit also ends abandoned worker threads (revision 12's residual) and turns a stack-overflow abort into one `worker failed` row.
- **U-14** (depth-first tree walks retain one decoded tree per level): a fourth HEAD tree bound counter, tree bytes visited ≤ 128 MiB, sizes via `find_header` before loading. It bounds gix-index's own `from_tree` too, since that walk visits the same trees; switching rollcall's walk to breadth-first would not.
- **U-15** (`objects/pack` listed unbounded at open and on every miss): `objects/pack` as a counted pre-open directory (10,000, regular files; alternates too) plus `refresh_never()`.
- Rejected for U-13: a pre-open pack validator is O(pack size) per scan (threatens §12.1 #17 unless cached); a gix-pack patch needs an R1 manifest edit and a maintained fork.
- **H timing:** only `pack-bombs` (ii) is expected to run to the 5 s deadline, so `rollcall scan H` should stay under 10 s without pinning `repo_budget_secs`; unmeasured until P35 runs on CI.
