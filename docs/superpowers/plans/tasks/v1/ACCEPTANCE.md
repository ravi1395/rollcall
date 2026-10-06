# rollcall v1 — acceptance criteria (spec revision 16)

Fixed acceptance criteria for the items the author approved on 2026-10-03 and refined on 2026-10-04 (`HANDOFF.md`, "Author decisions 2026-10-03", "Author design answers 2026-10-04"). The spec is the contract: `docs/superpowers/specs/2026-09-29-rollcall-design.md`, revision 16. These criteria stay fixed through the fix loop; a criterion is met only with evidence.

## Conventions (all items)

- Every command runs with `HOME`, `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `CLAUDE_CONFIG_DIR` and `CODEX_HOME` pointed at a scratch directory and `ROLLCALL_NOW` fixed (§10.1). Never against the real `~/.claude` / `~/.codex`.
- `$F`, `$F_clean`, `H` are the §10.1 fixture roots; new fixtures go in `tests/fixtures/MANIFEST.toml` (hand-written) and `rollcall-cli/tests/it/`.
- Each item ends with the R2 gate passing: `cargo fmt --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`.
- R1 for every item: `git diff --quiet <item base> -- Cargo.lock '*Cargo.toml'` exits 0, and no `mod` list or pinned interface signature changes. A needed change is a stop-and-report, not a criterion failure to work around.
- A criterion tagged **(assumption)** rests on a spec detail marked *assumption*; it stands until the author confirms or changes that detail, and then follows the decision.
- Items are implemented in workflow order I2 → I1 → I3 → I5a → I5c → I5e → I5d → I5b → I4, so criteria after I2 expect exit `4` for a complete scan with error rows.

## I2 Exit code 4 for a completed scan with error rows

Spec sections: §3.1 (exit codes), §10.2 (Deadline), §12.1 #2
Has logic: yes
Files (hint): `crates/rollcall-cli/src/scan.rs` (`EXIT_INCOMPLETE`, `run_scan`, `cached_exit`, `cached_with`, unit tests `an_error_row_exits_1`, `a_partial_walk_exits_1`), `crates/rollcall-cli/src/output.rs` (`doctor` uses `outcome.exit`), `crates/rollcall-cli/tests/it/discovery.rs:159,181,277,371`, `crates/rollcall-cli/tests/it/hostile.rs:308`, `crates/rollcall-cli/tests/it/shell.rs`

I2-1: Unit test in `scan.rs`: a fresh scan of a root whose only error is one `Repo.error` row, walk not partial, returns exit `4` (`exit_is(&outcome, 4)`); the same with only a `Worktree.error` returns `4`.
I2-2: Unit test: a scan whose walk is partial and that also has an error row returns `1`.
I2-3: Existing unit tests `a_lock_held_with_no_cache_has_no_scanned_at_and_exits_1` and `no_scan_without_a_cache_exits_1` pass unchanged (exit `1`).
I2-4: After a fresh scan of a root with an error row and no partial walk, `rollcall status <root> --no-scan --json` exits `4`, and an answer given while the test holds the scan lock exits `4` with `stale: true`.
I2-5: `rollcall status $F --json` exits `4`; `rollcall scan $F` exits `4`, and its one stderr line has the §3.1 summary shape and ends `(<E> errors)` with `<E>` ≥ 1.
I2-6: `rollcall status $F_clean --json` and `rollcall scan $F_clean` each exit `0`.
I2-7: `rollcall $F` with stdout a pipe (the non-TTY `status` form) exits `4`.
I2-8: The §10.2 budget fixture still gives `partial: true` and exit `1` (§12.1 #9), including when it also holds an error row.
I2-9: The §10.2 deadline run on `$F_clean` with `repo_budget_secs = 0.000001` marks every row `timed out` and exits `4`; `hostile.rs` asserts `rollcall scan H` exits `4`.
I2-10: `rc status $F` (the `rc` function from `rollcall init <shell>`) returns `4` in bash, zsh and fish, and leaves no resume temp file behind.
I2-11: The TUI quits with `q` → exit `0` (existing PTY test passes unchanged); `rollcall mcp` behaviour is unchanged (existing MCP tests pass unchanged).
I2-12 (assumption): `rollcall doctor $F` and `rollcall doctor $F --json` exit `1`.
I2-13 (assumption): Unit test in `scan.rs`: with a stored complete scan holding an error row (which alone answers `4`), a fresh scan that returns `Err` and answers from the cache (`cached_with(.., EXIT_INCOMPLETE)`) exits `1`, not `4`.

## I1 Dot-named files under refs/ are exempt from the loose-ref size cap

Spec sections: §4.4 (pre-open checks, maintenance rule), §10.2 (hostile `dot-ref`), §12.1 #8
Has logic: yes
Files (hint): `crates/rollcall-git/src/open.rs` (`check_refs_tree` :128-156, the `metadata_problem` call at :148; callers `check_git_dir` :230 and `check_admin_dirs` :219), `tests/fixtures/MANIFEST.toml`, `crates/rollcall-cli/tests/it/build_git.rs`, `crates/rollcall-cli/tests/it/hostile.rs`

I1-1: Unit test in `open.rs`: a `refs/` tree holding a 6,144-byte regular file `heads/.DS_Store` passes `check_refs_tree` (`Ok`). Removing the exemption makes this test fail.
I1-2: Unit test: a 6,144-byte regular file `heads/junk` fails with an error containing `untrusted git dir` and `larger than 4096 bytes`.
I1-3: Unit test: a symlink `heads/.x` pointing at a 100-byte regular file fails with an error containing `not a regular file`.
I1-4: Unit test: a 6,144-byte regular file `heads/.d/junk` (inside a dot-named directory) fails with `larger than 4096 bytes`.
I1-5: Unit test: 10,001 empty regular files named `.f0` … `.f10000` under `refs/heads/` fail with `more than 10000 ref entries`.
I1-6: Unit test: a linked worktree's `.git/worktrees/<name>/refs/.DS_Store` of 6,144 bytes passes the admin-dir checks.
I1-7: Hostile fixture `dot-ref` in `H` (§10.2): rows (i) and (ii) have `error: null`; rows (iii), (iv) and (v) carry an error starting `untrusted git dir`.
I1-8: Real-shape check: in a scratch repo with one commit and `head -c 6144 /dev/urandom > .git/refs/heads/.DS_Store`, `rollcall status <root> --json | jq -r '.repos[0].error'` prints `null` and the command exits `0`; `git -C <repo> for-each-ref --format='%(refname)'` lists no `.DS_Store`.
I1-9: Premise test in `rollcall-git` (guards the §4.4 maintenance rule): with a 6 KiB `refs/heads/.DS_Store`, the hardened open succeeds and `references().all()` and `references().prefixed("refs/remotes/")` yield no `Err` item and no name containing `.DS_Store`. A gix version that stops skipping invalid names makes this test fail.

## I3 Config `ignore` paths

Spec sections: §3.2 (`ignore`), §4.2 (ignore bullet), §10.2 (Ignore)
Has logic: yes
Files (hint): `crates/rollcall-cli/src/config.rs` (struct `Config`, known-keys list at :126), `crates/rollcall-cli/src/scan.rs` (`walk_options`, `build_rows`, cache reads), `crates/rollcall-git/src/walk.rs`, `crates/rollcall-git/src/root.rs`, `crates/rollcall-cli/src/output.rs`, `crates/rollcall-cli/src/mcp.rs`, `crates/rollcall-cli/src/tui.rs`, `crates/rollcall-cli/tests/it/`

Fixture for I3-3 to I3-9: root `$R` with repos `a`, `app`, `app2`, `grp/x`, `grp/y`, `z` (all clean, one commit each), and `$HOME` an ancestor of `$R`; config `ignore = ["$R/grp", "$R/app", "~/<path of $R/z relative to $HOME>"]` with the literal paths written in.

I3-1: With that config, `rollcall status $R` writes nothing to stderr that contains `unknown`.
I3-2: Config errors exit `2`: `ignore = "x"` (wrong type); (assumption) `ignore = ["rel/x"]`; (assumption) `ignore = ["~bob/x"]`.
I3-3: `rollcall status $R --json | jq -r '[.repos[].name] | sort | join(",")'` prints `a,app2` and exits `0` (`app2` shares a string prefix with the ignored `app` and stays).
I3-4: Plain `rollcall status $R` has exactly two repo lines, for `a` and `app2`; `rollcall doctor $R --json | jq '.repos_by_kind.normal'` prints `2`.
I3-5: MCP `where_was_i` and `list_stale` on `$R` return no repo named `app`, `x`, `y` or `z` (fixture variant: `grp/x` has a missing linked worktree, so `list_stale` would list it without the ignore).
I3-6: The TUI `TestBackend` snapshot of `$R` lists only `a` and `app2`.
I3-7: Scan `$R` with no `ignore` (6 repos in the cache), then add the config and run `rollcall status $R --no-scan --json`: only `a` and `app2` appear.
I3-8: Make `grp/x` a corrupted repo (an error row without the ignore): with the config, `rollcall status $R --json` exits `0`.
I3-9 (assumption): Add `$R/grp/locked` with mode `000`: with the config, `rollcall doctor $R --json | jq '.walk.unreadable_dirs'` prints `0` (the walk never entered `grp`); without the config it prints `1` (skip when euid is 0).
I3-10: A root inside repo `E`'s working tree with `ignore = ["<E>"]`: `status --json` has no row with `contains_root: true`.
I3-11 (assumption): `rollcall status $R/grp --json` with the config prints `repos: []`.
I3-12 (assumption): A repo `m` outside every ignored path with a linked worktree at `$R/grp/wt`: `m` and its worktree `$R/grp/wt` are both listed.

## I5a Decode HTML entities in plain-status session text

Spec sections: §8.2 (HTML entities), §9 (pipeline order)
Has logic: yes
Files (hint): `crates/rollcall-core/src/text.rs` (new decode beside `sanitise` / `scrub` / `truncate200` / `render_text`), `crates/rollcall-cli/src/output.rs` (`session_text`, `build_status`), `tests/fixtures/MANIFEST.toml`, `crates/rollcall-cli/tests/it/sessions.rs` or `privacy.rs`

I5a-1: Unit: decoding `&lt;command-name&gt;` gives `<command-name>`; `&gt;`, `&amp;`, `&quot;`, `&#39;` give `>`, `&`, `"`, `'`.
I5a-2: Unit: `&#65;` and `&#x41;` each give `A`; `&#x1F600;` gives U+1F600.
I5a-3: Unit: `&amp;lt;` gives `&lt;` (one pass).
I5a-4: Unit: each of `&nbsp;`, `&#xD800;`, `&#x110000;`, `&#;`, `&#xZZ;` and `&lt` (no `;`) is returned unchanged.
I5a-5: Fixture Claude session with `customTitle` `&lt;command-name&gt;/clear&lt;/command-name&gt;`: plain `status` shows `"<command-name>/clear</command-name>"` on that worktree line, and `status --json` keeps `.title` exactly `&lt;command-name&gt;/clear&lt;/command-name&gt;`.
I5a-6: Fixture title `deploy token&#61;abc123def`: the plain line contains `[redacted]` and not `abc123def`; JSON `.title` is unchanged.
I5a-7: Fixture title `x&#27;[2Jy`: the plain output contains no `0x1B` byte.
I5a-8: Fixture Claude session with `customTitle` of 300 repetitions of `&amp;` (Claude, not Codex: a Codex thread title over 200 graphemes is reclassified as a prompt at ingest, `codex_db.rs` `MAX_THREAD_TITLE_GRAPHEMES`, before any decoding): the plain quoted text is 199 `&` followed by `…` (truncation after decoding).
I5a-9 (assumption): An untitled session whose `last_prompt` is `a &amp; b` shows `"a & b"` in plain `status`.
I5a-10 (assumption): The TUI snapshot of the I5a-5 session shows the title undecoded, and MCP `where_was_i` returns it undecoded.

