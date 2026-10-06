//! Breaker (adversarial QA) tests for I2: exit code 4 for a completed scan with error rows
//! (spec rev 16 §3.1). Every test drives the real binary on a sandbox of its own.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

const NOW: &str = "2026-09-29T12:00:00Z";
const DEADLINE: Duration = Duration::from_secs(60);

struct Sb {
    _dir: TempDir,
    base: PathBuf,
    home: PathBuf,
    xdg_config: PathBuf,
    xdg_cache: PathBuf,
    root: PathBuf,
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
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
            xdg_cache: sub("xdg/cache"),
            root: sub("work"),
            _dir: dir,
            base,
        }
    }

    fn cache_dir(&self) -> PathBuf {
        rollcall_store::cache_dir(&self.home, Some(&self.xdg_cache))
    }

    fn write_config(&self, text: &str) {
        let dir = self.xdg_config.join("rollcall");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), text).unwrap();
    }

    fn rollcall(&self, args: &[&str]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rollcall"));
        cmd.env_clear()
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.xdg_config)
            .env("XDG_CACHE_HOME", &self.xdg_cache)
            .env("CLAUDE_CONFIG_DIR", self.base.join("claude"))
            .env("CODEX_HOME", self.base.join("codex"))
            .env("ROLLCALL_NOW", NOW)
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = cmd.spawn().unwrap();
        let id = child.id();
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });
        let out = rx.recv_timeout(DEADLINE).unwrap_or_else(|_| {
            // Best effort; the test fails regardless.
            let _ = Command::new("/bin/kill").arg(id.to_string()).status();
            panic!("rollcall {args:?} outlived {DEADLINE:?}")
        });
        let out = out.unwrap();
        Run {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn root_str(&self) -> &str {
        self.root.to_str().unwrap()
    }

    fn git(&self, dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_COMMITTER_DATE", "2026-09-20T10:00:00Z")
            .env("GIT_AUTHOR_DATE", "2026-09-20T10:00:00Z")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    /// A clean repo with one empty commit.
    fn repo(&self, path: &Path) {
        fs::create_dir_all(path).unwrap();
        self.git(path, &["init", "-q", "-b", "main"]);
        self.git(
            path,
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
        );
    }
}

/// A `.git` file naming a gitdir that does not exist: one `Repo.error` row.
fn bad_repo(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::write(path.join(".git"), "gitdir: ../does-not-exist\n").unwrap();
}

/// A root with one clean repo and one error row.
fn error_root() -> Sb {
    let sb = Sb::new();
    sb.repo(&sb.root.join("good"));
    bad_repo(&sb.root.join("bad"));
    sb
}

fn json(r: &Run) -> Value {
    serde_json::from_str(&r.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}\nstderr: {}", r.stdout, r.stderr))
}

fn any_error(doc: &Value) -> bool {
    doc["repos"].as_array().unwrap().iter().any(|r| {
        r["error"].is_string()
            || r["worktrees"]
                .as_array()
                .is_some_and(|w| w.iter().any(|w| w["error"].is_string()))
    })
}

/// `(<E> errors)` from the §3.1 summary line.
fn summary_errors(stderr: &str) -> u32 {
    let line = stderr.trim_end();
    assert_eq!(line.lines().count(), 1, "stderr: {stderr}");
    assert!(line.starts_with("scanned "), "stderr: {line}");
    line.strip_suffix(" errors)")
        .and_then(|head| head.rsplit_once('('))
        .and_then(|(_, n)| n.parse().ok())
        .unwrap_or_else(|| panic!("no `(<E> errors)` ending: {line}"))
}

/// Takes the root's scan lock once free (the harness is the other scanner).
fn hold_lock(sb: &Sb) -> rollcall_store::ScanLock {
    let end = Instant::now() + DEADLINE;
    loop {
        if let Some(lock) = rollcall_store::try_scan_lock(&sb.cache_dir(), &sb.root).unwrap() {
            return lock;
        }
        assert!(Instant::now() < end, "the scan lock was never released");
        thread::sleep(Duration::from_millis(10));
    }
}

// I2-4: an answer given while the scan lock is held takes its code from the stored complete scan: 4,
// `stale: true`. Also through `scan` (summary line) and `--no-scan`.
#[test]
fn breaker_lock_held_answers_4_from_a_stored_error_scan() {
    let sb = error_root();
    let first = sb.rollcall(&["status", "--json", sb.root_str()]);
    assert_eq!(first.code, Some(4), "stderr: {}", first.stderr);
    assert!(any_error(&json(&first)));

    let _lock = hold_lock(&sb);
    let held = sb.rollcall(&["status", "--json", sb.root_str()]);
    assert_eq!(held.code, Some(4), "stderr: {}", held.stderr);
    assert_eq!(json(&held)["stale"], Value::Bool(true));

    let scan = sb.rollcall(&["scan", sb.root_str()]);
    assert_eq!(scan.code, Some(4), "stderr: {}", scan.stderr);
    assert!(summary_errors(&scan.stderr) >= 1, "{}", scan.stderr);

    let no_scan = sb.rollcall(&["scan", "--no-scan", sb.root_str()]);
    assert_eq!(no_scan.code, Some(4), "stderr: {}", no_scan.stderr);
}

// I2-12: `doctor` keeps 1 for error rows on every path (fresh, --no-scan, lock held).
#[test]
fn breaker_doctor_exits_1_on_cache_answers() {
    let sb = error_root();
    let fresh = sb.rollcall(&["doctor", "--json", sb.root_str()]);
    assert_eq!(fresh.code, Some(1), "stderr: {}", fresh.stderr);
    let no_scan = sb.rollcall(&["doctor", "--no-scan", sb.root_str()]);
    assert_eq!(no_scan.code, Some(1), "stderr: {}", no_scan.stderr);
    let _lock = hold_lock(&sb);
    let held = sb.rollcall(&["doctor", sb.root_str()]);
    assert_eq!(held.code, Some(1), "stderr: {}", held.stderr);
}

// I2-1 via the binary: only a `Worktree.error` (a linked worktree whose admin dir lost `commondir`)
// exits 4, and the summary line counts it.
#[test]
fn breaker_worktree_error_alone_exits_4_and_is_counted() {
    let sb = Sb::new();
    let main = sb.root.join("main");
    sb.repo(&main);
    let linked = sb.root.join("linked");
    sb.git(&main, &["worktree", "add", "-q", linked.to_str().unwrap()]);
    let admin = fs::read_dir(main.join(".git/worktrees"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::remove_file(admin.join("commondir")).unwrap();

    let status = sb.rollcall(&["status", "--json", sb.root_str()]);
    let doc = json(&status);
    let repos = doc["repos"].as_array().unwrap();
    assert!(repos.iter().all(|r| r["error"].is_null()), "{doc}");
    assert!(any_error(&doc), "premise: a worktree error row\n{doc}");
    assert_eq!(status.code, Some(4), "stderr: {}", status.stderr);

    let scan = sb.rollcall(&["scan", sb.root_str()]);
    assert_eq!(scan.code, Some(4), "stderr: {}", scan.stderr);
    assert!(summary_errors(&scan.stderr) >= 1, "{}", scan.stderr);
}

// I2-2/I2-8 via the cache: a stored partial scan with an error row answers 1 when the lock is held
// and under --no-scan (1 wins over 4).
#[test]
fn breaker_stored_partial_scan_with_error_row_answers_1_from_cache() {
    let sb = Sb::new();
    let bad = sb.root.join("bad");
    bad_repo(&bad);
    fs::create_dir_all(bad.join("sub")).unwrap();
    for i in 0..10 {
        fs::write(bad.join(format!("sub/f{i}")), "x").unwrap();
    }
    sb.write_config("walk_budget_entries = 5\n");
    let fresh = sb.rollcall(&["status", "--json", sb.root_str()]);
    let doc = json(&fresh);
    assert_eq!(doc["partial"], Value::Bool(true), "{doc}");
    assert!(any_error(&doc), "premise: an error row\n{doc}");
    assert_eq!(fresh.code, Some(1));

    let no_scan = sb.rollcall(&["status", "--json", "--no-scan", sb.root_str()]);
    assert_eq!(no_scan.code, Some(1), "stderr: {}", no_scan.stderr);
    let _lock = hold_lock(&sb);
    let held = sb.rollcall(&["scan", sb.root_str()]);
    assert_eq!(held.code, Some(1), "stderr: {}", held.stderr);
}

// I2-13 via the binary: a stored complete scan with an error row (alone 4); the next fresh scan fails
// at commit (the test holds the SQLite write lock past the 5 s busy timeout) and falls back to the
// cache, which must exit 1, not 4.
#[test]
fn breaker_failed_fresh_scan_over_error_cache_exits_1() {
    let sb = error_root();
    let first = sb.rollcall(&["status", "--json", sb.root_str()]);
    assert_eq!(first.code, Some(4), "stderr: {}", first.stderr);

    let conn = rusqlite::Connection::open(sb.cache_dir().join("cache.sqlite")).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let failed = sb.rollcall(&["status", "--json", sb.root_str()]);
    conn.execute_batch("ROLLBACK").unwrap();
    assert!(
        failed.stderr.contains("failed"),
        "premise: the scan failed\nstderr: {}",
        failed.stderr
    );
    assert_eq!(json(&failed)["stale"], Value::Bool(true));
    assert_eq!(failed.code, Some(1), "stderr: {}", failed.stderr);
}

// I2-6 via cache paths: a clean root answers 0 fresh, under --no-scan and with the lock held.
#[test]
fn breaker_clean_root_exits_0_on_every_path() {
    let sb = Sb::new();
    sb.repo(&sb.root.join("good"));
    let fresh = sb.rollcall(&["scan", sb.root_str()]);
    assert_eq!(fresh.code, Some(0), "stderr: {}", fresh.stderr);
    assert_eq!(summary_errors(&fresh.stderr), 0);
    let no_scan = sb.rollcall(&["status", "--no-scan", sb.root_str()]);
    assert_eq!(no_scan.code, Some(0), "stderr: {}", no_scan.stderr);
    let _lock = hold_lock(&sb);
    let held = sb.rollcall(&["status", "--json", sb.root_str()]);
    assert_eq!(held.code, Some(0), "stderr: {}", held.stderr);
}

// §3.1 + §4.2: a submodule row is a `Repo`; with `--submodules` its error alone exits 4 (and is
// counted in the summary). Without `--submodules` the row is omitted, so a clean parent exits 0.
#[test]
fn breaker_submodule_error_row_exits_4_only_with_submodules() {
    let sb = Sb::new();
    let lib = sb.base.join("lib-src");
    sb.repo(&lib);
    let sup = sb.root.join("sup");
    sb.repo(&sup);
    sb.git(
        &sup,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            lib.to_str().unwrap(),
            "libs/sub",
        ],
    );
    // Break the submodule's git dir: no HEAD, so opening it fails.
    fs::remove_file(sup.join(".git/modules/libs/sub/HEAD")).unwrap();

    let with = sb.rollcall(&["status", "--json", "--submodules", sb.root_str()]);
    let doc = json(&with);
    let repos = doc["repos"].as_array().unwrap();
    assert!(repos.iter().all(|r| r["error"].is_null()), "{doc}");
    let sub_err = repos.iter().any(|r| {
        r["submodules"]
            .as_array()
            .is_some_and(|s| s.iter().any(|s| s["error"].is_string()))
    });
    assert!(sub_err, "premise: a submodule error row\n{doc}");
    assert_eq!(with.code, Some(4), "stderr: {}", with.stderr);

    let scan = sb.rollcall(&["scan", "--submodules", sb.root_str()]);
    assert_eq!(scan.code, Some(4), "stderr: {}", scan.stderr);
    assert!(summary_errors(&scan.stderr) >= 1, "{}", scan.stderr);

    let without = sb.rollcall(&["status", "--json", sb.root_str()]);
    let doc = json(&without);
    assert!(!any_error(&doc), "{doc}");
    assert_eq!(without.code, Some(0), "stderr: {}", without.stderr);
}

// I2-11 / §3.1: the TUI's `q` stays 0 even after its background scan found an error row.
#[test]
fn breaker_tui_q_exits_0_after_a_scan_with_error_rows() {
    use expectrl::{Eof, Expect, Session};
    let sb = error_root();
    // Warm the cache so the first paint already lists the error row.
    let warm = sb.rollcall(&["status", "--json", sb.root_str()]);
    assert_eq!(warm.code, Some(4), "stderr: {}", warm.stderr);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rollcall"));
    cmd.env_clear()
        .arg(sb.root_str())
        .env("HOME", &sb.home)
        .env("XDG_CONFIG_HOME", &sb.xdg_config)
        .env("XDG_CACHE_HOME", &sb.xdg_cache)
        .env("CLAUDE_CONFIG_DIR", sb.base.join("claude"))
        .env("CODEX_HOME", sb.base.join("codex"))
        .env("ROLLCALL_NOW", NOW)
        .env("TERM", "xterm-256color")
        .env("PATH", "/usr/bin:/bin");
    let mut p = Session::spawn(cmd).expect("spawn in a PTY");
    p.set_expect_timeout(Some(Duration::from_secs(30)));
    p.get_process_mut().set_window_size(120, 32).unwrap();
    p.expect("bad").expect("the error row was never drawn");
    p.send("q").expect("press q");
    p.expect(Eof).expect("the TUI never exited after q");
    let status = p.get_process_mut().wait().expect("wait");
    let shown = format!("{status:?}");
    assert!(
        shown.starts_with("Exited(") && shown.ends_with(", 0)"),
        "{shown}"
    );
}
