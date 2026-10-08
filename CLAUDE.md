# rollcall

Rust workspace (5 crates, edition 2024, no `unsafe`): a status board for git repos and the Claude/Codex/Copilot sessions in them.

- Spec (the contract): `docs/superpowers/specs/2026-09-29-rollcall-design.md`
- Plan: `docs/superpowers/plans/2026-09-29-rollcall-v0-plan.md` (phase briefs in `docs/superpowers/plans/tasks/v0/`; workers read `WORKER-RULES.md`)

## Rules (plan § Execution rules)

- R1: after P01/P04 no phase edits a `Cargo.toml`, `Cargo.lock`, a `mod` list, or a pinned interface signature; stop and report instead.
- R2: lanes run in isolated worktrees; the orchestrator integrates and runs `cargo fmt --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`.
- R3: tests first and red; every check must be able to fail; blocking tests run under a watchdog; never `std::env::set_var`.
- R4: ponytail governs how, never what (ladder below).

## Ponytail ladder

Adapted from github.com/dietrichgebert/ponytail. Stop at the first rung that holds: (1) not needed, (2) already in this repo, (3) Rust std, (4) platform feature, (5) a crate already in `Cargo.lock`, (6) one line, (7) minimum code. No unrequested abstractions; mark deliberate corner-cuts `// ponytail: <ceiling>, <upgrade path>`.

## Safety

- Never print prompt text from real agent logs (`~/.claude`, `~/.codex`, `~/.copilot`).
- Re-audit gix read and spawn sites on any gix bump (spec §4.4).
