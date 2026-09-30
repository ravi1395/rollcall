# gix 0.88 read-site audit

**UNCOVERED count: 0 open.** Part 1 (P14): 0 after spec revision 11 (was 3 at first audit: U-1, U-2, U-3 below — each now covered by a revision 11 check; rows keep their original analysis and carry a `→ covered (rev 11)` note). Part 2a (P15a): 0 after spec revision 12 (was **6 open**: U-4 .. U-9, "UNCOVERED findings from P15a" below), each reproduced or traced in the locked sources; §4.4 is not weakened, every proposed check only adds coverage. Parts 2b and 2c: not started, not counted.

| Part | Phase | State |
|---|---|---|
| 1: config, refs, open | P14 | written 2026-09-30; U-1, U-2, U-3 covered by spec rev 11 |
| 2a: index, status, worktree reads | P15a | written 2026-09-30 against spec rev 11; U-4 .. U-9 covered by spec rev 12 (in-process guards, §4.4) |
| 2b: object store and history | P15b | not started |
| 2c: spawn sites and API confirmation | P15c | not started |

Basis: spec `docs/superpowers/specs/2026-09-29-rollcall-design.md` §4.4 (rev 10 for part 1, rev 11 for part 2a), plan rev 5 § Phase 14 and § Phase 15a. Sources read: the exact locked crate versions under `~/.cargo/registry/src/index.crates.io-*/`; nothing else (part 2a also read `memmap2` 0.9.11 for IX1).

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

- `gix-worktree` (P15b remaining) owns the ignore/attribute `Stack` that `index_worktree_status` drives, i.e. the subdirectory `.gitignore` / `.gitattributes` reads of §4.4's documented limitation. It belongs with P15a's reachability trace. **Done in P15a** (rows WT1 .. WT3; the assignment table is unchanged, P15b need not re-trace those rows).
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
| G13 | `gix/src/filter.rs:worktree_file_to_object` (`File::open`, `read_link`) | worktree files for hashing | public API only; no gix-internal status path calls it | **unreachable** from the spec'd flow. P15a re-checked gix-status / gix-diff worktree reads: rows ST2, ST3, ST5, DF2 and U-6 |
| G14 | `gix/src/config/file_mut.rs:FileTransaction::open`, `repository/branch.rs`, `create.rs`, `init.rs`, `clone/*`, `remote/*`, `commit/sign.rs`, `repository/config/mod.rs:editor_command`, `repository/impls.rs:write_stream*` / `repository/object.rs:write_blob_stream` (`io::copy` from a caller-supplied reader), `repository/worktree.rs:worktree_archive` | write, init, clone/fetch, signing, editor spawn paths | none from open / refs / status | **unreachable**: mutation / network / spawn APIs; `worktree-archive` and network features not compiled. `config/snapshot/access.rs:commit` / `commit_inner` was checked: it calls `reread_values_and_clear_caches_replacing_config` (in-memory) and never `FileTransaction::open`, so stripping via `config_snapshot_mut()` does no file I/O |
| G15 | `gix/src/repository/location.rs:set_workdir` (`read_dir`), `gix/src/open/repository.rs:check_safe_directories` (`canonicalize`), `gix/src/interrupt.rs:Read::read` (reader wrapper) | directory handles / path resolution / wrapper | n/a | **not a content read** |
| G16 | `gix/src/shallow.rs`, `gix/src/repository/index.rs`, `gix/src/worktree/mod.rs` (snapshot storages) | snapshot wrappers; the reads are in `gix-shallow` / `gix-index` | lazily, from rev-walk / status | handed to **P15b** (`gix-shallow`: `<common_dir>/shallow`) and **P15a** (`gix-index`, done: rows IX1 .. IX8, U-4, U-8) |
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
| F2 | `gix-fs/src/read_dir.rs:read_dir` (wraps `gix_features::fs::read_dir`) | directory entries | `gix-dir` (status) | **P15a**, traced as row DR1; not on open / refs paths |
| F3 | `gix-fs/src/capabilities.rs:probe`, `probe_dir`, `probe_file_mode`, `probe_ignore_case` | *writes* probe files into the git dir | `gix/src/create.rs` (init) only; `access.rs:fs_capabilities` derives from config and does not probe | **unreachable** from open |
| F4 | `gix-fs/src/dir/remove.rs`, `dir/create.rs`, `symlink.rs` | mutation | none | **unreachable** |

## Part 2a (P15a): index, status, worktree reads

Written against spec §4.4 **revision 11** and the locked versions in the crate list (`gix-index` 0.56.0, `gix-status` 0.35.0, `gix-dir` 0.30.0, `gix-glob` 0.28.0, `gix-attributes` 0.36.0, `gix-ignore` 0.23.0, `gix-diff` 0.68.0, `gix-filter` 0.35.0, `gix-object` 0.65.0; also `gix-worktree` 0.57.0, see the assignment flag above). Crates P14 assigned to P15a: exactly those nine, no others.

### Method and principles (P15a additions)

1. **Candidate enumeration.** Over `src/` of the nine crates plus `gix-worktree`, tests excluded:
   `grep -rnE 'fs::read|read_to_end|read_to_string|io::copy|\.lines\(\)|File::open|OpenOptions|read_link|read_dir|std::fs::|read_exact|memmap|BufReader|open_options_no_follow|\.open\(|Command::new|fs::File|symlink_metadata|\.metadata\(\)'`, then every hit traced to an entry point or to "unreachable". Memory-growth paths that are not a file read (recursive decoders, parsers whose output is larger than their input, similarity diffs) were found by reading the decoders and callers reachable from the two entry points; they are listed in the tables too.
