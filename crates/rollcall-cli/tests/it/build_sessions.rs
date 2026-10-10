//! Session fixture builder (P28): Claude project dirs, the Codex thread database and rollouts, the big log,
//! `mcp_200`, and the perf and demo roots (git repos and logs). Synthetic data only; never reads real agent logs.
use crate::support::{
    Corpus, Kind, Manifest, Perf, Repo, RootId, Sandbox, Tokens, Which, World, git_path, manifest,
    spawn_watchdog,
};
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const STATE_DB: &str = "state_5.sqlite";
/// Agent dirs of the perf and demo roots live inside the root: `<root>/.agents/{claude,codex}`.
const AGENTS_DIR: &str = ".agents";
/// Codex threads without a rollout point here; the file never exists.
const MISSING_ROLLOUT_DIR: &str = "sessions/missing";
const GIT_DEADLINE: Duration = Duration::from_secs(60);
const PAD_TOKEN: &str = "$PAD:";
const NOEOL_TOKEN: &str = "$NOEOL";
/// Filler lines in perf logs carry this many `x`; the last line of a file takes up the remainder.
const FILLER_XS: usize = 2000;
const FILLER: [u8; FILLER_XS] = [b'x'; FILLER_XS];
/// Every fifth perf log is a Codex rollout, the rest are Claude sessions.
const CODEX_EVERY: usize = 5;
const PERF_LOGS_PER_DIR: usize = 50;
/// Day-of-month range of synthetic timestamps: all before `meta.now` (2026-09-28).
const DAYS: usize = 27;
const BIG_LOG_DIR: &str = "big-log";
/// `mcp_200` sessions sit in F worktrees that host no other corpus's session, so `matching` oracles stay valid.
const MCP_REPOS: [&str; 3] = ["sub-old-parent", "excludes", "no-default"];
const UUID_BIG_LOG: u32 = 0xc1a0_00b0;
const UUID_MCP: u32 = 0xc1a0_0009;
const UUID_PERF_CLAUDE: u32 = 0xc1a0_0008;
const UUID_PERF_CODEX: u32 = 0xc0d0_0006;

/// The default `F` / `F_clean` / `H` worlds keep EMPTY agent dirs (§10.1; P09's oracles expect no sessions).
/// Gates add the corpora they need with `build_corpora`, `build_big_log` and `build_mcp_200`.
pub fn build(_which: Which, _w: &mut World, _m: &Manifest) {}

/// DB mode writes `state_5.sqlite` with every selected `[[codex_thread]]` and its linked rollout; fallback mode
/// writes no database and every selected `[[codex_rollout]]` under `$CODEX_HOME`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexMode {
    Db,
    Fallback,
}

// ------------------------------------------------------------------ helpers

fn ok<T>(result: io::Result<T>, path: &Path) -> T {
    result.unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn write_file(path: &Path, content: &[u8]) {
    ok(
        fs::create_dir_all(path.parent().expect("file has a parent")),
        path,
    );
    ok(fs::write(path, content), path);
}

fn uuid(prefix: u32, n: usize) -> String {
    format!("{prefix:08x}-0000-4000-8000-{n:012}")
}

fn expand_pad(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(PAD_TOKEN) {
        let tail = &rest[i + PAD_TOKEN.len()..];
        let digits = tail
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(tail.len());
        let n: usize = tail[..digits]
            .parse()
            .unwrap_or_else(|e| panic!("bad {PAD_TOKEN}<n> in {s:.80}: {e}"));
        out.push_str(&rest[..i]);
        out.extend(std::iter::repeat_n('x', n));
        rest = &tail[digits..];
    }
    out.push_str(rest);
    out
}

/// The bytes of a log: path tokens substituted, `$PAD:<n>` expanded, a trailing `$NOEOL` leaving its line
/// without a newline.
pub fn log_bytes(tokens: &Tokens, lines: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    for raw in lines {
        let (text, newline) = match raw.strip_suffix(NOEOL_TOKEN) {
            Some(text) => (text, false),
            None => (raw.as_str(), true),
        };
        out.extend(expand_pad(&tokens.subst(text)).bytes());
        if newline {
            out.push(b'\n');
        }
    }
    out
}

/// Where a world's (or a materialised root's) agent data goes.
struct Dest<'a> {
    projects: PathBuf,
    codex_home: PathBuf,
    tokens: &'a Tokens,
}

