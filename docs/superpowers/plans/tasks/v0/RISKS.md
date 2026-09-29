# Open risks (orchestrator log; fed to the final gate)

- P11 `shell_quote` uses `to_string_lossy`: a non-UTF-8 path quotes to a different path (U+FFFD). Spec cases are all UTF-8. P31/P37/P38 may refuse such paths.
- P26 non-UTF-8 paths inside core types make `commit_scan` return Err (serde_json). No spec fixture has one; a real tree with one would fail the cache commit.
- P26 stores repo rows per root as `RepoRow` JSON (membership columns real) instead of a path-keyed repos table (§6.1 wording); observable Index behaviour matches MemIndex.
- P13 a non-existent cwd under a live worktree → `PathGone` (not matched by prefix). Spec §6.5 ambiguous; P08 fixture places its path_gone cwd outside any worktree.
- P07 worker ran `rm -rf /private/tmp/claude-501/-Users-ravichandrasekhar` (a mistyped scratch path; may have been another session's scratchpad for the home-dir project). Outside the repo; report to the author.
- P14 N1: spec §4.4 says `Config::isolated()`; P19 must use `gix::open::Options::isolated()` (the one that denies environment config). P14's "unreachable" verdicts depend on it.
- P14 C-2 do-not-call list (obligation on P20/P21): `Repository::worktrees()` and `Proxy::*`, reflog APIs, `rev_parse` with `@{}`, `Platform::pseudo()`, `open_mailmap`.