## I5c Risk summary line

Spec sections: §8.2 (risk summary line)
Has logic: yes
Files (hint): `crates/rollcall-cli/src/output.rs` (`plain_status`, `repo_cells`, `align`), `crates/rollcall-cli/tests/it/`

I5c-1: Fixture with repos `r1` (attention 3: `↑2`, upstream set), `r2` (attention 2: one dirty file, no remote configured, so no `no-upstream` badge), `r3` (attention 4: a `Worktree.error`, e.g. `git error: unsafe tracked file` on main, plus a dirty linked worktree), `r4` (attention 0). The first line of plain `status` is exactly `at risk: r1 ↑2; r2 ●1` (assumption: entry = name cell, space, badge cell).
I5c-2: A root whose repos all have attention 0, 1 or 4: no output line starts with `at risk:`.
I5c-3: Seven repos with attention 2 or 3: the risk line names the first five in §3.3 order and ends with `; +2 more`.
I5c-4: Removing the risk line from the I5c-1 output leaves exactly the lines plain `status` printed before revision 16 for that fixture (the risk line takes no part in column widths), checked against a hand-written expected file.
I5c-5: `status --json` for the I5c-1 fixture validates against `schema/status.json` and contains no `at risk` text.
I5c-6 (assumption): On the I5c-1 fixture (§3.3 order r3, r1, r2, r4), `status --top 2` prints `at risk: r1 ↑2` as its first line, and `status --top 1` (r3 only) prints no line starting `at risk:` (the line describes only the repos shown).