impl<'a> Dest<'a> {
    fn of(w: &'a World) -> Dest<'a> {
        Dest {
            projects: w.sb.claude_projects(),
            codex_home: w.sb.codex_home.clone(),
            tokens: &w.tokens,
        }
    }

    fn under(root: &Path, tokens: &'a Tokens) -> Dest<'a> {
        let agents = root.join(AGENTS_DIR);
        Dest {
            projects: agents.join("claude/projects"),
            codex_home: agents.join("codex"),
            tokens,
        }
    }

    fn write_claude(&self, project_dir: &str, file: &str, lines: &[String]) {
        let path = self.projects.join(project_dir).join(file);
        write_file(&path, &log_bytes(self.tokens, lines));
    }

    /// `rel` is under `$CODEX_HOME`, or an absolute (token) path, which `join` keeps as it is.
    fn codex_path(&self, rel: &str) -> PathBuf {
        self.codex_home.join(self.tokens.subst(rel))
    }

    fn write_rollout(&self, file: &str, lines: &[String]) {
        write_file(&self.codex_path(file), &log_bytes(self.tokens, lines));
    }
}

// ------------------------------------------------------------ Codex database

struct ThreadRow {
    id: String,
    rollout_path: String,
    created_at: i64,
    updated_at: i64,
    source: String,
    cwd: String,
    title: Option<String>,
    git_branch: Option<String>,
    archived: bool,
    first_user_message: Option<String>,
}

/// The `threads` columns are exactly §5.2's required list.
fn write_db(d: &Dest, rows: &[ThreadRow], edges: &[(String, String)]) {
    let path = d.codex_home.join(STATE_DB);
    ok(fs::create_dir_all(&d.codex_home), &d.codex_home);
    let conn = Connection::open(&path).unwrap_or_else(|e| panic!("create {}: {e}", path.display()));
    conn.execute_batch(
        "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT, created_at INTEGER, \
         updated_at INTEGER, source TEXT, cwd TEXT, title TEXT, git_branch TEXT, archived INTEGER, \
         first_user_message TEXT); \
         CREATE TABLE thread_spawn_edges (parent_thread_id TEXT, child_thread_id TEXT);",
    )
    .expect("create Codex tables");
    for r in rows {
        conn.execute(
            "INSERT INTO threads VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                r.id,
                r.rollout_path,
                r.created_at,
                r.updated_at,
                r.source,
                r.cwd,
                r.title,
                r.git_branch,
                r.archived,
                r.first_user_message
            ],
        )
        .unwrap_or_else(|e| panic!("insert thread {}: {e}", r.id));
    }
    for (parent, child) in edges {
        conn.execute(
            "INSERT INTO thread_spawn_edges VALUES (?1, ?2)",
            params![parent, child],
        )
        .unwrap_or_else(|e| panic!("insert edge {parent} -> {child}: {e}"));
    }
}

// ------------------------------------------------------------ public builders

/// Materialises every `[[claude]]`, `[[codex_thread]]` and `[[codex_rollout]]` row whose corpus is in `corpora`.
pub fn build_corpora(w: &World, corpora: &[Corpus], codex: CodexMode) {
    let m = manifest();
    let d = Dest::of(w);
    let wanted = |c: &Corpus| corpora.contains(c);
    for r in m.claude.iter().filter(|r| wanted(&r.corpus)) {
        d.write_claude(&r.project_dir, &r.file, &r.lines);
    }
    match codex {
        CodexMode::Fallback => {
            for r in m.codex_rollout.iter().filter(|r| wanted(&r.corpus)) {
                // Rollouts outside $CODEX_HOME are never scanned in fallback mode.
                if d.codex_path(&r.file).starts_with(&d.codex_home) {
                    d.write_rollout(&r.file, &r.lines);
                }
            }
        }
        CodexMode::Db => {
            let threads: Vec<_> = m
                .codex_thread
                .iter()
                .filter(|t| wanted(&t.corpus))
                .collect();
            let rows: Vec<ThreadRow> = threads
                .iter()
                .map(|t| ThreadRow {
                    id: t.id.clone(),
                    rollout_path: thread_rollout_path(&d, m, t),
                    created_at: t.created_at,
                    updated_at: t.updated_at,
                    source: d.tokens.subst(&t.source),
                    cwd: d.tokens.subst(&t.cwd),
                    title: t.title.as_deref().map(|s| d.tokens.subst(s)),
                    git_branch: t.git_branch.clone(),
                    archived: t.archived,
                    first_user_message: t.first_user_message.as_deref().map(|s| d.tokens.subst(s)),
                })
                .collect();
            let edges: Vec<_> = threads
                .iter()
                .filter_map(|t| Some((t.spawn_parent.clone()?, t.id.clone())))
                .collect();
            write_db(&d, &rows, &edges);
            for t in &threads {
                if let Some(id) = &t.rollout_id {
                    let r = rollout_row(m, id);
                    d.write_rollout(&r.file, &r.lines);
                }
            }
        }
    }
}

fn rollout_row<'m>(m: &'m Manifest, id: &str) -> &'m crate::support::CodexRollout {
    m.codex_rollout
        .iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("no [[codex_rollout]] with id {id}"))
}

