//! Breaker (adversarial QA) tests for I5a: plain `status` decodes HTML entities in its quoted
//! session text (spec rev 16 §8.2, §9). Integration tests drive the real binary on a sandbox of
//! their own; the decoder table calls `rollcall_core::text::decode_entities` directly.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use rollcall_core::text::decode_entities;
use serde_json::{Value, json};
use tempfile::TempDir;

const NOW: &str = "2026-09-29T12:00:00Z";
const DEADLINE: Duration = Duration::from_secs(60);
const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
const ENTITY_TITLE: &str = "&lt;command-name&gt;/clear&lt;/command-name&gt;";
const DECODED_TITLE: &str = "\"<command-name>/clear</command-name>\"";

struct Sb {
    _dir: TempDir,
    base: PathBuf,
    root: PathBuf,
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Sb {
    /// A sandbox whose root holds one repo `app` with one Claude session: a user record with
    /// `prompt` and, when given, a `custom-title` record with `title`.
    fn with_session(title: Option<&str>, prompt: &str) -> Sb {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        for sub in [
            "home",
            "xdg/config",
            "xdg/cache",
            "claude/projects/p",
            "codex",
        ] {
            fs::create_dir_all(base.join(sub)).unwrap();
        }
        let root = base.join("root");
        let app = root.join("app");
        repo(&base.join("home"), &app);
        let mut records = vec![json!({"type": "user", "sessionId": ID,
            "timestamp": "2026-09-28T10:00:00.000Z", "cwd": app.to_str().unwrap(),
            "gitBranch": "main", "isSidechain": false,
            "message": {"role": "user", "content": prompt}})];
        if let Some(title) = title {
            records.push(json!({"type": "custom-title", "sessionId": ID, "customTitle": title}));
        }
        let text: String = records.iter().map(|r| format!("{r}\n")).collect();
        fs::write(base.join(format!("claude/projects/p/{ID}.jsonl")), text).unwrap();
        Sb {
            _dir: dir,
            base,
            root,
        }
    }

    /// A sandbox whose root holds one repo `app` and whose `$CODEX_HOME/state_5.sqlite` holds
    /// one thread there (rollout file absent) with `title` and `first_user_message`.
    fn with_codex_thread(title: &str, first: &str) -> Sb {
        let sb = Sb::with_session(None, "p");
        fs::remove_file(sb.base.join(format!("claude/projects/p/{ID}.jsonl"))).unwrap();
        let conn = rusqlite::Connection::open(sb.base.join("codex/state_5.sqlite")).unwrap();
        conn.execute_batch(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT, created_at INTEGER, \
             updated_at INTEGER, source TEXT, cwd TEXT, title TEXT, git_branch TEXT, \
             archived INTEGER, first_user_message TEXT); \
             CREATE TABLE thread_spawn_edges (parent_thread_id TEXT, child_thread_id TEXT);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads VALUES (?1, ?2, 1790436000, 1790437200, 'cli', ?3, ?4, 'main', \
             0, ?5)",
            rusqlite::params![
                ID,
                sb.base.join("codex/sessions/none.jsonl").to_str().unwrap(),
                sb.root.join("app").to_str().unwrap(),
                title,
                first
            ],
        )
        .unwrap();
        sb
    }

    fn rollcall(&self, args: &[&str], stdin: &str) -> Run {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rollcall"))
            .env_clear()
            .args(args)
            .env("HOME", self.base.join("home"))
            .env("XDG_CONFIG_HOME", self.base.join("xdg/config"))
            .env("XDG_CACHE_HOME", self.base.join("xdg/cache"))
            .env("CLAUDE_CONFIG_DIR", self.base.join("claude"))
            .env("CODEX_HOME", self.base.join("codex"))
            .env("ROLLCALL_NOW", NOW)
            .env("PATH", "/usr/bin:/bin")
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
                panic!("rollcall {args:?} killed by the watchdog after {DEADLINE:?}");
            }
            thread::sleep(Duration::from_millis(20));
        };
        Run {
            code: status.code(),
            stdout: String::from_utf8_lossy(&out.join().unwrap()).into_owned(),
            stderr: String::from_utf8_lossy(&err.join().unwrap()).into_owned(),
        }
    }

    fn root_arg(&self) -> &str {
        self.root.to_str().unwrap()
    }

    /// Plain `status <root>` stdout (exit 0 asserted).
    fn plain(&self) -> String {
        let run = self.rollcall(&["status", self.root_arg()], "");
        assert_eq!(
            run.code,
            Some(0),
            "stdout {}\nstderr {}",
            run.stdout,
            run.stderr
        );
        run.stdout
    }

    /// `.title` of the one worktree's `last_session` in `status --json <root>`.
    fn json_title(&self) -> Value {
        let run = self.rollcall(&["status", "--json", self.root_arg()], "");
        assert_eq!(
            run.code,
            Some(0),
            "stdout {}\nstderr {}",
            run.stdout,
            run.stderr
        );
        let doc: Value = serde_json::from_str(&run.stdout).unwrap();
        doc["repos"][0]["worktrees"][0]["last_session"]["title"].clone()
    }
}

