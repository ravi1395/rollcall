# Worker rules — every rollcall v0 phase

You implement exactly one plan phase. Sources of truth, in order: the spec
`docs/superpowers/specs/2026-09-29-rollcall-design.md` (rev 12, the contract), the plan
`docs/superpowers/plans/2026-09-29-rollcall-v0-plan.md` (rev 6) — read its § Execution rules
R1–R6, § Interfaces, § Fixture manifest schema, and your phase section in full, plus every
spec section your phase names. Other workers share the codebase in other worktrees; preserve
their work and touch only your phase's files.

- **Files:** only the files your phase lists (plus `mech:` files it names). R1: never edit a
  `Cargo.toml`, `Cargo.lock`, a `mod` list, or a pinned § Interfaces signature after P01/P04.
  If you need one changed, stop and report `blocked` with the exact text you need.
- **Test-first (R3):** write the phase's tests, run them, see them fail on an assertion (or
  `unimplemented!()` panic), then implement. Every check must be able to fail.
- **Verification:** `cargo clippy -p <crate> --all-targets --locked -- -D warnings`, `cargo fmt
  -p <crate> -- --check` (or `rustfmt --check` on your files), plus the phase's
  "Expected test outputs" command. Never `--workspace` builds/tests unless your phase says so.
  Quote the full `test result: ok. N passed; 0 failed; …` line; a filtered run with `0 passed`
  is a failure.
- **Build env:** per-worktree target dir — test binaries of workspace members collide across
  worktrees in a shared one. From your worktree root: `[ -d target ] || cp -cR
  /Users/ravichandrasekhar/Projects/rollcall/target ./target; export CARGO_TARGET_DIR=$PWD/target`
  (APFS clone: instant, reuses compiled deps). In the main checkout use its own `target/`. Builds
  can take minutes: use Bash timeouts of 600000 ms.
- **Worktree base:** if you run in an isolated worktree, first check `git log --oneline -1`; if
  HEAD is not the `impl/v0` commit named in your brief, run `git restore HANDOFF.md 2>/dev/null;
  git merge --ff-only impl/v0` (the one git command you may run) and report it.
- **Probe hygiene:** the target dir is shared, and cargo fingerprints by mtime. After reverting any
  mutation probe, `touch` the reverted file before the next build, or you test a stale binary.
  Never leave a probe in the tree; diff before reporting.
- **Forbidden:** `std::env::set_var` / `remove_var` (edition 2024 `unsafe`; `unsafe_code` is
  forbidden) — vary env via `EnvPaths` or `Command::env`. Printing prompt text from real
  agent logs (`~/.claude`, `~/.codex`) — never read them. Network access in code.
- **Ponytail (R4):** simplest code that meets the spec; no unrequested abstractions. It never
  cuts spec scope.
- **Portability:** CI runs Linux (ext4) and macOS (APFS). Don't assume a fresh inode after
  remove-then-create (ext4 reuses it: create the replacement first, then rename over), sort
  directory listings before asserting order, and gate OS-specific behaviour with `cfg`.
- **Blocking tests:** anything that runs rollcall, opens FIFOs, or could hang runs under a
  watchdog (R3).
- **No git:** never `git add/commit/stash/checkout/reset/push`. Leave edits in the tree.
- **Receipt (final message):** JSON with `phase`, `model_id` (your exact model id),
  `status` (`passed` | `failed` | `blocked`), `files_changed`, `tests_run` (each command with
  the quoted result line), `criteria` (each phase acceptance criterion → how it is proven:
  test name or command + output), `decisions` (any choice the plan left open), `blockers`.