fn thread_rollout_path(d: &Dest, m: &Manifest, t: &crate::support::CodexThread) -> String {
    let path = match (&t.rollout_path, &t.rollout_id) {
        (Some(explicit), _) => PathBuf::from(d.tokens.subst(explicit)),
        (None, Some(id)) => d.codex_path(&rollout_row(m, id).file),
        (None, None) => d
            .codex_home
            .join(MISSING_ROLLOUT_DIR)
            .join(format!("rollout-{}.jsonl", t.id)),
    };
    path.to_string_lossy().into_owned()
}

/// Every `[[copilot]]` row as `<home>/session-state/<id>/events.jsonl`.
pub fn build_copilot(w: &World, home: &Path) {
    for r in &manifest().copilot {
        let path = home.join("session-state").join(&r.id).join("events.jsonl");
        write_file(&path, &log_bytes(&w.tokens, &r.lines));
    }
}

/// `[[big_log]]`: one Claude project dir holding a single `bytes`-long line without a newline.
pub fn build_big_log(w: &World) {
    let d = Dest::of(w);
    for (i, b) in manifest().big_log.iter().enumerate() {
        assert!(
            b.source == "claude" && b.single_line,
            "unsupported big_log {b:?}"
        );
        let path = d
            .projects
            .join(BIG_LOG_DIR)
            .join(format!("{}.jsonl", uuid(UUID_BIG_LOG, i)));
        ok(fs::create_dir_all(path.parent().unwrap()), &path);
        let mut out = BufWriter::new(ok(File::create(&path), &path));
        let (chunks, rest) = (
            b.bytes / FILLER_XS as u64,
            (b.bytes % FILLER_XS as u64) as usize,
        );
        for _ in 0..chunks {
            ok(out.write_all(&FILLER), &path);
        }
        ok(out.write_all(&FILLER[..rest]), &path);
        ok(out.flush(), &path);
    }
}

