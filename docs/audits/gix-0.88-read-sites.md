# gix 0.88 read-site audit

**UNCOVERED count: 0** after spec revision 11 (was 3 at first audit: U-1, U-2, U-3 below — each now covered by a revision 11 check; rows keep their original analysis and carry a `→ covered (rev 11)` note).

| Part | Phase | State |
|---|---|---|
| 1: config, refs, open | P14 | written 2026-09-30, **blocked** on U-1, U-2, U-3 |
| 2a: index, status, worktree reads | P15a | not started |
| 2b: object store and history | P15b | not started |
| 2c: spawn sites and API confirmation | P15c | not started |

Basis: spec `docs/superpowers/specs/2026-09-29-rollcall-design.md` §4.4 (rev 10), plan rev 5 § Phase 14. Sources read: the exact locked crate versions under `~/.cargo/registry/src/index.crates.io-*/`; nothing else.

## Crate list (mechanical, verbatim)

Command, run from the repo root at `74bbd27` (P01), after the worktree fast-forward (the list was re-derived and matched the first derivation byte for byte):

```
export CARGO_TARGET_DIR=/Users/ravichandrasekhar/Projects/rollcall/target
cargo tree -e normal -p rollcall-git --prefix none --locked | grep '^gix' | sed 's/ (\*)//' | sort -u
```

