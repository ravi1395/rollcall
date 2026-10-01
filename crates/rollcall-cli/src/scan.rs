use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use rollcall_core::text::sanitise;
use rollcall_core::{
    DiscoveredRepo, DoctorDoc, GitOptions, Index, MatchResult, RepoKind, RepoRow, RepoState,
    ReposByKind, SCHEMA_VERSION, ScanResult, SessionLocation, SourceKind, SourceScan, Uuid,
    WalkOptions, WorktreeCounts, WorktreeKey, match_sessions,
};
use rollcall_git::RootCheck;
use rollcall_sessions::{Claude, Codex, SessionSource};
use rollcall_store::{ScanLock, SqliteIndex};
use serde::{Deserialize, Serialize};

use crate::config::Ctx;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    /// Scan, and wait for it.
    Fresh,
    /// Answer from the cache only.
    NoScan,
    /// Scan, but answer stale from the cache after this long (MCP, §6.4).
    WaitAtMost(Duration),
}

pub struct ScanOutcome {
    pub index: SqliteIndex,
    /// The answer did not come from a scan completed by this invocation.
    pub stale: bool,
    pub exit: ExitCode,
}

const EXIT_OK: u8 = 0;
/// An error row, a partial walk, or no scan ever completed (§3.1).
const EXIT_INCOMPLETE: u8 = 1;
const EXIT_REFUSED: u8 = 3;

/// How often a scan that waits for another scanner's lock looks again.
const LOCK_POLL: Duration = Duration::from_millis(10);

pub fn scan(ctx: &Ctx, root: &Path, mode: ScanMode) -> ScanOutcome {
    let root = canonical_root(root);
    let index = open_index(ctx);
    if let RootCheck::Refused { suggestions } =
        rollcall_git::check_root(&root, &walk_options(ctx, &root))
    {
        eprint_refusal(&root, &suggestions);
        return outcome(index, true, EXIT_REFUSED);
    }
    match mode {
        ScanMode::NoScan => cached(index, &root),
        ScanMode::Fresh => match take_lock(ctx, &root) {
            Some(lock) => scan_here(ctx, &root, index, lock),
            None => cached(index, &root),
        },
        ScanMode::WaitAtMost(wait) => scan_within(ctx, root, index, wait),
    }
}