2. **Entry points traced.** `Repository::open_index` / `try_index` / `index` (`gix/src/repository/index.rs`); `Repository::index_worktree_status` (`gix/src/status/index_worktree.rs`) with `Options { dirwalk_options: Some(..), rewrites: None }`, `SubmoduleStatus` returning `Ok(None)`, and `FastEq` as the `CompareBlobs` (what gix's own `status::Platform` passes; §4.4 does not name one, see C-3); `Repository::tree_index_status` (`gix/src/status/tree_index.rs`) with `TrackRenames::Given(Rewrites { copies: None, percentage: Some(0.5), limit: 1000, track_empty: false })`, which reaches `index_from_tree` and `diff_resource_cache_for_tree_diff`.
3. **Principle: the index and the worktree are both attacker input, and the index decides what gets read.** Mode, stat, flags, extensions and blob ids of `.git/index` are attacker-chosen (the pre-open list only bounds its byte size). gix reads a tracked path's worktree content based on the *index-declared* mode, not the lstat kind; the stat size shortcut is defeated by `stat.size == 0` with a non-empty blob id. So "the worktree file is a symlink / FIFO / huge" cannot be assumed away, and any read gated on index data is attacker-selected.
4. **Principle: a "covered" verdict for growth needs a bound on the *retained* size, not on the input read.** Several decoders retain 17x to 70x their input (measured below). A cap on bytes read (pre-open list, `gitoxide.objects.allocLimit`) is only a cover if the parse is proportional to it or the parse itself is under the limit.
5. **Probe evidence.** The findings marked *reproduced* were run, not only read: a scratch crate (outside the repo) depending on `gix =0.88.0` with rollcall-git's feature set (`sha1`, `status`, `revision`), the workspace `Cargo.lock`, `open::Options::isolated()`, and `status()` -> `into_index_worktree_iter` with `Submodule::Given { ignore: All, check_dirty: false }` (the same `index_as_worktree_with_renames` path, `FastEq`, default dirwalk options). Fixtures were built with the gix API (`write_blob`, `edit_tree`, `index_from_tree`, `File::write`), a 300 MiB sparse file as the "big" target, FIFOs from `mkfifo`, peak RSS from `/usr/bin/time -l`, a 15 s alarm for blocking cases, macOS, debug build unless noted. Results are in "Probe results". They do not replace P18/P21 fixtures; the fixture names suggested per finding are chosen to turn each result into a gating test.

### The worktree-file read decision: what bounds reading a tracked file

`index_worktree_status` reads a tracked path's worktree content only if all of these hold (`gix-status/src/index_as_worktree/function.rs:process` and `compute_status`): the entry has none of `UPTODATE`, `SKIP_WORKTREE`, `ASSUME_VALID`, `FSMONITOR_VALID`; the pathspec lets it through; the path resolves through `SymlinkCheck::verified_path` (`gix-status/src/stack.rs:Delegate::push` does `symlink_metadata` on every leading component and rejects a symlinked directory, so a tracked path behind a symlinked directory is reported `Removed` and never opened); `live_metadata` (`lstat`, never follows) says it is not a directory; the mode comparison below does not return a type change; the stat comparison does not match (attacker-controlled: index stat is theirs); and `FastEq` does not short-circuit on size (`u64::from(entry.stat.size) != worktree_file_size`, skipped when `stat.size == 0` and the id is not the empty blob, again attacker-controlled). What happens next depends on the index mode and the lstat kind (`gix-index/src/entry/mode.rs:change_to_match_fs_with_values`; `is_executable` is `S_IFREG && S_IXUSR`):

| Index mode | Worktree lstat kind | gix decision | Content opened? |
|---|---|---|---|
| 100644 | symlink, FIFO, socket, device | `Mode::FILE if !is_file` -> `Change::Type` | **no** |
| 100644 or 100755 | regular file | no type change | yes (below) |
| 100755 | symlink, FIFO, socket, device | no arm matches a type change: `FILE_EXECUTABLE if executable_bit && !is_executable` gives `ExecutableBit` (or, with `core.fileMode=false`, `_ => None`), and status **continues to the content comparison** | **yes, and it follows a symlink** (`File::open`) and opens the FIFO |
| 120000 | symlink | `Mode::SYMLINK if is_symlink -> None`; content = `std::fs::read_link` (no follow), <= `PATH_MAX` | `readlink` only |
| 120000 | anything else, `core.symlinks` true | `Change::Type` | no |
| 120000 | regular file, `core.symlinks=false` (repo-local config is honoured) | none | yes, as a regular file |
| 160000 | directory | `SubmoduleStatus::status` -> `Ok(None)` | no |
| 160000 | not a directory | `Change::Type` | no |
| any | directory | `Removed` (or the submodule row above) | no |

So, for the two cases asked about. **Symlink to `/dev/zero`:** gix does `readlink` (never follows) when the index says symlink; reports a type change without opening when the index says 100644; but when the index says 100755 it follows the link (`std::fs::File::open` in `stream_worktree_file`). With no built-in filter it then hashes exactly `metadata().len()` bytes, where `len` is the lstat length of the *symlink* (its target string, <= 4,095 bytes) through a fixed 64 KiB buffer (`gix-object/src/lib.rs:compute_stream_hash`), so nothing grows. With any built-in filter (repo-local `core.autocrlf`, or `.gitattributes` `ident` / `text` / `eol` / `working-tree-encoding`) `gix-filter` `convert_to_git` does an unbounded `read_to_end` first (**U-6**). **FIFO:** never opened for 100644 / 120000 / 160000; for 100755 it is opened with plain `File::open`, which blocks with no writer; nothing bounds it but the per-repo deadline (the abandoned worker thread is never joined), and the interrupt flag is not consulted inside a blocking `open`. A regular tracked file of any size is streamed in constant memory when unfiltered and read whole when filtered (**U-6**); its hash is not interruptible (`HashEq`, which `FastEq` calls, passes `AtomicBool::default()`), so time is bounded only by the deadline.

### `gix` 0.88.0 glue reachable from status (P14 crate, traced here)

| # | Anchor | What it reads | Reachable from | Verdict |
|---|---|---|---|---|
| X1 | `gix/src/status/index_worktree.rs:index_worktree_status` | nothing itself: builds the attribute + ignore `Stack` (`Source::WorktreeThenIdMapping` for attributes, `WorktreeThenIdMappingIfNotSkipped` for ignores), a `diff::resource_cache` with `new_root: Some(workdir)`, then calls `gix_status::index_as_worktree_with_renames` | status | reads are in rows ST*, DR*, GL*, WT*; the resource cache is built but only used when `rewrites` is `Some` (ST5, DF2) |
| X2 | `gix/src/status/tree_index.rs:tree_index_status` | `index_from_tree(tree_id)` (IX8), `diff_resource_cache_for_tree_diff` (X3), `gix_diff::index` | tree_index_status | expansion **covered** by the HEAD tree bound; rename similarity **U-9** |
| X3 | `gix/src/repository/diff.rs:diff_resource_cache` | `index_or_load_from_head_or_empty` (`gix/src/repository/index.rs`): `try_index` (IX1) or, when `.git/index` is absent, `index_from_tree(HEAD tree)` **again**; attributes via `attributes_only(.., Source::IdMapping)` (index blobs) | tree_index_status when `rewrites` is `Some` (always, per §4.4) | HEAD-tree expansion **covered** only because the tree is the same one the bound walked (constraint C-7); index-blob attributes **U-7** |
| X4 | `gix/src/repository/index.rs:open_index` | `gix_index::File::at(self.index_path(), .., alloc_limit_bytes: self.config.alloc_limit_bytes)`; `index.skipHash` and `index.threads` from repo-local config | status | **covered** (IX1, IX4) |
| X5 | `gix/src/config/cache/access.rs:big_file_threshold` | `core.bigFileThreshold` from the resolved config, default 512 MiB, **`0` disables the check**; repo-local, attacker-controlled, not stripped by §4.4 | tree_index_status (diff pipeline options) | drives U-9 |
| X6 | `gix/src/dirwalk/options.rs` (defaults) | `recurse_repositories: false`, `classify_untracked_bare_repositories: false` | status dirwalk | the default `recurse_repositories: false` is what turns on the nested-repo probe of **U-5** |

### `gix-index` 0.56.0

| # | Anchor | What it reads | Reachable from | Verdict |
|---|---|---|---|---|
| IX1 | `gix-index/src/file/init.rs:at` (`File::open` follows symlinks and blocks on a FIFO; `memmap2 map_copy_read_only`; then `gix_hash::bytes` over `metadata().len() - hash_len` unless `skip_hash`) | `<git_dir>/index`, or `worktrees/<name>/index` for a linked worktree | `Repository::open_index` / `try_index` (every status, and X3) | **covered**: pre-open `index` lstat (regular, non-symlink, 256 MiB, on the git dir and, rev 11, each admin dir). The mapping is a private copy of a file another process may truncate (N8) |
| IX2 | `gix-index/src/file/init.rs:at_or_default` | same read as IX1 | no caller on the status path (`try_index` calls `open_index`) | **unreachable**; same coverage as IX1 if it were called |
| IX3 | `gix-index/src/extension/link.rs:dissolve_into` -> `File::at` (IX1 read semantics) | `<dir of the index>/sharedindex.<checksum>`, name chosen by the index's `link` extension; recursion if the shared index has its own `link` | `open_index`, any index carrying a `link` extension (split index) | **UNCOVERED (U-8)**: not on the pre-open list |
| IX4 | `gix-index/src/decode/mod.rs:from_bytes`, `ensure_in_alloc_limit`, `vec_with_capacity`; `entries::max_entries_possible` | entry table and path backing decoded from the mapped bytes | `open_index` | **covered**: allocation limit (`gitoxide.objects.allocLimit` = 64 MiB reaches `decode::Options::alloc_limit_bytes` via `config.alloc_limit_bytes`) applied to `num_entries * size_of::<Entry>()` and to the path-backing estimate; pre-open 256 MiB bounds the input |
| IX5 | `gix-index/src/extension/tree/decode.rs:one_recursive`, `extension/untracked_cache.rs:decode_directory_block` (one stack frame per nesting level; no depth argument) | `TREE` and `UNTR` extensions | `open_index` | **UNCOVERED (U-4a)**: allocation is under the limit, recursion depth is not; stack overflow aborts the process (reproduced) |
| IX6 | `gix-index/src/extension/resolve_undo.rs:decode` (`Vec::new()`, no limit argument; `extension::decode::all` passes `alloc_limit_bytes` only to `tree` and `untracked_cache`) | `REUC` extension | `open_index` | **UNCOVERED (U-4b)**: about 17x amplification of the extension size (reproduced) |
| IX7 | `gix-index/src/extension/fs_monitor.rs:decode`, `extension/link.rs:decode`, `gix_bitmap::ewah::decode` (P15b crate), `extension/sparse.rs`, `end_of_index_entry`, `index_entry_offset_table` | `FSMN`, link bitmaps, markers | `open_index` | **covered**: retained size is 1x the extension bytes (`ewah::decode` allocates only after `split_at_checked` proved the words exist); bounded by the 256 MiB pre-open cap. `for_each_set_bit` walks up to `num_bits` (u32) bits: CPU only, no growth |
| IX8 | `gix-index/src/init.rs:from_tree::State::from_tree` (traversal via `gix_traverse::tree::depthfirst`, P15b) | HEAD tree objects from the object database, retained as one `Entry` plus full path per file; the path deque holds one full path per pending subtree | `Repository::index_from_tree` <- `tree_index_status`, X3 | **covered**: HEAD tree bound (1,000,000 entries, 128 MiB of path bytes, depth 4,096; the walked id is the id passed on) + allocation limit per object. Not interruptible; the bound is the only stop. Traversal internals: P15b / P15c |
| IX9 | `gix-index/src/file/verify.rs:verify_integrity` (`metadata`, `bytes_of_file`) | the index file again | not called by `open_index`, status or diff | **unreachable**; **do not call** (C-2) |
| IX10 | `gix-index/src/fs.rs:Metadata::from_path_no_follow` (`symlink_metadata`), `from_file` | metadata only | status | not a content read |
| IX11 | `gix-index/src/write.rs`, `File::write` | write | none in rollcall | **unreachable** (`index_worktree_status` returns `NeedsUpdate` info but never writes) |

### `gix-status` 0.35.0

| # | Anchor | What it reads | Reachable from | Verdict |
|---|---|---|---|---|
| ST1 | `gix-status/src/index_as_worktree/function.rs:compute_status` (`live_metadata` -> `Metadata::from_path_no_follow`) | `lstat` of the tracked path | status | not a content read; no follow |
| ST2 | `gix-status/src/index_as_worktree/function.rs:stream_worktree_file` (`impl ReadData for ReadDataImpl`) | (a) `std::fs::read_link` when the index mode is symlink and `core_symlinks` (or lstat says symlink on a symlink entry): no follow, <= `PATH_MAX`; (b) otherwise `std::fs::File::open(path)` (follows symlinks, blocks on a FIFO) then `Pipeline::convert_to_git`; (c) `find_blob` for the `eol` index-object callback | status, once per entry that reaches content comparison (matrix above) | (a) **covered** (bounded by the kernel). (b) **UNCOVERED (U-6)**. (c) **covered**: allocation limit (P15b to confirm enforcement in `gix-odb` / `gix-pack`) |
| ST3 | `gix-status/src/index_as_worktree/traits.rs:compare_blobs` (`FastEq`, `HashEq`) | drives ST2; unfiltered stream hashed through `gix_object::compute_stream_hash(len = lstat length)`; filtered `Buffer` / `Process` output hashed from memory; `stream.read_to_end(buf)` when the length is unknown | status | memory **covered** for the unfiltered stream (64 KiB buffer, exactly `len` bytes); filtered: U-6. Time is bounded only by the deadline (not interruptible, `HashEq` passes `AtomicBool::default()`), N9 |
| ST4 | `gix-status/src/stack.rs:Delegate::push` (`symlink_metadata` on every leading component) | metadata only | status | **covered** (rejects symlinked parent directories; not a read) |
| ST5 | `gix-status/src/index_as_worktree_with_renames/mod.rs:calculate_worktree_id` (`File::open`, `convert_to_git`, `compute_stream_hash`, `read_link`) | worktree files and links for dirwalk rewrite tracking | only `if options.rewrites.is_some()` (the tracker and the filter clone are both gated on it) | **unreachable** under §4.4 (index-worktree rename tracking is off, `rewrites: None`). If it were enabled, this read is U-6's class plus a plain `File::open` on every untracked file, so C-3 makes `rewrites: None` a hard constraint |
| ST6 | `gix-status/src/index_as_worktree_with_renames/mod.rs` dirwalk delegate (`emit` checks `should_interrupt`) | none | status | the interrupt flag stops the dirwalk (§4.4 deadline) |
| ST7 | `gix-status/src/fscache.rs` | `read_dir` / metadata batching, Windows only (`core.fscache` default is `cfg!(windows)`) | status on Windows | not a content read |

### `gix-dir` 0.30.0

| # | Anchor | What it reads | Reachable from | Verdict |
|---|---|---|---|---|
| DR1 | `gix-dir/src/walk/readdir.rs:recursive` -> `gix_fs::read_dir` (P14's F2) | directory entry names (`std::fs::read_dir`), streamed; `should_interrupt` checked at the top of every directory and in `emit` | dirwalk in `index_worktree_status` | **covered**: deadline / interrupt for time; memory is names only (delegate obligation C-5). Recursion depth is bounded by the kernel's `PATH_MAX` (at most ~2,048 levels of one-character names on Linux, ~512 on macOS); probe with 380 nested directories passes even with `RUST_MIN_STACK=65536` (debug), so a Linux fixture at max depth is a P21 test, not a finding |
| DR2 | `gix-dir/src/walk/classify.rs:maybe_upgrade_to_repository` -> `gix_discover::is_git` (`gix-discover/src/is.rs:git_with_metadata`: `.git` file via `from_gitdir_file`, then `<gitdir>/HEAD` via `gix_ref::file::Store::find_loose`, unbounded `read_to_end`, follows symlinks, blocks on a FIFO; also `commondir` and `gitdir`) | for **every untracked, non-ignored directory** and every tracked gitlink directory: `<dir>/.git`, `<dir>/.git/HEAD`, and whatever a `gitdir:` line points at | dirwalk (any status) | **UNCOVERED (U-5)** (reproduced) |
| DR3 | `gix-dir/src/walk/classify.rs:path`, `walk/function.rs` (`symlink_metadata`, `metadata`) | metadata only | dirwalk | not a content read |
| DR4 | `gix-dir/src/walk/classify.rs:path` -> `Stack::at_entry` (excludes) | drives rows GL1, WT1 | dirwalk | see those rows |

### `gix-glob` 0.28.0 (the reader), `gix-attributes` 0.36.0, `gix-ignore` 0.23.0

| # | Anchor | What it reads | Reachable from | Verdict |
|---|---|---|---|---|
| GL1 | `gix-glob/src/search/pattern.rs:read_in_full_ignore_missing` (`File::open` if `follow_symlinks`, else `gix_features::fs::open_options_no_follow` = `O_NOFOLLOW` only, no `O_NONBLOCK`; then `read_to_end`) <- `List::from_file` <- `search::add_patterns_file` | (a) `core.excludesFile`; (b) `<common_dir>/info/exclude`; (c) `<common_dir>/info/attributes`; (d) worktree-root `.gitignore` / `.gitattributes`; (e) subdirectory `.gitignore` / `.gitattributes` | status (`Stack::at_entry`, dirwalk excludes, attribute lookups) | (a) **covered**: stripped and overridden (user file, else `/dev/null`, which reads as empty); the user file was checked by rollcall but gix re-opens it by path (own file, not attacker-controlled). (b), (c) **covered**: pre-open 1 MiB, common dir. (d) **covered**: pre-open `.gitignore` / `.gitattributes` lstat in the worktree root (regular, 1 MiB), which also neutralises the "untracked `.gitignore` follows symlinks" rule at the root. (e) **accepted exception** (§4.4 "Documented limitation"); see N6 for two precision points |
| GL2 | `gix-glob/src/wildmatch.rs:match_recursive` (`RECURSION_LIMIT = 64`) | no read | pattern matching | **covered**: depth is capped at 64; a pattern that hits it logs an error and does not match. Not a read site, listed because every pattern file feeds it |
| GL3 | `gix-glob/src/search/pattern.rs:List::from_bytes`, `gix-ignore/src/parse.rs`, `gix-attributes/src/parse.rs` (`lines()` over the buffer), `gix-attributes/src/search/attributes.rs:add_patterns_file` / `add_patterns_buffer` (`MetadataCollection::update_from_list`) | parse of an in-memory buffer | every GL1 row and every index-blob source (WT1, WT2) | retained size is **40x to 70x** the input (reproduced), so a 1 MiB file is fine (**covered** by the pre-open caps, N7) and a 64 MiB blob is not (**U-7**) |
| AT1 | `gix-attributes/src/search/attributes.rs:add_patterns_file` (delegates to `gix_glob::search::add_patterns_file`) | none of its own | WT2 | see GL1 / GL3; `follow_symlinks` is passed `false` for `.gitattributes` (the caller's variable is misleadingly named `no_follow_symlinks = false`; `false` means "do not follow") and `true` for `info/attributes` |
| IG1 | `gix-ignore/src/search.rs:from_git_dir` (two `from_file` calls, `follow_symlinks = true`) | `core.excludesFile`, `<common_dir>/info/exclude` | `Repository::excludes` / `attributes` stack build (P14 G8) | **covered** as GL1(a), (b) |

### `gix-worktree` 0.57.0 (P15b crate, traced here per P14's flag)

| # | Anchor | What it reads | Reachable from | Verdict |
|---|---|---|---|---|
| WT1 | `gix-worktree/src/stack/state/ignore.rs:push_directory` | `<dir>/.gitignore` via GL1 with `follow_symlinks = ignore_file_in_index.is_err()`, i.e. **follows symlinks only when the file is untracked**; if absent, the index blob (`find_blob`) **only when the index entry carries `SKIP_WORKTREE`** (`gix-worktree/src/stack/state/mod.rs:id_mappings_from_index`) | dirwalk, one call per directory entered | worktree file: root **covered**, subdirectory **accepted exception**. Blob fallback: **U-7** (parse amplification; the blob read itself is under the allocation limit) |
| WT2 | `gix-worktree/src/stack/state/attributes.rs:push_directory` | `<dir>/.gitattributes` via GL1 (no follow); index blob (`find_blob`) as the source in `Source::IdMapping` / `IdMappingThenWorktree`, or as the fallback when the worktree file is absent in `WorktreeThenIdMapping`; `info/attributes` once at the root (follow) | attribute lookups: status (`at_entry` in `stream_worktree_file`), the rename resource cache (`Source::IdMapping`, X3) | worktree file: root **covered**, subdirectory **accepted exception**, `info/attributes` **covered**. Blob source: **U-7** |
| WT3 | `gix-worktree/src/stack/delegate.rs` (`create_dir`, `remove_file`, `symlink_metadata`) | checkout-mode stacks only | none: status builds `AttributesAndIgnoreStack` / `AttributesStack`, never the creating states | **unreachable** |

### `gix-diff` 0.68.0

| # | Anchor | What it reads | Reachable from | Verdict |
|---|---|---|---|---|
| DF1 | `gix-diff/src/blob/pipeline.rs:convert_to_diffable`, object-database branch (`try_header`, then `try_find(id, out)` into the reusable `out` buffer) | a blob by id; skipped (`Data::Binary`) when `header.size > large_file_threshold_bytes` and the attribute state does not decide | `tree_index_status` similarity (`WorktreeRoots::default()`, mode `ToGit`, so no `convert_to_worktree` / binary-to-text) | per-blob load **covered** by the allocation limit (64 MiB, P15b confirms the enforcement site). `large_file_threshold_bytes` is `core.bigFileThreshold` (X5): repo-local, default 512 MiB (above the 64 MiB limit, so inert), `0` disables it; §4.4 sets no override, so it cannot be relied on. Growth after the load: U-9 |
| DF2 | same fn, worktree branch (`read_link`, `metadata`, `File::open`, `read_to_end`, `convert_to_git`) | worktree files and links | only with `WorktreeRoots.new_root = Some(..)` and a call into `set_resource` from ST5's tracker | **unreachable** under §4.4 (`rewrites: None`); the cache is constructed with a root but never asked |
| DF3 | `gix-diff/src/rewrites/tracker.rs:match_pairs_of_kind`, `find_match` | exact pass compares ids (no reads); inexact pass calls `set_resource` twice per candidate pair, then `prepare_diff`, `InternedInput::new`, `Diff::compute` | `tree_index_status` | the **pair count** is **covered**: the inexact pass is skipped when `num_src * num_dst > limit` (so `limit: 1000` bounds *permutations*, as §4.4 says); `limit == 0` means unlimited, and `TrackRenames::AsConfigured` would take `status.renameLimit` / `diff.renameLimit` from repo-local config (C-3). The **per-pair memory and time** is U-9 |
| DF4 | `gix-diff/src/blob/pipeline.rs:run_cmd`, `Driver::prepare_binary_to_text_cmd`, `gix_tempfile::new` | external diff / binary-to-text drivers | only for a configured `diff.<name>` driver | **unreachable**: all `diff` sections are stripped (`diff_drivers()` is empty); spawn analysis in P15c |
| DF5 | `gix-diff/src/index/*` (tree-to-index diff), `rewrites/tracker.rs` item list | in-memory over two `State`s | `tree_index_status` | **covered**: O(entries), entries bounded by IX8 and IX4 |

### `gix-filter` 0.35.0

| # | Anchor | What it reads | Reachable from | Verdict |
|---|---|---|---|---|
| FL1 | `gix-filter/src/pipeline/convert.rs:convert_to_git` (`src.read_to_end(&mut self.bufs.src)` when `apply_ident_filter \|\| encoding.is_some() \|\| would_convert_eol`) | the whole worktree file into memory, no size argument; `would_convert_eol` is true for repo-local `core.autocrlf` / `core.eol` (not stripped) or `text` / `eol` attributes; `ident` and `working-tree-encoding` come from attributes | ST2 (b) | **UNCOVERED (U-6)** (reproduced) |
| FL2 | same fn, `read.read_to_end(&mut self.bufs.src)` after a driver; `gix-filter/src/driver/apply.rs` (`io::copy` into the input buffer, `read_to_end` of the process output), `driver/process/client.rs` (`io::copy`) | external clean / smudge / process drivers | only when a `filter.<name>` driver is configured | **unreachable**: all `filter` sections are stripped, so the driver list is empty and `filter=lfs` attributes name nothing runnable (§10.2 positive control 2); spawn analysis in P15c |
| FL3 | `gix-filter/src/worktree/encode_to_git.rs:encode_to_git` | in-memory transcode, output buffer proportional to the input (<= about 3x for UTF-16 to UTF-8) | after FL1 | growth is FL1's; **covered** iff U-6 is |
| FL4 | `gix-filter/src/eol` `convert_to_git` `index_object` callback (`find_blob` of the index version) | one blob | ST2 (c) | **covered**: allocation limit |

### `gix-object` 0.65.0

| # | Anchor | What it reads | Reachable from | Verdict |
|---|---|---|---|---|
| OB1 | `gix-object/src/lib.rs:compute_stream_hash` (-> `gix_hash::bytes_with_hasher`: `read_exact` in 64 KiB chunks, exactly `stream_len` bytes, interrupt checked per chunk) | a stream | ST3, ST5 | **covered**: constant memory, length from the caller's `lstat`; `HashEq` (called by `FastEq`) passes a never-set flag (time: N9) |
| OB2 | `gix-object/src/find.rs:Never::write_stream_with_known_id` (`read_exact`) | write-side stub | none | **unreachable** |
| OB3 | `gix-object/src/signature/sign.rs`, `signature/verify.rs` (`fs::read`, subprocess output `.lines()`) | signing / verification helpers | only `sign` / `verify_signature` APIs | **unreachable** from status; spawn analysis in P15c |
| OB4 | `gix-object` decoders (`tree::EntryRefIter`, `find_blob`, `find_tree_iter`) and `commit/message/body.rs:lines()` | object bodies from the object database | IX8 (trees), ST2 (c), WT1 / WT2 (blobs), DF1 | body size **covered** by the allocation limit (P15b: `gix-odb` / `gix-pack`); what the caller retains is IX8, U-7, U-9 |

### Probe results (P15a)

Environment: `gix` 0.88.0 exactly as locked, macOS, `Options::isolated()`, `FastEq`, default dirwalk options, 300 MiB sparse file as the big target, peak RSS in MiB from `/usr/bin/time -l` (control runs 7 MiB), debug build unless noted.

| Scenario | Result |
|---|---|
| untracked `sub/.git/HEAD` = symlink to the sparse file | **607 MiB**; control (`sub/` without `.git`) 7 MiB |
| untracked `sub/.git/HEAD` = FIFO | **blocked** until the 15 s alarm |
| index 100755 `x`, worktree `x` = symlink to sparse file, `.gitattributes` `* ident` | **307 MiB**, item `Modified` |
| same without `.gitattributes` | 7 MiB, item `Modified` (hash bounded by lstat length) |
| index 100644 `x`, same symlink, `* ident` | item `TypeChange`, 7 MiB (control) |
| index 100755 `x`, worktree `x` = FIFO, no filter | **blocked** until the alarm |
| index 100644 `x`, worktree `x` = FIFO | item `TypeChange`, did not block (control) |
| tracked regular 300 MiB file, `* ident` | **307 MiB** |
| tracked regular 300 MiB file, repo-local `core.autocrlf = true`, no attributes at all | **307 MiB** |
| tracked regular 300 MiB file, no filter | 7 MiB, 12 s (the hash is not interruptible) |
| index has a 16 MiB `.gitattributes` blob (`a x` lines), no worktree copy | **1114 MiB** |
| `Search::add_patterns_buffer` on 16 MiB of `a\n` (ignore parse only) | **663 MiB** |
| index with a 32 MiB / 1 MiB `REUC` extension (`\0` + three `0\0`) | **552 MiB** / 25 MiB |
| index with a 1 MiB `TREE` extension, single-child nesting (release **and** debug) | `fatal runtime error: stack overflow, aborting` |
| `InternedInput` + `Diff::compute(Histogram)` of two 64 MiB blobs of `\n` (the shape of one similarity pair) | **~1 GiB** peak, 32 s; with distinct lines still running after 100 s |
| 380 nested directories | passes, also with `RUST_MIN_STACK=65536` |

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

## UNCOVERED findings from P15a (open)

Six findings, all in code reachable from `open_index`, `index_worktree_status` or `tree_index_status` with the hardened open as §4.4 rev 11 describes it. §4.4 is not weakened here; each proposed check only adds coverage, and the author picks among them. Suggested gating fixtures are named per finding.

Shared observation: U-4 to U-9 are all either **non-interruptible** (a blocking `open`, a hash, a similarity diff, none of which look at the flag), **process-fatal** (stack overflow), or **memory amplifying**, and they run on the per-repo worker thread. The deadline (`"timed out"`, thread abandoned, never joined) bounds what the row says, not what the abandoned thread keeps doing. A structural cover for the whole class would be to run each repo's git work in a killable unit (a child process of rollcall's own binary with `RLIMIT_AS` / `RLIMIT_CPU`, or an equivalent). §4.4 says "No subprocesses during scan", so that would need an explicit exception and is a design change for the author; each finding below lists a narrower check first.

### U-4: index extensions: unbounded recursion (a) and unbounded retained size (b)

→ **covered (rev 12):** rollcall-side index validation: depth ≤ 1,024; per-extension caps `TREE` 16 MiB, `UNTR` 4 MiB, `REUC` 1 MiB (gix's allocation limit is per node, not per extension); `index.threads=1` (when threaded with `EOIE`, gix reads the extension table from the `EOIE` offset on its own thread); 16 MiB worker stack as margin.

- **(a) What it reads:** the `TREE` extension (`gix-index/src/extension/tree/decode.rs:one_recursive`) and the `UNTR` extension (`extension/untracked_cache.rs:decode_directory_block`) are decoded by recursion with one native stack frame per nesting level and no depth limit. The allocation limit is applied to sizes (`subtree_count * size_of::<Tree>()`), not depth. A `TREE` node needs 6 bytes (`\0-1 1\n`), so the 256 MiB the pre-open list allows is ~42 million levels.
- **(b) What it reads:** the `REUC` extension (`extension/resolve_undo.rs:decode`) builds one `ResolvePath` (name + three optional stages, ~120 bytes) per 7 input bytes and takes no allocation limit: `extension/decode.rs:all` passes `alloc_limit_bytes` to `tree::decode` and `untracked_cache::decode` only.
- **Reachable from:** `Repository::open_index` -> `gix_index::File::at` -> `State::from_bytes` -> `extension::decode::all`, so every status of every worktree, and X3 for the rename resource cache. The bytes come from `<git_dir>/index`, which the pre-open list admits up to 256 MiB.
- **Effect (reproduced):** (a) a 1 MiB `TREE` chain aborts the process with `fatal runtime error: stack overflow, aborting` in both debug and release; this kills the whole scan, not one row, and the abandon-the-thread deadline cannot contain it. (b) a 32 MiB `REUC` gave 552 MiB peak RSS (about 17x); at the 256 MiB cap that is roughly 4.4 GB plus `Vec` doubling. `TREE` is written by ordinary git, so an "index has extension X" rejection would refuse most real repos.
- **Why §4.4 does not cover it:** the `index` cap bounds bytes, and `gitoxide.objects.allocLimit` reaches the decoder but is honoured by neither of these two extension decoders (`TREE` counts only children per node; `REUC` and depth are not counted).
- **Check that would cover it (author's choice):** (1) rollcall's pre-open pass validates the extension area iteratively before gix ever decodes it: locate the extension table (walk the entries, or use `EOIE` when present and verified), and refuse (`untrusted git dir`) when `TREE` / `UNTR` nesting exceeds a bound (a legitimate cache tree is as deep as the deepest directory, PATH_MAX-limited) or the `REUC` extension exceeds a small cap (a legitimate one holds only unresolved-merge paths); (2) lower the `index` cap so that the worst case fits the RSS budget and run the repo work on a thread with a stack sized for the depth cap; (3) the structural option above. Fixtures: `index-tree-bomb`, `index-reuc-bomb`.

### U-5: the dirwalk probes every untracked directory for a nested repository, reading `<dir>/.git`, `HEAD`, `commondir`, `gitdir` unbounded

→ **covered (rev 12):** `recurse_repositories(true)` + §3.3 `untracked_count` rule.

- **What it reads:** `gix-dir/src/walk/classify.rs:maybe_upgrade_to_repository` calls `gix_discover::is_git` on `<dir>/.git` (`gix-discover/src/is.rs:git_with_metadata`). For a `.git` directory it checks that `<dir>/.git/HEAD` exists and then calls `find_loose("HEAD")` (`gix-ref` `ref_contents`: `File::open`, `read_to_end`, symlinks followed, FIFO blocks; P14 R1); for a `.git` regular file it reads the `gitdir:` line (`from_gitdir_file`, D1, 64 KiB cap applied after the open) and probes the named directory the same way, plus `commondir` and `gitdir`.
- **Reachable from:** `index_worktree_status` with default dirwalk options (`recurse_repositories: false`, X6): once per untracked, non-ignored directory (`classify::path`, "kind = maybe_upgrade_to_repository(...)"), and once per tracked gitlink directory. With `classify_untracked_bare_repositories` (off by default) it would also read `<dir>/HEAD` of every untracked directory itself.
- **Why §4.4 does not cover it:** the pre-open list names the repo's own git dir and the three worktree-root files. A nested `.git` inside the worktree is a different repository that rollcall's §4.1 walk lists as its own `nested` row, but (i) the parent's status probes it regardless of what that row's pre-open check said, (ii) the §4.1 walk skips `node_modules`, `target`, `.venv` and friends and does not descend past its budget, while the dirwalk enters all of them, and (iii) the parent's status can run before or concurrently with the nested row.
- **Effect (reproduced):** untracked `sub/.git/HEAD` -> a 300 MiB sparse file: 607 MiB peak RSS (7 MiB without the `.git`); the same with a FIFO: hangs until the deadline abandons the worker. A `/dev/zero` target grows until the process dies.
- **Check that would cover it (author's choice):** (1) run the dirwalk with `recurse_repositories(true)`, which returns before `is_git`, and drop from the count any entry under a directory that contains a `.git` entry (rollcall already knows nested repos from §4.1; for `node_modules`-style skipped directories it must decide the counting rule), after confirming on a fixture that gitlink directories are still classified; or (2) before status, a bounded rollcall-side walk of the worktree directories that `lstat`s every `.git` entry, applies the pre-open rule to `.git` (file <= 4 KiB, or directory) and to `<dir>/.git/HEAD` (regular, 4 KiB), and follows a `gitdir:` target one hop with the same rule; that walk must itself honour the deadline. Fixtures: `nested-git-head-symlink`, `nested-git-head-fifo`, `nested-git-file-gitdir`.

### U-6: tracked worktree files are read with attacker-chosen kind and, under built-in filters, without a size bound

→ **covered (rev 12):** pre-status 100755 `lstat` check (a) + rollcall `CompareBlobs` 64 MiB cap and `thread_limit: Some(1)` (b).

- **(a) type confusion.** An index entry of mode 100755 whose worktree path is a symlink, FIFO, socket or device gets no type change from `change_to_match_fs_with_values`, so status opens it (`std::fs::File::open` follows the symlink; the FIFO blocks). Unfiltered, a followed symlink costs at most its lstat length in hashed bytes (bounded); filtered, see (b). A FIFO blocks the worker.
- **(b) unbounded filtered read.** `gix-filter/src/pipeline/convert.rs:convert_to_git` does `src.read_to_end` into memory whenever `ident`, `working-tree-encoding`, or an eol conversion applies: `ident` / `working-tree-encoding` / `text` / `eol` from any attribute source, or repo-local `core.autocrlf` / `core.eol` (not stripped by §4.4). No `.gitattributes` is needed: `core.autocrlf = true` in `.git/config` (regular, small, passes pre-open) suffices. The size shortcut does not help (attacker-set `stat.size == 0` with a non-empty blob id skips it). The file can be a regular file of any size the attacker managed to unpack (a sparse or highly compressible archive), or, via (a), a symlink to `/dev/zero`.
- **Reachable from:** `index_worktree_status` -> `index_as_worktree` -> `compute_status` -> `FastEq` / `HashEq` -> `stream_worktree_file` (ST2, ST3, FL1); once per worker thread per file, so concurrent large files multiply by `thread_limit` (C-4).
- **Why §4.4 does not cover it:** the pre-open list covers `.git`-side files and three worktree-root files, never tracked content. "Consequences (documented)" mentions filters making files appear dirty, not their memory. The per-repo deadline does not stop a growing `read_to_end`, and the interrupt flag is not read inside it.
- **Effect (reproduced):** 100755 + symlink to a 300 MiB sparse file + `* ident`: 307 MiB; 300 MiB regular file + `* ident`: 307 MiB; 300 MiB regular file + `core.autocrlf=true` only: 307 MiB; 100755 + FIFO: blocked until the alarm. 100644 + symlink and 100644 + FIFO were **not** read (`TypeChange`).
- **Check that would cover it (author's choice):** (1) a rollcall pre-pass over the index entries (bounded by IX4's entry limit) that `lstat`s each tracked, non-`SKIP_WORKTREE` regular-file entry's path and refuses (a distinct `error` string, or `untrusted git dir`) when the entry is 100755 (or any file mode) and the worktree object is not a regular file, and, since size-only rules cannot see filter applicability, when the file is larger than a cap (for example 64 MiB, matching `allocLimit`); this needs the same iterate-and-lstat pass that status already does, so its cost is one more `lstat` per tracked file; (2) supply a rollcall `CompareBlobs` that refuses when `worktree_blob_size` exceeds a cap: this covers (b) for regular files but not (a), because the trait sees only the entry, the lstat size and the two data accessors, never the kind or the path; (3) the structural option. Fixtures: `exec-symlink-zero`, `exec-fifo`, `big-tracked-autocrlf`, `big-tracked-ident`.

### U-7: pattern files taken from index blobs are parsed with 40x to 70x memory amplification and no size cap other than the 64 MiB allocation limit

→ **covered (rev 12):** pre-status `try_header` caps 256 KiB each / 1 MiB total; pre-open pattern caps 256 KiB.

- **What it reads:** `.gitattributes` and `.gitignore` content from the object database: `gix-worktree/src/stack/state/attributes.rs:push_directory` (`find_blob`) when the source is `IdMapping` (always for the rename resource cache, X3; the worktree file is not consulted there), `IdMappingThenWorktree`, or the fallback of `WorktreeThenIdMapping` when the worktree copy is absent; `state/ignore.rs:push_directory` when the index entry has `SKIP_WORKTREE` and the worktree copy is absent. The blob read is under the allocation limit; the parse is not: `gix-glob` / `gix-attributes` / `gix-ignore` keep about 40x (ignore) to 70x (attributes) their input.
- **Reachable from:** `tree_index_status` (rename similarity candidates ask for attributes of both paths, so the `.gitattributes` blobs of every directory along the path are parsed), and `index_worktree_status`. The stack keeps one parsed list per directory level on the current path, so an attacker who points `.gitattributes` in many nested directories at the same 64 MiB blob multiplies it by depth.
- **Why §4.4 does not cover it:** the 1 MiB pre-open caps apply to worktree files only; the same bytes arriving as a blob are bounded by `allocLimit`, 64x larger, and the amplification is not accounted for anywhere.
- **Effect (reproduced):** a 16 MiB `.gitattributes` blob (`a x` lines, via `index_worktree_status`): 1114 MiB; a 16 MiB ignore buffer: 663 MiB. At 64 MiB: about 4.4 GB and 2.6 GB.
- **Check that would cover it (author's choice):** before status, rollcall resolves every index entry named `.gitattributes` / `.gitignore` (any depth, cheap on the sorted entry table) and reads its header size (`try_header`), refusing the repo (`untrusted git dir`) above the same 1 MiB cap the worktree-root files get, and capping the total; or lowers `gitoxide.objects.allocLimit` to a value at which 70x fits the budget (this is coupled to every other allocation-limit cover and to blobs the rename check may load, so it needs the P15b enforcement audit first). Fixtures: `attrs-blob-bomb`, `ignore-blob-skip-worktree-bomb`.

### U-8: split index: `sharedindex.<checksum>` is read unbounded and is not on the pre-open list

→ **covered (rev 12):** `sharedindex.*` on the pre-open list, validated like `index`, chained `link` refused.

- **What it reads:** `gix-index/src/extension/link.rs:dissolve_into` opens `<dir of the index>/sharedindex.<checksum>` (name from the index's `link` extension) through `File::at`: `File::open` (follows symlinks, blocks on a FIFO), a private `mmap`, then hashes `metadata().len() - hash_len` bytes unless `index.skipHash`. The shared index may itself carry a `link` extension, so a chain of files is read, one per hash, each hash computable by the attacker back to front.
- **Reachable from:** `open_index` whenever the (pre-open-checked) index carries a `link` extension; any status.
- **Why §4.4 does not cover it:** `index` is on the list, `sharedindex.*` is not. The decode of the shared file is under the allocation limit; the read, map and hash are not.
- **Effect (traced, not reproduced):** a FIFO blocks the worker until the deadline abandons it; an oversized regular file (or a symlink to any large regular file) costs a full, non-interruptible hash; a symlink to `/dev/zero` should fail at `mmap` (length 0 gives `EINVAL`), not exercised. No unbounded memory growth beyond the allocation limit.
- **Check that would cover it:** add every entry of the form `sharedindex.*` in the index's directory (git dir, and each linked admin dir) to the pre-open list with the `index` rule (regular, non-symlink, 256 MiB). Fixture: `sharedindex-fifo`, `sharedindex-symlink`.

### U-9: rename similarity: per-pair memory and time are unbounded by the 64 MiB allocation limit; `core.bigFileThreshold` is repo-controlled

→ **covered (rev 12):** `core.bigFileThreshold=1m` override.

- **What it reads:** `gix-diff/src/rewrites/tracker.rs:find_match` loads both blobs of every candidate pair (`Platform::set_resource` -> `convert_to_diffable`, DF1) and runs `InternedInput::new` + `Diff::compute`. §4.4 covers this with "`Rewrites { .. limit: 1000 }` (at most 1,000 pair checks) and the allocation limit". The limit does bound the pair count (`num_src * num_dst > limit` skips the inexact pass) and the allocation limit bounds each blob to 64 MiB. Neither bounds what a pair costs.
- **Reachable from:** `tree_index_status` with `TrackRenames::Given(Rewrites { percentage: Some(0.5), .. })`: any repo whose HEAD-to-index difference contains at least one addition and one deletion of text blobs; up to 1,000 source-times-destination permutations are then compared.
- **Why §4.4 does not cover it:** the intended per-blob guard is `large_file_threshold_bytes` (`core.bigFileThreshold`, DF1, X5): default 512 MiB, above the allocation limit, `0` disables it, and it is read from repo-local config, which §4.4 does not override. `tree_index_status` takes no interrupt flag, so nothing stops a similarity diff once started.
- **Effect (reproduced on the diff primitive):** two 64 MiB blobs of one-byte lines: about 1 GiB peak RSS and 32 s (debug); with distinct lines the diff was still running after 100 s. That is one pair, above §10.2's 512 MB RSS acceptance criterion, and the abandoned worker keeps running after the deadline.
- **Check that would cover it (author's choice):** (1) add `core.bigFileThreshold` to the config overrides at open with a small value (for example 1 MiB): blobs above it are diffed as binary (size only), so the per-pair cost is bounded by the threshold and the similarity pass cannot load a large blob; it applies only to the diff pipeline (`diff_pipeline_options`), so nothing else changes; or (2) `Rewrites { percentage: None, .. }` (exact renames only: no blob loads at all; renames with edits then count as an add plus a delete, which changes `dirty_count`). Fixture: `rename-bomb` (two 60 MiB single-byte-line blobs, one staged rename), asserted against the 10 s / 512 MB criterion.

## Constraints on later phases and observations (not UNCOVERED, but the audit relies on them)

- **C-1 (P19/P21):** pass the resolved git dir with `open_path_as_is(true)`; never a worktree root or a `.git` file (D3). Otherwise gix probes `<path>/.git` first.
- **C-2 (P20/P21), do-not-call list:** `Repository::worktrees()`, `worktree_proxy_by_id`, `Proxy::lock_reason` / `base` (G11); reflog APIs, `rev_parse` with `@{}` (R7); `Platform::pseudo()` / `iter_pseudo` (R8); `open_mailmap*` (not compiled); `filter::Pipeline::worktree_file_to_object` (G13). P15a additions: C-6.
- **N1:** spec §4.4 says "`Config::isolated()` permissions"; the plan (P19) says `open::Options::isolated()`. Only the latter denies environment access and isolates attributes. The audit's "unreachable" verdicts for G6, G7, G8(c), G9(c) assume the full `Options::isolated()`; a P19 test asserting `Permissions::is_isolated()` on the options used would pin this.
- **N2:** `assemble_exclude_globals` and `assemble_attribute_globals` read `info/exclude` and `info/attributes` from `common_dir()`, not the admin dir; the pre-open location for these two files therefore follows the common dir (see U-1).
- **N3:** an unsorted `packed-refs` at the 64 MiB cap is parsed into an entry `Vec` and re-serialised into a second buffer (`packed/buffer.rs:open_with_backing`), so the RSS budget (§10.2: < 512 MB) should account for a small multiple of 64 MiB.
- **N4 (P15a hand-off, done):** the read behind G8(a)/(b) and G9(b), and `.gitignore` / `.gitattributes` reads (accepted exception for subdirectories only), are in `gix-glob` / `gix-ignore` / `gix-attributes` / `gix-worktree`; the root-level files (pre-open 1 MiB) are traced separately from subdirectory ones (documented limitation) in rows GL1, WT1, WT2.
- **N5 (P15b hand-off):** `gix_odb::Store::at_opts` runs at open but reads nothing; `objects/info/alternates`, packs, `shallow`, `commit-graph-chain` are read lazily (P15b), all from the common dir (U-1).
- **C-3 (P21), status call shape:** `index_worktree_status` must be called with `Options { rewrites: None, .. }` (ST5 and DF2 are unreachable only because of it; `Options::default()` has `rewrites: None`; do not enable index-worktree rename tracking) and a `SubmoduleStatus` returning `Ok(None)`. `tree_index_status` must be called with `TrackRenames::Given(Rewrites { copies: None, percentage: Some(0.5), limit: 1000, track_empty: false })`, never `AsConfigured` (it would take `status.renameLimit` / `diff.renameLimit` / `diff.renames` from repo-local config, where `0` means unlimited) and never with `limit: 0`. §4.4 does not name a `CompareBlobs`; the audit assumes `FastEq` (size shortcut, then hash). `HashEq` reads every stat-mismatched file, so it changes only how often U-6 is reachable, not whether.
- **C-4 (P21), thread count:** `gix_status::index_as_worktree` gives every worker thread its own clone of the attribute stack, filter pipeline and two buffers (`in_parallel_if` clones the `new_state` closure per thread), so parsed pattern-file memory and filtered-file buffers multiply by the thread count; the default is the logical core count. Set `Options::thread_limit` deliberately. Measured, release, 10 cores, 8,000 modified tracked files, a 1 MiB `.gitattributes` of `a x` lines: 138 MiB peak (one parse is about 70 MiB, so roughly two ran concurrently there).
- **C-5 (P21), delegate memory:** the `VisitEntry` delegate should count, not retain per-entry paths or `Item` clones: the dirwalk streams names, but a directory of millions of entries is bounded only by what the delegate keeps (the interrupt flag is checked per directory and per emitted entry).
- **C-6 (P20/P21), do-not-call, additions to C-2:** `gix_index::File::verify_integrity` (IX9), `gix_index::File::at_or_default` (IX2), dirwalk options with `classify_untracked_bare_repositories(true)` (it also reads `<dir>/HEAD` of every untracked directory) unless U-5 is covered, and any `Repository::status()` convenience that enables index-worktree rewrites.
- **C-7 (P21):** the tree id given to `tree_index_status` must be `HEAD^{tree}` of the same commit the HEAD tree bound walked: when `.git/index` is absent, `diff_resource_cache` calls `index_or_load_from_head_or_empty` and expands the HEAD tree a second time (X3, IX8), which is covered only because it is the same tree.
- **N6 (spec wording, subdirectory pattern files):** §4.4's "Documented limitation" says "untracked `.gitignore` / `.gitattributes` files in subdirectories". Two precision points from the code. (i) `.gitattributes` in a subdirectory is opened with `O_NOFOLLOW` whether tracked or not, so a symlink to `/dev/zero` there fails with an error and is not read; the symlink-to-`/dev/zero` case applies to `.gitignore`, and only when it is not in the index (`follow_symlinks = ignore_file_in_index.is_err()`). (ii) A *tracked* subdirectory `.gitignore` / `.gitattributes` whose worktree copy is a FIFO or a very large regular file is read the same way (worktree first, index blob only as fallback), so the limitation's class is "any `.gitignore` / `.gitattributes` in a subdirectory of the worktree", not only untracked ones. The audit treats rows GL1(e), WT1, WT2 on that basis; if the author intends the letter of the current text, the tracked cases are UNCOVERED-by-wording, and the fix is a one-line edit of the limitation (a wording precision, not a weakening).
- **N7 (RSS budget):** the pre-open pattern-file caps (root `.gitignore`, root `.gitattributes`, `info/exclude`, `info/attributes`, the user's excludes file: 1 MiB each) retain up to about 70x, i.e. roughly 5 x 70 MiB before the per-thread multiplication of C-4, against §10.2's 512 MB peak. Legal by the caps, but the budget is tight; the author may want a smaller cap for pattern files than for `config`.
- **N8 (index mapping):** `File::at` maps the index privately (`map_copy_read_only`); truncation of `.git/index` by another process while it is mapped raises `SIGBUS` in the reader. Real git replaces the index by rename, so this needs a hostile concurrent writer; noted, not a §4.4 item.
- **N9 (time, not memory):** `HashEq`'s stream hash (reached from `FastEq`) (and the index-checksum hash in IX1) pass a never-set interrupt flag and `tree_index_status` takes none, so a huge tracked file (unfiltered), a similarity diff or a HEAD tree expansion keeps running on the abandoned worker after the deadline. Memory is bounded for the unfiltered hash (64 KiB buffer); CPU is bounded only by the input size.
- **N10 (P15b hand-off):** every "covered by the allocation limit" verdict here (IX4, ST2(c), FL4, OB4, DF1) relies on `gitoxide.objects.allocLimit` being enforced at object decode in `gix-odb` / `gix-pack`, and IX8 on `gix-traverse` `depthfirst` behaving as the HEAD tree bound assumes; P15b confirms both. `gix-imara-diff` (P15b "remaining") is the crate behind U-9's `InternedInput` / `Diff::compute`; measured above, not read.
- **N11 (P15c hand-off):** spawn analysis is still owed for `gix-filter` drivers (FL2), the `gix-diff` binary-to-text driver (DF4) and `gix-object` signature helpers (OB3); all three are unreachable here only because §4.4 strips `filter.*` / `diff.*` sections and rollcall never signs or verifies. P15c should also confirm the `index_worktree_status` signature and `SubmoduleStatus` at 0.88 as listed in its brief.

## Maintenance

Re-run the crate-list command on every gix bump and diff it against the list above; re-audit any crate whose version changed, and re-check the anchors (they are function names, not line numbers, so a rename shows up as a failed grep).
