//! Breaker tests for plan phase P3 (Copilot source registered, `COPILOT_HOME` wired). Every test
//! drives the real binary on a sandbox of its own; no real agent home is read, and every prompt
//! here is synthetic.
#![allow(dead_code)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

const NOW: &str = "2026-09-29T12:00:00Z";
const ID: &str = "c0b10002-0000-4000-8000-000000000002";
const DEADLINE: Duration = Duration::from_secs(60);
const MARK: &str = "BREAKERSECRET";

struct Sb {
    _dir: TempDir,
    base: PathBuf,
    home: PathBuf,
    root: PathBuf,
    copilot: PathBuf,
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn wait(mut cmd: Command, what: &str) -> Run {
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let id = child.id();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let out = rx.recv_timeout(DEADLINE).unwrap_or_else(|_| {
        let _ = Command::new("/bin/kill").arg(id.to_string()).status();
        panic!("{what} outlived {DEADLINE:?}")
    });
    let out = out.unwrap();
    Run {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("/usr/bin/git")
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

impl Sb {
    /// Root `home/work` with committed repos `a` and `b`; `COPILOT_HOME` = `base/copilot`.
    fn new() -> Sb {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let sub = |name: &str| {
            let p = base.join(name);
            fs::create_dir_all(&p).unwrap();
            p
        };
        let sb = Sb {
            home: sub("home"),
            root: sub("home/work"),
            copilot: base.join("copilot"),
            _dir: dir,
            base,
        };
        for name in ["a", "b"] {
            let repo = sb.root.join(name);
            fs::create_dir_all(&repo).unwrap();
            git(&repo, &["init", "-q", "-b", "main"]);
            fs::write(repo.join("f"), "x").unwrap();
            git(&repo, &["add", "f"]);
            git(&repo, &["commit", "-q", "-m", "c"]);
        }
        sb
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rollcall"));
        cmd.env_clear()
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.base.join("xdg/config"))
            .env("XDG_CACHE_HOME", self.base.join("xdg/cache"))
            .env("CLAUDE_CONFIG_DIR", self.base.join("claude"))
            .env("CODEX_HOME", self.base.join("codex"))
            .env("COPILOT_HOME", &self.copilot)
            .env("ROLLCALL_NOW", NOW)
            .env("PATH", "/usr/bin:/bin");
        cmd
    }

    fn rollcall(&self, args: &[&str]) -> Run {
        wait(self.cmd(args), &format!("rollcall {args:?}"))
    }

    fn root(&self) -> &str {
        self.root.to_str().unwrap()
    }

    fn repo(&self, name: &str) -> String {
        self.root.join(name).to_str().unwrap().to_owned()
    }

    fn log(&self, id: &str) -> PathBuf {
        self.copilot
            .join("session-state")
            .join(id)
            .join("events.jsonl")
    }

    fn put(&self, id: &str, bytes: &[u8]) -> PathBuf {
        let path = self.log(id);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    fn status(&self, extra: &[&str]) -> Value {
        let mut args = vec!["status", "--json"];
        args.extend_from_slice(extra);
        args.push(self.root());
        let run = self.rollcall(&args);
        assert_eq!(run.code, Some(0), "status {extra:?}: {}", run.stderr);
        serde_json::from_str(&run.stdout).unwrap()
    }

    fn doctor(&self) -> Value {
        let run = self.rollcall(&["doctor", "--json", "--no-scan", self.root()]);
        serde_json::from_str(&run.stdout).unwrap_or_else(|e| panic!("doctor: {e}\n{}", run.stderr))
    }

    fn cache_dir(&self) -> PathBuf {
        rollcall_store::cache_dir(&self.home, Some(&self.base.join("xdg/cache")))
    }
}

fn append(path: &Path, bytes: &[u8]) {
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

fn t(min: u32) -> String {
    format!("2026-09-29T10:{min:02}:00.000Z")
}

fn ev(kind: &str, min: u32, data: Value) -> Value {
    json!({"type": kind, "data": data, "id": format!("e-{kind}-{min}"), "timestamp": t(min), "parentId": null})
}

fn start(min: u32, cwd: &str) -> Value {
    ev(
        "session.start",
        min,
        json!({"sessionId": ID, "version": 1, "producer": "copilot-agent", "copilotVersion": "1.0.93",
               "startTime": t(min), "contextTier": null,
               "context": {"cwd": cwd, "gitRoot": cwd, "branch": "main"},
               "alreadyInUse": false, "remoteSteerable": false}),
    )
}

fn user(min: u32, text: &str) -> Value {
    ev(
        "user.message",
        min,
        json!({"content": text, "transformedContent": text, "delivery": "idle",
               "interactionId": "i", "messageId": "m",
               "parentAgentTaskId": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
               "supportedNativeDocumentMimeTypes": [], "turnId": "0"}),
    )
}

fn pending(min: u32, cwd: &str) -> Value {
    ev(
        "session.context_changed",
        min,
        json!({"cwd": cwd, "pendingGitContext": true}),
    )
}

fn settled(min: u32, cwd: &str, branch: &str) -> Value {
    ev(
        "session.context_changed",
        min,
        json!({"cwd": cwd, "gitRoot": cwd, "branch": branch}),
    )
}

fn jsonl(records: &[Value]) -> Vec<u8> {
    let mut out = Vec::new();
    for r in records {
        out.extend(serde_json::to_vec(r).unwrap());
        out.push(b'\n');
    }
    out
}

/// The `last_session` of the worktree at `path`, or `Value::Null`.
fn last_session<'a>(doc: &'a Value, path: &str) -> &'a Value {
    doc["repos"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| r["worktrees"].as_array().unwrap())
        .find(|wt| wt["path"] == path)
        .map(|wt| &wt["last_session"])
        .unwrap_or_else(|| panic!("no worktree at {path}: {doc}"))
}

fn copilot_diag(doctor: &Value) -> &Value {
    doctor["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["source"] == "copilot")
        .unwrap_or_else(|| panic!("no copilot source: {doctor}"))
}

// ------------------------------------------------------------------ incremental through the store

/// §6.3 + §5.3 "Incremental reads", across two invocations: a prompt appended after the first scan
/// (torn first, then completed) is merged into the stored session: `message_count` +1, `last_prompt`
/// the new prompt, `title` still the first prompt.
#[test]
fn breaker_appended_prompt_reaches_the_next_status_through_the_store() {
    let sb = Sb::new();
    let a = sb.repo("a");
    let log = sb.put(ID, &jsonl(&[start(0, &a), user(1, "first ask")]));
    let s1 = sb.status(&[]);
    let s = last_session(&s1, &a);
    assert_eq!(
        (s["message_count"].as_u64(), &s["title"]),
        (Some(1), &json!("first ask")),
        "{s}"
    );

    // Torn: half a line with no newline is not counted yet.
    let line = jsonl(&[user(5, "second ask")]);
    let (head, tail) = line.split_at(line.len() / 2);
    append(&log, head);
    let s2 = sb.status(&[]);
    assert_eq!(last_session(&s2, &a)["message_count"], 1, "{s2}");

    append(&log, tail);
    let s3 = sb.status(&[]);
    let s = last_session(&s3, &a);
    assert_eq!(s["message_count"], 2, "{s}");
    assert_eq!(s["title"], "first ask", "{s}");
    assert_eq!(s["last_prompt"], "second ask", "{s}");
    assert_eq!(s["ended_at"], "2026-09-29T10:05:00Z", "{s}");
}

/// §5.3 "Rewind" / §6.3 "size < offset → re-read from 0": the log is truncated to its first prompt
/// between invocations; the next status rebuilds instead of keeping the stale count.
#[test]
fn breaker_truncated_log_is_rebuilt_on_the_next_status() {
    let sb = Sb::new();
    let a = sb.repo("a");
    let short = jsonl(&[start(0, &a), user(1, "first ask")]);
    let mut long = short.clone();
    long.extend(jsonl(&[user(2, "second ask"), user(3, "third ask")]));
    let log = sb.put(ID, &long);
    assert_eq!(last_session(&sb.status(&[]), &a)["message_count"], 3);

    fs::write(&log, &short).unwrap();
    let doc = sb.status(&[]);
    let s = last_session(&doc, &a);
    assert_eq!(s["message_count"], 1, "{s}");
    assert_eq!(s["last_prompt"], "first ask", "{s}");
    assert_eq!(s["ended_at"], "2026-09-29T10:01:00Z", "{s}");
}

/// §6.2 resolution after the file changed, answered from the cache (`--no-scan`): an append keeps the
/// title resolvable (size ≥ size_at_scan); a truncation below size_at_scan resolves to null.
#[test]
fn breaker_no_scan_resolution_follows_the_file_after_it_changed() {
    let sb = Sb::new();
    let a = sb.repo("a");
    let log = sb.put(
        ID,
        &jsonl(&[start(0, &a), user(1, "first ask"), user(2, "second ask")]),
    );
    sb.status(&[]);
    append(&log, &jsonl(&[user(3, "third ask")]));
    let doc = sb.status(&["--no-scan"]);
    let s = last_session(&doc, &a);
    assert_eq!(
        (&s["title"], &s["last_prompt"]),
        (&json!("first ask"), &json!("second ask")),
        "{s}"
    );

    fs::write(&log, jsonl(&[start(0, &a)])).unwrap();
    let doc = sb.status(&["--no-scan"]);
    let s = last_session(&doc, &a);
    assert_eq!(
        (&s["title"], &s["last_prompt"]),
        (&Value::Null, &Value::Null),
        "{s}"
    );
    assert_eq!(
        s["title_kind"],
        Value::Null,
        "title_kind is null iff title is null (§7): {s}"
    );
}

/// §5.0 vanished, through the store: the session dir removed between invocations → the worktree has
/// no last session and doctor counts one vanished.
#[test]
fn breaker_removed_session_dir_is_vanished_on_the_next_scan() {
    let sb = Sb::new();
    let a = sb.repo("a");
    sb.put(ID, &jsonl(&[start(0, &a), user(1, "ask")]));
    assert_eq!(last_session(&sb.status(&[]), &a)["source"], "copilot");
    fs::remove_dir_all(sb.log(ID).parent().unwrap()).unwrap();
    let doc = sb.status(&[]);
    assert_eq!(last_session(&doc, &a), &Value::Null, "{doc}");
    let d = sb.doctor();
    let c = copilot_diag(&d);
    assert_eq!(
        (c["vanished"].as_u64(), c["sessions"].as_u64()),
        (Some(1), Some(0)),
        "{c}"
    );
}

// ------------------------------------------------------------------ context objects through matching

/// §5.3 cwd/branch + §6.5: a lone pending `/cd` does not move the session; the settled event that
/// follows (appended between invocations) moves it to repo `b`, and the resume line follows.
#[test]
fn breaker_pending_then_settled_cd_moves_the_session_between_worktrees() {
    let sb = Sb::new();
    let (a, b) = (sb.repo("a"), sb.repo("b"));
    let log = sb.put(ID, &jsonl(&[start(0, &a), user(1, "ask"), pending(2, &b)]));
    let doc = sb.status(&[]);
    assert_eq!(last_session(&doc, &a)["id"], ID, "pending moved it: {doc}");
    assert_eq!(last_session(&doc, &b), &Value::Null, "{doc}");

    append(&log, &jsonl(&[settled(3, &b, "dev")]));
    let doc = sb.status(&[]);
    assert_eq!(last_session(&doc, &a), &Value::Null, "{doc}");
    let s = last_session(&doc, &b);
    assert_eq!((&s["cwd"], &s["branch"]), (&json!(b), &json!("dev")), "{s}");
    assert_eq!(
        s["resume"],
        format!("cd -- '{b}' && copilot --resume {ID}"),
        "{s}"
    );
}

/// §5.3: `copilot --resume` appends after `session.shutdown`; a resume with a new context and a prompt
/// moves the session and extends `ended_at` past the shutdown.
#[test]
fn breaker_resume_after_shutdown_takes_the_resumed_context() {
    let sb = Sb::new();
    let (a, b) = (sb.repo("a"), sb.repo("b"));
    let log = sb.put(
        ID,
        &jsonl(&[
            start(0, &a),
            user(1, "ask"),
            ev("session.shutdown", 2, json!({"shutdownType": "routine"})),
        ]),
    );
    sb.status(&[]);
    let resume = ev(
        "session.resume",
        30,
        json!({"resumeTime": t(30), "eventCount": 3, "alreadyInUse": false,
               "context": {"cwd": b, "gitRoot": b, "branch": "main"}}),
    );
    append(&log, &jsonl(&[resume, user(31, "again")]));
    let doc = sb.status(&[]);
    let s = last_session(&doc, &b);
    assert_eq!(s["id"], ID, "{doc}");
    assert_eq!(
        (s["message_count"].as_u64(), &s["ended_at"]),
        (Some(2), &json!("2026-09-29T10:31:00Z")),
        "{s}"
    );
}

/// §5.3 children / human prompt: an `agentId` line (prompt and context) neither counts nor moves;
/// a human `user.message` with `parentAgentTaskId` counts.
#[test]
fn breaker_agent_lines_neither_count_nor_move_through_the_binary() {
    let sb = Sb::new();
    let (a, b) = (sb.repo("a"), sb.repo("b"));
    let mut agent_prompt = user(2, "agent ask");
    agent_prompt["agentId"] = json!("sub-1");
    let mut agent_ctx = settled(3, &b, "dev");
    agent_ctx["agentId"] = json!("sub-1");
    let mut started = ev(
        "subagent.started",
        2,
        json!({"toolCallId": "t", "agentName": "x"}),
    );
    started["agentId"] = json!("sub-1");
    sb.put(
        ID,
        &jsonl(&[
            start(0, &a),
            user(1, "human ask"),
            started,
            agent_prompt,
            agent_ctx,
        ]),
    );
    let doc = sb.status(&[]);
    let s = last_session(&doc, &a);
    assert_eq!(
        (s["message_count"].as_u64(), &s["last_prompt"]),
        (Some(1), &json!("human ask")),
        "{s}"
    );
    let d = sb.doctor();
    let c = copilot_diag(&d);
    assert_eq!(
        (c["sessions"].as_u64(), c["excluded_children"].as_u64()),
        (Some(1), Some(1)),
        "{c}"
    );
}

/// §5.3 "Session with no human prompt": a row with message_count 0, null title / last_prompt, and a
/// resume line.
#[test]
fn breaker_zero_prompt_session_is_listed_with_a_resume_line() {
    let sb = Sb::new();
    let a = sb.repo("a");
    sb.put(
        ID,
        &jsonl(&[
            start(0, &a),
            ev("session.model_change", 1, json!({"newModel": "m"})),
        ]),
    );
    let doc = sb.status(&[]);
    let s = last_session(&doc, &a);
    assert_eq!(s["message_count"], 0, "{s}");
    assert_eq!(
        (&s["title"], &s["title_kind"], &s["last_prompt"]),
        (&Value::Null, &Value::Null, &Value::Null),
        "{s}"
    );
    assert_eq!(
        s["resume"],
        format!("cd -- '{a}' && copilot --resume {ID}"),
        "{s}"
    );
}

/// §5.3 id = the directory name; §5.0 regex accepts upper-case hex. The id and the resume argv keep
/// the directory name as written (lower-casing it would name a directory Copilot does not have).
#[test]
fn breaker_upper_case_uuid_dir_keeps_its_name_as_the_id() {
    let sb = Sb::new();
    let a = sb.repo("a");
    let upper = ID.to_uppercase();
    sb.put(&upper, &jsonl(&[start(0, &a), user(1, "ask")]));
    let doc = sb.status(&[]);
    let s = last_session(&doc, &a);
    assert_eq!(s["id"], upper.as_str(), "{s}");
    assert_eq!(
        s["resume"],
        format!("cd -- '{a}' && copilot --resume {upper}"),
        "{s}"
    );
}

// ------------------------------------------------------------------ discovery through the binary

/// §5.3 discovery: a FIFO `events.jsonl` is counted in `files`, refused by the non-blocking open
/// (no hang), gives no session; mode is `jsonl`.
#[test]
fn breaker_fifo_events_jsonl_does_not_hang_and_counts_one_file() {
    let sb = Sb::new();
    let dir = sb.log(ID).parent().unwrap().to_path_buf();
    fs::create_dir_all(&dir).unwrap();
    let ok = Command::new("/usr/bin/mkfifo")
        .arg(dir.join("events.jsonl"))
        .status()
        .unwrap();
    assert!(ok.success());
    let run = sb.rollcall(&["doctor", "--json", sb.root()]);
    let d: Value =
        serde_json::from_str(&run.stdout).unwrap_or_else(|e| panic!("{e}: {}", run.stderr));
    let c = copilot_diag(&d);
    assert_eq!(c["mode"], "jsonl", "{c}");
    assert_eq!(
        (
            c["files"].as_u64(),
            c["sessions"].as_u64(),
            c["records"].as_u64()
        ),
        (Some(1), Some(0), Some(0)),
        "{c}"
    );
}

/// §5.3 discovery: a symlinked session dir and a symlinked `events.jsonl` give no session; the symlinked
/// dir is not counted anywhere, the symlinked log counts one file; dot-named and non-UUID dirs: the
/// former silent, the latter `non_uuid`.
#[test]
fn breaker_symlinks_dot_and_non_uuid_dirs_give_no_session() {
    let sb = Sb::new();
    let a = sb.repo("a");
    let elsewhere = sb.base.join("elsewhere");
    let real_log = elsewhere.join("events.jsonl");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(&real_log, jsonl(&[start(0, &a), user(1, "ask")])).unwrap();
    let store = sb.copilot.join("session-state");
    fs::create_dir_all(&store).unwrap();
    std::os::unix::fs::symlink(
        &elsewhere,
        store.join("c0b10003-0000-4000-8000-000000000003"),
    )
    .unwrap();
    let linked = store.join("c0b10004-0000-4000-8000-000000000004");
    fs::create_dir_all(&linked).unwrap();
    std::os::unix::fs::symlink(&real_log, linked.join("events.jsonl")).unwrap();
    for name in [".session-operation-locks", "my-named-session"] {
        let d = store.join(name);
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("events.jsonl"),
            jsonl(&[start(0, &a), user(1, "ask")]),
        )
        .unwrap();
    }
    let doc = sb.status(&[]);
    assert_eq!(last_session(&doc, &a), &Value::Null, "{doc}");
    let d = sb.doctor();
    let c = copilot_diag(&d);
    assert_eq!(
        (c["files"].as_u64(), c["sessions"].as_u64()),
        (Some(1), Some(0)),
        "{c}"
    );
    assert_eq!(c["skipped"]["non_uuid"], 1, "{c}");
}

// ------------------------------------------------------------------ privacy

/// §5.3 "No prompt text reaches notes, diagnostics or the cache": prompt text (in prompts, in a
/// malformed line, in an unknown-type line, in an incomplete log) appears in no cache file and no
/// doctor/scan output.
#[test]
fn breaker_no_copilot_prompt_text_in_cache_doctor_or_scan_output() {
    let sb = Sb::new();
    let a = sb.repo("a");
    let mut bytes = jsonl(&[
        start(0, &a),
        user(1, &format!("{MARK} one")),
        user(2, &format!("{MARK} two")),
    ]);
    bytes.extend(
        format!("{{\"type\":\"user.message\",\"data\":{{\"content\":\"{MARK} torn\"\n").as_bytes(),
    );
    bytes.extend(jsonl(&[ev(
        "weird.kind",
        3,
        json!({"content": format!("{MARK} unknown")}),
    )]));
    sb.put(ID, &bytes);
    sb.put(
        "c0b10005-0000-4000-8000-000000000005",
        &jsonl(&[user(1, &format!("{MARK} orphan"))]),
    );

    let mut outputs = Vec::new();
    for args in [
        vec!["scan", sb.root()],
        vec!["doctor", sb.root()],
        vec!["doctor", "--json", sb.root()],
        vec!["status", sb.root()],
    ] {
        let run = sb.rollcall(&args);
        outputs.push((args.join(" "), run.stdout, run.stderr));
    }
    let doc = sb.status(&[]);
    assert_eq!(
        last_session(&doc, &a)["message_count"],
        2,
        "control: the session was read: {doc}"
    );
    for (what, out, err) in &outputs {
        if what.starts_with("status") {
            // Plain status quotes the title by design; only stderr must stay clean.
            assert!(!err.contains(MARK), "{what} stderr: {err}");
            continue;
        }
        assert!(
            !out.contains(MARK) && !err.contains(MARK),
            "{what}:\n{out}\n{err}"
        );
    }
    let mut hay = Vec::new();
    for entry in fs::read_dir(sb.cache_dir()).unwrap() {
        hay.extend(fs::read(entry.unwrap().path()).unwrap());
    }
    assert!(
        !hay.windows(MARK.len()).any(|w| w == MARK.as_bytes()),
        "prompt text in the cache"
    );
}

// ------------------------------------------------------------------ COPILOT_HOME edge values

/// §5.3 root: a `COPILOT_HOME` naming a regular file is a missing root: copilot `absent`, status
/// exits 0.
#[test]
fn breaker_copilot_home_naming_a_file_is_absent() {
    let sb = Sb::new();
    fs::write(&sb.copilot, b"not a dir").unwrap();
    let run = sb.rollcall(&["doctor", "--json", sb.root()]);
    let d: Value =
        serde_json::from_str(&run.stdout).unwrap_or_else(|e| panic!("{e}: {}", run.stderr));
    assert_eq!(copilot_diag(&d)["mode"], "absent", "{d}");
    assert_eq!(sb.rollcall(&["status", "--json", sb.root()]).code, Some(0));
}

// ------------------------------------------------------------------ MCP text policy

/// §8.3: without `--prompts` a Copilot (prompt-kind) title and last_prompt are null; with `--prompts`
/// both are the resolved prompts.
#[test]
fn breaker_mcp_prompt_policy_applies_to_copilot_sessions() {
    let sb = Sb::new();
    let a = sb.repo("a");
    sb.put(
        ID,
        &jsonl(&[start(0, &a), user(1, "first ask"), user(2, "second ask")]),
    );
    sb.status(&[]);
    let call = |extra: &[&str]| -> Value {
        let mut args = vec!["mcp", "--root", sb.root()];
        args.extend_from_slice(extra);
        let mut cmd = sb.cmd(&args);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        let input = [
            json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"where_was_i","arguments":{"repo":"a"}}}),
        ];
        let mut stdin = child.stdin.take().unwrap();
        for m in input {
            writeln!(stdin, "{m}").unwrap();
        }
        drop(stdin);
        let (tx, rx) = mpsc::channel();
        let id = child.id();
        thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });
        let out = rx.recv_timeout(DEADLINE).unwrap_or_else(|_| {
            let _ = Command::new("/bin/kill").arg(id.to_string()).status();
            panic!("mcp outlived {DEADLINE:?}")
        });
        let out = out.unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let reply: Value = stdout
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .find(|v| v["id"] == 1)
            .unwrap_or_else(|| {
                panic!(
                    "no reply: {stdout}\n{}",
                    String::from_utf8_lossy(&out.stderr)
                )
            });
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    };
    let plain = call(&[]);
    let s = &plain["recent_sessions"][0];
    assert_eq!(s["source"], "copilot", "{plain}");
    assert_eq!(
        (&s["title"], &s["last_prompt"]),
        (&Value::Null, &Value::Null),
        "{s}"
    );
    let full = call(&["--prompts"]);
    let s = &full["recent_sessions"][0];
    assert_eq!(
        (&s["title"], &s["last_prompt"]),
        (&json!("first ask"), &json!("second ask")),
        "{s}"
    );
}