/// The key every cache read and write for `root` uses: the realpath, else the path as given.
pub fn canonical_root(root: &Path) -> PathBuf {
    fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

/// `config::load` guarantees HOME, so every `Ctx` the commands build has it.
fn home(ctx: &Ctx) -> &Path {
    ctx.env
        .home
        .as_deref()
        .expect("config::load rejects a missing HOME")
}

fn open_index(ctx: &Ctx) -> SqliteIndex {
    SqliteIndex::open(&ctx.cache_dir).unwrap_or_else(|e| {
        // `ScanOutcome` carries an index, so there is no way to answer without one.
        eprintln!(
            "rollcall: cannot open the cache {}: {}",
            sanitise(&ctx.cache_dir.to_string_lossy()),
            sanitise(&e.to_string())
        );
        std::process::exit(i32::from(EXIT_INCOMPLETE))
    })
}

fn eprint_refusal(root: &Path, suggestions: &[PathBuf]) {
    eprintln!(
        "rollcall: refusing to scan {}: it is / or your home directory (pass --yes to scan it anyway)",
        sanitise(&root.to_string_lossy())
    );
    for dir in suggestions {
        eprintln!("  rollcall {}", sanitise(&dir.to_string_lossy()));
    }
}

fn walk_options(ctx: &Ctx, root: &Path) -> WalkOptions {
    let config = &ctx.config;
    let listed = config.roots.iter().any(|r| canonical_root(r) == root);
    WalkOptions {
        home: home(ctx).to_path_buf(),
        confirmed: config.yes || listed,
        follow_symlinks: config.follow_symlinks,
        submodules: config.submodules,
        skip_dirs: config.skip_dirs.clone(),
        budget_entries: config.walk_budget_entries,
        budget: Duration::from_secs(config.walk_budget_secs),
    }
}

fn git_options(ctx: &Ctx, root: &Path) -> GitOptions {
    let home = home(ctx);
    // An explicit root under a skipped path is allowed; the skip binds only from outside.
    let skip_paths = rollcall_git::classify::path_skips(home)
        .into_iter()
        .filter(|skip| !root.starts_with(skip))
        .collect();
    GitOptions {
        root: root.to_path_buf(),
        home: home.to_path_buf(),
        skip_paths,
        repo_budget: Duration::try_from_secs_f64(ctx.config.repo_budget_secs)
            .unwrap_or(Duration::MAX),
        excludes_file: rollcall_git::user_excludes_file(home, ctx.env.xdg_config_home.as_deref()),
    }
}

// ---- answering from the cache ----

fn outcome(index: SqliteIndex, stale: bool, exit: u8) -> ScanOutcome {
    ScanOutcome {
        index,
        stale,
        exit: ExitCode::from(exit),
    }
}

fn cached(index: SqliteIndex, root: &Path) -> ScanOutcome {
    let exit = cached_exit(&index, root);
    outcome(index, true, exit)
}

/// §3.1 from stored rows: no scan ever, a partial walk, or any error row is 1.
fn cached_exit(index: &SqliteIndex, root: &Path) -> u8 {
    if index.scanned_at(root).is_none() {
        return EXIT_INCOMPLETE;
    }
    let partial = index
        .last_diagnostics(root)
        .is_some_and(|doctor| doctor.walk.budget_hit);
    if partial || index.repos_for_root(root).iter().any(has_error) {
        EXIT_INCOMPLETE
    } else {
        EXIT_OK
    }
}

fn has_error(row: &RepoRow) -> bool {
    row.state.error.is_some()
        || row.state.worktrees.iter().any(|w| w.error.is_some())
        || row.submodules.iter().any(has_error)
}

// ---- locking ----

/// `None`: another scanner holds the lock (or it cannot be taken: the cache answers either way).
fn take_lock(ctx: &Ctx, root: &Path) -> Option<ScanLock> {
    match rollcall_store::try_scan_lock(&ctx.cache_dir, root) {
        Ok(lock) => lock,
        Err(e) => {
            eprintln!(
                "rollcall: cannot take the scan lock: {}",
                sanitise(&e.to_string())
            );
            None
        }
    }
}

fn scan_here(ctx: &Ctx, root: &Path, mut index: SqliteIndex, lock: ScanLock) -> ScanOutcome {
    let result = run_scan(ctx, root, &mut index);
    drop(lock);
    match result {
        Ok(exit) => outcome(index, false, exit),
        Err(e) => {
            eprintln!(
                "rollcall: scan of {} failed: {}",
                sanitise(&root.to_string_lossy()),
                sanitise(&e)
            );
            cached_with(index, root, EXIT_INCOMPLETE)
        }
    }
}

/// Stale, and at least `floor` whatever the cache says.
fn cached_with(index: SqliteIndex, root: &Path, floor: u8) -> ScanOutcome {
    let exit = cached_exit(&index, root).max(floor);
    outcome(index, true, exit)
}

/// `WaitAtMost`: the scan runs on its own thread; past `wait` the answer is the cache and the
/// thread finishes and commits on its own.
fn scan_within(ctx: &Ctx, root: PathBuf, index: SqliteIndex, wait: Duration) -> ScanOutcome {
    let now = Instant::now();
    let give_up = now.checked_add(wait).unwrap_or(now + FAR_FUTURE);
    let lock = loop {
        if let Some(lock) = take_lock(ctx, &root) {
            break lock;
        }
        if Instant::now() >= give_up {
            return cached(index, &root);
        }
        thread::sleep(LOCK_POLL);
    };
    drop(index);
    let (tx, rx) = mpsc::channel();
    let (scan_ctx, scan_root) = (ctx.clone(), root.clone());
    thread::spawn(move || {
        let outcome = scan_here(&scan_ctx, &scan_root, open_index(&scan_ctx), lock);
        // The receiver is gone once the caller gave up: the commit already happened.
        let _ = tx.send(outcome);
    });
    match rx.recv_timeout(give_up.saturating_duration_since(Instant::now())) {
        Ok(done) => done,
        Err(_) => cached(open_index(ctx), &root),
    }
}

// ---- one scan ----

/// The scan proper, under the lock: returns the exit code (0 or 1) once the result is committed.
fn run_scan(ctx: &Ctx, root: &Path, index: &mut SqliteIndex) -> Result<u8, String> {
    let scanned_at = ctx.now;
    let discovery = rollcall_git::discover(root, &walk_options(ctx, root));
    let fanout = git_states(&discovery.repos, &git_options(ctx, root));
    let mut notes = discovery.notes.clone();
    notes.extend(fanout.notes);
    let timeouts = fanout
        .results
        .iter()
        .filter(|r| r.state.error.as_deref() == Some(TIMED_OUT))
        .count();
    let abandoned = fanout
        .results
        .iter()
        .filter(|r| r.state.error.as_deref() == Some(TIMED_OUT) && r.abandoned)
        .count();
    let states = fanout.results.into_iter().map(|r| r.state).collect();
    let repos = build_rows(&discovery.repos, states);

    let mut sources = scan_sources(ctx, index);
    for source in &sources {
        notes.extend(source.notes.iter().cloned());
    }
    let matches = match_all(root, &repos, &mut sources);

    let diagnostics = DoctorDoc {
        notes,
        repo_timeouts: count(timeouts),
        repo_timeouts_abandoned: count(abandoned),
        sources: sources.iter().map(|s| s.diag.clone()).collect(),
        ..doctor_counts(root, scanned_at, &repos, discovery.walk)
    };
    let exit = if discovery.partial || repos.iter().any(has_error) {
        EXIT_INCOMPLETE
    } else {
        EXIT_OK
    };
    index
        .commit_scan(
            root,
            ScanResult {
                scanned_at,
                submodules: ctx.config.submodules,
                partial: discovery.partial,
                repos,
                sources,
                matches,
                diagnostics,
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(exit)
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn scan_sources(ctx: &Ctx, index: &SqliteIndex) -> Vec<SourceScan> {
    let home = home(ctx);
    let claude_root = ctx
        .env
        .claude_config_dir
        .clone()
        .unwrap_or_else(|| home.join(".claude"));
    let codex_home = ctx
        .env
        .codex_home
        .clone()
        .unwrap_or_else(|| home.join(".codex"));
    let sources: [Box<dyn SessionSource>; 2] = [
        Box::new(Claude::new(claude_root.join("projects"))),
        Box::new(Codex::new(codex_home)),
    ];
    sources
        .iter()
        .map(|source| source.scan(&index.source_state(source.kind())))
        .collect()
}

/// Top-level rows in discovery order; each submodule row goes under its parent.
fn build_rows(repos: &[DiscoveredRepo], states: Vec<RepoState>) -> Vec<RepoRow> {
    let mut rows: Vec<Option<RepoRow>> = repos
        .iter()
        .zip(states)
        .map(|(repo, state)| {
            let path = repo.workdir.clone().unwrap_or_else(|| repo.git_dir.clone());
            let name = path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into_owned();
            Some(RepoRow {
                path,
                name,
                kind: repo.kind,
                contains_root: repo.contains_root,
                state,
                submodules: Vec::new(),
            })
        })
        .collect();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); repos.len()];
    for (i, repo) in repos.iter().enumerate() {
        if let Some(parent) = repo.submodule_of.filter(|&p| p < repos.len() && p != i) {
            children[parent].push(i);
        }
    }
    // `take` hands every row out once, so a malformed parent cycle cannot loop.
    fn attach(i: usize, rows: &mut [Option<RepoRow>], children: &[Vec<usize>]) -> Option<RepoRow> {
        let mut row = rows[i].take()?;
        for &child in &children[i] {
            row.submodules.extend(attach(child, rows, children));
        }
        Some(row)
    }
    (0..repos.len())
        .filter(|&i| {
            repos[i]
                .submodule_of
                .is_none_or(|p| p >= repos.len() || p == i)
        })
        .filter_map(|i| attach(i, &mut rows, &children))
        .collect()
}

fn flatten<'a>(rows: &'a [RepoRow], out: &mut Vec<&'a RepoRow>) {
    for row in rows {
        out.push(row);
        flatten(&row.submodules, out);
    }
}

fn doctor_counts(
    root: &Path,
    scanned_at: rollcall_core::Timestamp,
    repos: &[RepoRow],
    walk: rollcall_core::WalkDiag,
) -> DoctorDoc {
    let mut all = Vec::new();
    flatten(repos, &mut all);
    let mut by_kind = ReposByKind::default();
    for row in repos {
        match row.kind {
            RepoKind::Normal => by_kind.normal += 1,
            RepoKind::Nested => by_kind.nested += 1,
            RepoKind::Bare => by_kind.bare += 1,
            RepoKind::ExternalMain => by_kind.external_main += 1,
        }
    }
    let mut worktrees = WorktreeCounts::default();
    for wt in all.iter().flat_map(|row| &row.state.worktrees) {
        worktrees.total += 1;
        worktrees.external += u32::from(!canonical_lenient(&wt.path).starts_with(root));
        worktrees.missing += u32::from(wt.missing);
        worktrees.untrusted_path += u32::from(
            wt.error
                .as_deref()
                .is_some_and(|e| e.starts_with("untrusted worktree path")),
        );
    }
    let with_error = |prefix: Option<&str>| {
        let hits = all.iter().filter(|row| match (&row.state.error, prefix) {
            (Some(error), Some(prefix)) => error.starts_with(prefix),
            (Some(_), None) => true,
            (None, _) => false,
        });
        count(hits.count())
    };
    DoctorDoc {
        schema_version: SCHEMA_VERSION,
        root: root.to_string_lossy().into_owned(),
        scanned_at: Some(scanned_at),
        walk,
        repos_by_kind: by_kind,
        worktrees,
        submodules: count(all.len() - repos.len()),
        repo_errors: with_error(None),
        untrusted_git_dirs: with_error(Some("untrusted git dir")),
        repo_timeouts: 0,
        repo_timeouts_abandoned: 0,
        notes: Vec::new(),
        sources: Vec::new(),
    }
}

// ---- matching ----

/// Lexically normalised, with the deepest existing ancestor replaced by its realpath (§4.3:
/// "canonicalised with realpath where they exist; otherwise lexically normalised").
fn canonical_lenient(path: &Path) -> PathBuf {
    let mut lexical = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                lexical.pop();
            }
            other => lexical.push(other),
        }
    }
    let mut existing = lexical.as_path();
    let mut tail: Vec<&OsStr> = Vec::new();
    loop {
        if let Ok(real) = fs::canonicalize(existing) {
            return tail.iter().rev().fold(real, |p, part| p.join(part));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                existing = parent;
            }
            _ => return lexical,
        }
    }
}