fn repo(home: &Path, path: &Path) {
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
            .env("HOME", home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_COMMITTER_DATE", "2026-09-20T10:00:00Z")
            .env("GIT_AUTHOR_DATE", "2026-09-20T10:00:00Z")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }
}

fn drain(mut r: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        buf
    })
}

/// The one worktree line: the line naming the Claude session.
fn session_line(text: &str) -> &str {
    let lines: Vec<&str> = text.lines().filter(|l| l.contains("claude ")).collect();
    assert_eq!(lines.len(), 1, "{text}");
    lines[0]
}

// ---- decoder table (I5a-1..4) and panic hunt ----

/// Pins I5a-1..4 from outside the crate, through the public API the CLI uses.
#[test]
fn breaker_decode_entities_criteria_table() {
    for (raw, want) in [
        ("&lt;command-name&gt;", "<command-name>"),
        ("&gt;", ">"),
        ("&amp;", "&"),
        ("&quot;", "\""),
        ("&#39;", "'"),
        ("&#65;", "A"),
        ("&#x41;", "A"),
        ("&#x1F600;", "\u{1F600}"),
        ("&amp;lt;", "&lt;"),
        ("&nbsp;", "&nbsp;"),
        ("&#xD800;", "&#xD800;"),
        ("&#x110000;", "&#x110000;"),
        ("&#;", "&#;"),
        ("&#xZZ;", "&#xZZ;"),
        ("&lt", "&lt"),
    ] {
        assert_eq!(decode_entities(raw), want, "{raw:?}");
    }
}

/// Multibyte characters between `&` and `;` must never split a char boundary (no panic) and are
/// left as written (§8.2 "any other `&…;`").
#[test]
fn breaker_decode_entities_multibyte_and_edges_do_not_panic() {
    for raw in [
        "&é;",
        "&#é;",
        "&😀;",
        "&#x😀;",
        "&#١;",
        "&;",
        "&",
        "a&",
        "&&&",
        "&#",
        "&#x",
        "&#xé",
        "&ééééééééééé;",
    ] {
        assert_eq!(decode_entities(raw), raw, "{raw:?}");
    }
    assert_eq!(decode_entities("😀&lt;😀&#x1F600;"), "😀<😀😀");
}

// ---- through the binary ----

/// I5a-5: plain `status` decodes; JSON keeps the logged text exactly.
#[test]
fn breaker_plain_status_decodes_and_json_keeps_entities() {
    let sb = Sb::with_session(Some(ENTITY_TITLE), "p");
    let plain = sb.plain();
    assert!(session_line(&plain).ends_with(DECODED_TITLE), "{plain}");
    assert!(!plain.contains("&lt;"), "{plain}");
    let title = sb.json_title();
    assert_eq!(title, ENTITY_TITLE);
}

/// §3.1: `rollcall <root>` with stdout not a TTY behaves as `status`, so it decodes too.
#[test]
fn breaker_non_tty_default_command_decodes() {
    let sb = Sb::with_session(Some(ENTITY_TITLE), "p");
    let run = sb.rollcall(&[sb.root_arg()], "");
    assert_eq!(run.code, Some(0), "stderr {}", run.stderr);
    assert!(
        session_line(&run.stdout).ends_with(DECODED_TITLE),
        "{}",
        run.stdout
    );
}

/// I5a-5 via `--no-scan`: the cached answer is plain `status` too.
#[test]
fn breaker_cached_plain_status_decodes() {
    let sb = Sb::with_session(Some(ENTITY_TITLE), "p");
    sb.plain();
    let run = sb.rollcall(&["status", "--no-scan", sb.root_arg()], "");
    assert_eq!(run.code, Some(0), "stderr {}", run.stderr);
    assert!(
        session_line(&run.stdout).ends_with(DECODED_TITLE),
        "{}",
        run.stdout
    );
}