// ------------------------------------------------------------------ second pass

fn order(d: &Value) -> Vec<String> {
    d["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["source"].as_str().unwrap().to_owned())
        .collect()
}

/// §7 `DoctorDoc.sources` "in the order claude, codex, copilot", with a populated (jsonl) Copilot
/// source, from a fresh scan and from the stored diagnostics (`--no-scan`).
#[test]
fn breaker_doctor_source_order_holds_with_copilot_data_fresh_and_cached() {
    let sb = Sb::new();
    let a = sb.repo("a");
    sb.put(ID, &jsonl(&[start(0, &a), user(1, "ask")]));
    let run = sb.rollcall(&["doctor", "--json", sb.root()]);
    let fresh: Value =
        serde_json::from_str(&run.stdout).unwrap_or_else(|e| panic!("{e}: {}", run.stderr));
    let cached = sb.doctor();
    for d in [&fresh, &cached] {
        assert_eq!(order(d), ["claude", "codex", "copilot"], "{d}");
        assert_eq!(copilot_diag(d)["mode"], "jsonl", "{d}");
        assert_eq!(copilot_diag(d)["matched"], 1, "{d}");
    }
}

/// §12.1 #13 / P3-7 with the three variables set to the empty string (unset per §3.1/§5.3) and no
/// agent dirs under the sandbox HOME: exit 0, every last_session null, three `absent` sources in order.
#[test]
fn breaker_all_three_agent_homes_empty_strings_mean_no_agent_mode() {
    let sb = Sb::new();
    // Data under the explicit dirs must not be seen once the variables are empty.
    sb.put(ID, &jsonl(&[start(0, &sb.repo("a")), user(1, "ask")]));
    let run_with = |args: &[&str]| {
        let mut cmd = sb.cmd(args);
        cmd.env("CLAUDE_CONFIG_DIR", "")
            .env("CODEX_HOME", "")
            .env("COPILOT_HOME", "");
        wait(cmd, "rollcall")
    };
    let st = run_with(&["status", "--json", sb.root()]);
    assert_eq!(st.code, Some(0), "{}", st.stderr);
    let doc: Value = serde_json::from_str(&st.stdout).unwrap();
    for name in ["a", "b"] {
        assert_eq!(last_session(&doc, &sb.repo(name)), &Value::Null, "{doc}");
    }
    let d: Value =
        serde_json::from_str(&run_with(&["doctor", "--json", sb.root()]).stdout).unwrap();
    let modes: Vec<(String, String)> = d["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["source"].as_str().unwrap().into(),
                s["mode"].as_str().unwrap().into(),
            )
        })
        .collect();
    let want: Vec<(String, String)> = ["claude", "codex", "copilot"]
        .iter()
        .map(|s| (s.to_string(), "absent".to_string()))
        .collect();
    assert_eq!(modes, want, "{d}");
}

