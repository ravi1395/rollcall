//! Tests for the S11 follow-ups (author decisions of 2026-10-06): lenient `--top`, repeated
//! `status` flags taking the last value, and locked missing worktrees out of the prune hint.
//! Every test drives the real binary on a sandbox of its own.
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

    /// A linked worktree `<repo>-<branch>` outside the root, deleted from disk: its admin dir
    /// stays, so it is `missing`. Returns `(worktree path, admin dir)`.
    fn missing(&self, repo: &Path, branch: &str) -> (PathBuf, PathBuf) {
        let name = format!("{}-{branch}", repo.file_name().unwrap().to_str().unwrap());
        let linked = self.base.join("linked").join(&name);
        fs::create_dir_all(linked.parent().unwrap()).unwrap();
        let add = ["worktree", "add", "-q", "-b", branch];
        self.git(
            repo,
            DATE,
            &[&add[..], &[linked.to_str().unwrap()]].concat(),
        );
        fs::remove_dir_all(&linked).unwrap();
        (linked, repo.join(".git/worktrees").join(name))
    }

    /// `git worktree lock` on a (missing) worktree.
    fn lock(&self, repo: &Path, linked: &Path) {
        self.git(repo, DATE, &["worktree", "lock", linked.to_str().unwrap()]);
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

fn hints(plain: &str) -> Vec<&str> {
    plain.lines().filter(|l| l.starts_with("hint:")).collect()
}

/// S11-7: a `--top` past `usize::MAX` keeps every repo, exactly like no `--top`.
#[test]
fn s11_7_a_top_past_the_largest_count_keeps_every_repo() {
    let sb = Sb::fixture();
    let all = sb.status(&[]);
    assert_eq!(all.code, Some(0), "{}", all.stderr);
    assert_eq!(names(&all.stdout).len(), 4, "{}", all.stdout);
    for huge in ["18446744073709551616", "99999999999999999999999999"] {
        let run = sb.status(&["--top", huge]);
        assert_eq!(run.code, all.code, "--top {huge}: {}", run.stderr);
        assert_eq!(run.stdout, all.stdout, "--top {huge}");
    }
    // Control: a count that fits still cuts, so the overflow arm is what keeps everything.
    assert_eq!(names(&sb.status(&["--top", "1"]).stdout).len(), 1);
}

/// S11-9: a repeated `status` flag takes its last value and is not a usage error.
#[test]
fn s11_9_repeated_filter_flags_take_the_last_value() {
    let sb = Sb::fixture();
    let pairs: [(&[&str], &[&str]); 4] = [
        (&["--dirty", "--dirty"], &["--dirty"]),
        (&["--stale", "--stale"], &["--stale"]),
        (&["--top", "1", "--top", "2"], &["--top", "2"]),
        (&["--top", "2", "--top", "1"], &["--top", "1"]),
    ];
    for (twice, once) in pairs {
        let (a, b) = (sb.status(twice), sb.status(once));
        assert_eq!(a.code, Some(0), "{twice:?}: {}", a.stderr);
        assert_eq!(a.stdout, b.stdout, "{twice:?} vs {once:?}");
    }
    // Controls: the pairs differ from each other, so equality above is not vacuous.
    assert_ne!(
        sb.status(&["--top", "2"]).stdout,
        sb.status(&["--top", "1"]).stdout
    );
    assert_ne!(
        sb.status(&["--dirty"]).stdout,
        sb.status(&["--stale"]).stdout
    );
}

/// S11-10 (assumption): a repeated `--color` takes the last value.
#[test]
fn s11_10_a_repeated_color_takes_the_last_value() {
    let sb = Sb::fixture();
    let never = sb.status(&["--color", "always", "--color", "never"]);
    assert_eq!(never.code, Some(0), "{}", never.stderr);
    assert!(!never.stdout.contains('\x1b'), "{:?}", never.stdout);
    let always = sb.status(&["--color", "never", "--color", "always"]);
    assert_eq!(always.code, Some(0), "{}", always.stderr);
    assert!(always.stdout.contains("\x1b["), "{:?}", always.stdout);
}

/// The TUI form keeps rejecting status flags, repeated or not (I5d-8); only `status` overrides.
#[test]
fn s11_9_the_tui_form_still_rejects_a_repeated_filter() {
    let sb = Sb::fixture();
    let run = sb.rollcall(&[sb.root.to_str().unwrap(), "--dirty", "--dirty"]);
    assert_eq!(run.code, Some(2), "{}", run.stdout);
    assert!(run.stdout.is_empty(), "{}", run.stdout);
}

/// S11-14: a locked missing worktree is not counted by the prune hint.
#[test]
fn s11_14_the_prune_hint_counts_only_unlocked_missing_worktrees() {
    // Two missing, one locked: singular.
    let sb = Sb::new();
    let p = sb.repo_at("p", DATE);
    let (a, _) = sb.missing(&p, "a");
    sb.missing(&p, "b");
    sb.lock(&p, &a);
    let plain = sb.status(&[]).stdout;
    let hint = format!(
        "hint: 1 missing worktree — run 'git worktree prune' in {}",
        p.display()
    );
    assert_eq!(plain.lines().last(), Some(hint.as_str()), "{plain}");
    // Locked ones keep their worktree line and badge (S11-15).
    let badged = plain.lines().filter(|l| l.ends_with(" missing")).count();
    assert_eq!(badged, 2, "{plain}");

    // Both locked: no hint.
    let sb = Sb::new();
    let p = sb.repo_at("p", DATE);
    let (a, _) = sb.missing(&p, "a");
    let (b, admin_b) = sb.missing(&p, "b");
    sb.lock(&p, &a);
    fs::write(admin_b.join("locked"), "").unwrap(); // the same thing by hand
    let _ = b;
    let plain = sb.status(&[]).stdout;
    assert!(hints(&plain).is_empty(), "{plain}");
    assert_eq!(
        plain.lines().filter(|l| l.ends_with(" missing")).count(),
        2,
        "{plain}"
    );

    // Three missing, one locked: plural.
    let sb = Sb::new();
    let p = sb.repo_at("p", DATE);
    let (a, _) = sb.missing(&p, "a");
    sb.missing(&p, "b");
    sb.missing(&p, "c");
    sb.lock(&p, &a);
    let plain = sb.status(&[]).stdout;
    let hint = format!(
        "hint: 2 missing worktrees — run 'git worktree prune' in {}",
        p.display()
    );
    assert_eq!(plain.lines().last(), Some(hint.as_str()), "{plain}");

    // The lock survives a filter that drops other repos (keyed by path, not by index).
    let q = sb.repo_at("q", DATE);
    let (qa, _) = sb.missing(&q, "a");
    sb.lock(&q, &qa);
    let plain = sb.status(&["--stale"]).stdout;
    assert_eq!(hints(&plain).len(), 1, "{plain}");
    assert!(
        hints(&plain)[0].starts_with("hint: 2 missing worktrees"),
        "{plain}"
    );
}

/// S11-15: `--json` is byte-identical with and without the `locked` files.
#[test]
fn s11_15_json_does_not_show_the_lock() {
    let sb = Sb::new();
    let p = sb.repo_at("p", DATE);
    let (a, _) = sb.missing(&p, "a");
    let (_, admin_b) = sb.missing(&p, "b");
    let before = sb.status(&["--json"]);
    assert_eq!(before.code, Some(0), "{}", before.stderr);
    sb.lock(&p, &a);
    fs::write(admin_b.join("locked"), "why").unwrap();
    let after = sb.status(&["--json"]);
    assert_eq!(after.stdout, before.stdout);
    // Control: the lock files were seen, because the plain hint changed.
    assert!(hints(&sb.status(&[]).stdout).is_empty());
    assert!(before.stdout.contains("\"stale_reason\": \"missing\""));
}