fn collect_worktrees(rows: &[RepoRow], out: &mut Vec<(PathBuf, PathBuf, bool)>) {
    for row in rows {
        for wt in &row.state.worktrees {
            out.push((canonical_lenient(&wt.path), wt.path.clone(), wt.missing));
        }
        collect_worktrees(&row.submodules, out);
    }
}

/// Matches every session to a worktree under `root`, fills each source's `matched` /
/// `unmatched`, and returns the `(source, id, worktree)` triples `commit_scan` stores.
fn match_all(
    root: &Path,
    repos: &[RepoRow],
    sources: &mut [SourceScan],
) -> Vec<(SourceKind, Uuid, Option<PathBuf>)> {
    let mut found = Vec::new();
    collect_worktrees(repos, &mut found);
    // §6.5 candidates are worktrees under the root; submodules are never hidden here because
    // discovery leaves them out without `--submodules`.
    found.retain(|(canonical, _, _)| canonical.starts_with(root));
    let keys: Vec<WorktreeKey> = found
        .iter()
        .map(|(canonical, _, missing)| WorktreeKey {
            canonical_path: canonical.clone(),
            hidden: false,
            missing: *missing,
        })
        .collect();

    let sessions: Vec<(usize, &rollcall_core::Session)> = sources
        .iter()
        .enumerate()
        .flat_map(|(i, s)| s.sessions.iter().map(move |session| (i, session)))
        .collect();
    let locations: Vec<SessionLocation> = sessions
        .iter()
        .map(|(_, s)| {
            let cwd_canonical = canonical_lenient(&s.cwd);
            SessionLocation {
                under_root: cwd_canonical.starts_with(root),
                cwd_exists: fs::metadata(&s.cwd).is_ok(),
                cwd_canonical,
            }
        })
        .collect();
    let results = match_sessions(&keys, &locations);

    let mut triples = Vec::with_capacity(sessions.len());
    let mut tallies = vec![(0_u32, rollcall_core::UnmatchedCounts::default()); sources.len()];
    for ((source, session), result) in sessions.iter().zip(results) {
        let tally = &mut tallies[*source];
        let worktree = match result {
            MatchResult::Matched(i) => {
                tally.0 += 1;
                Some(found[i].1.clone())
            }
            MatchResult::OutsideRoot => {
                tally.1.outside_root += 1;
                None
            }
            MatchResult::PathGone => {
                tally.1.path_gone += 1;
                None
            }
            MatchResult::NonRepo => {
                tally.1.non_repo += 1;
                None
            }
        };
        triples.push((session.source, session.id.clone(), worktree));
    }
    for (source, (matched, unmatched)) in sources.iter_mut().zip(tallies) {
        source.diag.matched = matched;
        source.diag.unmatched = unmatched;
    }
    triples
}

// ---- per-repo worker process (spec §4.4) ----

const MAX_RESULT_BYTES: usize = 16 * 1024 * 1024;
const RSS_LIMIT_KIB: u64 = 256 * 1024;
const MONITOR_TICK: Duration = Duration::from_millis(10);
/// The parent's kill comes this long after the child's own deadline: a backstop.
const KILL_GRACE: Duration = Duration::from_secs(1);
const PS_PATH: &str = "/bin/ps";
const TIMED_OUT: &str = "timed out";
const MEMORY_LIMIT: &str = "git error: memory limit";
const WORKER_FAILED: &str = "git error: worker failed";
/// Stands in for a deadline too far out to add to an `Instant`.
const FAR_FUTURE: Duration = Duration::from_secs(10 * 365 * 24 * 60 * 60);

#[derive(Debug, Serialize, Deserialize)]
struct WorkerInput {
    repo: DiscoveredRepo,
    opts: GitOptions,
}

#[derive(Debug, Serialize, Deserialize)]
struct RepoResult {
    state: RepoState,
    /// The worker thread was still running when `repo_state` gave up (or the process was killed).
    abandoned: bool,
}

fn error_result(error: &str, abandoned: bool) -> RepoResult {
    RepoResult {
        state: RepoState {
            default_branch: None,
            has_remote: false,
            error: Some(error.to_string()),
            worktrees: Vec::new(),
            pending: None,
        },
        abandoned,
    }
}

fn worker_failed() -> RepoResult {
    error_result(WORKER_FAILED, false)
}

/// The hidden subcommand's body: one `{repo, opts}` JSON on `input`, one `RepoResult` JSON on
/// `output`. Nothing is written when the input is unreadable or the result cannot be encoded.
pub fn run_worker(mut input: impl Read, mut output: impl Write) -> Result<(), String> {
    let mut bytes = Vec::new();
    input
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read input: {e}"))?;
    let WorkerInput { repo, opts } =
        serde_json::from_slice(&bytes).map_err(|e| format!("bad input: {e}"))?;
    let state = rollcall_git::repo_state(&repo, &opts);
    let abandoned = state.pending.is_some();
    let encoded = serde_json::to_vec(&RepoResult { state, abandoned })
        .map_err(|e| format!("cannot encode the result: {e}"))?;
    output
        .write_all(&encoded)
        .and_then(|()| output.flush())
        .map_err(|e| format!("cannot write the result: {e}"))
}

/// Entry point of the hidden `__worker` subcommand. Returning ends the process, and with it any
/// abandoned worker thread.
pub fn worker_main() -> ExitCode {
    match run_worker(std::io::stdin().lock(), std::io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rollcall: worker: {}", sanitise(&e));
            ExitCode::FAILURE
        }
    }
}

/// A worker's whole stdout as a result; empty, truncated, malformed or oversize output is
/// `worker failed`.
fn parse_result(bytes: &[u8]) -> RepoResult {
    if bytes.len() > MAX_RESULT_BYTES {
        return worker_failed();
    }
    serde_json::from_slice(bytes).unwrap_or_else(|_| worker_failed())
}

/// `ps -o pid=,rss=` output (procps and BSD alike: two whitespace-separated numbers per line,
/// `rss` in KiB). No lines is no live children.
fn parse_ps(stdout: &str) -> Result<Vec<(u32, u64)>, String> {
    let mut rows = Vec::new();
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        let mut fields = line.split_whitespace();
        let row = match (fields.next(), fields.next(), fields.next()) {
            (Some(pid), Some(rss), None) => pid.parse().ok().zip(rss.parse().ok()),
            _ => None,
        };
        rows.push(row.ok_or_else(|| format!("unparseable ps line {line:?}"))?);
    }
    Ok(rows)
}

