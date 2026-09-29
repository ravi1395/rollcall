# Open risks (orchestrator log; fed to the final gate)

- P11 `shell_quote` uses `to_string_lossy`: a non-UTF-8 path quotes to a different path (U+FFFD). Spec cases are all UTF-8. P31/P37/P38 may refuse such paths.
- P26 non-UTF-8 paths inside core types make `commit_scan` return Err (serde_json). No spec fixture has one; a real tree with one would fail the cache commit.
- P26 stores repo rows per root as `RepoRow` JSON (membership columns real) instead of a path-keyed repos table (§6.1 wording); observable Index behaviour matches MemIndex.
- P13 a non-existent cwd under a live worktree → `PathGone` (not matched by prefix). Spec §6.5 ambiguous; P08 fixture places its path_gone cwd outside any worktree.
