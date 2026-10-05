//! Acceptance tests for I5e: the `git worktree prune` hint of plain `status` (spec rev 16 §8.2).
//! Every test drives the real binary on a sandbox of its own. I5e-6 (`--dirty`) waits for I5d's
//! flag: it is pinned at the unit level in `output.rs`, which renders the filtered doc.

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

    /// Plain `status` of the root; stdout is a pipe.
    fn plain(&self) -> String {
        let run = self.rollcall(&["status", self.root.to_str().unwrap()]);
        assert!(run.stderr.is_empty(), "stderr: {}", run.stderr);
        run.stdout
    }

    fn git(&self, dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_COMMITTER_DATE", DATE)
            .env("GIT_AUTHOR_DATE", DATE)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    /// A clean repo `name` under the root with one commit and no remote.
    fn repo(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::create_dir_all(&path).unwrap();
        self.git(&path, &["init", "-q", "-b", "main"]);
        self.git(&path, &["commit", "-q", "--allow-empty", "-m", "c"]);
        path
    }

    /// A linked worktree `<repo>-<branch>` outside the root, then deleted from disk: its admin
    /// dir `.git/worktrees/<repo>-<branch>` stays, so the worktree is `missing`.
    fn missing(&self, repo: &Path, branch: &str) -> PathBuf {
        let name = format!("{}-{branch}", repo.file_name().unwrap().to_str().unwrap());
        let linked = self.base.join("linked").join(&name);
        fs::create_dir_all(linked.parent().unwrap()).unwrap();
        self.git(
            repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                linked.to_str().unwrap(),
            ],
        );
        fs::remove_dir_all(&linked).unwrap();
        repo.join(".git/worktrees").join(name)
    }
}

fn hints(plain: &str) -> Vec<&str> {
    plain.lines().filter(|l| l.starts_with("hint:")).collect()
}

#[test]
fn i5e_1_two_missing_worktrees_end_the_output_with_the_plural_hint() {
    let sb = Sb::new();
    let repo = sb.repo("r");
    sb.missing(&repo, "a");
    sb.missing(&repo, "b");
    let plain = sb.plain();
    // Control: the fixture really holds two missing worktrees.
    assert_eq!(
        plain.lines().filter(|l| l.ends_with(" missing")).count(),
        2,
        "{plain}"
    );
    let want = format!(
        "hint: 2 missing worktrees — run 'git worktree prune' in {}",
        repo.display()
    );
    assert_eq!(plain.lines().last(), Some(want.as_str()), "{plain}");
}

#[test]
fn i5e_2_one_missing_worktree_reads_singular() {
    let sb = Sb::new();
    let repo = sb.repo("r");
    sb.missing(&repo, "a");
    let plain = sb.plain();
    let want = format!(
        "hint: 1 missing worktree — run 'git worktree prune' in {}",
        repo.display()
    );
    assert_eq!(plain.lines().last(), Some(want.as_str()), "{plain}");
}

#[test]
fn i5e_3_each_repo_gets_a_line_after_all_rows_in_output_order() {
    let sb = Sb::new();
    let (p, q) = (sb.repo("p"), sb.repo("q"));
    let clean = sb.repo("clean");
    sb.missing(&p, "a");
    sb.missing(&q, "a");
    sb.missing(&q, "b");
    let plain = sb.plain();
    let lines: Vec<&str> = plain.lines().collect();
    let hint = |n: usize, repo: &Path| {
        let s = if n == 1 { "" } else { "s" };
        format!(
            "hint: {n} missing worktree{s} — run 'git worktree prune' in {}",
            repo.display()
        )
    };
    // The §3.3 order of p and q is whatever the scan chose; the hints must follow it.
    let first_repo_row = |name: &str| lines.iter().position(|l| l.starts_with(name)).unwrap();
    let (pi, qi) = (first_repo_row("p "), first_repo_row("q "));
    let (first, second) = if pi < qi {
        (hint(1, &p), hint(2, &q))
    } else {
        (hint(2, &q), hint(1, &p))
    };
    let n = lines.len();
    assert_eq!(
        &lines[n - 2..],
        [first.as_str(), second.as_str()],
        "{plain}"
    );
    assert_eq!(hints(&plain).len(), 2, "{plain}");
    // Every repo and worktree row comes before the first hint; the clean repo has none.
    assert!(lines[..n - 2].iter().all(|l| !l.starts_with("hint:")));
    assert!(
        plain.contains(clean.file_name().unwrap().to_str().unwrap()),
        "{plain}"
    );
    assert!(
        !plain.contains(&format!("in {}", clean.display())),
        "{plain}"
    );
}

#[test]
fn i5e_4_no_missing_worktree_no_hint() {
    let sb = Sb::new();
    let repo = sb.repo("r");
    // A live linked worktree is not missing.
    let linked = sb.base.join("linked/r-live");
    fs::create_dir_all(linked.parent().unwrap()).unwrap();
    sb.git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "live",
            linked.to_str().unwrap(),
        ],
    );
    let plain = sb.plain();
    assert!(plain.contains("live"), "control: {plain}");
    assert!(!plain.contains("missing"), "{plain}");
    assert!(hints(&plain).is_empty(), "{plain}");
}

#[test]
fn i5e_5_rollcall_never_prunes_and_json_carries_no_hint() {
    let sb = Sb::new();
    let repo = sb.repo("r");
    let admin = [sb.missing(&repo, "a"), sb.missing(&repo, "b")];
    let plain = sb.plain();
    assert_eq!(hints(&plain).len(), 1, "control: {plain}");
    let run = sb.rollcall(&["status", "--json", sb.root.to_str().unwrap()]);
    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr);
    assert!(!run.stdout.contains("hint"), "{}", run.stdout);
    assert!(!run.stdout.contains("prune"), "{}", run.stdout);
    assert!(
        run.stdout.contains("\"missing\""),
        "control: {}",
        run.stdout
    );
    for dir in admin {
        assert!(dir.is_dir(), "{} was pruned", dir.display());
    }
}
