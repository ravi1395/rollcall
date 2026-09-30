# Open risks (orchestrator log; fed to the final gate)

- P11 `shell_quote` uses `to_string_lossy`: a non-UTF-8 path quotes to a different path (U+FFFD). Spec cases are all UTF-8. P31/P37/P38 may refuse such paths.
- P26 non-UTF-8 paths inside core types make `commit_scan` return Err (serde_json). No spec fixture has one; a real tree with one would fail the cache commit.
- P26 stores repo rows per root as `RepoRow` JSON (membership columns real) instead of a path-keyed repos table (§6.1 wording); observable Index behaviour matches MemIndex.
- P13 a non-existent cwd under a live worktree → `PathGone` (not matched by prefix). Spec §6.5 ambiguous; P08 fixture places its path_gone cwd outside any worktree.
- P07 worker ran `rm -rf /private/tmp/claude-501/-Users-ravichandrasekhar` (a mistyped scratch path; may have been another session's scratchpad for the home-dir project). Outside the repo; report to the author.
- P14 N1: spec §4.4 says `Config::isolated()`; P19 must use `gix::open::Options::isolated()` (the one that denies environment config). P14's "unreachable" verdicts depend on it.
- P14 C-2 do-not-call list (obligation on P20/P21): `Repository::worktrees()` and `Proxy::*`, reflog APIs, `rev_parse` with `@{}`, `Platform::pseudo()`, `open_mailmap`.
- P16 unverified locally: different-device skip (needs a second mount), SF_DATALESS on a real file (predicate only), wall-clock budget beyond zero duration. Dedup of the `untrusted_path` fixture's on-disk `.git` file against its admin dir (row path `$HOME`) is P07's oracle assumption — verify at P27b/P32.
- P16 FIFO named `.git` → `bad gitdir:` error row (spec silent); P08 told to align the manifest.
- P19 rule (d) literally makes the enclosing repo's worktree `untrusted worktree path` (its workdir is an ancestor of the root; spec §4.1 lists "enclosing" among the worktrees the rule applies to). Followed literally; oracles must match. Author may want to revisit.
- P19 finding: at `Trust::Reduced` gix keeps repo-local sections and filters per lookup; plain snapshot getters ignore trust. P20 must use trust-filtered accessors.
- P25 applies the §5.0 prompt filter to Codex `user_message` text too (orchestrator reading of "Common rules").
- Spec rev 12 (P15a U-4..U-9): the author chose in-process guards over a child process (2026-09-30); per-finding options are orchestrator picks, recorded in the red-team log. Unrun until P18b/P20/P21: gitlink recursion under `recurse_repositories(true)` (read from source), the 16 MiB stack (margin only: the P15a-rev-12 skeptic estimates 1,024 frames fit a default 2 MiB stack, so no test proves the pairing), and the ≈ 160 MiB pattern budget (arithmetic from the audit's 70× figure).
- Rev 12 skeptic (round 2 PASS) residual risks: the `UNTR` decoder was never run end to end (4 MiB cap sized from a `BString` proxy, ~19×), and no real monorepo index with `core.untrackedCache=true` was measured against it; the scan-level RSS bound composes per-repo figures (≈ 440 MiB) with an unspecified number of parallel repo workers (§3.4), so it holds for `H` by enumeration only.
