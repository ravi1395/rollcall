//! Breaker (adversarial QA) tests for the v0 integration. Each test encodes one spec-stated
//! behaviour; see the comment on each for the spec section it checks.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use rollcall_cli::output::{TextPolicy, render_session};
use rollcall_core::{SourceState, TitleKind};
use rollcall_sessions::{Codex, SessionSource};
use serde_json::Value;
use tempfile::TempDir;

const NOW: &str = "2026-09-29T12:00:00Z";
const DEADLINE: Duration = Duration::from_secs(60);
const ID_A: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
const ID_B: &str = "7c9e6679-7425-40de-944b-e07fc1f90ae7";

struct Sb {
    _dir: TempDir,
    base: PathBuf,
    home: PathBuf,
    xdg_config: PathBuf,
    claude: PathBuf,
    codex: PathBuf,
}

impl Sb {
    fn new() -> Sb {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let sub = |name: &str| {
            let p = base.join(name);
            fs::create_dir_all(&p).unwrap();
            p
        };
        Sb {
            home: sub("home"),
            xdg_config: sub("xdg/config"),
            claude: sub("claude"),
            codex: sub("codex"),
            _dir: dir,
            base,
        }
    }

    fn write_config(&self, text: &str) {
        let dir = self.xdg_config.join("rollcall");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), text).unwrap();
    }

    fn rollcall(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rollcall"));
        cmd.env_clear()
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.xdg_config)
            .env("XDG_CACHE_HOME", self.base.join("xdg/cache"))
            .env("CLAUDE_CONFIG_DIR", &self.claude)
            .env("CODEX_HOME", &self.codex)
            .env("ROLLCALL_NOW", NOW)
            .env("PATH", "/usr/bin:/bin");
        cmd
    }

    /// A repo with one empty commit at `path`.
    fn repo(&self, path: &Path) {
        fs::create_dir_all(path).unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        ] {
            let ok = Command::new("git")
                .args(args)
                .current_dir(path)
                .env("HOME", &self.home)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_COMMITTER_DATE", "2026-09-20T10:00:00Z")
                .env("GIT_AUTHOR_DATE", "2026-09-20T10:00:00Z")
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        }
    }

    /// A Claude log `projects/p/<file_id>.jsonl` with the given records.
    fn claude_log(&self, file_id: &str, records: &[Value]) {
        let dir = self.claude.join("projects/p");
        fs::create_dir_all(&dir).unwrap();
        let text: String = records.iter().map(|r| format!("{r}\n")).collect();
        fs::write(dir.join(format!("{file_id}.jsonl")), text).unwrap();
    }
}

fn user(id: &str, cwd: &Path, at: &str, text: &str) -> Value {
    serde_json::json!({"type": "user", "sessionId": id, "timestamp": at,
        "cwd": cwd.to_str().unwrap(), "gitBranch": "main",
        "message": {"role": "user", "content": text}})
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn drain(mut r: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        buf
    })
}

/// Runs `cmd` with `stdin` fed and closed, killed after `DEADLINE`.
fn run(mut cmd: Command, stdin: &str) -> Run {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = drain(child.stdout.take().unwrap());
    let err = drain(child.stderr.take().unwrap());
    let mut input = child.stdin.take().unwrap();
    let _ = input.write_all(stdin.as_bytes());
    drop(input);
    let end = Instant::now() + DEADLINE;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= end {
            let _ = child.kill();
            let _ = child.wait();
            panic!("killed by the watchdog after {DEADLINE:?}");
        }
        thread::sleep(Duration::from_millis(20));
    };
    Run {
        code: status.code(),
        stdout: String::from_utf8_lossy(&out.join().unwrap()).into_owned(),
        stderr: String::from_utf8_lossy(&err.join().unwrap()).into_owned(),
    }
}

const INIT: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"breaker","version":"0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
"#;
const WHERE: &str = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"where_was_i","arguments":{}}}"#;

/// The `where_was_i` document from reply id 2.
fn where_doc(r: &Run) -> Value {
    let reply: Value = r
        .stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["id"] == 2)
        .unwrap_or_else(|| {
            panic!(
                "no reply to id 2: code {:?}\nstdout {}\nstderr {}",
                r.code, r.stdout, r.stderr
            )
        });
    let text = reply["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("not a tool result: {reply}"));
    serde_json::from_str(text).unwrap()
}

// §9: "Paths ... are sanitised in TUI and plain output". The §4.3 refusal lists
// `rollcall <dir>` suggestions on stderr; a repo-bearing dir under $HOME named with an escape
// sequence must not reach the terminal raw.
#[test]
fn breaker_refusal_suggestions_are_sanitised() {
    let sb = Sb::new();
    fs::create_dir_all(sb.home.join("evil\x1b[2Jdir/.git")).unwrap();
    let r = run(sb.rollcall(&["status", sb.home.to_str().unwrap()]), "");
    assert_eq!(r.code, Some(3), "stderr: {}", r.stderr);
    assert!(
        r.stderr.contains("evil"),
        "control: the suggestion is listed\n{}",
        r.stderr
    );
    assert!(
        !r.stderr.contains('\x1b'),
        "raw ESC in refusal output: {:?}",
        r.stderr
    );
}