## I5e Prune hint for missing worktrees

Spec sections: §8.2 (prune hint)
Has logic: yes
Files (hint): `crates/rollcall-cli/src/output.rs` (`plain_status`), `crates/rollcall-cli/tests/it/`

I5e-1: A repo at `<p>` with two missing linked worktrees: the last line of plain `status` is exactly `hint: 2 missing worktrees — run 'git worktree prune' in <p>` (sanitised path).
I5e-2 (assumption): One missing worktree: `hint: 1 missing worktree — run 'git worktree prune' in <p>`.
I5e-3 (assumption): Two repos with missing worktrees: two hint lines, after every repo and worktree line, in the repos' output order.
I5e-4: A root with no missing worktree: no line starts with `hint:`.
I5e-5: After `rollcall status`, every `.git/worktrees/<name>` admin dir of the missing worktrees still exists (rollcall did not prune), and `status --json` contains no `hint` text.
I5e-6 (assumption): `status --dirty` on a root whose only repo with a missing worktree is clean: no hint line.

## I5d Filters --dirty, --stale, --top N

Spec sections: §3.1 (`status` row, exit codes), §8.2 (filters)
Has logic: yes
Files (hint): `crates/rollcall-cli/src/cli.rs` (`Command::Status`), `crates/rollcall-cli/src/output.rs` (`status`, `plain_status`), `crates/rollcall-cli/tests/it/`