/// Runs `ps`; only "cannot run" and "cannot parse" are failures, never its exit status (it
/// exits 1 with no output once every listed pid has gone, and other systems differ).
fn probe_ps(ps: &Path, pids: &[u32]) -> Result<Vec<(u32, u64)>, String> {
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let out = Command::new(ps)
        .args(["-o", "pid=,rss=", "-p", &list])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run {}: {e}", ps.display()))?;
    parse_ps(&String::from_utf8_lossy(&out.stdout))
}

/// Our children whose resident set passes `limit_kib`; pids `ps` did not list have exited.
fn over_limit(rows: &[(u32, u64)], live: &[u32], limit_kib: u64) -> Vec<u32> {
    rows.iter()
        .filter(|(pid, rss)| *rss > limit_kib && live.contains(pid))
        .map(|(pid, _)| *pid)
        .collect()
}

struct Workers {
    program: PathBuf,
    args: Vec<OsString>,
    ps: PathBuf,
    rss_limit_kib: u64,
    grace: Duration,
    parallelism: usize,
}

struct Fanout {
    results: Vec<RepoResult>,
    notes: Vec<String>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A running child the monitor may kill.
struct Slot {
    child: Mutex<Child>,
    pid: u32,
    deadline: Instant,
    /// Why the monitor killed it; the first reason wins.
    reason: Mutex<Option<&'static str>>,
}

impl Slot {
    fn kill(&self, reason: Option<&'static str>) {
        if let Some(reason) = reason {
            lock(&self.reason).get_or_insert(reason);
        }
        // Already exited (and not yet reaped) is fine: the result decides, not this call.
        let _ = lock(&self.child).kill();
    }
}

struct Shared<'a> {
    workers: &'a Workers,
    live: Mutex<Vec<Arc<Slot>>>,
    /// Set by a probe failure: no child is spawned after it.
    no_more_spawns: AtomicBool,
    done: AtomicBool,
    notes: Mutex<Vec<String>>,
}

/// Every repo's state, each read by its own child process, at most `parallelism` at once.
fn fan_out(workers: &Workers, repos: &[DiscoveredRepo], opts: &GitOptions) -> Fanout {
    let shared = Shared {
        workers,
        live: Mutex::new(Vec::new()),
        no_more_spawns: AtomicBool::new(false),
        done: AtomicBool::new(false),
        notes: Mutex::new(Vec::new()),
    };
    let next = AtomicUsize::new(0);
    let threads = workers.parallelism.clamp(1, repos.len().max(1));
    let mut indexed: Vec<(usize, RepoResult)> = Vec::with_capacity(repos.len());
    thread::scope(|outer| {
        let monitor = outer.spawn(|| monitor(&shared));
        thread::scope(|inner| {
            let pool: Vec<_> = (0..threads)
                .map(|_| {
                    inner.spawn(|| {
                        let mut mine = Vec::new();
                        loop {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            let Some(repo) = repos.get(i) else { break mine };
                            mine.push((i, read_state(&shared, repo, opts)));
                        }
                    })
                })
                .collect();
            for handle in pool {
                indexed.extend(handle.join().expect("repo thread panicked"));
            }
        });
        shared.done.store(true, Ordering::Release);
        monitor.join().expect("monitor thread panicked");
    });
    indexed.sort_by_key(|(i, _)| *i);
    Fanout {
        results: indexed.into_iter().map(|(_, r)| r).collect(),
        notes: shared
            .notes
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner),
    }
}

/// One repo through one child (spec §4.4 (a)-(c)).
fn read_state(shared: &Shared, repo: &DiscoveredRepo, opts: &GitOptions) -> RepoResult {
    if repo.error.is_some() {
        // A discovery error row: `repo_state` returns it at once, with no gix call to bound.
        let state = rollcall_git::repo_state(repo, opts);
        return RepoResult {
            abandoned: false,
            state,
        };
    }
    let input = WorkerInput {
        repo: repo.clone(),
        opts: opts.clone(),
    };
    let Ok(input) = serde_json::to_vec(&input) else {
        // A path that is not UTF-8 cannot be written as JSON.
        return worker_failed();
    };
    if shared.no_more_spawns.load(Ordering::Acquire) {
        return worker_failed();
    }
    let workers = shared.workers;
    let spawned = Command::new(&workers.program)
        .args(&workers.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else {
        return worker_failed();
    };
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let started = Instant::now();
    let deadline = [opts.repo_budget, workers.grace]
        .iter()
        .try_fold(started, |at, d| at.checked_add(*d))
        .unwrap_or(started + FAR_FUTURE);
    let slot = Arc::new(Slot {
        pid: child.id(),
        child: Mutex::new(child),
        deadline,
        reason: Mutex::new(None),
    });
    {
        let mut live = lock(&shared.live);
        live.push(Arc::clone(&slot));
        // A probe failure that raced the spawn has already killed everything it could see.
        if shared.no_more_spawns.load(Ordering::Acquire) {
            slot.kill(Some(WORKER_FAILED));
        }
    }

    // Written and closed before stdout is read; a child that died early shows up as no result.
    if let Some(mut stdin) = stdin {
        let _ = stdin.write_all(&input);
    }
    let mut output = Vec::new();
    if let Some(stdout) = stdout {
        let _ = stdout
            .take(MAX_RESULT_BYTES as u64 + 1)
            .read_to_end(&mut output);
    }
    if output.len() > MAX_RESULT_BYTES {
        slot.kill(None);
    }
    let status = reap(&slot);
    lock(&shared.live).retain(|s| !Arc::ptr_eq(s, &slot));

    if status.is_some_and(|s| s.success()) {
        return parse_result(&output);
    }
    match *lock(&slot.reason) {
        Some(TIMED_OUT) => error_result(TIMED_OUT, true),
        Some(reason) => error_result(reason, false),
        None => worker_failed(),
    }
}

/// Waits for the child without holding its lock across the wait, so the monitor can still kill it.
fn reap(slot: &Slot) -> Option<ExitStatus> {
    loop {
        match lock(&slot.child).try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => thread::sleep(Duration::from_millis(1)),
            Err(_) => return None,
        }
    }
}

/// The one monitor thread (spec §4.4 (a), (b), (d)): kills children past their deadline or over
/// the memory limit, and on a probe failure kills them all and stops further spawns.
fn monitor(shared: &Shared) {
    while !shared.done.load(Ordering::Acquire) {
        thread::sleep(MONITOR_TICK);
        let live: Vec<Arc<Slot>> = lock(&shared.live).clone();
        if live.is_empty() {
            continue;
        }
        let now = Instant::now();
        for slot in live.iter().filter(|s| now >= s.deadline) {
            slot.kill(Some(TIMED_OUT));
        }
        let pids: Vec<u32> = live.iter().map(|s| s.pid).collect();
        match probe_ps(&shared.workers.ps, &pids) {
            Ok(rows) => {
                for pid in over_limit(&rows, &pids, shared.workers.rss_limit_kib) {
                    if let Some(slot) = live.iter().find(|s| s.pid == pid) {
                        slot.kill(Some(MEMORY_LIMIT));
                    }
                }
            }
            Err(why) => probe_failed(shared, &why),
        }
    }
}