/// I5a-6: a decoded `=` makes a §9 secret pattern match in plain output only.
#[test]
fn breaker_decoded_secret_is_scrubbed_in_plain_only() {
    let title = "deploy token&#61;abc123def";
    let sb = Sb::with_session(Some(title), "p");
    let plain = sb.plain();
    let line = session_line(&plain);
    assert!(line.contains("[redacted]"), "{plain}");
    assert!(!plain.contains("abc123def"), "{plain}");
    assert_eq!(sb.json_title(), title);
}

/// Decode before scrub with a different pattern: `sk&#45;…` decodes to `sk-…` (§9 `sk-[…]{16,}`).
#[test]
fn breaker_decoded_dash_completes_an_sk_key() {
    let sb = Sb::with_session(Some("key sk&#45;abcdefghijklmnop"), "p");
    let plain = sb.plain();
    assert!(
        session_line(&plain).ends_with("\"key [redacted]\""),
        "{plain}"
    );
}

/// I5a-7: a decoded ESC is removed in plain output; JSON keeps the entity text (no ESC either).
#[test]
fn breaker_decoded_escape_never_reaches_plain_output() {
    let title = "x&#27;[2Jy";
    let sb = Sb::with_session(Some(title), "p");
    let plain = sb.plain();
    assert!(!plain.as_bytes().contains(&0x1b), "{plain:?}");
    assert!(session_line(&plain).ends_with("\"xy\""), "{plain:?}");
    assert_eq!(sb.json_title(), title);
}

/// §9 sanitising after decoding: newline becomes a space, NUL / C1 CSI / RLO are removed.
#[test]
fn breaker_decoded_controls_are_sanitised() {
    let sb = Sb::with_session(Some("a&#10;b&#0;c&#155;d&#x202E;e&#x7F;f"), "p");
    let plain = sb.plain();
    assert!(session_line(&plain).ends_with("\"a bcdef\""), "{plain:?}");
}

/// I5a-8: truncation counts decoded graphemes.
#[test]
fn breaker_truncation_after_decoding() {
    let sb = Sb::with_session(Some(&"&amp;".repeat(300)), "p");
    let plain = sb.plain();
    let want = format!("\"{}…\"", "&".repeat(199));
    assert!(session_line(&plain).ends_with(&want), "{plain}");
}

/// I5a-9: an untitled session's quoted prompt is decoded too.
#[test]
fn breaker_untitled_prompt_is_decoded() {
    let sb = Sb::with_session(None, "a &amp; b");
    let plain = sb.plain();
    assert!(session_line(&plain).ends_with("\"a & b\""), "{plain}");
}

/// §8.2 *assumption* "whichever field supplies it": a Codex thread title (resolved from the DB
/// at render time, a separate branch of `render_impl`) decodes in plain and stays raw in JSON.
#[test]
fn breaker_codex_thread_title_decodes_in_plain_only() {
    let sb = Sb::with_codex_thread(ENTITY_TITLE, "first");
    let plain = sb.rollcall(&["status", sb.root_arg()], "").stdout;
    let lines: Vec<&str> = plain.lines().filter(|l| l.contains("codex ")).collect();
    assert_eq!(lines.len(), 1, "{plain}");
    assert!(lines[0].ends_with(DECODED_TITLE), "{plain}");
    assert_eq!(sb.json_title(), ENTITY_TITLE);
}

/// An untitled Codex thread: the title comes from `first_user_message` and is decoded too.
#[test]
fn breaker_codex_untitled_thread_prompt_decodes() {
    let sb = Sb::with_codex_thread("", "a &amp; b");
    let plain = sb.rollcall(&["status", sb.root_arg()], "").stdout;
    let lines: Vec<&str> = plain.lines().filter(|l| l.contains("codex ")).collect();
    assert_eq!(lines.len(), 1, "{plain}");
    assert!(lines[0].ends_with("\"a & b\""), "{plain}");
}

/// I5a-10 (MCP half) through the real server: `where_was_i` keeps the title undecoded.
#[test]
fn breaker_mcp_where_was_i_keeps_entities() {
    const INIT: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"breaker","version":"0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"where_was_i","arguments":{}}}
"#;
    let sb = Sb::with_session(Some(ENTITY_TITLE), "p");
    let run = sb.rollcall(&["mcp", "--root", sb.root_arg()], INIT);
    let reply: Value = run
        .stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["id"] == 2)
        .unwrap_or_else(|| panic!("no reply to id 2\n{}\n{}", run.stdout, run.stderr));
    let doc: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(doc["recent_sessions"][0]["id"], ID, "{doc}");
    assert_eq!(doc["recent_sessions"][0]["title"], ENTITY_TITLE, "{doc}");
}
