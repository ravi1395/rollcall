# rollcall v0 plan — red-team round 1

**Date:** 2026-09-29
**Target:** `2026-09-29-rollcall-v0-plan.md` revision 1 @ `eeb8bc9`, against spec revision 6.
**Reviewers:** five independent Opus 5.5 agents (`skeptical-reviewer`, `model: opus`), read-only, ≤ 40 tool calls each, one lens each.
**Result:** plan revision 2 and spec revision 7.

| Lens | Verdict |
|---|---|
| Spec coverage / fidelity | Stands with fixes |
| Executor readiness | **Does not stand** |
| Waves, merge order, green tree | **Does not stand** |
| Ponytail efficiency | Stands with fixes |
| Test validity / feasibility | Stands with fixes |

## Orchestrator re-verification (this round)

Load-bearing reviewer claims were re-measured before anything entered the spec or plan. Scratch crates under the session scratchpad; no repo code exists.

| Claim | Command / source | Result |
|---|---|---|
| `gix-transport` is always in the tree | scratch crate, `gix = { version = "0.88", default-features = false, features = ["sha1"] }`; `cargo tree -i gix-transport -e normal`; `cargo tree -e features -i gix-transport` | `gix-transport v0.60.0 └── gix-protocol v0.66.0 └── gix v0.88.0`; only `feature "default"` enabled |
| `gix-command` (spawns processes) is always in the tree | same crate, `cargo tree -i gix-command -e normal` | `gix-command v0.11.0 └── gix-transport v0.60.0` |
| `jsonschema` defaults pull `reqwest` | same crate + `jsonschema = "0.58"` dev-dep; `cargo tree -i reqwest` | `reqwest v0.13.5 └── jsonschema v0.58.2 [dev-dependencies]` |
| Trimmed `jsonschema` removes it | `default-features = false, features = ["resolve-file"]`; `cargo tree -i reqwest` / `-i hyper` | both "did not match any packages" |
| gix 0.88 reads `config.worktree` uncapped | `~/.cargo/registry/src/*/gix-0.88.0/src/config/cache/incubate.rs` | `extensions.worktreeConfig` → `load_config(git_dir.join("config.worktree"), …)`; `load_config` does `std::fs::File::open` then `std::io::copy` |
| `tree_index_status` exists at 0.88 | same source tree, `status/tree_index.rs` | `pub fn tree_index_status<'repo>(` present |
| jiff `Display` format | `jiff 0.2.37`: print a whole-second, a millisecond and a `from_second` timestamp | `2026-09-28T18:04:11Z 2026-09-28T18:04:11.532Z 2026-09-21T14:13:20Z` |
| SQLite side files inherit the DB mode | `umask 000`; pre-create DB `0600`; WAL; write; `stat` | `w.db`, `w.db-wal`, `w.db-shm` all `0600` |
| `SF_DATALESS` value | `grep SF_DATALESS $(xcrun --show-sdk-path)/usr/include/sys/stat.h` | `0x40000000` |

Reviewer-run measurements accepted as reported (not re-run here): `File::try_lock` stable since 1.89 and `WouldBlock` on a second fd; the std APIs in plan R6 compile under `forbid(unsafe_code)`; binary-crate dead-code errors under `clippy -D warnings`; naive 10,000 × 100 prefix scan ≈ 120 ms.

## Findings → disposition

Reviewer tags: **C** coverage, **E** executor, **W** waves, **P** ponytail, **T** test validity.

### Blockers

