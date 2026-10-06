//! Breaker (adversarial QA) tests for I1: dot-named files under `refs/` are exempt from the
//! loose-ref size cap (spec rev 16 §4.4, §10.2 `dot-ref`). Every test drives the real binary on a
//! sandbox of its own.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;

const NOW: &str = "2026-09-29T12:00:00Z";
const DEADLINE: Duration = Duration::from_secs(60);
/// Far past the 256 MiB worker memory cap: a read of one of these could not go unnoticed.
const HUGE: u64 = 3 << 30;

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

    fn status(&self) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rollcall"));
        cmd.env_clear()
            .args(["status", "--json", self.root.to_str().unwrap()])
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
            let _ = Command::new("/bin/kill").arg(id.to_string()).status();
            panic!("rollcall outlived {DEADLINE:?}")
        });
        let out = out.unwrap();
        Run {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn git(&self, dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_COMMITTER_DATE", "2026-09-20T10:00:00Z")
            .env("GIT_AUTHOR_DATE", "2026-09-20T10:00:00Z")
            .stdout(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    /// A clean repo with one empty commit; returns its `.git`.
    fn repo(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::create_dir_all(&path).unwrap();
        self.git(&path, &["init", "-q", "-b", "main"]);
        self.git(
            &path,
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
        path.join(".git")
    }
}

/// A sparse regular file of `len` bytes (parents created).
fn sparse(path: &Path, len: u64) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::File::create(path).unwrap().set_len(len).unwrap();
}

fn json(r: &Run) -> Value {
    serde_json::from_str(&r.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}\nstderr: {}", r.stdout, r.stderr))
}

/// Every `error` (repo row and its worktree rows) of the repo whose path ends with `/<name>`.
fn errors(doc: &Value, name: &str) -> Vec<String> {
    let suffix = format!("/{name}");
    let row = doc["repos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["path"].as_str().unwrap().ends_with(&suffix))
        .unwrap_or_else(|| panic!("no row for {name}: {doc}"));
    std::iter::once(&row["error"])
        .chain(
            row["worktrees"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| &w["error"]),
        )
        .filter_map(|e| e.as_str().map(str::to_owned))
        .collect()
}

// §4.4 rev 16: gix skips a dot-named loose ref "without reading it", so the exemption carries no
// size bound. 3 GiB sparse dot files in every `refs/` tree the checks walk must leave every row clean
// and exit 0 (§3.1); a read would trip the worker memory cap or the deadline.
#[test]
fn breaker_huge_dot_named_refs_everywhere_are_never_read() {
    let sb = Sb::new();
    let git = sb.repo("r");
    for rel in [
        "refs/.DS_Store",
        "refs/heads/.DS_Store",
        "refs/tags/.DS_Store",
        "refs/remotes/origin/.DS_Store",
    ] {
        sparse(&git.join(rel), HUGE);
    }
    sb.git(
        &sb.root.join("r"),
        &["worktree", "add", "-q", "-b", "wt", "../r-wt"],
    );
    sparse(&git.join("worktrees/r-wt/refs/.DS_Store"), HUGE);
    sparse(&git.join("worktrees/r-wt/refs/bisect/.DS_Store"), HUGE);

    let run = sb.status();
    let doc = json(&run);
    assert_eq!(errors(&doc, "r"), Vec::<String>::new(), "{doc}");
    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr);
}

// §4.4 rev 16 "after `git pack-refs --all`": the exemption still holds once the real refs are packed.
#[test]
fn breaker_dot_named_ref_after_pack_refs_is_clean() {
    let sb = Sb::new();
    let git = sb.repo("r");
    sb.git(&sb.root.join("r"), &["pack-refs", "--all"]);
    sparse(&git.join("refs/heads/.DS_Store"), 6144);
    let run = sb.status();
    let doc = json(&run);
    assert_eq!(errors(&doc, "r"), Vec::<String>::new(), "{doc}");
    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr);
}

// A symbolic ref aimed at a huge dot-named file: gix must reject the target name, never read it.
// Spec (§4.4 rev 16): only a lookup of the literal name fails; it must not become a memory or time stop.
#[test]
fn breaker_head_symref_to_a_huge_dot_ref_is_refused_without_reading_it() {
    let sb = Sb::new();
    let git = sb.repo("r");
    sparse(&git.join("refs/heads/.big"), HUGE);
    fs::write(git.join("HEAD"), "ref: refs/heads/.big\n").unwrap();
    let origin = sb.repo("o");
    sparse(&origin.join("refs/remotes/origin/.big"), HUGE);
    fs::write(
        origin.join("refs/remotes/origin/HEAD"),
        "ref: refs/remotes/origin/.big\n",
    )
    .unwrap();

    let doc = json(&sb.status());
    for e in errors(&doc, "r") {
        assert!(e.starts_with("git error: "), "{e}");
        assert!(
            !e.contains("memory limit") && !e.contains("timed out"),
            "{e}"
        );
    }
    assert_eq!(errors(&doc, "o"), Vec::<String>::new(), "{doc}");
}

// §10.2 (iii)-(v) assert only the `untrusted git dir` prefix; check each refusal names the entry
// and the reason the spec gives, so a refusal for an unrelated cause cannot pass.
#[test]
fn breaker_non_exempt_entries_are_refused_for_the_right_reason() {
    let sb = Sb::new();
    let junk = sb.repo("junk");
    sparse(&junk.join("refs/heads/junk"), 6144);
    let link = sb.repo("link");
    fs::write(link.join("target-file"), [b'x'; 100]).unwrap();
    std::os::unix::fs::symlink(link.join("target-file"), link.join("refs/heads/.x")).unwrap();
    let dotdir = sb.repo("dotdir");
    sparse(&dotdir.join("refs/heads/.d/junk"), 6144);
    let fifo = sb.repo("fifo");
    let ok = Command::new("/usr/bin/mkfifo")
        .arg(fifo.join("refs/heads/.p"))
        .status()
        .unwrap()
        .success();
    assert!(ok, "mkfifo");

    let run = sb.status();
    let doc = json(&run);
    let expect = |name: &str, entry: &str, why: &str| {
        let errs = errors(&doc, name);
        assert!(
            errs.iter().any(|e| e.starts_with("untrusted git dir")
                && e.contains(entry)
                && e.contains(why)),
            "{name}: {errs:?}"
        );
    };
    expect("junk", "refs/heads/junk", "larger than 4096 bytes");
    expect("link", "refs/heads/.x", "not a regular file");
    expect("dotdir", "refs/heads/.d/junk", "larger than 4096 bytes");
    expect("fifo", "refs/heads/.p", "not a regular file");
    assert_eq!(run.code, Some(4), "stderr: {}", run.stderr);
}
