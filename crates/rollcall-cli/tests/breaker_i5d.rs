//! Breaker (adversarial QA) tests for I5d: the `--dirty`, `--stale` and `--top N` filters of
//! plain `status` (spec rev 16 §8.2, §3.1), through channels `filters_i5d.rs` does not drive:
//! the cached answer, the risk line and prune hint under a filter, the `inactive` stale reason,
//! `unpushed` without an upstream, flag order, and `--json` on an exit-4 scan.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tempfile::TempDir;

const NOW: &str = "2026-09-29T12:00:00Z";
const DATE: &str = "2026-09-20T10:00:00Z";
const DEADLINE: Duration = Duration::from_secs(60);

struct Sb {
    _dir: TempDir,
    base: PathBuf,
    home: PathBuf,
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
            root: sub("home/work"),
            _dir: dir,
            base,
        }
    }

    fn rollcall(&self, args: &[&str]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rollcall"));
        cmd.env_clear()
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.base.join("xdg/config"))
            .env("XDG_CACHE_HOME", self.base.join("xdg/cache"))
            .env("CLAUDE_CONFIG_DIR", self.base.join("claude"))
            .env("CODEX_HOME", self.base.join("codex"))
            .env("ROLLCALL_NOW", NOW)
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = cmd.spawn().unwrap();
        let id = child.id();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });
        let out = rx.recv_timeout(DEADLINE).unwrap_or_else(|_| {
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

    /// `rollcall status <root> <extra…>`; stdout is a pipe.
    fn status(&self, extra: &[&str]) -> Run {
        let mut args = vec!["status", self.root.to_str().unwrap()];
        args.extend_from_slice(extra);
        self.rollcall(&args)
    }

    fn git(&self, dir: &Path, date: &str, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_COMMITTER_DATE", date)
            .env("GIT_AUTHOR_DATE", date)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    /// A clean repo `name` under the root with one commit made at `date`, no remote.
    fn repo_at(&self, name: &str, date: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::create_dir_all(&path).unwrap();
        self.git(&path, date, &["init", "-q", "-b", "main"]);
        self.git(&path, date, &["commit", "-q", "--allow-empty", "-m", "c"]);
        path
    }

    /// `name` with a linked worktree `feat` outside the root.
    fn with_linked(&self, name: &str, date: &str) -> (PathBuf, PathBuf) {
        let path = self.repo_at(name, date);
        let linked = self.base.join(format!("linked/{name}-feat"));
        fs::create_dir_all(linked.parent().unwrap()).unwrap();
        let add = ["worktree", "add", "-q", "-b", "feat"];
        self.git(
            &path,
            date,
            &[&add[..], &[linked.to_str().unwrap()]].concat(),
        );
        (path, linked)
    }

    /// `m`: clean main, a linked worktree with a commit of its own and an untracked file.
    fn m(&self) {
        let (_, linked) = self.with_linked("m", DATE);
        self.git(&linked, DATE, &["commit", "-q", "--allow-empty", "-m", "f"]);
        fs::write(linked.join("u"), "x").unwrap();
    }

    /// `u`: one commit ahead of an upstream.
    fn u(&self) {
        let path = self.repo_at("u", DATE);
        let remote = self.base.join("remotes/u.git");
        fs::create_dir_all(&remote).unwrap();
        self.git(&remote, DATE, &["init", "-q", "--bare"]);
        let url = remote.to_str().unwrap();
        self.git(&path, DATE, &["remote", "add", "origin", url]);
        self.git(&path, DATE, &["push", "-q", "-u", "origin", "main"]);
        self.git(&path, DATE, &["commit", "-q", "--allow-empty", "-m", "c"]);
    }

    /// `s`: a linked worktree whose branch is merged, in a repo idle for weeks.
    fn s(&self) {
        self.with_linked("s", "2026-08-01T10:00:00Z");
    }

    /// Attention 4 through a `Worktree.error` and nothing dirty: an executable tracked file
    /// replaced by a FIFO (`git error: unsafe tracked file`).
    fn corrupt(&self, name: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = self.repo_at(name, DATE);
        self.git(&path, DATE, &["config", "core.autocrlf", "true"]);
        fs::write(path.join("run.sh"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(path.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        self.git(&path, DATE, &["add", "run.sh"]);
        self.git(&path, DATE, &["commit", "-q", "-m", "exec"]);
        fs::remove_file(path.join("run.sh")).unwrap();
        assert!(
            Command::new("mkfifo")
                .arg(path.join("run.sh"))
                .status()
                .unwrap()
                .success()
        );
    }

    fn fixture() -> Sb {
        let sb = Sb::new();
        sb.m();
        sb.u();
        sb.s();
        sb.repo_at("c", DATE);
        sb
    }
}

/// `(name, worktree lines)` per repo of a plain `status`, skipping the risk line and hints.
fn repos(plain: &str) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for line in plain.lines() {
        if line.starts_with("at risk:") || line.starts_with("hint:") {
            continue;
        }
        if line.starts_with("  ") {
            out.last_mut().expect("worktree line after a repo").1 += 1;
        } else {
            let name = line.split_whitespace().next().unwrap_or_default();
            out.push((name.to_owned(), 0));
        }
    }
    out
}

fn names(plain: &str) -> Vec<String> {
    repos(plain).into_iter().map(|(n, _)| n).collect()
}

fn risk(plain: &str) -> Option<&str> {
    plain.lines().find(|l| l.starts_with("at risk: "))
}

fn hints(plain: &str) -> Vec<&str> {
    plain.lines().filter(|l| l.starts_with("hint: ")).collect()
}

impl Sb {
    /// `name` with an untracked file: attention 2.
    fn untracked(&self, name: &str) {
        let path = self.repo_at(name, DATE);
        fs::write(path.join("new"), "x").unwrap();
    }

    /// `name` with a linked worktree registered and then deleted (`missing`).
    fn missing(&self, name: &str) {
        let (_, linked) = self.with_linked(name, DATE);
        fs::remove_dir_all(linked).unwrap();
    }
}

/// §3.1: a cached answer takes its code from the stored scan; the filter never changes it.
#[test]
fn breaker_cached_answer_under_dirty_keeps_exit_4() {
    let sb = Sb::fixture();
    sb.corrupt("bad");
    assert_eq!(sb.status(&[]).code, Some(4), "control: fresh scan");
    let all = sb.status(&["--no-scan"]);
    assert_eq!(all.code, Some(4), "control: {}", all.stderr);
    assert!(
        names(&all.stdout).contains(&"bad".to_owned()),
        "{}",
        all.stdout
    );
    let run = sb.status(&["--no-scan", "--dirty"]);
    assert_eq!(run.code, Some(4), "{}", run.stderr);
    assert_eq!(names(&run.stdout), ["u", "m"], "{}", run.stdout);
}

/// §8.2: the risk line describes only the repos the filters keep, `--top` included.
#[test]
fn breaker_top_trims_the_risk_line_and_its_more_tail() {
    let sb = Sb::new();
    for n in ["a", "b", "c", "d", "e", "f", "g"] {
        sb.untracked(n);
    }
    let all = sb.status(&[]);
    assert_eq!(all.code, Some(0), "{}", all.stderr);
    let line = risk(&all.stdout).expect("control: risk line");
    assert!(line.ends_with("; +2 more"), "control: {line}");
    let three = sb.status(&["--top", "3"]);
    let line = risk(&three.stdout).expect("risk line");
    assert_eq!(line.matches("; ").count(), 2, "{line}");
    assert!(!line.contains("more"), "{line}");
    let six = sb.status(&["--top", "6"]);
    let line = risk(&six.stdout).expect("risk line");
    assert!(line.ends_with("; +1 more"), "{line}");
    // `--stale` keeps nothing risky here: no line at all.
    assert_eq!(risk(&sb.status(&["--stale"]).stdout), None);
}

/// §8.2: the prune hint describes only kept repos; a `missing` worktree has a `stale_reason`.
#[test]
fn breaker_prune_hint_follows_the_filters() {
    let sb = Sb::new();
    sb.missing("p");
    sb.untracked("d");
    let all = sb.status(&[]);
    assert_eq!(hints(&all.stdout).len(), 1, "control: {}", all.stdout);
    let dirty = sb.status(&["--dirty"]);
    assert_eq!(names(&dirty.stdout), ["d"], "{}", dirty.stdout);
    assert!(hints(&dirty.stdout).is_empty(), "{}", dirty.stdout);
    let stale = sb.status(&["--stale"]);
    assert_eq!(names(&stale.stdout), ["p"], "{}", stale.stdout);
    assert_eq!(hints(&stale.stdout).len(), 1, "{}", stale.stdout);
}

/// §3.3 `stale_reason` `inactive`: an unmerged linked worktree idle past `stale_days`.
#[test]
fn breaker_stale_keeps_an_inactive_worktree() {
    let sb = Sb::new();
    let old = "2026-08-01T10:00:00Z";
    let (_, linked) = sb.with_linked("i", old);
    sb.git(&linked, old, &["commit", "-q", "--allow-empty", "-m", "f"]);
    sb.repo_at("c", DATE);
    let all = sb.status(&[]);
    assert_eq!(names(&all.stdout), ["i", "c"], "control: {}", all.stdout);
    assert!(all.stdout.contains("stale"), "control: {}", all.stdout);
    let run = sb.status(&["--stale"]);
    assert_eq!(names(&run.stdout), ["i"], "{}", run.stdout);
    assert_eq!(repos(&run.stdout)[0].1, 2, "both worktrees: {}", run.stdout);
}

/// §3.3 `unpushed` without an upstream but with a remote counts commits no remote ref reaches;
/// with no remote it is null, which does not match `--dirty`.
#[test]
fn breaker_dirty_keeps_unpushed_without_an_upstream() {
    let sb = Sb::new();
    let r = sb.repo_at("r", DATE);
    let remote = sb.base.join("remotes/r.git");
    fs::create_dir_all(&remote).unwrap();
    sb.git(&remote, DATE, &["init", "-q", "--bare"]);
    sb.git(
        &r,
        DATE,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    sb.repo_at("n", DATE);
    let all = sb.status(&[]);
    assert_eq!(names(&all.stdout).len(), 2, "control: {}", all.stdout);
    let run = sb.status(&["--dirty"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(names(&run.stdout), ["r"], "{}", run.stdout);
}

#[test]
fn breaker_flags_before_the_path_give_the_same_output() {
    let sb = Sb::fixture();
    let after = sb.status(&["--stale", "--top", "1"]);
    let root = sb.root.to_str().unwrap();
    let before = sb.rollcall(&["status", "--stale", "--top", "1", root]);
    assert_eq!(before.code, Some(0), "{}", before.stderr);
    assert_eq!(names(&before.stdout), ["s"], "{}", before.stdout);
    assert_eq!(before.stdout, after.stdout);
}

/// I5d-7 on a scan that exits 4: JSON stays byte-identical and the code stays 4.
#[test]
fn breaker_json_ignores_the_filters_on_an_exit_4_scan() {
    let sb = Sb::fixture();
    sb.corrupt("bad");
    let json = sb.status(&["--json"]);
    assert_eq!(json.code, Some(4), "{}", json.stderr);
    for flags in [&["--dirty"][..], &["--stale", "--top", "1"]] {
        let mut args = vec!["--json"];
        args.extend_from_slice(flags);
        let run = sb.status(&args);
        assert_eq!(run.code, Some(4), "{flags:?}: {}", run.stderr);
        assert_eq!(run.stdout, json.stdout, "{flags:?}");
    }
}