(The `sed` only drops cargo's "already listed" `(*)` marker so each crate appears once; `sort -u` alone leaves both spellings.) Output, 50 crates:

```
gix v0.88.0
gix-actor v0.43.0
gix-attributes v0.36.0
gix-bitmap v0.5.0
gix-chunk v0.9.0
gix-command v0.11.0
gix-commitgraph v0.40.0
gix-config v0.61.0
gix-config-value v0.20.0
gix-date v0.17.0
gix-diff v0.68.0
gix-dir v0.30.0
gix-discover v0.56.0
gix-error v0.4.0
gix-features v0.50.0
gix-filter v0.35.0
gix-fs v0.23.0
gix-glob v0.28.0
gix-hash v0.27.0
gix-hashtable v0.17.0
gix-ignore v0.23.0
gix-imara-diff v0.3.0
gix-index v0.56.0
gix-lock v25.0.0
gix-macros v0.2.0 (proc-macro)
gix-object v0.65.0
gix-odb v0.85.0
gix-pack v0.75.0
gix-packetline v0.23.0
gix-path v0.13.0
gix-pathspec v0.21.0
gix-protocol v0.66.0
gix-quote v0.9.0
gix-ref v0.68.0
gix-refspec v0.46.0
gix-revision v0.50.0
gix-revwalk v0.36.0
gix-sec v0.15.0
gix-shallow v0.14.0
gix-status v0.35.0
gix-submodule v0.35.0
gix-tempfile v25.0.0
gix-trace v0.2.0
gix-transport v0.60.0
gix-traverse v0.62.0
gix-url v0.39.0
gix-utils v0.4.0
gix-validate v0.12.0
gix-worktree v0.57.0
gix-zlib v0.2.0
```

## Crate to part assignment

Every crate above is assigned exactly once (7 + 9 + 6 + 28 = 50).

| Part | Crates |
|---|---|
| **P14** (7) | `gix`, `gix-config`, `gix-ref`, `gix-discover`, `gix-path`, `gix-sec`, `gix-fs` |
| **P15a** (9) | `gix-index`, `gix-status`, `gix-dir`, `gix-glob`, `gix-attributes`, `gix-ignore`, `gix-diff`, `gix-filter`, `gix-object` |
| **P15b** (6 named) | `gix-odb`, `gix-pack`, `gix-commitgraph`, `gix-revwalk`, `gix-traverse`, `gix-shallow` |
| **P15b "remaining"** (28) | `gix-actor`, `gix-bitmap`, `gix-chunk`, `gix-command`, `gix-config-value`, `gix-date`, `gix-error`, `gix-features`, `gix-hash`, `gix-hashtable`, `gix-imara-diff`, `gix-lock`, `gix-macros`, `gix-packetline`, `gix-pathspec`, `gix-protocol`, `gix-quote`, `gix-refspec`, `gix-revision`, `gix-submodule`, `gix-tempfile`, `gix-trace`, `gix-transport`, `gix-url`, `gix-utils`, `gix-validate`, `gix-worktree`, `gix-zlib` |

Assignment flags for the orchestrator (crates left where the plan puts them; the audit found reads that live there):

- `gix-worktree` (P15b remaining) owns the ignore/attribute `Stack` that `index_worktree_status` drives, i.e. the subdirectory `.gitignore` / `.gitattributes` reads of §4.4's documented limitation. It belongs with P15a's reachability trace.
- `gix-features` (P15b remaining) provides `fs::walkdir_sorted_new` (the loose-ref directory walk in `gix-ref`, traced here) and `fs::read_dir`.
- `gix-command` and `gix-path::env` (spawn sites) are P15c work; this part only records that open does not reach them.
- `gix-protocol`, `gix-transport`, `gix-packetline`, `gix-url` are in the tree but the network features are not enabled (see next section).

## Method and principles

1. **Principle: every gix-internal cap is a post-`open()` size check.** `read_regular_file_content_with_size_limit` (gix-discover) calls `File::open` first, then `metadata().len()`. `File::open` on a FIFO blocks; `metadata().len()` of a symlink to `/dev/zero` is 0, so the cap passes and `read_to_end` never ends. So a verdict of "covered" must name a **pre-open lstat** (symlink / not-regular-file rejection plus the size cap), never a gix cap. §4.4's pre-open checks do exactly that; sites they do not name are UNCOVERED.
2. **Candidate enumeration.** `grep -rnE 'fs::read|read_to_end|read_to_string|io::copy|\.lines\(\)|File::open|OpenOptions|read_link|read_dir|std::fs::|read_exact|memmap|BufReader|open_options_no_follow|\.open\(|Command::new'` over `src/` of the seven P14 crates (tests excluded), then traced callers to entry points.
3. **Entry points traced:** `ThreadSafeRepository::open_opts` with `open::Options::isolated()` plus config overrides; then `refs.find("HEAD")` (done inside open), `Repository::head()`, `find_reference`, `references()` / `refs.iter()`, symbolic-ref following (`peel_to_id`), and packed-refs loading.
4. **Compiled features** (`cargo tree -e features -p rollcall-git --locked`, `gix feature` lines): `attributes`, `blob-diff`, `command`, `dirwalk`, `excludes`, `gix-status`, `index`, `revision`, `sha1`, `status`. **Not compiled:** `mailmap`, `worktree-archive`, `blocking-network-client` / `async-network-client`, `credentials`. `Repository::modules()` and `open_modules_file` are compiled (`attributes`).
5. **Isolated permissions verified in code.** `open::Options::isolated()` is `Options::default().permissions(Permissions::isolated())` (`gix/src/open/options.rs:isolated`); `Permissions::isolated()` = `Config::isolated()` (`git_binary`, `system`, `git`, `user`, `env`, `includes` all `false`) + `Attributes::isolated()` (all `false`) + `Environment::isolated()` (every field `Deny`) (`gix/src/open/permissions.rs`). The audit relies on the full `Options::isolated()` (the plan's P19 wording). Spec §4.4 says "`Config::isolated()` permissions"; `Config::isolated()` alone leaves `Environment` at its default, which would allow `xdg_config_path("ignore")` to read `$XDG_CONFIG_HOME/git/ignore` / `~/.config/git/ignore` (G8) and `GIT_*` overrides. See note N1.

Verdicts: **covered** (names the §4.4 pre-open lstat or other §4.4/§4.1 check that bounds it), **accepted exception** (only §4.4's "Documented limitation"), **UNCOVERED**, **unreachable** (with the reason and, where it is a caller obligation, a "do not call" flag). Anchors are `crate/path/file.rs:fn_name` relative to the crate's `src/` dir (crate version omitted; versions are in the list above).

## Confirmations requested by the phase

### `config.worktree` (rev 7 known site): confirmed, covered

`gix/src/config/cache/incubate.rs:StageOne::new` reads `extensions.worktreeConfig` from the repo-local `config`; when true it calls `load_config(git_dir.join("config.worktree"), ...)`. `load_config` does `std::fs::File::open` then `std::io::copy(&mut file, buf)` into a `Vec` with no bound, following symlinks and blocking on a FIFO. The `git_dir` argument is the directory passed to open (the admin dir `worktrees/<name>/` for a linked worktree). Covered by the §4.4 pre-open `config.worktree` lstat in the git dir and each `worktrees/<name>/` admin dir (regular file, 1 MiB). `lenient_config` only swallows the error, it never bounds the read.

### `include.path` under isolated permissions: includes are never followed; a FIFO `include.path` is never opened

Chain, all in the locked sources:

1. `gix/src/config/cache/incubate.rs:load_config` (repo-local `config` and `config.worktree`) hard-codes `includes: gix_config::file::includes::Options::no_follow()`.
2. `gix/src/config/cache/init.rs:load` selects `Options::follow(...)` only `if use_includes`; `Config::isolated()` sets `includes: false`, so it uses `no_follow()`. It then calls `globals.resolve_includes(options)`.
3. `gix-config/src/file/includes/types.rs:no_follow` returns `max_depth: 0`. `gix-config/src/file/includes/mod.rs:resolve_includes` returns immediately when `max_depth == 0`; `resolve_includes_recursive` (reached via `from_bytes_owned` -> `includes::resolve`) returns at `depth == options.includes.max_depth` before it looks at any section, so no `include` / `includeIf` header is ever inspected and `insert_includes_recursively` (the only include reader) is never entered.
4. `File::from_env` (the other `includes::resolve` caller) is skipped: `use_env` is `false` under isolated.

Verdict for the spec's hang-probe fixture (`include.path` -> FIFO): not followed, unreachable (see C1).

## Per-crate tables

Reachable-from key: **open** = `open_opts`; **refs** = `refs.find`, `head()`, `find_reference`, iteration, peel; **status** = `index_worktree_status` / `tree_index_status` (traced only where a P14 crate's code decides the path; the read itself is P15a).

### `gix` 0.88.0

| # | Anchor | What it reads | Reachable from | Verdict |
|---|---|---|---|---|
| G1 | `gix/src/config/cache/incubate.rs:load_config` (via `StageOne::new`), file `<git_dir>/config.worktree` | worktree config, `File::open` + `io::copy`, unbounded | open (only if `extensions.worktreeConfig`) | **covered**: pre-open `config.worktree` lstat, 1 MiB (git dir and each `worktrees/<name>/`) |
| G2 | same fn, file `<common_dir>/config` | repo-local config, unbounded | open | **covered** when `common_dir == git_dir` (pre-open `config`, 1 MiB). **UNCOVERED (U-1)** when `commondir` redirects: pre-open does not check `<resolved common dir>/config` |
| G3 | `gix/src/open/repository.rs:open_from_paths` -> `gix_discover::path::from_plain_file(git_dir/commondir)` | `commondir` file (see D1) | open | **covered**: pre-open `commondir` lstat, 1 MiB. The *target directory* it names is U-1 |
| G4 | `gix/src/open/repository.rs:open_from_paths` -> `refs.find("HEAD")` | `HEAD` (R1) and packed-refs (R4, `find` calls `assure_packed_refs_uptodate` first) | open | **covered**: pre-open `HEAD` 4 KiB, `packed-refs` 64 MiB. Does not follow the symref (find returns the unresolved `Reference`) |
| G5 | `gix/src/open/repository.rs:open_from_paths` -> `refs.iter().prefixed(replace ref base)` (`replacement_objects_refs_prefix`) | every loose ref under `refs/replace/` (R2) + packed-refs | open (skipped when replace refs are disabled) | **covered**: pre-open every entry under `refs/` (regular, 4 KiB, 10,000-entry stop) + `packed-refs` 64 MiB |
| G6 | `gix/src/config/cache/init.rs:source_path` | nothing (resolves candidate paths for GitInstallation / System / Git / User) | open | **unreachable as a read**: returns `None` before `storage_location` because `permitted` is `false` for all four sources under isolated; `git_binary=false` also prevents any git-binary lookup |
| G7 | `gix/src/config/cache/init.rs:load` -> `File::from_paths_metadata_buf` (C2), `File::from_env` (C4), `resolve_includes` (C1) | global/system/user config, `GIT_CONFIG_*` env, includes | open | **unreachable**: empty `metas`, `use_env=false`, `max_depth=0` (see confirmations) |
| G8 | `gix/src/config/cache/access.rs:assemble_exclude_globals` (called from `gix/src/repository/attributes.rs:attributes` and `excludes`) | (a) `core.excludesFile` path from resolved config, read by `gix-ignore` `Search::from_git_dir` via `File::open` + `read_to_end` (follows symlinks, unbounded); (b) `<common_dir>/info/exclude` (same reader); (c) `xdg_config_path("ignore")` fallback | status (untracked / ignored detection) | (a) **UNCOVERED (U-3)** when the user has no `core.excludesFile` (no override is passed) and the repo-local config sets one; **covered** when the rollcall override is set (Api source wins in `trusted_file_path` / `path_filter`, last value). (b) **covered**: pre-open `info/exclude` 1 MiB, in the *common* dir (so U-1 applies). (c) **unreachable**: `Environment::isolated()` makes `permission.check` return `None` (requires `Options::isolated()`, note N1). The read itself is P15a (`gix-ignore`, `gix-glob`) |
| G9 | `gix/src/config/cache/access.rs:assemble_attribute_globals` (called from `gix/src/repository/attributes.rs:attributes` and `attributes_only`) | (a) `core.attributesFile` from config; (b) `<common_dir>/info/attributes`; (c) xdg / system / git-binary attribute files | status | (a) **covered** by §4.4 stripping of `core.attributesFile` (no path is left to read; the strip must be committed before the first attribute-stack build). (b) **covered**: pre-open `info/attributes` 1 MiB, common dir (U-1 applies). (c) **unreachable**: `Attributes::isolated()` = all `false` |
| G10 | `gix/src/repository/submodule.rs:open_modules_file` (via `modules()`) | worktree-root `.gitmodules`, `fs::read` after `symlink_metadata` symlink skip | not reached at open; `modules()` / `submodules()` only if called (rollcall's `SubmoduleStatus` returns `Ok(None)`) | **covered** if reached: pre-open `.gitmodules` lstat (regular, 1 MiB, worktree root); gix additionally skips symlinks. `fs::read` on a FIFO would block, hence the pre-open non-regular rejection is load-bearing |
| G11 | `gix/src/repository/worktree.rs:worktrees` (`read_dir(common_dir/worktrees)`), `gix/src/worktree/proxy.rs:lock_reason` (`fs::read(<admin>/locked)`, unbounded, `locked` is not on the pre-open list), `Proxy::base` (reads `<admin>/gitdir` via D1) | admin-dir names, `locked`, `gitdir` | only via `Repository::worktrees()` / `worktree_proxy_by_id` | **unreachable** by construction: §4.1 enumerates `.git/worktrees/*` itself and opens each admin dir through the hardened open. **Do not call** `worktrees()`, `Proxy::*` (P20/P21 obligation) |
| G12 | `gix/src/repository/mailmap.rs:open_mailmap*` | `.mailmap`, `mailmap.file` | n/a | **unreachable**: feature `mailmap` not compiled |
| G13 | `gix/src/filter.rs:worktree_file_to_object` (`File::open`, `read_link`) | worktree files for hashing | public API only; no gix-internal status path calls it | **unreachable** from the spec'd flow. P15a re-checks gix-status / gix-diff worktree reads |
| G14 | `gix/src/config/file_mut.rs:FileTransaction::open`, `repository/branch.rs`, `create.rs`, `init.rs`, `clone/*`, `remote/*`, `commit/sign.rs`, `repository/config/mod.rs:editor_command`, `repository/impls.rs:write_stream*` / `repository/object.rs:write_blob_stream` (`io::copy` from a caller-supplied reader), `repository/worktree.rs:worktree_archive` | write, init, clone/fetch, signing, editor spawn paths | none from open / refs / status | **unreachable**: mutation / network / spawn APIs; `worktree-archive` and network features not compiled. `config/snapshot/access.rs:commit` / `commit_inner` was checked: it calls `reread_values_and_clear_caches_replacing_config` (in-memory) and never `FileTransaction::open`, so stripping via `config_snapshot_mut()` does no file I/O |
| G15 | `gix/src/repository/location.rs:set_workdir` (`read_dir`), `gix/src/open/repository.rs:check_safe_directories` (`canonicalize`), `gix/src/interrupt.rs:Read::read` (reader wrapper) | directory handles / path resolution / wrapper | n/a | **not a content read** |
| G16 | `gix/src/shallow.rs`, `gix/src/repository/index.rs`, `gix/src/worktree/mod.rs` (snapshot storages) | snapshot wrappers; the reads are in `gix-shallow` / `gix-index` | lazily, from rev-walk / status | handed to **P15b** (`gix-shallow`: `<common_dir>/shallow`) and **P15a** (`gix-index`) |
| G17 | `gix/src/open/repository.rs:open_with_environment_overrides` | env `GIT_DIR` / `GIT_WORK_TREE` | n/a | **unreachable**: rollcall calls `open_opts` only |

### `gix-config` 0.61.0

| # | Anchor | What it reads | Reachable from | Verdict |
|---|---|---|---|---|
| C1 | `gix-config/src/file/includes/mod.rs:insert_includes_recursively` | files named by `include.path` / `includeIf`, `File::open` + `io::copy` | only if includes are followed | **unreachable**: `no_follow()` has `max_depth: 0` on both config load paths (see confirmations); FIFO `include.path` is never opened |
| C2 | `gix-config/src/file/init/from_paths.rs:from_paths_metadata_buf` | system / global / user config files | open (called by `init.rs:load`) | **unreachable as a read**: iterator is empty under isolated (G6) |
| C3 | `gix-config/src/file/init/from_paths.rs:from_path_no_includes` | one config file | `gix/src/repository/branch.rs` (delete branches), `clone/fetch/util.rs` | **unreachable**: mutation / clone paths |
| C4 | `gix-config/src/file/init/from_env.rs:from_env` | `GIT_CONFIG_COUNT` etc. (env only, then includes) | open, only if `use_env` | **unreachable**: `use_env=false` |
| C5 | `gix-config/src/file/init/comfort.rs:from_globals`, `from_git_dir`, `from_environment_overrides` | global config / `<git_dir>/config` | not called by `gix` 0.88 | **unreachable**: no caller in the locked `gix` sources |
| C6 | `gix-config/src/file/init/mod.rs:from_bytes_owned` | parses the already-read buffer | open | not a file read; memory is bounded by the 1 MiB `config` / `config.worktree` cap (parse-event overhead is a constant factor of that) |

### `gix-ref` 0.68.0

| # | Anchor | What it reads | Reachable from | Verdict |
|---|---|---|---|---|
| R1 | `gix-ref/src/store/file/find.rs:ref_contents` (`File::open` + `read_to_end`, follows symlinks, blocks on FIFO) | one loose ref file by full name: `HEAD`, `refs/**`, and (see U-2) names outside `refs/` | open (`HEAD`), refs (`head()`, `find_reference`, symref follow) | **covered** for `HEAD` (4 KiB) and every `refs/**` file (regular, 4 KiB, no symlinks; pre-open scan of every entry under `refs/`, 10,000-entry stop). **UNCOVERED (U-2)** for symbolic targets that map outside `refs/` |
| R2 | `gix-ref/src/store/file/overlay_iter.rs:convert_loose` (`File::open` + `read_to_end`) | each loose ref returned by iteration | refs iteration (`references()`, replace-refs at open) | **covered**: same as R1 (`refs/**` only, because iteration roots are `refs/`) |
| R3 | `gix-ref/src/store/file/loose/iter.rs:SortedLoosePaths::next` (directory walk via `gix-features` `walkdir_sorted_new`; default `walkdir` does not follow symlinks; only `file_type().is_file()` entries are returned) | directory entries under `<git_dir>/refs` and `<common_dir>/refs` (no content) | refs iteration | **covered**: 10,000-entry stop. Note the walk skips symlinks while R1 follows them; both are bounded by the pre-open symlink rejection |
| R4 | `gix-ref/src/store/packed/buffer.rs:open` (`fs::read` if `<= 32 KiB` mmap threshold, else `mmap`) and `open_with_backing` | `<common_dir>/packed-refs` | open (via `find`), refs, peel | **covered**: pre-open `packed-refs` lstat, 64 MiB. `open_with_backing` re-serialises an *unsorted* file into a second in-memory buffer plus an entry `Vec`, so peak RSS is a small multiple of 64 MiB (note N3). The common-dir location is U-1 |
| R5 | `gix-ref/src/store/file/packed.rs:assure_packed_refs_uptodate` / `open_packed_buffer` | stat of `packed-refs`, then R4 | every `find`, `iter`, peel | **covered** (stat only, then R4) |
| R6 | `gix-ref/src/store/file/raw_ext.rs:follow_packed` -> `find.rs:find_one_with_verified_input` -> `find_inner` | follows `ref:` targets, up to 5 hops | `head_id`, `peel_to_id`, `Reference::follow`, upstream / tracking-ref lookup | **UNCOVERED (U-2)** |
| R7 | `gix-ref/src/store/file/loose/reflog.rs:reflog_iter_inner` / `reflog_iter_rev_inner` (`File::open` + `read_to_end` / `read_exact`), `gix-ref/src/store/file/log/iter.rs:reverse` | `logs/<ref>` (unbounded; `logs/` is not on the pre-open list) | only reflog APIs (`Head::log_iter`, `Reference::log_iter`, `rev_parse` `@{n}`) | **unreachable**: not called by open / find / iteration / peel; §3.3 fields do not need reflogs. **Do not call** reflog APIs or `rev_parse` with `@{}` (P20/P21 obligation) |
| R8 | `gix-ref/src/store/file/overlay_iter.rs:iter_pseudo` (via `Platform::pseudo`) | every `*HEAD` file in the git-dir root (`ORIG_HEAD`, `FETCH_HEAD`, ...), unbounded | only if `pseudo()` is called | **unreachable**; **do not call**. The same names are reachable through U-2 |
| R9 | `gix-ref/src/store/file/transaction/*`, `store/packed/transaction.rs`, `loose/reflog.rs:reflog_create_or_append` | write / remove | none | **unreachable**: rollcall performs no ref edits |
| R10 | `gix-ref/src/store/general/init.rs:at_opts` (`read_dir(git_dir)`) | directory listing only | `#[expect(dead_code)]`; open uses `file::Store::at_opts` | not a content read |

### `gix-discover` 0.56.0

| # | Anchor | What it reads | Reachable from | Verdict |
|---|---|---|---|---|
| D1 | `gix-discover/src/path.rs:read_regular_file_content_with_size_limit` (called by `from_plain_file`, `from_plain_file_relative_to_file`, `from_gitdir_file`) | `<git_dir>/commondir`, `<git_dir>/gitdir`, and a `.git` *file*; 64 KiB cap applied **after** `File::open` | open (`is_git` and `open_from_paths` both read `commondir`; `is_git` reads `gitdir` for admin dirs) | `commondir` / `gitdir`: **covered** by pre-open lstat (regular, non-symlink, 1 MiB): this rejects the FIFO and `/dev/zero`-symlink cases that gix's post-open cap cannot. `.git` file: see D3 |
| D2 | `gix-discover/src/is.rs:git_with_metadata` -> `gix_ref::file::Store::find_loose("HEAD")` (R1) | `<git_dir>/HEAD` | open (`is_git`, called once or twice per open) | **covered**: pre-open `HEAD`, 4 KiB. Other calls are `exists()` / `is_dir()` / `metadata()` (no content) |
| D3 | `gix-discover/src/path.rs:from_gitdir_file` | the worktree-root `.git` regular file | `open_opts` only if it is handed a worktree root or a `.git` file (it then probes `<path>/.git`) | **covered** by §4.1 (`.git` regular file read <= 4 KiB with `O_NOFOLLOW` and `O_NONBLOCK`, `fstat`-checked) if it is reached at all; **constraint C-1**: P19/P21 should pass the resolved git dir with `open_path_as_is(true)` so gix never re-reads `.git` |
| D4 | `gix-discover/src/upwards/*` (`probe_repository`, `is_git` per ancestor) | ancestor `.git` entries | only `gix::discover*` | **unreachable**: rollcall never calls `discover` |

### `gix-path` 0.13.0

| # | Anchor | What it reads | Reachable from | Verdict |
|---|---|---|---|---|
| P1 | `gix-path/src/realpath.rs:realpath_opts` (`read_link`, bounded by `MAX_SYMLINKS`) | symlink target strings | open (`check_safe_directories`, `worktree_dir_from_repository_config`) | **not a content read**; bounded |
| P2 | `gix-path/src/env/mod.rs:shell_command`, `exe_invocation`; `env/git/mod.rs:git_cmd` (spawns `git`) | subprocess output | only `installation_config()`, `system_config()`, `core_dir()`, `installation_program()`; open reaches none of them: `gix/src/path.rs:install_dir` is `std::env::current_exe()`, and `source_path` returns before `storage_location` | **unreachable from open** (spawn analysis is P15c). No other file reads in this crate |

### `gix-sec` 0.15.0

| # | Anchor | What it reads | Reachable from | Verdict |
|---|---|---|---|---|
| S1 | `gix-sec/src/identity.rs:is_path_owned_by_current_user` (`symlink_metadata`) | inode owner only | open (`Trust::from_path_ownership`) | **not a content read**. No other read sites in the crate |

### `gix-fs` 0.23.0

| # | Anchor | What it reads | Reachable from | Verdict |
|---|---|---|---|---|
| F1 | `gix-fs/src/snapshot.rs:recent_snapshot` / `force_refresh` | mtime via a caller closure; the read is the closure (R4, `submodule`, `index`, `shallow`) | open / refs / status | wrapper, no read of its own |
| F2 | `gix-fs/src/read_dir.rs:read_dir` (wraps `gix_features::fs::read_dir`) | directory entries | `gix-dir` (status) | **P15a**; not on open / refs paths |
| F3 | `gix-fs/src/capabilities.rs:probe`, `probe_dir`, `probe_file_mode`, `probe_ignore_case` | *writes* probe files into the git dir | `gix/src/create.rs` (init) only; `access.rs:fs_capabilities` derives from config and does not probe | **unreachable** from open |
| F4 | `gix-fs/src/dir/remove.rs`, `dir/create.rs`, `symlink.rs` | mutation | none | **unreachable** |

## UNCOVERED findings (resolved by spec revision 11)

- U-1 → covered: trust rule (b) checked before open from rollcall's own read of `commondir`; pre-open list also runs on linked admin dirs and the resolved common dir (§4.1, §4.4 rev 11).
- U-2 → covered: all-caps (`[A-Z_]+`) entries in the git dir, common dir and each `worktrees/<name>/`, plus `worktrees/<name>/refs/**`, pre-open checked as loose refs (§4.4 rev 11).
- U-3 → covered: `core.excludesFile` stripped and always overridden (user file, else `/dev/null`) (§4.4 rev 11).

Each one needs an author decision on a §4.4 revision. §4.4 is not weakened here; the proposed checks only add coverage.

### U-1: `commondir` redirects every common-dir read to a directory the pre-open checks never inspect

- **What it reads (all unbounded, symlink-following):** `<common_dir>/config` (`gix/src/config/cache/incubate.rs:load_config`), `<common_dir>/packed-refs` (`gix-ref/src/store/packed/buffer.rs:open`, path from `gix-ref/src/store/file/packed.rs:packed_refs_path` = `common_dir_resolved()/packed-refs`), `<common_dir>/refs/**` (`overlay_iter.rs:convert_loose`, `find.rs:ref_contents` for `refs/heads|tags|remotes|...`), `<common_dir>/info/exclude` and `<common_dir>/info/attributes` (`gix/src/repository/attributes.rs` passes `self.common_dir()`), and, for P15b, `<common_dir>/shallow`, `objects/info/alternates`, `objects/**`.
- **Reachable from:** `open_opts(git_dir)`. `gix/src/open/repository.rs:open_from_paths` reads `<git_dir>/commondir` (`gix-discover` `from_plain_file`) and sets `common_dir = git_dir.join(<content>)` whenever the file exists, for **any** repo, not only linked worktrees (`known_common_dir` is `None` from `open_opts`). For a linked worktree the git dir passed is `worktrees/<name>/`, so `config`, `packed-refs`, `refs/` do not exist there and every such read lands in the redirected directory.
- **Why §4.4 does not cover it:** the pre-open list says "in the git dir, `lstat` `config`, `HEAD`, `packed-refs`, ..." and caps the `commondir` *file* at 1 MiB, but says nothing about repeating the checks at the directory `commondir` resolves to. Trust rule (b) ("the repo's common dir resolves to the `.git` of the repo that enumerated it") runs after the hardened open, i.e. after these reads. The `commondir-escape` fixture (§10.2) only points at another fixture repo, which is itself pre-open checked when discovered; an attacker-owned plain directory with `objects/` and `refs/` (enough to pass `is_git`) and `config` -> `/dev/zero` is not that. Contrast the alternates chain, which §4.4 explicitly follows pre-open.
- **Check that would cover it (author's choice):** (a) resolve `commondir` pre-open (the file is already vetted: regular, <= 1 MiB) and refuse (`untrusted git dir`) unless it resolves to the `.git` of the enumerating repo, i.e. trust rule (b) moved before the open; the enumerating repo's own git dir has already passed pre-open. Or (b) run the full pre-open list (`config`, `packed-refs`, every `refs/` entry, `info/exclude`, `info/attributes`, `shallow`, `objects/info/alternates`, `commit-graph-chain`) on the resolved common dir as well as the git dir. P18's per-file tests would then need a linked-worktree variant.

### U-2: symbolic-ref targets can name files outside `refs/`, in the git-dir root, the common-dir root, or another admin dir

- **What it reads:** `<git_dir>/<NAME>` for any all-`[A-Z_]` single-component name; `<common_dir>/<NAME>` for `main-worktree/<NAME>`; `<common_dir>/worktrees/<n>/<NAME>` for `worktrees/<n>/<NAME>`. `File::open` + `read_to_end` in `gix-ref/src/store/file/find.rs:ref_contents`: unbounded, follows symlinks, blocks on a FIFO. None of these names is on the §4.4 pre-open list (only `HEAD` and `refs/**` entries are; `worktrees/<name>/` gets `config.worktree` only).
- **Chain (each hop confirmed in the locked source):** a loose ref file (`HEAD`, or any `refs/**` file, both pre-open-checked and <= 4 KiB) contains `ref: EVIL` -> `gix-ref/src/store/file/loose/reference/decode.rs:parse` returns `UnvalidatedPath` -> `Target::try_from` validates with `gix_validate::reference::name` (`gix-validate/src/reference.rs:validate`, `Mode::Complete`: a name with no slash is accepted if it is all uppercase / `_`; a name with a slash is accepted regardless of case) -> `raw_ext.rs:follow_packed` -> `try_find_packed` -> `find.rs:find_one_with_verified_input` -> `find_inner(..., consider_pseudo_ref = true)` -> `name.rs:construct_full_name_ref` (`looks_like_full_name` is true for an all-caps name, `main-worktree/...`, `worktrees/...`, so **no `refs/` prefix is added**; every other name is prefixed `refs/` and stays inside the checked tree) -> `to_base_dir_and_relative_name` (`PseudoRef` -> `git_dir`; `MainPseudoRef` -> `commondir/<NAME>`; `LinkedPseudoRef` -> `commondir/worktrees/<n>/<NAME>`) -> `ref_contents`.
- **Reachable from:** `Repository::head()` followed by `head_id()` / `peel_to_commit()` / `peel_to_id()`, `Reference::follow`, and any `find_reference` on a symbolic ref (so P20's HEAD / default-branch resolution). A second route: `Repository::branch_remote_tracking_ref_name` (`gix/src/repository/config/branch.rs:matching_remote`) maps through `remote.<n>.fetch` refspecs and validates the destination only as a full ref name, so `+refs/heads/main:EVIL` yields the tracking ref `EVIL`, which a later `find_reference` reads from the git-dir root (the `upstream` field, §3.3).
- **Effect:** an unpacked tree with `.git/HEAD` = `ref: EVIL` and `.git/EVIL` -> `/dev/zero` exhausts memory (`read_to_end`); `EVIL` as a FIFO hangs the worker until the deadline abandons it. The deadline does not bound the memory case.
- **Check that would cover it (author's choice):** (a) extend the pre-open list with every entry in the git-dir root whose name matches `^[A-Z_]+$` (regular, non-symlink, 4 KiB, the same rule as loose refs) and the same in the common-dir root and in each `worktrees/<name>/` admin dir (plus `worktrees/<name>/refs/**`); combined with U-1 this becomes "root-level pseudo-ref entries of every directory gix will read refs from". Or (b) never let gix follow: resolve `HEAD` and tracking refs in rollcall (read the pre-checked file, accept only `refs/heads/*`, `refs/remotes/*`, `refs/tags/*` targets, reject the rest as `untrusted git dir`) and use `find_reference` only with full `refs/...` names.

### U-3: repo-local `core.excludesFile` is read uncapped when the user has no `core.excludesFile` override

- **What it reads:** the path in `core.excludesFile`, via `gix/src/config/cache/access.rs:assemble_exclude_globals` -> `gix-ignore` `Search::from_git_dir` -> `gix-glob/src/search/pattern.rs:read_in_full_ignore_missing` (`File::open` follows symlinks, `read_to_end` unbounded). Arbitrary absolute path chosen by the repo.
- **Reachable from:** `index_worktree_status` untracked / ignored detection (any `Repository::attributes()` / `excludes()` stack build). The value is honoured when the repo-local config section is trusted (`gix/src/config/mod.rs:is_trusted`: `Trust::Full`, i.e. any tree owned by the current user, such as an unpacked tarball).
- **Why §4.4 does not cover it:** §4.4 strips `core.attributesFile`, `core.worktree`, `core.sshCommand`, `core.hooksPath`, `core.fsmonitor`, but **not `core.excludesFile`**. Its only protection is the rollcall-read user value passed as a config override, which wins over the repo-local value (Source::Api is appended last and `path_filter` takes the last match). The plan (P18) says an unreadable / oversized `~/.gitconfig` gives `None`, and a user with no `core.excludesFile` also gives `None`; with no override the repo-local value is used. Covered only for users who have set it.
- **Check that would cover it:** add `core.excludesFile` to the stripped keys **and** always set it via override (a sentinel path, e.g. a nonexistent file, when the user has none), or strip it in the same `config_snapshot_mut()` pass before setting the user's value. Either way the repo-local value can no longer reach gix-ignore.

## Constraints on later phases and observations (not UNCOVERED, but the audit relies on them)

- **C-1 (P19/P21):** pass the resolved git dir with `open_path_as_is(true)`; never a worktree root or a `.git` file (D3). Otherwise gix probes `<path>/.git` first.
- **C-2 (P20/P21), do-not-call list:** `Repository::worktrees()`, `worktree_proxy_by_id`, `Proxy::lock_reason` / `base` (G11); reflog APIs, `rev_parse` with `@{}` (R7); `Platform::pseudo()` / `iter_pseudo` (R8); `open_mailmap*` (not compiled); `filter::Pipeline::worktree_file_to_object` (G13).
- **N1:** spec §4.4 says "`Config::isolated()` permissions"; the plan (P19) says `open::Options::isolated()`. Only the latter denies environment access and isolates attributes. The audit's "unreachable" verdicts for G6, G7, G8(c), G9(c) assume the full `Options::isolated()`; a P19 test asserting `Permissions::is_isolated()` on the options used would pin this.
- **N2:** `assemble_exclude_globals` and `assemble_attribute_globals` read `info/exclude` and `info/attributes` from `common_dir()`, not the admin dir; the pre-open location for these two files therefore follows the common dir (see U-1).
- **N3:** an unsorted `packed-refs` at the 64 MiB cap is parsed into an entry `Vec` and re-serialised into a second buffer (`packed/buffer.rs:open_with_backing`), so the RSS budget (§10.2: < 512 MB) should account for a small multiple of 64 MiB.
- **N4 (P15a hand-off):** the read behind G8(a)/(b) and G9(b), and `.gitignore` / `.gitattributes` reads (accepted exception for subdirectories only), are in `gix-glob` / `gix-ignore` / `gix-attributes` / `gix-worktree`; trace the root-level `.gitignore` / `.gitattributes` (pre-open 1 MiB) separately from subdirectory ones (documented limitation).
- **N5 (P15b hand-off):** `gix_odb::Store::at_opts` runs at open but reads nothing; `objects/info/alternates`, packs, `shallow`, `commit-graph-chain` are read lazily (P15b), all from the common dir (U-1).

## Maintenance

Re-run the crate-list command on every gix bump and diff it against the list above; re-audit any crate whose version changed, and re-check the anchors (they are function names, not line numbers, so a rename shows up as a failed grep).
