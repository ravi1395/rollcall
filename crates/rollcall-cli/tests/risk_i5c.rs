//! Acceptance tests for I5c: the risk summary line of plain `status` (spec rev 16 §8.2). Every
//! test drives the real binary on a sandbox of its own. I5c-6 (`--top`) waits for I5d's flag: it
//! is pinned at the unit level in `output.rs`, which renders the same fixture's first N repos.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;

const NOW: &str = "2026-09-29T12:00:00Z";
const DEADLINE: Duration = Duration::from_secs(60);

/// The plain `status` lines of the I5c-1 fixture as they were before revision 16, written by
/// hand from §8.2 (`{r3}` is r3's path, shown for its branchless main worktree): the name column
/// stops at 2 + 6 (`  feat`), the badge column at 8 + 37 (the error badge) + 2, and every commit
/// is 9 days before `NOW`. The over-cap path shifts only its own line.
const BEFORE_REV_16: &str = "\
r3      ●1                                     9d
  {r3}  error: git error: unsafe tracked file
  feat  ext ●1
r1      ↑2                                     9d
  main  ↑2
r2      ●1                                     9d
  main  ●1
r4                                             quiet 9d
  main  ✓ clean
";

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

    fn repo(&self, name: &str) -> PathBuf {
        self.repo_at(name, "2026-09-20T10:00:00Z")
    }

    /// Attention 2: one untracked file, no remote.
    fn dirty(&self, name: &str, date: &str) {
        let path = self.repo_at(name, date);
        fs::write(path.join("u"), "x").unwrap();
    }

    /// Attention 3: two commits ahead of an upstream.
    fn ahead(&self, name: &str) {
        let path = self.repo(name);
        let remote = self.base.join(format!("remotes/{name}.git"));
        fs::create_dir_all(&remote).unwrap();
        let date = "2026-09-20T10:00:00Z";
        self.git(&remote, date, &["init", "-q", "--bare"]);
        self.git(
            &path,
            date,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        self.git(&path, date, &["push", "-q", "-u", "origin", "main"]);
        for _ in 0..2 {
            self.git(&path, date, &["commit", "-q", "--allow-empty", "-m", "c"]);
        }
    }

    /// Attention 4 through a `Worktree.error`: an executable tracked file replaced by a FIFO
    /// (`git error: unsafe tracked file`), plus a dirty linked worktree.
    fn worktree_error(&self, name: &str) {
        let path = self.root.join(name);
        fs::create_dir_all(&path).unwrap();
        let date = "2026-09-20T10:00:00Z";
        self.git(&path, date, &["init", "-q", "-b", "main"]);
        self.git(&path, date, &["config", "core.autocrlf", "true"]);
        fs::write(path.join("run.sh"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(path.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        self.git(&path, date, &["add", "run.sh"]);
        self.git(&path, date, &["commit", "-q", "-m", "exec"]);
        fs::remove_file(path.join("run.sh")).unwrap();
        let fifo = Command::new("mkfifo").arg(path.join("run.sh")).status();
        assert!(fifo.unwrap().success());
        let linked = self.base.join(format!("linked/{name}-feat"));
        fs::create_dir_all(linked.parent().unwrap()).unwrap();
        self.git(
            &path,
            date,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat",
                linked.to_str().unwrap(),
            ],
        );
        // One commit of its own keeps `feat` from counting as merged (and so stale).
        self.git(&linked, date, &["commit", "-q", "--allow-empty", "-m", "f"]);
        fs::write(linked.join("u"), "x").unwrap();
    }

    /// The I5c-1 fixture: r1 `↑2`, r2 `●1`, r3 error, r4 clean; §3.3 order r3, r1, r2, r4.
    fn fixture() -> Sb {
        let sb = Sb::new();
        sb.ahead("r1");
        sb.dirty("r2", "2026-09-20T10:00:00Z");
        sb.worktree_error("r3");
        sb.repo("r4");
        sb
    }
}

fn risk_lines(plain: &str) -> Vec<&str> {
    plain
        .lines()
        .filter(|l| l.starts_with("at risk:"))
        .collect()
}

#[test]
fn i5c_1_the_first_line_lists_the_dirty_and_unpushed_repos_but_not_the_error_one() {
    let sb = Sb::fixture();
    let plain = sb.plain();
    assert_eq!(
        plain.lines().next(),
        Some("at risk: r1 ↑2; r2 ●1"),
        "{plain}"
    );
    assert_eq!(risk_lines(&plain).len(), 1, "{plain}");
}

#[test]
fn i5c_2_no_line_when_every_repo_has_attention_0_1_or_4() {
    let sb = Sb::new();
    sb.repo("clean");
    sb.worktree_error("broken");
    // Attention 1: a merged linked worktree of a repo whose last commit is long past.
    let stale = sb.repo_at("old", "2026-08-01T10:00:00Z");
    let linked = sb.base.join("linked/old-feat");
    fs::create_dir_all(linked.parent().unwrap()).unwrap();
    sb.git(
        &stale,
        "2026-08-01T10:00:00Z",
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat",
            linked.to_str().unwrap(),
        ],
    );
    let plain = sb.plain();
    // Control: the fixture really holds attention 1 and 4 rows, so the check could fail.
    assert!(plain.contains("merged·stale"), "{plain}");
    assert!(plain.contains("error: "), "{plain}");
    assert!(risk_lines(&plain).is_empty(), "{plain}");
}

#[test]
fn i5c_3_more_than_five_repos_name_the_first_five_in_sort_order_then_count_the_rest() {
    let sb = Sb::new();
    // Newest commit first: the §3.3 order is d4, d2, d7, d1, d6, d3, d5 (not path order).
    let order = ["d4", "d2", "d7", "d1", "d6", "d3", "d5"];
    for (i, name) in order.iter().enumerate() {
        sb.dirty(name, &format!("2026-09-{:02}T10:00:00Z", 27 - i));
    }
    let plain = sb.plain();
    assert_eq!(
        plain.lines().next(),
        Some("at risk: d4 ●1; d2 ●1; d7 ●1; d1 ●1; d6 ●1; +2 more"),
        "{plain}"
    );
}

#[test]
fn i5c_4_the_rest_of_the_output_is_what_status_printed_before_the_line() {
    let sb = Sb::fixture();
    let plain = sb.plain();
    let (first, rest) = plain.split_once('\n').unwrap();
    assert!(first.starts_with("at risk:"), "{plain}");
    let want = BEFORE_REV_16.replace("{r3}", sb.root.join("r3").to_str().unwrap());
    assert_eq!(rest, want);
}

#[test]
fn i5c_5_json_validates_and_has_no_risk_text() {
    let sb = Sb::fixture();
    let run = sb.rollcall(&["status", "--json", sb.root.to_str().unwrap()]);
    assert_eq!(run.code, Some(4), "stderr: {}", run.stderr);
    assert!(!run.stdout.contains("at risk"), "{}", run.stdout);
    let doc: Value = serde_json::from_str(&run.stdout).unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema");
    let load = |name: &str| -> Value {
        serde_json::from_str(&fs::read_to_string(dir.join(format!("{name}.json"))).unwrap())
            .unwrap()
    };
    let registry = ["status", "doctor"]
        .iter()
        .fold(jsonschema::Registry::new(), |b, name| {
            b.add(format!("rollcall:{name}.json"), load(name)).unwrap()
        })
        .prepare()
        .unwrap();
    let validator = jsonschema::options()
        .with_registry(&registry)
        .build(&load("status"))
        .unwrap();
    let errors: Vec<String> = validator.iter_errors(&doc).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "{errors:?}");
}
