# ORIENTATION — rollcall v0 run (2026-09-30)

**Verdict:** plan rev 5 assumptions sound against the current tree (docs only, no code) and re-probed external facts; no drift. One setup gap fixed in the plan (`9d39000`: `impl/v0` lives in the main checkout).

## Confirmed this session

- Tree: tracked files are docs + `scripts/probe-plan-facts.sh` only; `cargo test` → `could not find Cargo.toml` (no code).
- `grep -c '^## Phase' plan` → 45.
- `scripts/probe-plan-facts.sh`: gix 0.88.0, rusqlite 0.40.2, ratatui 0.30.2, crossterm 0.29.0, jiff 0.2.37, clap 4.6.7 (latest on crates.io); gitoxide checkout at `5708de4` (0.86.0) for audit cross-reference.
- Toolchain: rustc/cargo 1.97.1 (Homebrew). Tools: hyperfine 1.20.0, fish 4.9.3, cargo-deny 0.20.2, cargo-audit 0.22.2, `dist` 0.33.0 (binary name `dist`), vhs, jq, zsh, python3 3.14.6 (`tomllib` ok). No GNU `time` (macOS `/usr/bin/time -l` is the local RSS path).
- `origin` = private `github.com/ravi1395/rollcall`.

## Run mechanics (author's model roles, from the consumed handoff `.handoffs/20260930-000405-HANDOFF.md`)

| Role | Model | Binding |
|---|---|---|
| Orchestrator | Opus 5.5 | main session: dispatch, integrate, full gate, commits, pushes |
| Implementers | Sonnet 5.5 | `Agent` `plan-surgeon-surgeon`, explicit `model: "sonnet"`; parallel lanes add `isolation: "worktree"`; serial phases run in the main checkout |
| Breaker | Opus 5.5 | `breaker`, explicit `model: "opus"` |
| Final gate | Opus 5.5 | `skeptical-reviewer`, explicit `model: "opus"` |

- `/pexec` is not used unmodified (hard-codes fable + shared-tree psurgeon).
- Workers never commit. The orchestrator commits `P<nn>: <title>` per phase (in the lane worktree, then merges into `impl/v0`), runs the R2 full gate after each merge, and pushes `impl/v0` after each lane merge (Q1).
- Per-worktree `CARGO_TARGET_DIR` seeded by `cp -cR <main>/target` (APFS clone). A shared target dir was tried first and failed: workspace-member test binaries collide across worktrees (P12 ran a foreign binary: `0 passed; 72 filtered out`).
- Terminal condition: the Opus skeptic reports every spec §12.1 criterion 1–18 MET. #16–#18 need CI runs on `origin` (evidence via `gh run`).

## Gotchas

- A session hook rewrites `HANDOFF.md` (on push and other events). `git update-index --skip-worktree HANDOFF.md` is set for the run — unset it (`--no-skip-worktree`) before writing the run-end handoff. Never `git commit -a`; stage explicit paths.
- `block-dangerous-git.sh` blocks `reset --hard`, `branch -D`, `clean -f`, `checkout .`, `restore .`, force pushes. Use `git branch -d` after merges, `git worktree remove` for clean worktrees.
- Agent worktree isolation does NOT reliably fork from `impl/v0`: P14's worktree came from `19bfdee` (old `main`). After every isolated dispatch check `git worktree list`; fast-forward a stale one (`git -C <wt> restore HANDOFF.md && git -C <wt> merge --ff-only impl/v0`) and message the agent. Worktrees live under `.claude/worktrees/` (locally excluded via `.git/info/exclude`).
- claimd Stop hook blocked every turn end; author chose (2026-09-30) to ignore it via a local, uncommitted `.claims/config.json` with `lint.ignore = ["**"]` (excluded in `.git/info/exclude`). A committed ignore would trip claimd's monotone-tighten check.
- Always `cd /Users/ravichandrasekhar/Projects/rollcall &&` — a `cd` into a worktree moves the session's cwd.
- Edition 2024 `set_var` is `unsafe`; `unsafe_code` is forbidden (R3/R6).
- Never print prompt text from real logs.

## Not verified this session

- That `model: "sonnet"` / `"opus"` resolve to Sonnet 5.5 / Opus 5.5 — first receipt must state its model id.
- GitHub runner labels `macos-14`, `macos-15-intel`, `ubuntu-24.04-arm` — checked at P02.
- Spec revisions 6–10 have no author review (spec header); the `/goal` run proceeds on them — record in the run-end handoff.