fn probe_failed(shared: &Shared, why: &str) {
    // Under the registry lock, so a child registering concurrently either is killed here or
    // sees the flag and kills itself.
    let registry = lock(&shared.live);
    if !shared.no_more_spawns.swap(true, Ordering::AcqRel) {
        lock(&shared.notes).push(format!(
            "memory probe failed ({why}): the remaining repos' state was not read"
        ));
    }
    for slot in registry.iter() {
        slot.kill(Some(WORKER_FAILED));
    }
}

/// Production: each repo in a child of this same binary.
#[cfg(not(test))]
fn git_states(repos: &[DiscoveredRepo], opts: &GitOptions) -> Fanout {
    let Ok(program) = std::env::current_exe() else {
        return Fanout {
            results: repos.iter().map(|_| worker_failed()).collect(),
            notes: vec!["cannot locate rollcall's own executable: no git state was read".into()],
        };
    };
    let workers = Workers {
        program,
        args: vec!["__worker".into()],
        ps: PathBuf::from(PS_PATH),
        rss_limit_kib: RSS_LIMIT_KIB,
        grace: KILL_GRACE,
        parallelism: thread::available_parallelism().map_or(1, usize::from),
    };
    fan_out(&workers, repos, opts)
}

/// Unit tests run inside the test harness, whose `current_exe` is not rollcall: they call
/// `repo_state` in this process. The real child path is proven against the built binary.
#[cfg(test)]
fn git_states(repos: &[DiscoveredRepo], opts: &GitOptions) -> Fanout {
    let results = repos
        .iter()
        .map(|repo| {
            let state = rollcall_git::repo_state(repo, opts);
            let abandoned = state.pending.is_some();
            RepoResult { state, abandoned }
        })
        .collect();
    Fanout {
        results,
        notes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::sync::mpsc;
    use std::time::Instant;

    use tempfile::TempDir;

    use super::*;
    use crate::config::{Config, EnvPaths};

    const NOW: &str = "2026-09-29T12:00:00Z";
    const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
    const WATCHDOG: Duration = Duration::from_secs(60);

    fn code(n: u8) -> String {
        format!("{:?}", ExitCode::from(n))
    }

    fn exit_is(outcome: &ScanOutcome, n: u8) {
        assert_eq!(format!("{:?}", outcome.exit), code(n));
    }

    /// Runs `f` on a thread; fails the test if it does not finish inside the watchdog.
    fn within<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(WATCHDOG).expect("watchdog expired")
    }

    /// A scan that waits for the scan lock instead of answering from the cache. `flock` belongs
    /// to the open file description, and a child that another test thread forks while the lock
    /// is open keeps that description until it execs: a lock the previous scan dropped can still
    /// look held for a moment, which would make a follow-up `Fresh` scan answer stale.
    fn fresh(w: &World, root: &Path) -> ScanOutcome {
        within({
            let (ctx, root) = (w.ctx.clone(), root.to_path_buf());
            move || scan(&ctx, &root, ScanMode::WaitAtMost(WATCHDOG / 2))
        })
    }

    /// Takes the scan lock once the previous scan's has been released (see [`fresh`]).
    fn lock_when_free(w: &World) -> ScanLock {
        let deadline = Instant::now() + WATCHDOG / 2;
        loop {
            if let Some(lock) = rollcall_store::try_scan_lock(&w.ctx.cache_dir, &w.root).unwrap() {
                return lock;
            }
            assert!(
                Instant::now() < deadline,
                "the scan lock was never released"
            );
            std::thread::sleep(LOCK_POLL);
        }
    }

    struct World {
        _dir: TempDir,
        home: PathBuf,
        root: PathBuf,
        ctx: Ctx,
    }

    /// A HOME with empty agent dirs and a `work/` root inside it.
    fn world() -> World {
        let dir = TempDir::new().unwrap();
        let home = std::fs::canonicalize(dir.path()).unwrap();
        let root = home.join("work");
        std::fs::create_dir_all(&root).unwrap();
        let ctx = Ctx {
            config: Config::default(),
            now: NOW.parse().unwrap(),
            env: EnvPaths {
                home: Some(home.clone()),
                claude_config_dir: Some(home.join("claude")),
                codex_home: Some(home.join("codex")),
                ..EnvPaths::default()
            },
            cache_dir: home.join("cache"),
        };
        World {
            _dir: dir,
            home,
            root,
            ctx,
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_DATE", "2026-09-10T09:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-09-10T09:00:00Z")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    fn init_repo(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        git(path, &["init", "-q", "-b", "main"]);
        std::fs::write(path.join("f"), "x").unwrap();
        git(path, &["add", "f"]);
        git(path, &["commit", "-q", "-m", "c"]);
    }

    fn open(w: &World) -> SqliteIndex {
        SqliteIndex::open(&w.ctx.cache_dir).unwrap()
    }

    fn write_claude_session(w: &World, cwd: &Path) {
        let dir = w.home.join("claude/projects/p");
        std::fs::create_dir_all(&dir).unwrap();
        let record = serde_json::json!({
            "type": "user", "sessionId": ID, "timestamp": "2026-09-28T18:04:11.532Z",
            "cwd": cwd, "gitBranch": "main", "isSidechain": false,
            "message": {"role": "user", "content": "hello"}
        });
        std::fs::write(dir.join(format!("{ID}.jsonl")), format!("{record}\n")).unwrap();
    }

    // ---- orchestration ----

    #[test]
    fn a_lock_held_elsewhere_answers_from_the_cache_unchanged() {
        let mut w = world();
        init_repo(&w.root.join("a"));
        let first = within({
            let (ctx, root) = (w.ctx.clone(), w.root.clone());
            move || scan(&ctx, &root, ScanMode::Fresh).index.scanned_at(&root)
        });
        assert_eq!(first, Some(NOW.parse().unwrap()));
        let before = open(&w).repos_for_root(&w.root);
        assert_eq!(before.len(), 1);

        w.ctx.now = "2026-09-30T00:00:00Z".parse().unwrap();
        let _held = lock_when_free(&w);
        let outcome = within({
            let (ctx, root) = (w.ctx.clone(), w.root.clone());
            move || scan(&ctx, &root, ScanMode::Fresh)
        });
        assert!(outcome.stale);
        exit_is(&outcome, 0);
        assert_eq!(outcome.index.repos_for_root(&w.root), before);
        assert_eq!(outcome.index.scanned_at(&w.root), first);
    }

    #[test]
    fn a_lock_held_with_no_cache_has_no_scanned_at_and_exits_1() {
        let w = world();
        rollcall_store::ensure_cache_dir(&w.ctx.cache_dir).unwrap();
        let _held = rollcall_store::try_scan_lock(&w.ctx.cache_dir, &w.root)
            .unwrap()
            .unwrap();
        let outcome = scan(&w.ctx, &w.root, ScanMode::Fresh);
        assert!(outcome.stale);
        assert_eq!(outcome.index.scanned_at(&w.root), None);
        exit_is(&outcome, 1);
    }

    #[test]
    fn no_scan_without_a_cache_exits_1() {
        let w = world();
        let outcome = scan(&w.ctx, &w.root, ScanMode::NoScan);
        assert!(outcome.stale);
        assert_eq!(outcome.index.scanned_at(&w.root), None);
        exit_is(&outcome, 1);
    }

    #[test]
    fn a_clean_scan_exits_0_and_is_not_stale() {
        let w = world();
        init_repo(&w.root.join("a"));
        let outcome = within({
            let (ctx, root) = (w.ctx.clone(), w.root.clone());
            move || scan(&ctx, &root, ScanMode::Fresh)
        });
        assert!(!outcome.stale);
        exit_is(&outcome, 0);
        let rows = outcome.index.repos_for_root(&w.root);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, RepoKind::Normal);
        assert_eq!(rows[0].name, "a");
        assert_eq!(rows[0].state.error, None);
    }

    #[test]
    fn an_error_row_exits_1() {
        let w = world();
        let bad = w.root.join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join(".git"), "gitdir: ../does-not-exist\n").unwrap();
        let outcome = within({
            let (ctx, root) = (w.ctx.clone(), w.root.clone());
            move || scan(&ctx, &root, ScanMode::Fresh)
        });
        assert!(!outcome.stale);
        exit_is(&outcome, 1);
        let rows = outcome.index.repos_for_root(&w.root);
        assert!(
            rows[0]
                .state
                .error
                .as_deref()
                .unwrap()
                .starts_with("bad gitdir: ")
        );
        // A cache answer recomputes the same code from the stored rows.
        exit_is(&scan(&w.ctx, &w.root, ScanMode::NoScan), 1);
    }

    #[test]
    fn a_partial_walk_exits_1() {
        let mut w = world();
        init_repo(&w.root.join("a"));
        w.ctx.config.walk_budget_entries = 1;
        let outcome = within({
            let (ctx, root) = (w.ctx.clone(), w.root.clone());
            move || scan(&ctx, &root, ScanMode::Fresh)
        });
        exit_is(&outcome, 1);
        assert!(
            outcome
                .index
                .last_diagnostics(&w.root)
                .unwrap()
                .walk
                .budget_hit
        );
    }

    #[test]
    fn a_refused_root_exits_3_unless_confirmed() {
        let mut w = world();
        init_repo(&w.root.join("a"));
        let refused = within({
            let (ctx, home) = (w.ctx.clone(), w.home.clone());
            move || scan(&ctx, &home, ScanMode::Fresh)
        });
        exit_is(&refused, 3);
        assert_eq!(refused.index.scanned_at(&w.home), None);

        w.ctx.config.yes = true;
        let yes = fresh(&w, &w.home);
        assert!(!yes.stale);
        assert_eq!(yes.index.repos_for_root(&w.home).len(), 1);

        // Config `roots` count as confirmed (§4.3), compared by canonical path.
        w.ctx.config.yes = false;
        w.ctx.config.roots = vec![w.home.join("work/..")];
        let listed = fresh(&w, &w.home);
        assert!(!listed.stale);
    }

    #[test]
    fn an_unreadable_codex_db_keeps_claude_and_git_results_and_notes_why() {
        let w = world();
        let repo = w.root.join("a");
        init_repo(&repo);
        write_claude_session(&w, &repo);
        std::fs::create_dir_all(w.home.join("codex")).unwrap();
        std::fs::write(w.home.join("codex/state_5.sqlite"), "not a database").unwrap();
        let outcome = within({
            let (ctx, root) = (w.ctx.clone(), w.root.clone());
            move || scan(&ctx, &root, ScanMode::Fresh)
        });
        assert!(!outcome.stale);
        let rows = outcome.index.repos_for_root(&w.root);
        assert_eq!(rows.len(), 1);
        let sessions = outcome.index.sessions_for(&w.root, &repo);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].source, SourceKind::Claude);
        let doctor = outcome.index.last_diagnostics(&w.root).unwrap();
        assert!(!doctor.notes.is_empty(), "{doctor:?}");
        let claude = doctor
            .sources
            .iter()
            .find(|s| s.source == SourceKind::Claude)
            .unwrap();
        assert_eq!(claude.matched, 1);
        assert_eq!(claude.sessions, 1);
    }

    #[test]
    fn sessions_are_matched_by_canonical_cwd_and_the_rest_counted_unmatched() {
        let w = world();
        let repo = w.root.join("a");
        init_repo(&repo);
        // Reached through a `..` hop and a subdirectory: matching is by canonical path.
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        write_claude_session(&w, &repo.join("sub/../sub"));
        let outcome = within({
            let (ctx, root) = (w.ctx.clone(), w.root.clone());
            move || scan(&ctx, &root, ScanMode::Fresh)
        });
        assert_eq!(outcome.index.sessions_for(&w.root, &repo).len(), 1);

        write_claude_session(&w, &w.home.join("elsewhere"));
        let outcome = fresh(&w, &w.root);
        assert!(outcome.index.sessions_for(&w.root, &repo).is_empty());
        let claude = &outcome.index.last_diagnostics(&w.root).unwrap().sources[0];
        assert_eq!((claude.matched, claude.unmatched.outside_root), (0, 1));
    }

    #[test]
    fn two_scans_at_the_same_now_commit_identical_rows() {
        let w = world();
        let repo = w.root.join("a");
        init_repo(&repo);
        git(&repo, &["branch", "other"]);
        write_claude_session(&w, &repo);
        let run = || {
            let outcome = fresh(&w, &w.root);
            let idx = outcome.index;
            let mut doctor = idx.last_diagnostics(&w.root).unwrap();
            // `records` counts what this scan read; the second one reads nothing new.
            doctor.sources.iter_mut().for_each(|s| s.records = 0);
            (
                idx.repos_for_root(&w.root),
                idx.scanned_at(&w.root),
                doctor,
                idx.sessions_for(&w.root, &repo),
            )
        };
        let (first, second) = (run(), run());
        assert!(!first.0.is_empty());
        assert_eq!(first, second);
    }

    #[test]
    fn no_scan_with_another_submodules_flag_than_the_stored_scan_is_stale() {
        let mut w = world();
        init_repo(&w.root.join("a"));
        let stored = within({
            let (ctx, root) = (w.ctx.clone(), w.root.clone());
            move || scan(&ctx, &root, ScanMode::Fresh)
        });
        assert!(!stored.stale);
        assert_eq!(stored.index.scanned_submodules(&w.root), Some(false));
        w.ctx.config.submodules = true;
        let cached = scan(&w.ctx, &w.root, ScanMode::NoScan);
        assert!(cached.stale);
        assert_eq!(cached.index.scanned_submodules(&w.root), Some(false));
    }

    #[test]
    fn wait_at_most_answers_stale_in_time_and_the_detached_scan_commits_later() {
        let w = world();
        init_repo(&w.root.join("a"));
        rollcall_git::set_state_delay(&w.root, Some(Duration::from_millis(600)));
        let started = Instant::now();
        let outcome = within({
            let (ctx, root) = (w.ctx.clone(), w.root.clone());
            move || scan(&ctx, &root, ScanMode::WaitAtMost(Duration::from_millis(50)))
        });
        let waited = started.elapsed();
        assert!(outcome.stale);
        assert_eq!(outcome.index.scanned_at(&w.root), None);
        assert!(waited < Duration::from_millis(450), "{waited:?}");

        let deadline = Instant::now() + Duration::from_secs(20);
        let committed = loop {
            let fresh = open(&w);
            if let Some(at) = fresh.scanned_at(&w.root) {
                break at;
            }
            assert!(Instant::now() < deadline, "detached scan never committed");
            std::thread::sleep(Duration::from_millis(20));
        };
        rollcall_git::set_state_delay(&w.root, None);
        assert_eq!(committed, NOW.parse().unwrap());
    }

    #[test]
    fn wait_at_most_returns_the_scan_when_it_finishes_inside_the_wait() {
        let w = world();
        init_repo(&w.root.join("a"));
        let outcome = within({
            let (ctx, root) = (w.ctx.clone(), w.root.clone());
            move || scan(&ctx, &root, ScanMode::WaitAtMost(Duration::from_secs(30)))
        });
        assert!(!outcome.stale);
        assert_eq!(outcome.index.repos_for_root(&w.root).len(), 1);
    }

    // ---- worker process: protocol ----

    fn discovered(w: &World) -> (DiscoveredRepo, GitOptions) {
        let repo = w.root.join("a");
        init_repo(&repo);
        let walk = WalkOptions {
            home: w.home.clone(),
            confirmed: false,
            follow_symlinks: false,
            submodules: false,
            skip_dirs: Vec::new(),
            budget_entries: 10_000,
            budget: Duration::from_secs(10),
        };
        let found = rollcall_git::discover(&w.root, &walk).repos;
        assert_eq!(found.len(), 1);
        let opts = GitOptions {
            root: w.root.clone(),
            home: w.home.clone(),
            skip_paths: Vec::new(),
            repo_budget: Duration::from_secs(30),
            excludes_file: None,
        };
        (found.into_iter().next().unwrap(), opts)
    }

    #[test]
    fn the_worker_round_trips_a_repo_to_the_in_process_state() {
        let w = world();
        let (repo, opts) = discovered(&w);
        let input = serde_json::to_vec(&WorkerInput {
            repo: repo.clone(),
            opts: opts.clone(),
        })
        .unwrap();
        let mut out = Vec::new();
        run_worker(input.as_slice(), &mut out).unwrap();
        let result: RepoResult = serde_json::from_slice(&out).unwrap();
        let direct = rollcall_git::repo_state(&repo, &opts);
        assert_eq!(direct.error, None);
        assert_eq!(result.state, direct);
        assert!(!result.abandoned);
        assert_eq!(result.state.worktrees.len(), 1);
    }

    #[test]
    fn the_worker_writes_nothing_for_unreadable_input() {
        for input in [&b""[..], b"{", b"{\"repo\":1}"] {
            let mut out = Vec::new();
            assert!(run_worker(input, &mut out).is_err(), "{input:?}");
            assert!(out.is_empty());
        }
    }

    fn failed(result: &RepoResult) -> bool {
        result.state.error.as_deref() == Some("git error: worker failed")
    }

    #[test]
    fn the_result_parser_maps_empty_truncated_and_oversize_output_to_worker_failed() {
        let good = serde_json::to_vec(&RepoResult {
            state: RepoState {
                default_branch: Some("main".into()),
                has_remote: false,
                error: None,
                worktrees: Vec::new(),
                pending: None,
            },
            abandoned: false,
        })
        .unwrap();
        let parsed = parse_result(&good);
        assert_eq!(parsed.state.default_branch.as_deref(), Some("main"));
        assert!(!failed(&parsed));

        assert!(failed(&parse_result(b"")));
        assert!(failed(&parse_result(&good[..good.len() - 1])));
        assert!(failed(&parse_result(b"not json")));
        let mut padded = good.clone();
        padded.resize(MAX_RESULT_BYTES, b' ');
        assert!(!failed(&parse_result(&padded)), "exactly the cap is fine");
        padded.push(b' ');
        assert!(failed(&parse_result(&padded)), "one byte over is not");
    }

    // ---- worker process: ps probe ----

    #[test]
    fn the_ps_parser_handles_procps_and_bsd_formats() {
        let procps = "    1 204\n 4242 1048576\n";
        let bsd = "  1   204\n4242 1048576\n";
        let want = vec![(1, 204), (4242, 1_048_576)];
        assert_eq!(parse_ps(procps).unwrap(), want);
        assert_eq!(parse_ps(bsd).unwrap(), want);
        assert_eq!(parse_ps("").unwrap(), Vec::new());
        assert_eq!(parse_ps("\n  \n").unwrap(), Vec::new());
    }

    #[test]
    fn the_ps_parser_rejects_output_it_cannot_read() {
        for bad in [
            "PID RSS\n",
            "1\n",
            "1 2 3\n",
            "x 2\n",
            "1 -2\n",
            "  1 2\nusage: ps\n",
        ] {
            assert!(parse_ps(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn exited_pids_are_ignored_when_picking_children_over_the_limit() {
        // pid 11 has exited (absent from ps); pid 99 is not one of ours.
        let rows = [(10, 5), (12, 400_000), (99, 900_000)];
        assert_eq!(over_limit(&rows, &[10, 11, 12], 320 * 1024), vec![12]);
        assert!(over_limit(&rows, &[10, 11], 320 * 1024).is_empty());
        assert_eq!(
            over_limit(&[(10, 320 * 1024)], &[10], 320 * 1024),
            Vec::<u32>::new()
        );
    }

    fn fake_ps(dir: &Path, name: &str, script: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn a_ps_exit_status_is_ignored_and_empty_output_is_no_live_children() {
        let dir = TempDir::new().unwrap();
        let empty_exit_1 = fake_ps(dir.path(), "ps1", "exit 1");
        assert_eq!(probe_ps(&empty_exit_1, &[1, 2]).unwrap(), Vec::new());
        let empty_exit_0 = fake_ps(dir.path(), "ps0", "exit 0");
        assert_eq!(probe_ps(&empty_exit_0, &[1]).unwrap(), Vec::new());
        // Mixed live and dead, exit 0; and the same with a non-zero status.
        let mixed = fake_ps(dir.path(), "mixed", "printf '  10 5\\n 12 400000\\n'");
        assert_eq!(
            probe_ps(&mixed, &[10, 11, 12]).unwrap(),
            vec![(10, 5), (12, 400_000)]
        );
        let mixed_1 = fake_ps(dir.path(), "mixed1", "printf '  10 5\\n'; exit 1");
        assert_eq!(probe_ps(&mixed_1, &[10, 11]).unwrap(), vec![(10, 5)]);
    }

    #[test]
    fn a_ps_that_cannot_run_or_prints_nonsense_is_a_probe_failure() {
        let dir = TempDir::new().unwrap();
        assert!(probe_ps(&dir.path().join("missing"), &[1]).is_err());
        let nonsense = fake_ps(dir.path(), "junk", "echo 'ps: illegal option'; exit 1");
        assert!(probe_ps(&nonsense, &[1]).is_err());
    }

    #[test]
    fn the_real_ps_reports_this_process() {
        let me = std::process::id();
        let rows = probe_ps(Path::new(PS_PATH), &[me]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, me);
        assert!(rows[0].1 > 0);
    }

    // ---- worker process: the parent's bounds, against stand-in workers ----

    fn repo_at(path: &str) -> DiscoveredRepo {
        DiscoveredRepo {
            git_dir: PathBuf::from(path),
            workdir: None,
            kind: RepoKind::Bare,
            contains_root: false,
            submodule_of: None,
            linked: Vec::new(),
            error: None,
        }
    }

    fn git_opts(budget: Duration) -> GitOptions {
        GitOptions {
            root: PathBuf::from("/"),
            home: PathBuf::from("/"),
            skip_paths: Vec::new(),
            repo_budget: budget,
            excludes_file: None,
        }
    }

    /// `sh -c script` standing in for the worker; it always drains stdin first.
    fn sh_workers(script: &str) -> Workers {
        Workers {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), format!("cat >/dev/null; {script}").into()],
            ps: PathBuf::from(PS_PATH),
            rss_limit_kib: RSS_LIMIT_KIB,
            grace: KILL_GRACE,
            parallelism: 1,
        }
    }

    fn good_output(dir: &Path) -> PathBuf {
        let path = dir.join("good.json");
        let result = RepoResult {
            state: RepoState {
                default_branch: Some("main".into()),
                has_remote: true,
                error: None,
                worktrees: Vec::new(),
                pending: None,
            },
            abandoned: false,
        };
        std::fs::write(&path, serde_json::to_vec(&result).unwrap()).unwrap();
        path
    }

    fn error_of(fanout: &Fanout, i: usize) -> Option<&str> {
        fanout.results[i].state.error.as_deref()
    }

    #[test]
    fn a_worker_that_prints_a_result_and_exits_cleanly_is_read() {
        let dir = TempDir::new().unwrap();
        let good = good_output(dir.path());
        let workers = sh_workers(&format!("cat '{}'", good.display()));
        let out = within(move || {
            fan_out(
                &workers,
                &[repo_at("/x")],
                &git_opts(Duration::from_secs(5)),
            )
        });
        assert_eq!(out.results[0].state.default_branch.as_deref(), Some("main"));
        assert_eq!(error_of(&out, 0), None);
        assert!(out.notes.is_empty());
    }

    #[test]
    fn a_worker_that_crashes_prints_garbage_or_exits_non_zero_is_worker_failed() {
        let dir = TempDir::new().unwrap();
        let good = good_output(dir.path());
        for script in [
            "exit 7".to_string(),
            "printf garbage".to_string(),
            "kill -9 $$".to_string(),
            format!("cat '{}'; exit 3", good.display()),
        ] {
            let workers = sh_workers(&script);
            let out = within(move || {
                fan_out(
                    &workers,
                    &[repo_at("/x")],
                    &git_opts(Duration::from_secs(5)),
                )
            });
            assert_eq!(
                error_of(&out, 0),
                Some("git error: worker failed"),
                "{script}"
            );
            assert!(!out.results[0].abandoned);
        }
    }

    #[test]
    fn a_worker_that_cannot_be_spawned_is_worker_failed() {
        let mut workers = sh_workers("true");
        workers.program = PathBuf::from("/nonexistent/rollcall");
        let out = fan_out(
            &workers,
            &[repo_at("/x")],
            &git_opts(Duration::from_secs(5)),
        );
        assert_eq!(error_of(&out, 0), Some("git error: worker failed"));
    }

    #[test]
    fn output_over_the_cap_is_worker_failed_and_the_worker_is_killed() {
        let workers = sh_workers("yes | head -c 20000000; exec sleep 30");
        let started = Instant::now();
        let out = within(move || {
            fan_out(
                &workers,
                &[repo_at("/x")],
                &git_opts(Duration::from_secs(20)),
            )
        });
        assert_eq!(error_of(&out, 0), Some("git error: worker failed"));
        assert!(started.elapsed() < Duration::from_secs(15));
    }

    #[test]
    fn a_path_that_is_not_utf8_cannot_cross_and_is_worker_failed() {
        let workers = sh_workers("true");
        let bad = repo_at("/tmp");
        let bad = DiscoveredRepo {
            git_dir: PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/\xff")),
            ..bad
        };
        let out = fan_out(&workers, &[bad], &git_opts(Duration::from_secs(5)));
        assert_eq!(error_of(&out, 0), Some("git error: worker failed"));
    }

    #[test]
    fn a_worker_still_running_at_the_budget_plus_grace_is_killed_as_timed_out() {
        let mut workers = sh_workers("exec sleep 30");
        workers.grace = Duration::from_millis(150);
        let started = Instant::now();
        let out = within(move || {
            fan_out(
                &workers,
                &[repo_at("/x")],
                &git_opts(Duration::from_millis(50)),
            )
        });
        assert_eq!(error_of(&out, 0), Some("timed out"));
        assert!(out.results[0].abandoned);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_worker_over_the_memory_limit_is_killed() {
        let mut workers = sh_workers("exec sleep 30");
        workers.rss_limit_kib = 0;
        workers.grace = Duration::from_secs(120);
        let out = within(move || {
            fan_out(
                &workers,
                &[repo_at("/x")],
                &git_opts(Duration::from_secs(120)),
            )
        });
        assert_eq!(error_of(&out, 0), Some("git error: memory limit"));
        assert!(out.notes.is_empty());
    }

    #[test]
    fn a_failing_ps_kills_live_workers_spawns_no_more_and_notes_why() {
        let dir = TempDir::new().unwrap();
        let junk = fake_ps(dir.path(), "junk", "echo nonsense");
        for ps in [dir.path().join("missing"), junk] {
            let mut workers = sh_workers("exec sleep 30");
            workers.ps = ps;
            workers.grace = Duration::from_secs(120);
            let repos = [repo_at("/x"), repo_at("/y"), repo_at("/z")];
            let out =
                within(move || fan_out(&workers, &repos, &git_opts(Duration::from_secs(120))));
            for i in 0..3 {
                assert_eq!(error_of(&out, i), Some("git error: worker failed"), "{i}");
            }
            assert_eq!(out.notes.len(), 1, "{:?}", out.notes);
        }
    }

    #[test]
    fn a_ps_exiting_1_with_no_output_does_not_fail_the_scan() {
        let dir = TempDir::new().unwrap();
        let good = good_output(dir.path());
        let mut workers = sh_workers(&format!("sleep 0.2; cat '{}'", good.display()));
        workers.ps = fake_ps(dir.path(), "ps", "exit 1");
        let out = within(move || {
            fan_out(
                &workers,
                &[repo_at("/x")],
                &git_opts(Duration::from_secs(5)),
            )
        });
        assert_eq!(error_of(&out, 0), None);
        assert!(out.notes.is_empty());
    }

    #[test]
    fn repos_keep_their_order_and_error_rows_are_not_spawned() {
        let dir = TempDir::new().unwrap();
        let good = good_output(dir.path());
        let mut workers = sh_workers(&format!("cat '{}'", good.display()));
        workers.parallelism = 3;
        let mut errored = repo_at("/e");
        errored.error = Some("bad gitdir: x".into());
        let repos = [repo_at("/a"), errored, repo_at("/c")];
        let out = within(move || fan_out(&workers, &repos, &git_opts(Duration::from_secs(5))));
        assert_eq!(error_of(&out, 0), None);
        assert_eq!(error_of(&out, 1), Some("bad gitdir: x"));
        assert_eq!(error_of(&out, 2), None);
    }
}
