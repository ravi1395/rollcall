# HANDOFF — 2026-10-06 main (~/Projects/rollcall), end of session 13

**Status: ACTIVE.** Supersedes the session 12 handoff (byte copy: `.handoffs/20261006-235942-superseded-HANDOFF.md`, which holds every author decision from v1, sessions 8–12). v1 is finished: spec rev 16 `docs/superpowers/specs/2026-09-29-rollcall-design.md`, criteria `docs/superpowers/plans/tasks/v1/ACCEPTANCE.md`.

**Next session: Copilot integration (author roadmap, 2026-10-06).** Nothing is designed yet. Start with `superpowers:brainstorming`, then a spec revision (rev 17) for author review, then `/phases`. Branch from `origin/main`, not `impl/v0`.

## Done (verified)
- **PR https://github.com/ravi1395/rollcall/pull/6 merged by the author** (`gh pr view 6` → `MERGED`, 2026-10-06T22:54:47Z, merge commit `4f102a7`). `impl/v0` is fully merged. Do not add new work to it.
- **Dependabot PRs #2–#5 merged by the author after #6.** `origin/main` = `474cf23`. Diff `4f102a7..474cf23`: `Cargo.lock` (jsonschema 0.58.2→0.58.5, libc 0.2.189→0.2.190, insta 1.48.0→1.49.0) and `.github/workflows/ci.yml` (taiki-e/install-action 2.87.22→2.87.23).
- **Local gate on `origin/main` (`474cf23`): green**, see Test state.

## In progress
- Nothing. `git status --short` (on `impl/v0`, before this handoff's commit) → `?? .handoffs/20261006-235942-superseded-HANDOFF.md`, `?? graphify-out/` (untracked; never commit `graphify-out/`).

## Decisions + why
- This handoff is committed on `impl/v0` (local only, not pushed), the same way earlier handoffs were. The next session reads the file from the working tree. Copy or carry it into the new branch when you branch from `main`.

## Test state
- `cargo fmt --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked` on a detached worktree at `origin/main` `474cf23` (macOS) → exit 0. The 28 `test result:` lines sum to **973 passed / 0 failed / 4 ignored**, identical to the `1147a53` baseline, so the Dependabot bumps changed nothing. Ignored by design: `materialize_perf`, `materialize_demo`, `fs.rs::modes_child`, `fs.rs::lock_child`.
- CI: never run on `main`. Every job in runs `37543476409` (PR), `37543647682` and `37543667177` (main) is `failure` with 0 steps. Annotation: "The job was not started because recent account payments have failed…". Linux is still untested.

## Gotchas
- **CI is billing-blocked, not red.** Check annotations with `gh api repos/ravi1395/rollcall/check-runs/<job id>/annotations` before debugging. Once billing is fixed, re-run CI on `main`; use `ci-divergence-triage` if it goes red.
- **`ubuntu-latest` becomes Ubuntu 26 from 2026-10-19** (runner annotation). If CI is still blocked by then, the first Linux run tests a new OS too. Option: pin `ubuntu-24.04` (author decision; not asked).
- **R1 and new crates.** Copilot support probably means a new module in `crates/rollcall-sessions/src/` (today: `claude.rs`, `codex_db.rs`, `codex_rollout.rs`, `jsonl.rs`). That edits a `mod` list and maybe a `Cargo.toml`. R1 forbids that after P01/P04 of a plan, so the new plan's first phase must own those edits, with the author's approval.
- **Autonomy from 2026-10-04 covered v1 only.** It does not carry over to Copilot. Spec rev 17 needs author review before any code.
- Carried over: never print prompt text from `~/.claude` / `~/.codex` (and the same goes for Copilot logs); use a scratch `HOME` for manual runs; `git config handoff.auto false`; concurrent sessions may edit branches (stage explicit paths); Python `re.sub` replacement strings turn `\n` into real newlines.

## Suggested skills
- `brief` (vault context), `superpowers:brainstorming` → spec rev 17 → `/phases` → `/executor-grade-plan-audit` → `/pchunker` + `/pexec`.
- `ci-divergence-triage` once CI runs.

## Copilot: what's on this machine (session 13; names and counts only, no contents read)
- **VS Code Copilot Chat:** `~/Library/Application Support/Code/User/workspaceStorage/<hash>/chatSessions/` exists in 5 of 7 workspaces (2+2+2+2+10 = 18 files), plus `chatEditingSessions/` (43). Each `<hash>/workspace.json` should name the workspace folder, which is how a session would map to a repo (§6.5). Formats are unverified.
- **Copilot CLI:** `~/.copilot/` holds only `config.json`, `ide/`, `logs/`, `mcp-config.json`. There is no session store and no `copilot` binary on PATH; VS Code bundles a shim under `globalStorage/github.copilot-chat/copilotCli/`. So there is no local CLI session data to test against.
- `~/.config/github-copilot/apps.json` likely holds auth tokens. Never read or print it.

## Copilot: open questions for brainstorming (none answered)
- Which Copilot: GitHub Copilot CLI sessions, VS Code Copilot Chat / agent mode, Copilot coding agent (cloud PR sessions), or several?
- Where each one stores session data on disk, in what format, and whether it records the cwd/repo (needed for the spec §6.5 session → worktree match).
- Same treatment as Claude/Codex (spec §5.0 common rules, §6.3 incremental reads, prompt redaction), or a reduced one?
- Read-only local files only (as today), or does it need the network/GitHub API? The latter changes the §4.4 security posture.

## Not started
- Copilot integration (above).
- Re-run CI on `main` after billing is fixed (author action first).
- Release: create `ravi1395/homebrew-rollcall`, add the `HOMEBREW_TAP_TOKEN` secret, tag (author actions).
- Optional carry-overs: the `lock_when_free` fork-race window; the full-F MCP reply has 0 `recent_sessions`; `scripts/probe-gix-p15b/sample.py` prints `first>320MiB`; a non-UTF-8 missing-worktree lock key collides with its twin; on macOS the cache ignores `XDG_CACHE_HOME`.