// §6.4: with no cached rows, an MCP call "waits for the first scan (bounded by
// walk_budget_secs + 30 s)"; §3.2 accepts any integer `walk_budget_secs` (only wrong types
// exit 2), and the walk itself tolerates a far deadline (`checked_add`). The server must answer,
// not panic.
#[test]
fn breaker_mcp_first_call_with_a_huge_walk_budget_answers() {
    let sb = Sb::new();
    let root = sb.base.join("root");
    fs::create_dir_all(root.join("empty")).unwrap();
    sb.write_config("walk_budget_secs = 9223372036854775807\n");
    let r = run(
        sb.rollcall(&["mcp", "--root", root.to_str().unwrap()]),
        &format!("{INIT}{WHERE}\n"),
    );
    assert_eq!(r.code, Some(0), "stderr: {}", r.stderr);
    let doc = where_doc(&r);
    assert_eq!(doc["repos"], Value::Array(vec![]));
}

// §9 / §3.2: `redact_emails = true` adds emails to scrubbing of title and last_prompt in every
// output, MCP included (RISKS: no MCP email-redaction test existed).
#[test]
fn breaker_mcp_scrubs_emails_with_redact_emails() {
    let sb = Sb::new();
    let root = sb.base.join("root");
    let app = root.join("app");
    sb.repo(&app);
    sb.claude_log(
        ID_A,
        &[
            user(
                ID_A,
                &app,
                "2026-09-28T10:00:00.000Z",
                "mail alice@example.com now",
            ),
            serde_json::json!({"type": "custom-title", "sessionId": ID_A,
                "customTitle": "ping bob@example.org"}),
        ],
    );
    let mcp = |sb: &Sb| {
        run(
            sb.rollcall(&["mcp", "--root", root.to_str().unwrap(), "--prompts"]),
            &format!("{INIT}{WHERE}\n"),
        )
    };
    let plain = where_doc(&mcp(&sb));
    let session = &plain["recent_sessions"][0];
    assert_eq!(session["id"], ID_A, "control: session matched\n{plain}");
    assert!(
        plain.to_string().contains("bob@example.org"),
        "control: emails shown without redact_emails\n{plain}"
    );

    sb.write_config("redact_emails = true\n");
    let doc = where_doc(&mcp(&sb));
    let text = doc.to_string();
    assert!(
        !text.contains("alice@example.com") && !text.contains("bob@example.org"),
        "email in MCP output: {doc}"
    );
    assert_eq!(doc["recent_sessions"][0]["title"], "ping [redacted]");
    assert_eq!(
        doc["recent_sessions"][0]["last_prompt"],
        "mail [redacted] now"
    );
}

// §6.1: "Sessions keyed by (source, id)". §5.1: id = `sessionId` of the first record carrying
// one. Two Claude logs whose records carry the same sessionId are one session, not two: the
// worktree must not count the second as an older session.
#[test]
fn breaker_claude_duplicate_session_id_is_one_session() {
    let sb = Sb::new();
    let root = sb.base.join("root");
    let app = root.join("app");
    sb.repo(&app);
    sb.claude_log(
        ID_A,
        &[user(ID_A, &app, "2026-09-28T10:00:00.000Z", "first file")],
    );
    sb.claude_log(
        ID_B,
        &[user(ID_A, &app, "2026-09-28T11:00:00.000Z", "second file")],
    );
    let r = run(
        sb.rollcall(&["status", "--json", root.to_str().unwrap()]),
        "",
    );
    let doc: Value = serde_json::from_str(&r.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}\n{}", r.stdout, r.stderr));
    let wt = &doc["repos"][0]["worktrees"][0];
    assert_eq!(wt["last_session"]["id"], ID_A, "{doc}");
    assert_eq!(
        wt["older_sessions"], 0,
        "one (source, id) counted as several sessions: {wt}"
    );
}

// §8.3: without `--prompts`, "title is returned only when title_kind is custom, ai, or thread
// (never prompt-derived)"; §6.2: resolution re-applies §5 extraction and "any mismatch → null".
// A Codex thread scanned with a `Thread` title whose row later loses its title resolves to the
// first user message (§5.2: empty title → first_user_message, kind `Prompt`), which must not be
// shown under the stored `Thread` kind. Reachable whenever the answer is served from the cache
// (`--no-scan`, lock held, MCP over its 2 s wait).
#[test]
fn breaker_codex_title_cleared_after_scan_does_not_leak_the_prompt() {
    let sb = Sb::new();
    let db = sb.codex.join("state_5.sqlite");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT, created_at INTEGER, \
         updated_at INTEGER, source TEXT, cwd TEXT, title TEXT, git_branch TEXT, \
         archived INTEGER, first_user_message TEXT)",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO threads VALUES (?1, '', 1000, 2000, '\"cli\"', '/w/app', \
         'Refactor auth flow', 'main', 0, 'my private prompt text')",
        [ID_A],
    )
    .unwrap();

    let codex = Codex::new(sb.codex.clone());
    let scan = codex.scan(&SourceState::default());
    let session = scan
        .sessions
        .iter()
        .find(|s| s.id.as_str() == ID_A)
        .expect("thread scanned");
    assert!(
        matches!(session.title, Some((_, TitleKind::Thread))),
        "control: scanned as a Thread title: {:?}",
        session.title
    );
    let mcp_default = TextPolicy {
        prompts: false,
        redact: false,
    };
    let before = render_session(session, &codex, mcp_default);
    assert_eq!(before.title.as_deref(), Some("Refactor auth flow"));

    conn.execute("UPDATE threads SET title = '' WHERE id = ?1", [ID_A])
        .unwrap();
    let after = render_session(session, &codex, mcp_default);
    assert_ne!(
        after.title.as_deref(),
        Some("my private prompt text"),
        "prompt-derived text shown without --prompts as title_kind {:?}",
        after.title_kind
    );
}
