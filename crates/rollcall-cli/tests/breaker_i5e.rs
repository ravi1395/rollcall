//! Breaker (adversarial QA) tests for I5e: the `git worktree prune` hint of plain `status`
//! (spec rev 16 §8.2). Every test drives the real binary on a sandbox of its own.

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
        let home = base.join("home");
        let root = home.join("work");
        fs::create_dir_all(&root).unwrap();
        Sb {
            _dir: dir,
            base,
            home,
            root,
        }
    }

    fn rollcall(&self, args: &[&str]) -> Run {
        let child = Command::new(env!("CARGO_BIN_EXE_rollcall"))
            .env_clear()
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
            panic!("rollcall {args:?} outlived {DEADLINE:?}")
        });
        let out = out.unwrap();
        Run {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn plain(&self, extra: &[&str]) -> Run {
        let mut args = vec!["status"];
        args.extend_from_slice(extra);
        args.push(self.root.to_str().unwrap());
        self.rollcall(&args)
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

    fn repo_at(&self, path: &Path) -> PathBuf {
        fs::create_dir_all(path).unwrap();
        self.git(path, &["init", "-q", "-b", "main"]);
        self.git(path, &["commit", "-q", "--allow-empty", "-m", "c"]);
        path.to_owned()
    }

    /// A linked worktree at `linked`, added from `repo` on new branch `branch`, then deleted.
    fn missing_at(&self, repo: &Path, branch: &str, linked: &Path) {
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
        fs::remove_dir_all(linked).unwrap();
    }

    fn missing(&self, repo: &Path, branch: &str) {
        let linked = self.base.join("linked").join(branch);
        self.missing_at(repo, branch, &linked);
    }
}

fn hint(n: usize, path: &str) -> String {
    let s = if n == 1 { "" } else { "s" };
    format!("hint: {n} missing worktree{s} — run 'git worktree prune' in {path}")
}

fn last(text: &str) -> &str {
    text.lines().last().unwrap_or("")
}

/// A newline and a tab in the repo's path become spaces (§9), so the hint stays one line and
/// is still the last line.
#[test]
fn breaker_control_chars_in_repo_path_keep_the_hint_one_line() {
    let sb = Sb::new();
    let repo = sb.repo_at(&sb.root.join("a\nb\tc"));
    sb.missing(&repo, "x");
    let run = sb.plain(&[]);
    let want = hint(1, &format!("{}/a b c", sb.root.display()));
    assert_eq!(last(&run.stdout), want, "{:?}", run.stdout);
}

/// A linked worktree that lived under the root and was deleted is still `missing`.
#[test]
fn breaker_missing_worktree_that_lived_under_the_root() {
    let sb = Sb::new();
    let repo = sb.repo_at(&sb.root.join("r"));
    sb.missing_at(&repo, "x", &sb.root.join("r-x"));
    let run = sb.plain(&[]);
    assert_eq!(
        last(&run.stdout),
        hint(1, repo.to_str().unwrap()),
        "{}",
        run.stdout
    );
}

/// The cached answer (`--no-scan`) renders through the same path, so it carries the hint too.
#[test]
fn breaker_cached_answer_keeps_the_hint() {
    let sb = Sb::new();
    let repo = sb.repo_at(&sb.root.join("r"));
    sb.missing(&repo, "x");
    sb.missing(&repo, "y");
    let first = sb.plain(&[]);
    assert_eq!(last(&first.stdout), hint(2, repo.to_str().unwrap()));
    let cached = sb.plain(&["--no-scan"]);
    assert_eq!(cached.stdout, first.stdout, "stderr: {}", cached.stderr);
}

/// `rollcall <PATH>` with stdout not a TTY behaves as `status` (§3.1), hint included.
#[test]
fn breaker_no_subcommand_off_a_tty_prints_the_hint() {
    let sb = Sb::new();
    let repo = sb.repo_at(&sb.root.join("r"));
    sb.missing(&repo, "x");
    let run = sb.rollcall(&[sb.root.to_str().unwrap()]);
    assert_eq!(
        last(&run.stdout),
        hint(1, repo.to_str().unwrap()),
        "{}",
        run.stdout
    );
}

/// The hint does not change the exit code: a clean scan with missing worktrees exits 0.
#[test]
fn breaker_hint_keeps_exit_zero_and_stays_off_stderr() {
    let sb = Sb::new();
    let repo = sb.repo_at(&sb.root.join("r"));
    sb.missing(&repo, "x");
    let run = sb.plain(&[]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(!run.stderr.contains("hint"), "{}", run.stderr);
}

/// A repo whose main worktree is outside the root (`external_main`, found through a live
/// linked worktree under the root) with a second, missing linked worktree: the hint names the
/// repo's path as JSON reports it.
#[test]
fn breaker_external_main_repo_with_a_missing_worktree() {
    let sb = Sb::new();
    let main = sb.repo_at(&sb.base.join("outside/m"));
    sb.git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "live",
            sb.root.join("m-live").to_str().unwrap(),
        ],
    );
    sb.missing(&main, "x");
    let run = sb.plain(&[]);
    let json = sb.plain(&["--json"]);
    assert!(
        json.stdout.contains("\"external_main\""),
        "control: {}",
        json.stdout
    );
    assert!(
        json.stdout.contains("\"missing\""),
        "control: {}",
        json.stdout
    );
    assert_eq!(
        last(&run.stdout),
        hint(1, main.to_str().unwrap()),
        "{}",
        run.stdout
    );
}

/// Probe: a locked missing worktree. `git worktree prune` leaves it, yet §8.2 counts every
/// `missing` worktree. Records current behaviour (AMBIGUOUS, see the report).
#[test]
fn breaker_probe_locked_missing_worktree_is_counted() {
    let sb = Sb::new();
    let repo = sb.repo_at(&sb.root.join("r"));
    let linked = sb.base.join("linked/x");
    fs::create_dir_all(linked.parent().unwrap()).unwrap();
    sb.git(
        &repo,
        &["worktree", "add", "-q", "-b", "x", linked.to_str().unwrap()],
    );
    sb.git(&repo, &["worktree", "lock", linked.to_str().unwrap()]);
    fs::remove_dir_all(&linked).unwrap();
    let run = sb.plain(&[]);
    assert_eq!(
        last(&run.stdout),
        hint(1, repo.to_str().unwrap()),
        "{}",
        run.stdout
    );
}