/// `[mcp_200]`: Claude sessions with a long title and last prompt, in worktrees of F (`$F` tokens).
pub fn build_mcp_200(w: &World) {
    let m = manifest();
    for path in MCP_REPOS {
        assert!(
            m.repo.iter().any(|r| r.root == RootId::F && r.path == path),
            "{path} is not an F repo in the manifest"
        );
    }
    let d = Dest::of(w);
    for i in 0..m.mcp_200.sessions as usize {
        let id = uuid(UUID_MCP, i);
        let cwd = format!("$F/{}", MCP_REPOS[i % MCP_REPOS.len()]);
        let day = 1 + i % DAYS;
        let hour = i / DAYS % 24;
        let at = |minute: u32| format!("2026-09-{day:02}T{hour:02}:{minute:02}:00.000Z");
        let title = format!(
            "mcp session {i:03}: {}",
            "lorem ipsum dolor sit amet ".repeat(6)
        );
        let prompt = format!(
            "prompt {i:03}: {}",
            "keep the answer long enough to truncate ".repeat(8)
        );
        let lines = [
            format!(
                r#"{{"type":"user","sessionId":"{id}","timestamp":"{}","cwd":"{cwd}","gitBranch":"main","isSidechain":false,"message":{{"role":"user","content":"start {i:03}"}}}}"#,
                at(0)
            ),
            format!(
                r#"{{"type":"assistant","sessionId":"{id}","timestamp":"{}","cwd":"{cwd}","gitBranch":"main","message":{{"role":"assistant","content":[{{"type":"text","text":"ok"}}]}}}}"#,
                at(1)
            ),
            format!(r#"{{"type":"ai-title","sessionId":"{id}","aiTitle":"{title}"}}"#),
            format!(
                r#"{{"type":"last-prompt","sessionId":"{id}","leafUuid":"00000000-0000-4000-8000-00000000beef","lastPrompt":"{prompt}"}}"#
            ),
        ];
        d.write_claude(&format!("mcp-{i:03}"), &format!("{id}.jsonl"), &lines);
    }
}

// ---------------------------------------------------------------- git helper

/// A sandboxed `git` for the perf and demo roots. `build_git` owns its `Ctx` privately (and needs a `World`), so
/// these roots, which are built outside any world, get this small driver.
struct Git {
    sb: Sandbox,
}

impl Git {
    fn new(dir: &Path) -> Git {
        Git {
            sb: Sandbox::new(dir),
        }
    }

    fn run(&self, dir: &Path, date: Option<&str>, args: &[&str]) -> String {
        let meta = &manifest().meta;
        let mut cmd = self.sb.command(git_path());
        cmd.current_dir(dir)
            .args(["-c", "gc.auto=0", "-c", "commit.gpgsign=false"])
            .args(["-c", "advice.detachedHead=false"])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", &meta.author.name)
            .env("GIT_AUTHOR_EMAIL", &meta.author.email)
            .env("GIT_COMMITTER_NAME", &meta.committer.name)
            .env("GIT_COMMITTER_EMAIL", &meta.committer.email);
        if let Some(date) = date {
            cmd.env("GIT_AUTHOR_DATE", date)
                .env("GIT_COMMITTER_DATE", date);
        }
        let out = spawn_watchdog(cmd, GIT_DEADLINE)
            .unwrap_or_else(|e| panic!("git {args:?} in {}: {e}", dir.display()));
        assert!(
            out.status.success(),
            "git {args:?} in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit(&self, dir: &Path, files: &BTreeMap<String, String>, message: &str, date: &str) {
        for (rel, content) in files {
            write_file(&dir.join(rel), content.as_bytes());
        }
        let mut add = vec!["add", "--"];
        add.extend(files.keys().map(String::as_str));
        self.run(dir, None, &add);
        self.run(dir, Some(date), &["commit", "-q", "-m", message]);
    }

    /// The manifest's commit grammar: a commit on a new branch forks from the previous entry's commit, on an
    /// existing branch it continues it. Returns the commit ids in array order.
    fn init_with_commits(&self, r: &Repo, dir: &Path) -> Vec<String> {
        ok(fs::create_dir_all(dir), dir);
        let first = r.commits.first().map_or("main", |c| c.branch.as_str());
        self.run(dir, None, &["init", "-q", "-b", first]);
        let mut ids: Vec<String> = Vec::new();
        let mut known: Vec<&str> = Vec::new();
        for (i, c) in r.commits.iter().enumerate() {
            if i > 0 && known.contains(&c.branch.as_str()) {
                self.run(dir, None, &["checkout", "-q", &c.branch, "--"]);
            } else if i > 0 {
                self.run(dir, None, &["checkout", "-q", "-b", &c.branch, &ids[i - 1]]);
            }
            self.commit(dir, &c.files, &c.message, &c.date);
            ids.push(self.run(dir, None, &["rev-parse", "HEAD"]));
            known.push(&c.branch);
        }
        for (name, &i) in &r.branches {
            self.run(dir, None, &["branch", name, &ids[i]]);
        }
        if let Some(head) = &r.head {
            assert!(
                !head.starts_with("detached:"),
                "detached heads are not built here"
            );
            self.run(dir, None, &["checkout", "-q", head, "--"]);
        }
        ids
    }

    /// Bare repos with commits: scratch work repo, then push.
    fn build_bare(&self, r: &Repo, scratch: &Path, path: &Path) {
        let work = scratch.join(&r.id);
        self.init_with_commits(r, &work);
        ok(fs::create_dir_all(path), path);
        let head = r.head.as_deref().unwrap_or("main");
        self.run(path, None, &["init", "-q", "--bare", "-b", head]);
        let url = path.to_string_lossy();
        self.run(
            &work,
            None,
            &["push", "-q", &url, "refs/heads/*:refs/heads/*"],
        );
    }
}

// ---------------------------------------------------------------------- demo

fn repo_row<'m>(m: &'m Manifest, id: &str) -> &'m Repo {
    m.repo
        .iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("no [[repo]] with id {id}"))
}