Fixture: repos `m` (clean main + one dirty linked worktree), `u` (only `↑1`), `s` (only a merged stale linked worktree), `c` (clean).

I5d-1 (assumption): `rollcall status <root> --dirty` lists repos `m` and `u` only, and `m` shows both of its worktree lines (the clean main included).
I5d-2 (assumption): `--stale` lists `s` only.
I5d-3 (assumption): `--dirty --stale` lists `m`, `u` and `s`.
I5d-4: `--top 2` lists the first two repos of the unfiltered §3.3 order; (assumption) `--stale --top 1` lists `s` (the filter runs before `--top`, although `s` is not first overall).
I5d-5: `--top 0`, `--top x` and `--top -1` each exit `2` (`--top 0`: assumption).
I5d-6 (assumption): Add a corrupted repo to the fixture: `rollcall status <root> --dirty` (which hides it) exits `4`.
I5d-7 (assumption): `status --json --dirty`, `status --json --stale` and `status --json --top 1` each print output byte-identical to `status --json` on the same fixture (filters accepted and ignored with `--json`; JSON unchanged per decision 5).
I5d-8: `rollcall <root> --dirty` (the TUI form) exits `2`.

## I5b Colour and terminal-width truncation in plain status, `--color`

Spec sections: §3.1 (`status` row, Environment), §8.2 (colour, width truncation)
Has logic: yes
Files (hint): `crates/rollcall-cli/src/cli.rs` (`Command::Status`, `dispatch`, `stdout_is_tty`), `crates/rollcall-cli/src/config.rs` (`EnvPaths.no_color`), `crates/rollcall-cli/src/output.rs` (`align`, `state_badges`, `repo_cells`), `crates/rollcall-cli/tests/it/shell.rs` (PTY harness, `expectrl`)

I5b-1: `rollcall status $F --color always` contains `ESC[33m↑` for an unpushed badge, `ESC[34m●` for a dirty badge, `ESC[2m` before a `missing` or `stale` badge and `ESC[31merror: ` for an error badge, each badge followed by `ESC[0m`.
I5b-2: `rollcall status $F --color never` and `rollcall status $F` with stdout a pipe each contain no `0x1B` byte.
I5b-3: Removing every `ESC[` … `m` sequence from `--color always` output gives output byte-identical to `--color never`.
I5b-4: Under a PTY of 120 columns, `rollcall status $F` output contains `ESC[`; with `NO_COLOR=1` it contains no `0x1B` byte; (assumption) with `NO_COLOR=1 --color always` it contains `ESC[`.
I5b-5: Under a PTY of 60 columns with a fixture whose session title makes a worktree line wider than 60, every output line, with SGR sequences and `\r` removed, is at most 60 display columns; that line ends in `…`; the number of lines equals the piped output's.
I5b-6: Piped, the same fixture prints that line in full (wider than 60 columns, no added `…`).
I5b-7 (assumption): Under the 60-column PTY with `--color never`, lines are still truncated.
I5b-8: Unit: truncating `ab漢字` (display width 6) to 4 columns gives `ab…`; `e` + U+0301 is never split from its accent; a line already within the width is returned unchanged.
I5b-9 (assumption): Unit: with no readable width (or width 0) the line is returned unchanged.
I5b-10: `--color bogus` exits `2`; `status --json --color always` prints output byte-identical to `status --json` (assumption: accepted and ignored).
I5b-11: The TUI colour tests of §12.1 #15 pass unchanged.
I5b-12 (assumption): `rollcall status <root> --color always` on a fixture with a nested repo, a bare repo, an `external_main` repo, a worktree with no upstream and a repo at attention 0 has `ESC[2m` immediately before each `nested`, `bare`, `ext`, `no-upstream` badge and before the `quiet …` last-activity cell, each badge or cell followed by `ESC[0m`.

