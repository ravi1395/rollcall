//! Shared `#[cfg(test)]` helpers: the real `git` (absolute path, fixed dates, sandboxed
//! config) and small builders for repos, worktrees and submodules.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::Duration;

const FIXED_DATE: &str = "2026-01-01T00:00:00Z";
const HOME_SANDBOX: &str = "/nonexistent-rollcall-test-home";

/// Deadline for anything a test runs under [`within`].
pub const WATCHDOG: Duration = Duration::from_secs(20);

fn git_bin() -> &'static Path {
    static GIT: OnceLock<PathBuf> = OnceLock::new();
    GIT.get_or_init(|| {
        let path = std::env::var_os("PATH").expect("PATH is set");
        std::env::split_paths(&path)
            .map(|dir| dir.join("git"))
            .find(|candidate| candidate.is_file())
            .expect("a `git` binary on PATH")
    })
}

/// Runs the real git in `dir`, panicking on failure; returns stdout.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(git_bin())
        .current_dir(dir)
        .args(["-c", "init.defaultBranch=main"])
        .args(["-c", "protocol.file.allow=always"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", HOME_SANDBOX)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_AUTHOR_DATE", FIXED_DATE)
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_DATE", FIXED_DATE)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A temp dir plus its realpath (macOS tempdirs sit behind a `/var` symlink).
pub fn tempdir_real() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let real = dir.path().canonicalize().expect("canonicalize tempdir");
    (dir, real)
}

/// `git init` (creating `dir`) and one commit, so worktrees and submodules can be added.
pub fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("create repo dir");
    git(dir, &["init", "-q"]);
    commit_file(dir, "README", "hello\n", "initial");
}

pub fn commit_file(dir: &Path, name: &str, content: &str, message: &str) {
    std::fs::write(dir.join(name), content).expect("write file");
    git(dir, &["add", name]);
    git(dir, &["commit", "-q", "-m", message]);
}

pub fn init_bare(dir: &Path) {
    std::fs::create_dir_all(dir).expect("create bare dir");
    git(dir, &["init", "-q", "--bare"]);
}

pub fn add_worktree(main: &Path, worktree: &Path, branch: &str) {
    let path = worktree.to_str().expect("utf-8 path");
    git(main, &["worktree", "add", "-q", "-b", branch, path]);
}

/// Adds `source` (an existing repo) as a modern submodule of `superproject` at `rel`.
pub fn add_submodule(superproject: &Path, source: &Path, rel: &str) {
    let url = source.to_str().expect("utf-8 path");
    git(superproject, &["submodule", "add", "-q", url, rel]);
}

pub fn mkfifo(path: &Path) {
    let status = Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("spawn mkfifo");
    assert!(status.success(), "mkfifo {} failed", path.display());
}

/// Runs `f` on a thread and panics if it has not finished within [`WATCHDOG`].
pub fn within<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(WATCHDOG)
        .expect("test body exceeded its watchdog")
}
