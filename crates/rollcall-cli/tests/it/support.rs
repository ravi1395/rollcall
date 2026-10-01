//! Test harness (spec 10.1): manifest types, env sandbox, watchdog runner, expected-file loading,
//! schema validation and the fixture roots. Builders (P27b, P28) plug in through `build_root`.
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const ROLLCALL_BIN: &str = env!("CARGO_BIN_EXE_rollcall");
const TIME_BIN: &str = "/usr/bin/time";
const REQUIRE_TOOLS_VAR: &str = "ROLLCALL_REQUIRE_TOOLS";
const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// Generous default for gates with no spec time bound; bounded gates pass their own deadline.
pub const RUN_DEADLINE: Duration = Duration::from_secs(60);
const SCHEMAS: [&str; 4] = ["status", "doctor", "where_was_i", "list_stale"];

// ---------------------------------------------------------------- manifest

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub meta: Meta,
    pub home: Home,
    pub repo: Vec<Repo>,
    pub claude: Vec<Claude>,
    pub codex_thread: Vec<CodexThread>,
    pub codex_rollout: Vec<CodexRollout>,
    pub big_log: Vec<BigLog>,
    pub expect_counts: ExpectCounts,
    pub matching: Matching,
    pub perf: Perf,
    pub mcp_200: Mcp200,
    pub demo: Demo,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    pub now: String,
    pub author: Person,
    pub committer: Person,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Person {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Home {
    pub files: BTreeMap<String, String>,
    pub expected_suggestions: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum RootId {
    F,
    #[serde(rename = "EXT")]
    Ext,
    H,
    #[serde(rename = "budget")]
    Budget,
    #[serde(rename = "home")]
    Home,
    #[serde(rename = "perf")]
    Perf,
    #[serde(rename = "demo")]
    Demo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Normal,
    Bare,
    Nested,
    Submodule,
    SubmoduleOld,
    Special,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Special {
    FifoGit,
    SymlinkFanout,
    SymlinkOutOfRoot,
    UnreadableDir,
    Corrupted,
    BadGitdir,
    UntrustedPath,
    Drivers,
    SubmoduleLfs,
    WorktreeEscape,
    CommondirEscape,
    SymlinkedFiles,
    WorktreeConfigSymlink,
    BigObject,
    HangProbe,
    HangProbeWorktreeConfig,
    EnclosingRoot,
    TreeBomb,
    TreeBombLong,
    ExcludesFile,
    IndexBomb,
    IndexDeepOk,
    NestedProbe,
    ExecType,
    BigTracked,
    PatternBlobBomb,
    RenameBomb,
    GitoxideKeys,
    PackBomb,
    ObjectsPackFlood,
    TreeDeepBig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repo {
    pub id: String,
    pub root: RootId,
    pub path: String,
    pub kind: Kind,
    pub special: Option<Special>,
    /// Absent means true.
    pub in_clean: Option<bool>,
    pub target: Option<String>,
    #[serde(default)]
    pub covers: Vec<String>,
    pub parent: Option<String>,
    #[serde(default)]
    pub commits: Vec<Commit>,
    #[serde(default)]
    pub branches: BTreeMap<String, usize>,
    pub head: Option<String>,
    #[serde(default)]
    pub remotes: Vec<Remote>,
    #[serde(default)]
    pub upstreams: BTreeMap<String, String>,
    pub origin_head: Option<String>,
    #[serde(default)]
    pub worktrees: Vec<WorktreeSpec>,
    pub state: Option<RepoState>,
    #[serde(default)]
    pub edit: BTreeMap<String, String>,
    #[serde(default)]
    pub config: BTreeMap<String, String>,
}

impl Repo {
    pub fn in_clean(&self) -> bool {
        self.in_clean.unwrap_or(true)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Commit {
    pub branch: String,
    pub message: String,
    pub date: String,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Remote {
    pub name: String,
    pub target_repo_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeSpec {
    pub path: String,
    pub branch: Option<String>,
    #[serde(default)]
    pub missing: bool,
    #[serde(default)]
    pub external: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoState {
    #[serde(default)]
    pub staged: Vec<String>,
    #[serde(default)]
    pub unstaged: Vec<String>,
    #[serde(default)]
    pub untracked: Vec<String>,
    #[serde(default)]
    pub ignored: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corpus {
    Matching,
    ClaudeRules,
    CodexRules,
    Incremental,
    Privacy,
    Sanitise,
    Shell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Append,
    Truncate,
    Replace,
    SymlinkSwap,
    Delete,
    Move,
}

/// `truncate.to` is a line count; `move.to` is a path (an untagged enum, per the manifest notes).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum StepTo {
    Count(u64),
    Path(String),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub op: Op,
    #[serde(default)]
    pub lines: Vec<String>,
    pub to: Option<StepTo>,
    #[serde(default = "default_true")]
    pub then_scan: bool,
    pub expect: Option<StepExpect>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepExpect {
    pub records: Option<u64>,
    pub offset: Option<u64>,
    pub sessions: Option<u64>,
    pub vanished: Option<u64>,
    pub message_count: Option<u64>,
    pub title: Option<String>,
    pub title_kind: Option<String>,
    pub resolves: Option<bool>,
}

/// Absent key means null (TOML has no null).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    pub title: Option<String>,
    pub title_kind: Option<String>,
    pub last_prompt: Option<String>,
    pub branch: Option<String>,
    pub cwd: Option<String>,
    pub message_count: Option<u64>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeRef {
    pub repo: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claude {
    pub id: String,
    pub project_dir: String,
    pub file: String,
    pub corpus: Corpus,
    #[serde(default)]
    pub lines: Vec<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
    pub expect: Option<Expect>,
    pub excluded: Option<String>,
    pub worktree: Option<WorktreeRef>,
    pub unmatched: Option<String>,
    #[serde(default)]
    pub covers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexThread {
    pub id: String,
    pub corpus: Corpus,
    pub rollout_id: Option<String>,
    pub rollout_path: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub source: String,
    pub cwd: String,
    pub title: Option<String>,
    pub git_branch: Option<String>,
    #[serde(default)]
    pub archived: bool,
    pub first_user_message: Option<String>,
    pub spawn_parent: Option<String>,
    pub expect: Option<Expect>,
    pub excluded: Option<String>,
    pub worktree: Option<WorktreeRef>,
    #[serde(default)]
    pub covers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexRollout {
    pub id: String,
    pub file: String,
    pub corpus: Corpus,
    #[serde(default)]
    pub lines: Vec<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
    pub expect: Option<Expect>,
    pub excluded: Option<String>,
    pub worktree: Option<WorktreeRef>,
    #[serde(default)]
    pub covers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BigLog {
    pub source: String,
    pub bytes: u64,
    pub single_line: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectCounts {
    #[serde(rename = "F_repos")]
    pub f_repos: u32,
    #[serde(rename = "F_worktrees")]
    pub f_worktrees: u32,
    pub claude_sessions: u32,
    pub codex_db_sessions: u32,
    pub codex_rollout_sessions: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Matching {
    pub matched: u32,
    pub non_repo: u32,
    pub path_gone: u32,
    pub outside_root: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Perf {
    pub repos: u32,
    pub linked_worktrees: u32,
    pub logs: u32,
    pub bytes: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mcp200 {
    pub sessions: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Demo {
    pub repos: Vec<String>,
    pub claude: Vec<DemoClaude>,
    pub codex_thread: Vec<DemoThread>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemoClaude {
    pub id: String,
    pub project_dir: String,
    pub file: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemoThread {
    pub id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub source: String,
    pub cwd: String,
    pub title: Option<String>,
    pub git_branch: Option<String>,
    #[serde(default)]
    pub archived: bool,
    pub first_user_message: Option<String>,
}

pub fn parse_manifest(text: &str) -> Result<Manifest, toml::de::Error> {
    toml::from_str(text)
}

/// The workspace root (two levels above this crate).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn manifest_path() -> PathBuf {
    repo_root().join("tests/fixtures/MANIFEST.toml")
}

pub fn manifest() -> &'static Manifest {
    static MANIFEST: OnceLock<Manifest> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        let text = std::fs::read_to_string(manifest_path()).expect("read MANIFEST.toml");
        parse_manifest(&text).expect("MANIFEST.toml parses")
    })
}

// ------------------------------------------------------------------- tools

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let hit = std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())?;
    std::path::absolute(hit).ok()
}

/// The absolute `git` every fixture build and sandboxed `PATH` uses.
pub fn git_path() -> &'static Path {
    static GIT: OnceLock<PathBuf> = OnceLock::new();
    GIT.get_or_init(|| find_in_path("git").expect("git is required to build fixtures"))
}

/// Path of an optional tool (a bare name searched in `PATH`, or an absolute path). When it is absent the
/// caller's test skips (`None`), unless `ROLLCALL_REQUIRE_TOOLS=1` (CI), where absence fails the test.
pub fn require_tool(name: &str) -> Option<PathBuf> {
    let found = if name.contains('/') {
        Path::new(name).exists().then(|| PathBuf::from(name))
    } else {
        find_in_path(name)
    };
    if found.is_none() {
        assert!(
            std::env::var_os(REQUIRE_TOOLS_VAR).is_none_or(|v| v != "1"),
            "required tool `{name}` is missing and {REQUIRE_TOOLS_VAR}=1"
        );
        eprintln!("skip: `{name}` is not installed");
    }
    found
}

// ---------------------------------------------------------------- watchdog

fn drain(pipe: Option<impl Read + Send + 'static>) -> JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            pipe.read_to_end(&mut buf)?;
        }
        Ok(buf)
    })
}

fn collect(reader: JoinHandle<std::io::Result<Vec<u8>>>) -> Result<Vec<u8>, String> {
    reader
        .join()
        .map_err(|_| "pipe reader thread panicked".to_string())?
        .map_err(|e| format!("reading child output: {e}"))
}

/// Runs `cmd` to completion. The child is polled with `try_wait` against `deadline`; on expiry it is killed
/// and `Err` returned. On expiry the reader threads are left to end when the pipes close, so a grandchild
/// holding a pipe cannot delay the failure.
pub fn spawn_watchdog(mut cmd: Command, deadline: Duration) -> Result<Output, String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn {:?}: {e}", cmd.get_program()))?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let end = Instant::now() + deadline;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= end => {
                // The child may have exited between the poll and the kill; either way it is gone.
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("killed by the watchdog after {deadline:?}"));
            }
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
            Err(e) => return Err(format!("try_wait: {e}")),
        }
    };
    Ok(Output {
        status,
        stdout: collect(stdout)?,
        stderr: collect(stderr)?,
    })
}

/// `max resident set size` of `cmd`, in bytes, via `/usr/bin/time` (GNU `-v` reports kbytes, macOS `-l`
/// bytes). The wrapper has exactly `cmd`'s explicit environment, so build `cmd` with `Sandbox::command`.
/// Callers gate on `require_tool("/usr/bin/time")` first.
pub fn peak_rss(cmd: Command, deadline: Duration) -> (Output, u64) {
    let flag = if cfg!(target_os = "macos") {
        "-l"
    } else {
        "-v"
    };
    let mut timed = Command::new(TIME_BIN);
    timed
        .env_clear()
        .arg(flag)
        .arg(cmd.get_program())
        .args(cmd.get_args());
    for (key, value) in cmd.get_envs() {
        if let Some(value) = value {
            timed.env(key, value);
        }
    }
    if let Some(dir) = cmd.get_current_dir() {
        timed.current_dir(dir);
    }
    let out = spawn_watchdog(timed, deadline).unwrap_or_else(|e| panic!("peak_rss: {e}"));
    let report = String::from_utf8_lossy(&out.stderr).into_owned();
    let rss = parse_max_rss(&report)
        .unwrap_or_else(|| panic!("no max-RSS line in {TIME_BIN} output:\n{report}"));
    (out, rss)
}

fn parse_max_rss(report: &str) -> Option<u64> {
    report.lines().find_map(|line| {
        let line = line.trim();
        if cfg!(target_os = "macos") {
            line.strip_suffix("maximum resident set size")?
                .trim()
                .parse()
                .ok()
        } else {
            let kbytes: u64 = line
                .strip_prefix("Maximum resident set size (kbytes):")?
                .trim()
                .parse()
                .ok()?;
            Some(kbytes * 1024)
        }
    })
}

// ----------------------------------------------------------------- sandbox

/// Every test's environment: all paths are temp directories, `ROLLCALL_NOW` is the manifest's instant.
pub struct Sandbox {
    pub home: PathBuf,
    pub xdg_config: PathBuf,
    pub xdg_cache: PathBuf,
    pub xdg_data: PathBuf,
    pub xdg_state: PathBuf,
    pub claude_config_dir: PathBuf,
    pub codex_home: PathBuf,
}

impl Sandbox {
    /// Creates the sandbox directories under `dir` (canonical).
    pub fn new(dir: &Path) -> Sandbox {
        let sub = |name: &str| {
            let p = dir.join(name);
            std::fs::create_dir_all(&p).expect("create sandbox dir");
            p
        };
        Sandbox {
            home: sub("home"),
            xdg_config: sub("xdg/config"),
            xdg_cache: sub("xdg/cache"),
            xdg_data: sub("xdg/data"),
            xdg_state: sub("xdg/state"),
            claude_config_dir: sub("claude"),
            codex_home: sub("codex"),
        }
    }

    pub fn claude_projects(&self) -> PathBuf {
        self.claude_config_dir.join("projects")
    }

    /// A command with nothing but the sandbox environment; `PATH` holds the absolute git's directory.
    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let git_dir = git_path().parent().expect("git has a parent directory");
        let path = std::env::join_paths([git_dir, Path::new("/usr/bin"), Path::new("/bin")])
            .expect("PATH entries contain no ':'");
        let mut cmd = Command::new(program);
        cmd.env_clear()
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.xdg_config)
            .env("XDG_CACHE_HOME", &self.xdg_cache)
            .env("XDG_DATA_HOME", &self.xdg_data)
            .env("XDG_STATE_HOME", &self.xdg_state)
            .env("CLAUDE_CONFIG_DIR", &self.claude_config_dir)
            .env("CODEX_HOME", &self.codex_home)
            .env("ROLLCALL_NOW", &manifest().meta.now)
            .env("PATH", path);
        cmd
    }

    /// The `rollcall` binary under the sandbox; `env` entries override the sandbox's.
    pub fn rollcall(&self, args: &[&str], env: &[(&str, &str)]) -> Command {
        let mut cmd = self.command(ROLLCALL_BIN);
        cmd.args(args).envs(env.iter().copied());
        cmd
    }
}

pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl From<Output> for Run {
    fn from(out: Output) -> Run {
        Run {
            // A signal death has no code; -1 never equals a spec exit code.
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

pub fn run_rollcall(w: &World, args: &[&str]) -> Run {
    run_rollcall_with(w, args, &[], RUN_DEADLINE)
}

/// Fails the test (panics) when rollcall outlives `deadline`.
pub fn run_rollcall_with(
    w: &World,
    args: &[&str],
    env: &[(&str, &str)],
    deadline: Duration,
) -> Run {
    let out = spawn_watchdog(w.sb.rollcall(args, env), deadline)
        .unwrap_or_else(|e| panic!("rollcall {args:?}: {e}"));
    out.into()
}

// ------------------------------------------------------------ path tokens

/// Path tokens (`$F $EXT $H $HOME $CODEX_HOME $CLAUDE_PROJECTS`, builders add `$DEMO`) and `$FIFO:<name>`.
pub struct Tokens {
    map: Vec<(String, PathBuf)>,
    fifo_dir: PathBuf,
}

impl Tokens {
    pub fn new(fifo_dir: PathBuf) -> Tokens {
        Tokens {
            map: Vec::new(),
            fifo_dir,
        }
    }

    pub fn add(&mut self, key: &str, path: PathBuf) {
        self.map.push((key.to_string(), path));
        // Longest first, so `$HOME` wins over `$H`.
        self.map.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
    }

    /// The FIFO `<fifo_dir>/<name>`, created on first use.
    pub fn fifo(&self, name: &str) -> PathBuf {
        let path = self.fifo_dir.join(name);
        if !path.exists() {
            std::fs::create_dir_all(&self.fifo_dir).expect("create fifo dir");
            let status = Command::new("mkfifo")
                .arg(&path)
                .status()
                .expect("run mkfifo");
            assert!(status.success(), "mkfifo {} failed", path.display());
        }
        path
    }

    /// Replaces every known token in `s`; an unknown `$` is left as it is.
    pub fn subst(&self, s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while let Some(i) = rest.find('$') {
            out.push_str(&rest[..i]);
            rest = &rest[i..];
            if let Some(tail) = rest.strip_prefix("$FIFO:") {
                let len = tail
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                    .unwrap_or(tail.len());
                out.push_str(&self.fifo(&tail[..len]).to_string_lossy());
                rest = &tail[len..];
            } else if let Some((key, path)) =
                self.map.iter().find(|(k, _)| rest.starts_with(k.as_str()))
            {
                out.push_str(&path.to_string_lossy());
                rest = &rest[key.len()..];
            } else {
                out.push('$');
                rest = &rest[1..];
            }
        }
        out.push_str(rest);
        out
    }
}

// ------------------------------------------------------------------ roots

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    F,
    FClean,
    H,
}

/// One built fixture world: `root` (the scan root, canonical), `ext` (outside-root targets), its sandbox and tokens.
pub struct World {
    _tmp: TempDir,
    pub base: PathBuf,
    pub which: Which,
    pub root: PathBuf,
    pub ext: PathBuf,
    pub sb: Sandbox,
    pub tokens: Tokens,
}

impl World {
    /// An empty world: directories and tokens only; `build_root` fills it.
    fn empty(which: Which) -> World {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let base = std::fs::canonicalize(tmp.path()).expect("canonicalize temp dir");
        let mkdir = |name: &str| {
            let p = base.join(name);
            std::fs::create_dir_all(&p).expect("create world dir");
            p
        };
        let root = mkdir("root");
        let ext = mkdir("ext");
        let sb = Sandbox::new(&base.join("env"));
        let mut tokens = Tokens::new(base.join("fifos"));
        tokens.add(if which == Which::H { "$H" } else { "$F" }, root.clone());
        tokens.add("$EXT", ext.clone());
        tokens.add("$HOME", sb.home.clone());
        tokens.add("$CODEX_HOME", sb.codex_home.clone());
        tokens.add("$CLAUDE_PROJECTS", sb.claude_projects());
        World {
            _tmp: tmp,
            base,
            which,
            root,
            ext,
            sb,
            tokens,
        }
    }

    /// A scratch directory (for example an isolated `XDG_CACHE_HOME`), removed on drop.
    pub fn scratch(&self) -> TempDir {
        tempfile::tempdir_in(&self.base).expect("create scratch dir")
    }
}

/// P27b / P28 hook: materialise every manifest row that belongs to `which` into `w` (its `root`, `ext`,
/// sandbox `HOME` files and session trees), registering extra tokens such as `$DEMO` on `w.tokens`.
fn build_root(which: Which, w: &mut World, m: &Manifest) {
    crate::build_git::build(which, w, m);
    crate::build_sessions::build(which, w, m);
}

/// Takes the scan lock once a previous `rollcall` run's has been released. flock belongs to the open file
/// description, so a child another test thread forked while that fd was open keeps the lock until it execs.
pub fn lock_when_free(cache_dir: &Path, root: &Path) -> rollcall_store::ScanLock {
    let deadline = Instant::now() + RUN_DEADLINE;
    loop {
        if let Some(lock) = rollcall_store::try_scan_lock(cache_dir, root).expect("lock file opens")
        {
            return lock;
        }
        assert!(
            Instant::now() < deadline,
            "the scan lock was never released"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// A private world for a test that mutates its fixtures or needs its own cache.
pub fn fresh(which: Which) -> World {
    let mut w = World::empty(which);
    build_root(which, &mut w, manifest());
    w
}

fn cached(cell: &'static OnceLock<World>, which: Which) -> &'static World {
    cell.get_or_init(|| fresh(which))
}

/// Read-only roots, built once per test binary. Tests that scan with a cache, or mutate, use `fresh`.
pub fn f() -> &'static World {
    static F: OnceLock<World> = OnceLock::new();
    cached(&F, Which::F)
}

pub fn f_clean() -> &'static World {
    static F_CLEAN: OnceLock<World> = OnceLock::new();
    cached(&F_CLEAN, Which::FClean)
}

pub fn h() -> &'static World {
    static H: OnceLock<World> = OnceLock::new();
    cached(&H, Which::H)
}

// --------------------------------------------------------------- oracles

fn prepare(v: Value, tokens: &Tokens) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.into_iter()
                .filter(|(k, _)| !k.starts_with('_'))
                .map(|(k, v)| (k, prepare(v, tokens)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(|v| prepare(v, tokens)).collect()),
        Value::String(s) => Value::String(tokens.subst(&s)),
        other => other,
    }
}

/// Oracle JSON text with `_`-prefixed keys (comments) stripped and path tokens substituted. The tokens
/// hold canonical paths, and rollcall prints canonical paths, so both sides compare canonical.
pub fn expected_from(text: &str, tokens: &Tokens) -> Value {
    prepare(
        serde_json::from_str(text).expect("oracle is valid JSON"),
        tokens,
    )
}

/// `tests/expected/<name>.json` for the world `w`.
pub fn expected(name: &str, w: &World) -> Value {
    let path = repo_root()
        .join("tests/expected")
        .join(format!("{name}.json"));
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    expected_from(&text, &w.tokens)
}

/// Errors compare by prefix (spec 12.1 #1, #2): `expected` is null (no error) or the prefix of `actual`.
pub fn assert_error_prefix(what: &str, actual: &Value, expected: &Value) {
    match (actual, expected) {
        (Value::Null, Value::Null) => {}
        (Value::String(a), Value::String(prefix)) => {
            assert!(
                a.starts_with(prefix.as_str()),
                "{what}: error {a:?} does not start with {prefix:?}"
            );
        }
        _ => panic!("{what}: error mismatch: actual {actual}, expected prefix {expected}"),
    }
}

// ---------------------------------------------------------------- schemas

/// Validates `doc` against `schema/<schema>.json` with every schema registered under its `rollcall:` `$id`
/// (they `$ref` each other). `Err` lists the violations.
pub fn validate(schema: &str, doc: &Value) -> Result<(), Vec<String>> {
    let dir = repo_root().join("schema");
    let load = |name: &str| -> Value {
        let path = dir.join(format!("{name}.json"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
    };
    let mut builder = jsonschema::Registry::new();
    for name in SCHEMAS {
        builder = builder
            .add(format!("rollcall:{name}.json"), load(name))
            .unwrap_or_else(|e| panic!("register {name}: {e}"));
    }
    let registry = builder.prepare().expect("prepare schema registry");
    let validator = jsonschema::options()
        .with_registry(&registry)
        .build(&load(schema))
        .unwrap_or_else(|e| panic!("compile schema {schema}: {e}"));
    let errors: Vec<String> = validator.iter_errors(doc).map(|e| e.to_string()).collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