## I4 Homebrew tap renamed to ravi1395/homebrew-rollcall

Spec sections: §11 (Releases)
Has logic: no
Files (hint): `dist-workspace.toml:25`, `.github/workflows/release.yml:304`, `README.md:32`

I4-1: `grep -n '^tap = ' dist-workspace.toml` prints `tap = "ravi1395/homebrew-rollcall"`.
I4-2: `grep -rnE 'homebrew-tap|ravi1395/tap' dist-workspace.toml .github/` prints nothing.
I4-3: `grep -c 'repository: "ravi1395/homebrew-rollcall"' .github/workflows/release.yml` prints `1`.
I4-4: `git diff --numstat <item base> -- .github/workflows/release.yml` prints `1	1	.github/workflows/release.yml` (only the tap line changed).
I4-5: `grep -E '^\s*(- )?uses:' .github/workflows/release.yml | grep -vE '@[0-9a-f]{40}( |$)'` prints nothing (every action still pinned by SHA, §11).
I4-6: `grep -n '^allow-dirty' dist-workspace.toml` prints `allow-dirty = ["ci"]`.
I4-7: The committed workflow matches what `dist` generates apart from the pins: in a scratch copy of the tree with `allow-dirty = []`, `dist generate` followed by `git diff -U0 -- .github/workflows/release.yml | grep '^[-+] ' | grep -v 'uses:'` prints nothing. (It prints the `repository:` line whenever the toml and the workflow name different taps, e.g. when only one of them was edited; verified 2026-10-04 with dist 0.33.0 on a scratch copy.)
I4-8: No release is started: `git tag -l` is unchanged across the item and no workflow run is triggered.
I4-9 (assumption): `grep -n 'brew install' README.md` prints only `brew install ravi1395/rollcall/rollcall-cli`, and `grep -rnE 'homebrew-tap|ravi1395/tap' README.md` prints nothing. The author's 2026-10-04 answer names only `dist-workspace.toml` and `release.yml` for the tap work; this criterion stands until the author confirms or drops the README change.

## S11 Follow-ups

Spec sections: §3.1 (exit codes, `status` row), §4.4 (pre-open checks), §8.2 (width truncation, filters, prune hint, author decisions 2026-10-06)
Has logic: yes
Files (hint): `crates/rollcall-cli/src/config.rs` (`Ctx`, `load`), `crates/rollcall-cli/src/cli.rs` (`StatusFilters`, `Command::Status`, `run_status`), `crates/rollcall-cli/src/output.rs` (`status`, `build_status_with`, `plain_status`, `prune_hints`), `crates/rollcall-cli/Cargo.toml`, `Cargo.lock`, `crates/rollcall-core/src/lib.rs` (`WorktreeState`), `crates/rollcall-git/src/state.rs` (`missing_worktree_state`), `crates/rollcall-cli/tests/breaker_i5b.rs`

Item base: `9cb5a77`. Author decisions 2026-10-06 A–D (handoff of session 11), R1 exceptions approved for `Ctx`, `output::status` and `WorktreeState`, plus the `rustix` dependency line.

A. Filters and render mode in `Ctx`
S11-1: `crates/rollcall-cli/src/output.rs` declares `pub fn status(ctx: &Ctx, outcome: &ScanOutcome, root: &Path, json: bool, out: &mut dyn Write) -> ExitCode` (no other parameters), and `Ctx` has the fields `status_filters: StatusFilters` and `render: Render` in addition to the pinned four.
S11-2: Unit: the `Ctx` returned by `config::load` has `status_filters == StatusFilters::default()` and `render == Render::default()`.
S11-3: Behaviour is unchanged: `git diff 9cb5a77 -- crates/rollcall-cli/tests/` changes no existing assertion; the only change to an existing line in that directory is the removed `#[ignore …]` of S11-6 (and its doc comment). The full R2 gate passes, which re-runs every I5b, I5c, I5d and I5e test.

