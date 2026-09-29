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
