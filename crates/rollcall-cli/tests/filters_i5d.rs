//! Acceptance tests for I5d: the `--dirty`, `--stale` and `--top N` filters of plain `status`
//! (spec rev 16 §8.2, §3.1). Every test drives the real binary on a sandbox of its own.
//!
//! Fixture (§3.3 order): `u` (`↑1`, attention 3), `m` (clean main plus a dirty linked worktree,
//! 2), `s` (a merged, stale linked worktree, 1), `c` (clean, 0).

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

#[test]
fn control_the_unfiltered_fixture_has_the_expected_order() {
    let sb = Sb::fixture();
    let run = sb.status(&[]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(
        repos(&run.stdout),
        [("u", 1), ("m", 2), ("s", 2), ("c", 1)].map(|(n, w)| (n.to_owned(), w)),
        "{}",
        run.stdout
    );
}

#[test]
fn i5d_1_dirty_keeps_m_and_u_and_all_of_m_s_worktrees() {
    let sb = Sb::fixture();
    let run = sb.status(&["--dirty"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(
        repos(&run.stdout),
        [("u", 1), ("m", 2)].map(|(n, w)| (n.to_owned(), w)),
        "{}",
        run.stdout
    );
}

#[test]
fn i5d_2_stale_keeps_s_only() {
    let sb = Sb::fixture();
    let run = sb.status(&["--stale"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(names(&run.stdout), ["s"], "{}", run.stdout);
}

#[test]
fn i5d_3_dirty_and_stale_combine_as_or() {
    let sb = Sb::fixture();
    let run = sb.status(&["--dirty", "--stale"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(names(&run.stdout), ["u", "m", "s"], "{}", run.stdout);
}

#[test]
fn i5d_4_top_keeps_the_first_n_of_the_sort_after_the_filters() {
    let sb = Sb::fixture();
    let all = names(&sb.status(&[]).stdout);
    let run = sb.status(&["--top", "2"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(names(&run.stdout), all[..2], "{}", run.stdout);
    // `s` is third overall: the filter runs first, so it is the first of what is left.
    let run = sb.status(&["--stale", "--top", "1"]);
    assert_eq!(names(&run.stdout), ["s"], "{}", run.stdout);
    // More than there are repos keeps them all.
    assert_eq!(names(&sb.status(&["--top", "9"]).stdout), all);
}

#[test]
fn i5d_5_a_top_that_is_not_a_positive_integer_is_a_usage_error() {
    let sb = Sb::fixture();
    // Control: a valid `--top` is accepted, so the exit 2 below is about the value.
    assert_eq!(sb.status(&["--top", "1"]).code, Some(0));
    for bad in ["0", "x", "-1"] {
        let run = sb.status(&["--top", bad]);
        assert_eq!(run.code, Some(2), "--top {bad}: {}", run.stdout);
        assert!(run.stdout.is_empty(), "--top {bad}: {}", run.stdout);
        assert!(!run.stderr.is_empty(), "--top {bad}");
    }
    // Validated even though `--json` ignores it.
    assert_eq!(sb.status(&["--top", "0", "--json"]).code, Some(2));
}

#[test]
fn i5d_6_the_exit_code_reflects_the_whole_scan_not_the_rows_shown() {
    let sb = Sb::fixture();
    sb.corrupt("bad");
    let all = sb.status(&[]);
    assert_eq!(all.code, Some(4), "{}", all.stderr);
    assert!(names(&all.stdout).contains(&"bad".to_owned()), "control");
    for flags in [&["--dirty"][..], &["--stale"], &["--top", "1"]] {
        let run = sb.status(flags);
        assert_eq!(run.code, Some(4), "{flags:?}: {}", run.stderr);
    }
    let run = sb.status(&["--dirty"]);
    assert!(!names(&run.stdout).contains(&"bad".to_owned()), "hidden");
}

#[test]
fn i5d_7_json_ignores_the_filters() {
    let sb = Sb::fixture();
    let plain = sb.status(&["--json"]);
    assert_eq!(plain.code, Some(0), "{}", plain.stderr);
    for flags in [
        &["--dirty"][..],
        &["--stale"],
        &["--dirty", "--stale"],
        &["--top", "1"],
    ] {
        let mut args = vec!["--json"];
        args.extend_from_slice(flags);
        let run = sb.status(&args);
        assert_eq!(run.code, Some(0), "{flags:?}: {}", run.stderr);
        assert_eq!(run.stdout, plain.stdout, "{flags:?}");
    }
}

#[test]
fn i5d_8_the_tui_form_rejects_the_filters() {
    let sb = Sb::fixture();
    let root = sb.root.to_str().unwrap();
    for flag in [&["--dirty"][..], &["--stale"], &["--top", "1"]] {
        let mut args = vec![root];
        args.extend_from_slice(flag);
        let run = sb.rollcall(&args);
        assert_eq!(run.code, Some(2), "{flag:?}: {}", run.stdout);
        assert!(run.stdout.is_empty(), "{flag:?}: {}", run.stdout);
    }
}