B. Width from stdout's own terminal
S11-4: `git diff 9cb5a77 -- crates/rollcall-cli/Cargo.toml` adds exactly one line, a `rustix` dependency whose features include `termios`; `git diff 9cb5a77 -- Cargo.lock` adds exactly one line, `"rustix",`, inside the `name = "rollcall-cli"` package's `dependencies`, and removes none; the number of `[[package]]` entries in `Cargo.lock` is unchanged.
S11-5: `grep -rn "terminal::size" crates/rollcall-cli/src/` prints nothing; the status width comes from `rustix::termios::tcgetwinsize` on stdout; `grep -rn "unsafe" crates/rollcall-cli/src/` prints no new line versus `9cb5a77`.
S11-6: `breaker_width_is_that_of_the_stdout_terminal_not_the_controlling_one` (`crates/rollcall-cli/tests/breaker_i5b.rs`) has no `#[ignore]` and passes; with the width read reverted to `ratatui::crossterm::terminal::size()` it fails (mutation red quoted in the gate record).

C. Lenient flags
S11-7: `rollcall status <root> --top 18446744073709551616` and `--top 99999999999999999999999999` each exit with the code of `rollcall status <root>` and print output byte-identical to it (all repos).
S11-8: `--top 0`, `--top x` and `--top -1` each still exit `2` (I5d-5 unchanged).
S11-9: On the I5d fixture: `--dirty --dirty` prints exactly what `--dirty` prints, `--stale --stale` exactly what `--stale` prints, `--top 1 --top 2` exactly what `--top 2` prints (two repos) and `--top 2 --top 1` exactly what `--top 1` prints; none exits `2`.
S11-10 (assumption): A repeated `--color` takes the last value: `--color always --color never` contains no `0x1B` byte and `--color never --color always` contains `ESC[`.
S11-11: Repeatable arguments elsewhere keep accumulating: `rollcall mcp --root A --root B` still serves both roots (the existing MCP tests for repeated `--root` pass unchanged).

D. Locked missing worktrees
S11-12: `WorktreeState` has `locked: bool` marked `#[serde(default)]`; unit: deserialising a serialised `WorktreeState` with the `locked` key removed succeeds with `locked == false`, and a `WorktreeState` with `locked: true` round-trips through serde with `locked == true` (the worker process and the cache carry it).
S11-13: Unit (`rollcall-git`): for a missing linked worktree, `missing_worktree_state` gives `locked == true` when `<admin_dir>/locked` exists as an empty file, a file with a reason, a mode-000 file, a dangling symlink or a FIFO (the FIFO case returns under a watchdog, proving the file is never opened); `locked == false` when it does not exist.
S11-14: Plain `status` on a repo `<p>` with two missing linked worktrees: with one of them locked (`git worktree lock` or a `locked` file) the last line is `hint: 1 missing worktree — run 'git worktree prune' in <p>`; with both locked no line starts with `hint:`; with three missing and one locked the line reads `hint: 2 missing worktrees — …`.
S11-15: Locked missing worktrees still print their worktree line with the `missing` badge (as unlocked ones do), and `rollcall status --json` output is byte-identical with and without the `locked` files; `git diff 9cb5a77 -- crates/rollcall-core/src/lib.rs` changes no field of `Repo`, `Worktree` or `StatusDoc`.
S11-16: The `locked` probe is one `symlink_metadata` on the admin dir entry, made where `missing_worktree_state` already runs (before any pre-open check, as the admin `HEAD` read is today); spec §4.4 records it.

Record
S11-17: R1: the plan's pinned entries for `Ctx`, `output.rs: status(…)` and `WorktreeState {…}` show the new shape annotated as the 2026-10-06 author-approved exceptions (like the `WalkOptions { … ignore … }` entry); spec §3.1/§8.2 carry the 2026-10-06 notes (A–D, `tcgetwinsize` in place of `crossterm::terminal::size()`, lenient `--top` and repeated flags, locked worktrees out of the hint, `hint:`-named repo and empty `error:` badge); no other pinned signature, `mod` list or `Cargo.toml` changes versus `9cb5a77`.