1. **`gix-transport` ban unsatisfiable** (E, P). → Spec rev 7 §11 bans the client features; plan P01/P02 check features, not the crate. `gix-command` spawn sites added to the P15 audit.
2. **`jsonschema` default features pull `reqwest`/`hyper`** (E, P). → R5 trims it.
3. **`config.worktree` uncapped read** (T). → Spec rev 7 §4.4 pre-open list, hostile fixtures, maintenance rule; plan P14/P18.
4. **CLI binary-only: TUI tests can't reach it; unwired modules fail `-D warnings`** (E, W, P). → cli is a library + one-line `main.rs`; all library modules `pub mod`; TUI tests in-file.
5. **No owner for `main.rs` wiring** (E, W). → P04 writes the complete dispatch against pinned entry points (§ Interfaces).
6. **Parallel workers in one tree break each other's compile/tests** (E, W). → R2: lanes run in isolated worktrees; orchestrator integrates and runs the full gate.
7. **P32 ↔ P33 cycle (TUI `R` ↔ resume)** (E, W, C). → Lane D is serial; `app.rs` calls the pinned `resume::resume`.
8. **fsck self-test hangs on the hang-probe** (T). → Runs over `F_clean` only, under a watchdog, with the corrupted repo as negative control.

### Major

9. Hidden dependency edges: C2 needs P12's text fns; P21 needs the trait from P23; P31 needs MCP; P27 needs `doctor` output; P08's `order.json` needs sessions (E, W, C). → Lane table rebuilt; trait pinned in P04; MCP-privacy moved to P34; P30 criteria restated at `Index` level; `order.json` moved to P10.
10. Manifest schema undefined; `# covers:` comments invisible to parsers (E). → § Fixture manifest schema; `covers` is a real key; `scripts/check-manifest.py` gives P07/P08 a phase-local check.
11. No demo root; no way for CI to run the test-only generator (E, W, C). → `[demo]` section; `materialize_perf` / `materialize_demo` `#[ignore]` tests.
12. Per-session fields (#4/#5) and offsets (#7) not observable via CLI (C). → In-process through the cli library (`scan` + `render_session`); offsets via `SqliteIndex::source_state`.
13. Matches not keyed by root (C). → Spec rev 7 §3.4/§6.1; P03 and P26 test it.
14. Trust rule (d) inputs missing from `GitOptions` (C). → `GitOptions { root, home, skip_paths }`.
15. `doctor` has no field for fallback reasons / layouts (C). → Spec rev 7 `DoctorDoc.notes`.
16. `dirty_count` needs HEAD↔index (T). → Spec rev 7 §3.3; P20 tests staged-only, both, rename.
17. Hostile positive controls don't prove the vectors live for gix, and pollute the tree (T). → Separate `fresh(H)` builds; gix-default positive control in P19; same-size edits; `diff=x`.
18. Symlink-loop time bound can't detect a missing visited set (T). → Fan-out fixture + entry-count bound.
19. No watchdog on hang tests (T). → R3.
20. 1 ms deadline run nondeterministic (T). → Spec rev 7: 1 µs budget, deadline covers open, late results discarded.
21. Untracked oracle counted directories (T). → `--untracked-files=all` + hard-coded expected values.
22. Snapshots depend on tempdir paths and wall clock (T). → `MemIndex` models, synthetic paths, `scanned_at = now` (spec rev 7 §3.3).
23. `big-object` never read; no zlib route (T, E). → HEAD root tree; `python3` zlib.
24. Unspecified dependency placement / features, `interrupt` over-enable risk, `[[bench]]` stub (E, P, W). → R5 per-crate table; `interrupt` excluded; bench stub in P01.
25. P13 audit too big and halts by construction (E). → Split into P14/P15; accepted exceptions pre-listed.
26. `excludesFile` reader unowned; diagnostics carriers undefined (E). → `user_excludes_file` in P18; `RepoState.pending`; `WalkDiag` = `DoctorDoc.walk`.
27. `Deserialize` missing for stored diagnostics (W). → All §7 docs derive both.
28. MCP, TUI, sanitising, §4.4 behaviours without criteria (C). → Criteria added to P31, P34, P36, P37, P18–P20.
29. Local tools and remote missing (E). → Q1/Q2 retained; CI-only criteria marked UNVERIFIABLE until CI; `cargo insta` replaced by `INSTA_UPDATE=no`.

### Minor (all applied unless noted)

Vacuous phase checks (`jq empty` only) → counts and scripts. P24 title-grep can't fail at store level → dropped. Timing asserts (P11, P15, P19) → counters. `/` refusal vs peek → counter bound. Budget overrun by workers → exact claim-before-read. `std::thread::scope` can't abandon → `spawn` for repo workers. Duplicate test git helpers → `testgit.rs`. Duplicate types → `RepoRow { state }`, `DoctorDoc` stored. Direct `crossterm` → `ratatui::crossterm`. `xxhash-rust` → `gix-hash` SHA-1. Trimmed `arboard` / `ratatui` / `criterion` defaults. Dropped CI `sudo chown` branch. `task-notification` cut case added. Codex `resolve` / `vanished` tests added. `peak_rss` unit normalisation; GNU `time` installed in CI. Effective-uid probe created fresh per call. WAL test holds the writer open. Q3 (b)/(d) reworded as spec deviations. Resume file `O_NONBLOCK` (spec rev 7). Merge P24+P25 (store) → done as P26.

Not applied: none rejected outright. P's suggestion to keep only safety rules in `CLAUDE.md` is applied partially (one line per rule plus pointers).

## Still unverified (owner phase)

- cargo-deny `[[bans.features]]` actually denies `gix-transport` features → P02 negative check.
- cargo-dist edits root `Cargo.toml`; SHA-pinned `release.yml` needs `allow-dirty = ["ci"]` → P41.
- GNU `/usr/bin/time` on `ubuntu-latest` → P02 installs `time`; `require_tool` fails loudly.
- 4 GiB sparse files on runner temp filesystems; PTY stability on `macos-14`; `fish read -z` → first CI run.
- §12.1 #17 medians on `ubuntu-latest` → P40.
- Whether any further uncovered gix 0.88 read or spawn site exists → P14/P15.

---

# Round 2

**Target:** plan revision 2 / spec revision 7 @ `031051e`. **Reviewers:** three Opus 5.5 `skeptical-reviewer` agents, read-only, ≤ 35 tool calls each.
**Result:** plan revision 3 and spec revision 8.

| Lens | Verdict |
|---|---|
| Executor readiness (round-1 closure + new gaps) | Does not stand — 1 blocker, 13 majors, all local edits |
| Lanes and green tree | Stands with fixes |
| Fresh correctness of the round-1 edits | **Does not stand** — spec rev 7 introduced a memory hole |

Round-1 items: the executor reviewer reported items 4, 5, 6, 10, 25 and 26 as only partly closed; the round-2 fixes below cover them.

## Orchestrator re-verification

| Claim | Source / command | Result |
|---|---|---|
| `tree_index_status` has no interrupt and expands HEAD fully | gix-0.88.0 `status/tree_index.rs` `tree_index_status` | signature has no `should_interrupt`; body calls `self.index_from_tree(tree_id)` first |
| audit crate lists missed the actual readers | grep in the locked registry sources | `gix-glob-0.28.0` `search/pattern.rs` `File::open` + `read_to_end`; `gix-shallow-0.14.0` `lib.rs` `std::fs::read(shallow_file)`; `gix-diff-0.68.0` `blob/pipeline.rs` `File::open` + `read_to_end` |

Accepted as reported (reviewer-run, not re-run here): tree bomb of 3 × 200-entry nested trees → 8,000,000 paths, ~3 GB peak RSS (release build); default-options gix + `index_worktree_status` on a same-size edit spawns `filter.x.clean`; index↔worktree rename tracking removes an untracked file from the untracked set; `std::env::set_var` fails to compile under edition 2024 + `forbid(unsafe_code)`; clap parses a bare `frobnicate` as `[PATH]`; `repositoryformatversion = 7` fails gix open and `git fsck`, garbage `HEAD` does not fail open; jsonschema 0.58.2 rejects relative refs against an opaque `rollcall:` base; unused stub parameters warn under `-D warnings`; `O_WRONLY|O_NONBLOCK` on a reader-less FIFO → `ENXIO` on macOS.

## Findings → disposition

Blocker / security:
1. **HEAD tree expansion unbounded (rev 7 regression).** → Spec rev 8 §4.4 HEAD tree bound (1,000,000 entries, deadline-checked); `tree-bomb` fixture; P20 unit test; P35 gate; P15c lists non-interruptible memory-growth paths.
2. **`set_var` unusable; env not injectable** (both reviewers). → R3 rule; `EnvPaths` pinned; `Model.no_color`; `handle_key` takes `&EnvPaths` and returns `Action`; `resume` takes `&EnvPaths` and a `uid`; `find_agent` takes the `PATH` value.
3. **`display_line` stub until P38 but rendered from P31** (lanes). → Moved to `output.rs`, P31.

Major:
4. `lib.rs` entry points without an owning phase (both). → P04 writes one-line delegations to module `pub fn`s; later phases never touch `lib.rs`.
5. P19 criteria computed by P20 (lanes). → Moved to P20; P19's positive control runs status on the hardened repo explicitly.
6. `check-manifest.py count` unowned, circular, parallel edits (both). → P07 writes the whole script; counts are hand-written `[expect_counts]`.
7. Rebase after R1 edits not conflict-free (lanes). → R1: edits to open-lane files go through that lane's worker at a phase boundary.
8. P31 non-TTY criterion needs the binary; unit tests can't reach `validate` (lanes). → Moved to P32; `output.rs` tests build their own validator.
9. `corrupted` fixture opens fine in gix (executor). → Pinned as format version 7; P21 tightened to `open failed: `.
10. Audit crate lists miss real readers (executor). → Mechanical list from `cargo tree`; P15 split into 15a/15b/15c.
11. Session corpora, incremental steps, path tokens, and several keys undefined (executor). → Manifest schema: `corpus`, `steps` (carries #7's oracle), path tokens, `target`, `edit`, `rollout_id`, `[home].files`, `[[big_log]]`, `[expect_counts]`, commit-index scope, per-gate corpus list.
12. `WaitAtMost` semantics unpinned (executor). → Pinned in § Interfaces; P30 tests it.
13. Test-crate dead code (both). → `#![allow(dead_code)]` in `tests/it/main.rs`; builders exercised by `build_all_*` self-tests.
14. `matched`/`unmatched` fill unowned (executor). → P30.
15. P27 too big (executor). → Split into 27a/27b.
16. Rename-tracking scope changes `untracked_count` (correctness). → Spec rev 8 §3.3; P20 cases.
17. No carrier for source notes (correctness). → Spec rev 8 `SourceScan.notes`.
18. `frobnicate` parses as PATH (executor). → P04 tests `status --frobnicate`.

Minor (all applied): stub params `_`-prefixed; `[[bin]] name = "rollcall"`; root-relative test paths; P01 `[lints]` grep and P02 path fixed, count ≥ 5; pass floors on P21/P25; single-mutation invalid schema examples; absolute `rollcall:` refs, `resolve-file` dropped; snapshot creation reviewed by the orchestrator; `Model` fields pinned; parallel sub-chains get their own worktrees; FIFO writer released after assertion; deadline rules normative in §4.4; matches stored with the `--submodules` flag; `doctor` notes sanitised; §11 feature list; `repo_budget_secs` finite; `ScanDiagnostics = DoctorDoc`; `WorktreeKey.missing` in spec; trust (d) uses path skips only (plan decision, noted in § Interfaces); SHA-1 collision `Err` → re-read; P28 checks the Codex DB column list.

## Still unverified (owner phase)

Carried from round 1, plus: Linux `O_NONBLOCK` behaviour on the resume file (P38, CI); rebuild under `--locked` after a lane rebase (first R1 edit); `derive(Deserialize)` dead-code behaviour outside `support.rs` (covered by the crate-level allow).