/// One `root = "demo"` row: commits, remotes (fetched, with upstreams), worktrees, edits and state.
fn build_demo_repo(g: &Git, m: &Manifest, r: &Repo, root: &Path, remotes: &Path) {
    assert!(
        r.kind == Kind::Normal && r.special.is_none(),
        "demo row {} is not a plain repo",
        r.id
    );
    let dir = root.join(&r.path);
    g.init_with_commits(r, &dir);
    for remote in &r.remotes {
        let url = remotes.join(&repo_row(m, &remote.target_repo_id).path);
        g.run(
            &dir,
            None,
            &["remote", "add", &remote.name, &url.to_string_lossy()],
        );
        g.run(&dir, None, &["fetch", "-q", &remote.name]);
    }
    for (branch, upstream) in &r.upstreams {
        g.run(
            &dir,
            None,
            &[
                "branch",
                "-q",
                &format!("--set-upstream-to={upstream}"),
                branch,
            ],
        );
    }
    if let Some(head) = &r.origin_head {
        g.run(&dir, None, &["remote", "set-head", "origin", head]);
    }
    for wt in &r.worktrees {
        let branch = wt.branch.as_deref().expect("demo worktrees name a branch");
        assert!(
            !wt.missing && !wt.external,
            "demo worktree {} must be plain",
            wt.path
        );
        g.run(
            &dir,
            None,
            &[
                "worktree",
                "add",
                "-q",
                &root.join(&wt.path).to_string_lossy(),
                branch,
            ],
        );
    }
    for (rel, content) in &r.edit {
        write_file(&dir.join(rel), content.as_bytes());
    }
    let Some(state) = &r.state else { return };
    assert!(state.ignored.is_empty(), "demo rows have no ignored files");
    if !state.staged.is_empty() {
        let mut add = vec!["add", "--"];
        add.extend(state.staged.iter().map(String::as_str));
        g.run(&dir, None, &add);
    }
    for rel in &state.untracked {
        write_file(&dir.join(rel), b"untracked\n");
    }
}

