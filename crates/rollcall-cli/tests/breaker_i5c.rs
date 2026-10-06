//! Breaker (adversarial QA) tests for I5c: the risk summary line of plain `status` (spec rev 16
//! §8.2). Every test drives the real binary on a sandbox of its own.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tempfile::TempDir;

const NOW: &str = "2026-09-29T12:00:00Z";
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

    fn root_str(&self) -> &str {
        self.root.to_str().unwrap()
    }

    fn plain(&self, args: &[&str]) -> String {
        let run = self.rollcall(args);
        assert!(run.stderr.is_empty(), "stderr: {}", run.stderr);
        assert_eq!(run.code, Some(0), "{}", run.stdout);
        run.stdout
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

    /// A clean repo `name` with one commit at `date`, no remote: attention 0.
    fn repo_at(&self, name: &str, date: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::create_dir_all(&path).unwrap();
        self.git(&path, date, &["init", "-q", "-b", "main"]);
        self.git(&path, date, &["commit", "-q", "--allow-empty", "-m", "c"]);
        path
    }

    /// Attention 2: one untracked file, no remote.
    fn dirty(&self, name: &str, date: &str) {
        let path = self.repo_at(name, date);
        fs::write(path.join("u"), "x").unwrap();
    }

    /// Attention 3: `n` commits ahead of an upstream.
    fn ahead(&self, name: &str, date: &str, n: usize) {
        let path = self.repo_at(name, date);
        let remote = self.base.join(format!("remotes/{name}.git"));
        fs::create_dir_all(&remote).unwrap();
        self.git(&remote, date, &["init", "-q", "--bare"]);
        self.git(
            &path,
            date,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        self.git(&path, date, &["push", "-q", "-u", "origin", "main"]);
        for _ in 0..n {
            self.git(&path, date, &["commit", "-q", "--allow-empty", "-m", "c"]);
        }
    }
}

fn first_line(plain: &str) -> &str {
    plain.lines().next().unwrap_or("")
}

fn risk_lines(plain: &str) -> Vec<&str> {
    plain
        .lines()
        .filter(|l| l.starts_with("at risk:"))
        .collect()
}

/// Boundary: exactly five risky repos fill the line with no `+0 more` tail.
#[test]
fn breaker_exactly_five_risky_repos_have_no_more_tail() {
    let sb = Sb::new();
    for (i, name) in ["a", "b", "c", "d", "e"].iter().enumerate() {
        sb.dirty(name, &format!("2026-09-{:02}T10:00:00Z", 27 - i));
    }
    let plain = sb.plain(&["status", sb.root_str()]);
    assert_eq!(
        first_line(&plain),
        "at risk: a ●1; b ●1; c ●1; d ●1; e ●1",
        "{plain}"
    );
    assert!(!plain.contains("more"), "{plain}");
}

/// Boundary: six risky repos give `; +1 more`.
#[test]
fn breaker_six_risky_repos_count_one_more() {
    let sb = Sb::new();
    for (i, name) in ["a", "b", "c", "d", "e", "f"].iter().enumerate() {
        sb.dirty(name, &format!("2026-09-{:02}T10:00:00Z", 27 - i));
    }
    let plain = sb.plain(&["status", sb.root_str()]);
    assert_eq!(
        first_line(&plain),
        "at risk: a ●1; b ●1; c ●1; d ●1; e ●1; +1 more",
        "{plain}"
    );
}

/// §3.3 order is attention first: older unpushed (3) repos precede newer dirty (2) ones, and
/// attention-0 repos interleaved by date never appear or break the count.
#[test]
fn breaker_risk_line_orders_attention_three_before_two_then_by_activity() {
    let sb = Sb::new();
    sb.dirty("new2", "2026-09-27T10:00:00Z");
    sb.repo_at("quiet", "2026-09-26T10:00:00Z");
    sb.dirty("mid2", "2026-09-25T10:00:00Z");
    sb.ahead("old3", "2026-09-10T10:00:00Z", 1);
    sb.ahead("older3", "2026-09-05T10:00:00Z", 3);
    sb.dirty("x2", "2026-09-24T10:00:00Z");
    sb.dirty("y2", "2026-09-23T10:00:00Z");
    let plain = sb.plain(&["status", sb.root_str()]);
    assert_eq!(
        first_line(&plain),
        "at risk: old3 ↑1; older3 ↑3; new2 ●1; mid2 ●1; x2 ●1; +1 more",
        "{plain}"
    );
    assert_eq!(risk_lines(&plain).len(), 1, "{plain}");
}

/// Wiring: `rollcall <root>` with stdout a pipe behaves as `status` (§3.1), risk line included.
#[test]
fn breaker_bare_rollcall_piped_prints_the_same_risk_line() {
    let sb = Sb::new();
    sb.ahead("r1", "2026-09-20T10:00:00Z", 2);
    sb.dirty("r2", "2026-09-20T10:00:00Z");
    sb.repo_at("r4", "2026-09-20T10:00:00Z");
    let status = sb.plain(&["status", sb.root_str()]);
    let bare = sb.plain(&[sb.root_str()]);
    assert_eq!(first_line(&bare), "at risk: r1 ↑2; r2 ●1", "{bare}");
    assert_eq!(bare, status);
}

/// Wiring: an answer from the cache (`--no-scan`) renders the same risk line as the scan.
#[test]
fn breaker_cache_answer_prints_the_same_risk_line() {
    let sb = Sb::new();
    sb.ahead("r1", "2026-09-20T10:00:00Z", 2);
    sb.dirty("r2", "2026-09-20T10:00:00Z");
    let scanned = sb.plain(&["status", sb.root_str()]);
    let cached = sb.plain(&["status", "--no-scan", sb.root_str()]);
    assert_eq!(first_line(&cached), "at risk: r1 ↑2; r2 ●1", "{cached}");
    assert_eq!(cached, scanned);
}

/// No scan has ever completed and none may run: `repos: []`, so no risk line (exit 1, §3.1).
#[test]
fn breaker_no_scan_without_cache_prints_no_risk_line() {
    let sb = Sb::new();
    sb.dirty("r2", "2026-09-20T10:00:00Z");
    let run = sb.rollcall(&["status", "--no-scan", sb.root_str()]);
    assert_eq!(run.code, Some(1), "{} {}", run.stdout, run.stderr);
    assert!(risk_lines(&run.stdout).is_empty(), "{}", run.stdout);
}

/// The spec's example entry carries a multi-badge cell (`CRM ●264 no-upstream`): a repo with a
/// remote but no upstream (attention 3 via §3.3's no-upstream `unpushed`) names its whole cell.
#[test]
fn breaker_entry_carries_the_whole_badge_cell_with_no_upstream() {
    let sb = Sb::new();
    let date = "2026-09-20T10:00:00Z";
    let path = sb.repo_at("nu", date);
    let remote = sb.base.join("remotes/nu.git");
    fs::create_dir_all(&remote).unwrap();
    sb.git(&remote, date, &["init", "-q", "--bare"]);
    sb.git(
        &path,
        date,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let plain = sb.plain(&["status", sb.root_str()]);
    // Control: the repo row itself shows the two-badge cell.
    assert!(
        plain
            .lines()
            .any(|l| l.starts_with("nu ") && l.contains("↑1 no-upstream")),
        "{plain}"
    );
    assert_eq!(first_line(&plain), "at risk: nu ↑1 no-upstream", "{plain}");
}