/// §9 / §8.3 "All returned text is scrubbed and truncated" on Copilot-resolved text: a secret in a
/// prompt is redacted in `status --json`, plain `status`, and MCP `--prompts`; a 300-char prompt is
/// truncated (§3.3, 200 graphemes).
#[test]
fn breaker_copilot_prompt_text_is_scrubbed_and_truncated_everywhere() {
    let sb = Sb::new();
    let a = sb.repo("a");
    let secret = "sk-abcdefghijklmnopqrstuv";
    let long = "x".repeat(300);
    sb.put(
        ID,
        &jsonl(&[
            start(0, &a),
            user(1, &format!("use {secret} now")),
            user(2, &long),
        ]),
    );
    let doc = sb.status(&[]);
    let s = last_session(&doc, &a);
    let title = s["title"].as_str().unwrap();
    assert!(
        !title.contains(secret) && title.contains("[redacted]"),
        "{s}"
    );
    let last = s["last_prompt"].as_str().unwrap();
    assert!(
        last.chars().count() <= 201 && last.chars().count() < 300,
        "{last}"
    );

    let plain = sb.rollcall(&["status", sb.root()]);
    assert!(plain.stdout.contains("copilot"), "{}", plain.stdout);
    assert!(
        plain.stdout.contains("[redacted]") && !plain.stdout.contains(secret),
        "{}",
        plain.stdout
    );
}