/// `[demo]` into `root`: its repos at `<root>/<path>`, agent data in `<root>/.agents/{claude,codex}`
/// (`CLAUDE_CONFIG_DIR=<root>/.agents/claude`, `CODEX_HOME=<root>/.agents/codex`). `$DEMO` is `root`.
pub fn build_demo_root(root: &Path) {
    let m = manifest();
    let tmp = tempfile::tempdir().expect("create temp dir");
    let g = Git::new(&tmp.path().join("sandbox"));
    let (scratch, remotes) = (tmp.path().join("scratch"), tmp.path().join("remotes"));
    let rows: Vec<&Repo> = m.demo.repos.iter().map(|id| repo_row(m, id)).collect();
    // Remote targets exist only while their remote-tracking refs are fetched.
    let targets: BTreeSet<&str> = rows
        .iter()
        .flat_map(|r| r.remotes.iter().map(|rm| rm.target_repo_id.as_str()))
        .collect();
    for id in targets {
        let target = repo_row(m, id);
        g.build_bare(target, &scratch, &remotes.join(&target.path));
    }
    for r in rows {
        build_demo_repo(&g, m, r, root, &remotes);
    }

    let mut tokens = Tokens::new(tmp.path().join("fifos"));
    tokens.add("$DEMO", root.to_path_buf());
    let d = Dest::under(root, &tokens);
    for c in &m.demo.claude {
        d.write_claude(&c.project_dir, &c.file, &c.lines);
    }
    let threads: Vec<ThreadRow> = m
        .demo
        .codex_thread
        .iter()
        .map(|t| ThreadRow {
            id: t.id.clone(),
            rollout_path: d
                .codex_home
                .join(MISSING_ROLLOUT_DIR)
                .join(format!("rollout-{}.jsonl", t.id))
                .to_string_lossy()
                .into_owned(),
            created_at: t.created_at,
            updated_at: t.updated_at,
            source: t.source.clone(),
            cwd: tokens.subst(&t.cwd),
            title: t.title.clone(),
            git_branch: t.git_branch.clone(),
            archived: t.archived,
            first_user_message: t.first_user_message.clone(),
        })
        .collect();
    write_db(&d, &threads, &[]);
}

// ---------------------------------------------------------------------- perf

/// A log of exactly `size` bytes: `head` (whole lines), then filler lines `prefix + x… + suffix`, the last one
/// shortened to land on `size`.
fn write_sized(path: &Path, head: &str, prefix: &str, suffix: &str, size: u64) {
    let wrapper = (prefix.len() + suffix.len() + 1) as u64; // a filler line without its `x`, newline included
    let head_len = head.len() as u64;
    assert!(
        size >= head_len + wrapper,
        "{size} bytes cannot hold {}",
        path.display()
    );
    let full = wrapper + FILLER_XS as u64;
    ok(fs::create_dir_all(path.parent().unwrap()), path);
    let mut out = BufWriter::new(ok(File::create(path), path));
    ok(out.write_all(head.as_bytes()), path);
    let mut line = |xs: &[u8]| {
        for part in [prefix.as_bytes(), xs, suffix.as_bytes(), b"\n"] {
            ok(out.write_all(part), path);
        }
    };
    let mut remaining = size - head_len;
    while remaining > 0 {
        // Never leave a remainder that cannot hold a line: split the last two lines instead.
        let xs = if remaining == full || remaining >= full + wrapper {
            FILLER_XS
        } else if remaining > full {
            (remaining - 2 * wrapper) as usize
        } else {
            (remaining - wrapper) as usize
        };
        line(&FILLER[..xs]);
        remaining -= wrapper + xs as u64;
    }
    ok(out.flush(), path);
}

/// `[perf]` into `root`: `repos - linked_worktrees` repos plus `linked_worktrees` linked worktrees, and
/// `logs` logs of `bytes / logs` bytes each (the last takes the remainder), Claude and Codex (fallback mode, no
/// database), in `<root>/.agents/{claude,codex}`. Returns the total log bytes.
pub fn build_perf_root(root: &Path, spec: &Perf) -> u64 {
    let meta = &manifest().meta;
    let tmp = tempfile::tempdir().expect("create temp dir");
    let g = Git::new(&tmp.path().join("sandbox"));
    let mains = (spec.repos - spec.linked_worktrees) as usize;
    assert!(mains > 0, "perf needs at least one primary repo");
    let mut cwds: Vec<PathBuf> = Vec::new();
    for i in 0..mains {
        let dir = root.join(format!("repo-{i:02}"));
        ok(fs::create_dir_all(&dir), &dir);
        g.run(&dir, None, &["init", "-q", "-b", "main"]);
        let files = BTreeMap::from([("README.md".to_string(), format!("# repo {i}\n"))]);
        g.commit(&dir, &files, "init", &meta.now);
        cwds.push(dir);
    }
    for k in 0..spec.linked_worktrees as usize {
        let parent = root.join(format!("repo-{:02}", k % mains));
        let wt = root.join(format!("wt-{k:02}"));
        let args = [
            "worktree",
            "add",
            "-q",
            "-b",
            &format!("wt-{k:02}"),
            &wt.to_string_lossy(),
        ];
        g.run(&parent, None, &args);
        cwds.push(wt);
    }

    let agents = root.join(AGENTS_DIR);
    let logs = spec.logs as usize;
    let (per_log, remainder) = (spec.bytes / logs as u64, spec.bytes % logs as u64);
    let mut written = 0;
    for i in 0..logs {
        let size = per_log + if i + 1 == logs { remainder } else { 0 };
        let cwd = cwds[i % cwds.len()].to_string_lossy();
        let (day, at) = (1 + i % DAYS, |hms: &str| {
            format!("2026-09-{:02}T{hms}.000Z", 1 + i % DAYS)
        });
        if i % CODEX_EVERY == CODEX_EVERY - 1 {
            let id = uuid(UUID_PERF_CODEX, i);
            let head = format!(
                concat!(
                    r#"{{"timestamp":"{t}","type":"session_meta","payload":{{"id":"{id}","timestamp":"{t}","cwd":"{cwd}","git":{{"branch":"main"}}}}}}"#,
                    "\n",
                    r#"{{"timestamp":"{t}","type":"event_msg","payload":{{"type":"user_message","message":"perf prompt {i}"}}}}"#,
                    "\n"
                ),
                t = at("10:00:00"),
                id = id,
                cwd = cwd,
                i = i
            );
            let prefix = format!(
                r#"{{"timestamp":"{}","type":"response_item","payload":{{"type":"message","role":"assistant","content":[{{"type":"output_text","text":""#,
                at("10:30:00")
            );
            let name = format!("rollout-2026-09-{day:02}T10-00-00-{id}.jsonl");
            let path = agents
                .join(format!("codex/sessions/2026/09/{day:02}"))
                .join(name);
            write_sized(&path, &head, &prefix, r#""}]}}"#, size);
        } else {
            let id = uuid(UUID_PERF_CLAUDE, i);
            let head = format!(
                r#"{{"type":"user","sessionId":"{id}","timestamp":"{}","cwd":"{cwd}","gitBranch":"main","isSidechain":false,"message":{{"role":"user","content":"perf prompt {i}"}}}}"#,
                at("10:00:00")
            ) + "\n";
            let prefix = format!(
                r#"{{"type":"assistant","sessionId":"{id}","timestamp":"{}","cwd":"{cwd}","gitBranch":"main","message":{{"role":"assistant","content":[{{"type":"text","text":""#,
                at("10:30:00")
            );
            let dir = format!("claude/projects/perf-{:03}", i / PERF_LOGS_PER_DIR);
            let path = agents.join(dir).join(format!("{id}.jsonl"));
            write_sized(&path, &head, &prefix, r#""}]}}"#, size);
        }
        written += size;
    }
    written
}
