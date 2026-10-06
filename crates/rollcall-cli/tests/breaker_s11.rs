//! Breaker tests for the S11 follow-ups (ACCEPTANCE.md `## S11 Follow-ups`, spec rev 16 notes of
//! 2026-10-06): lenient `--top`, repeated `status` flags, stdout's own width, locked missing
//! worktrees out of the prune hint. Every test drives the real binary on a sandbox of its own.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use expectrl::{Eof, Expect, Session};
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

    fn envs(&self) -> Vec<(&'static str, PathBuf)> {
        vec![
            ("HOME", self.home.clone()),
            ("XDG_CONFIG_HOME", self.base.join("xdg/config")),
            ("XDG_CACHE_HOME", self.base.join("xdg/cache")),
            ("CLAUDE_CONFIG_DIR", self.base.join("claude")),
            ("CODEX_HOME", self.base.join("codex")),
            ("ROLLCALL_NOW", PathBuf::from(NOW)),
            ("PATH", PathBuf::from("/usr/bin:/bin")),
        ]
    }

    fn rollcall(&self, args: &[&str]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rollcall"));
        cmd.env_clear().args(args).envs(self.envs());
        cmd.stdin(Stdio::null())
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

    fn status(&self, extra: &[&str]) -> Run {
        let mut args = vec!["status", self.root.to_str().unwrap()];
        args.extend_from_slice(extra);
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

    fn repo(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::create_dir_all(&path).unwrap();
        self.git(&path, &["init", "-q", "-b", "main"]);
        self.git(&path, &["commit", "-q", "--allow-empty", "-m", "c"]);
        path
    }

    /// `git worktree add -b <branch> <linked>`; returns the admin dir.
    fn add(&self, repo: &Path, branch: &str, linked: &Path) -> PathBuf {
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
        let name = linked.file_name().unwrap();
        repo.join(".git/worktrees").join(name)
    }

    /// A linked worktree `<base>/linked/<repo>-<branch>`, deleted: `(path, admin dir)`.
    fn missing(&self, repo: &Path, branch: &str) -> (PathBuf, PathBuf) {
        let name = format!("{}-{branch}", repo.file_name().unwrap().to_str().unwrap());
        let linked = self.base.join("linked").join(&name);
        let admin = self.add(repo, branch, &linked);
        fs::remove_dir_all(&linked).unwrap();
        (linked, admin)
    }

    fn lock(&self, repo: &Path, linked: &Path) {
        self.git(repo, &["worktree", "lock", linked.to_str().unwrap()]);
    }

    fn cache_db(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.home.join("Library/Caches/rollcall/cache.sqlite")
        } else {
            self.base.join("xdg/cache/rollcall/cache.sqlite")
        }
    }
}

fn hints(plain: &str) -> Vec<&str> {
    plain.lines().filter(|l| l.starts_with("hint:")).collect()
}

fn hint_for(n: usize, repo: &Path) -> String {
    let s = if n == 1 { "" } else { "s" };
    format!(
        "hint: {n} missing worktree{s} — run 'git worktree prune' in {}",
        repo.display()
    )
}

// ---- D. locked missing worktrees ---------------------------------------------------------------

/// §8.2 (author decision 2026-10-06): "`<N>` counts only the unlocked ones" — per repo. Repo `q`'s
/// missing worktree is not locked (`git worktree prune` in `q` removes it), so `q` gets a hint
/// even though repo `p` has a locked missing worktree registered at the same path.
#[test]
fn breaker_a_lock_in_one_repo_does_not_hide_another_repos_missing_worktree_at_the_same_path() {
    let sb = Sb::new();
    let p = sb.repo("p");
    let q = sb.repo("q");
    let shared = sb.base.join("linked/x");
    sb.add(&p, "a", &shared);
    fs::remove_dir_all(&shared).unwrap();
    sb.add(&q, "b", &shared);
    fs::remove_dir_all(&shared).unwrap();

    // Control: unlocked, both repos get a hint.
    let before = sb.status(&[]);
    assert_eq!(before.code, Some(0), "{}", before.stderr);
    assert_eq!(hints(&before.stdout).len(), 2, "{}", before.stdout);

    sb.lock(&p, &shared);
    let plain = sb.status(&[]).stdout;
    assert_eq!(
        hints(&plain),
        vec![hint_for(1, &q).as_str()],
        "q's unlocked missing worktree lost its hint:\n{plain}"
    );
}

/// §3.3/§4.4: `locked` applies to a missing worktree only. A `locked` file on a present
/// worktree changes neither the plain output (hint of the other, missing one included) nor JSON.
#[test]
fn breaker_a_lock_on_a_present_worktree_changes_nothing() {
    let sb = Sb::new();
    let p = sb.repo("p");
    let present = sb.base.join("linked/p-here");
    sb.add(&p, "here", &present);
    sb.missing(&p, "gone");
    let (plain, json) = (sb.status(&[]), sb.status(&["--json"]));
    assert_eq!(hints(&plain.stdout), vec![hint_for(1, &p).as_str()]);
    sb.lock(&p, &present);
    assert_eq!(sb.status(&[]).stdout, plain.stdout);
    assert_eq!(sb.status(&["--json"]).stdout, json.stdout);
}

/// §3.3: locked "when `.git/worktrees/<name>/locked` exists" — a directory exists too (git's
/// `prune` uses `file_exists`, which is true for a directory).
#[test]
fn breaker_a_locked_directory_counts_as_locked() {
    let sb = Sb::new();
    let p = sb.repo("p");
    let (_, admin_a) = sb.missing(&p, "a");
    sb.missing(&p, "b");
    fs::create_dir(admin_a.join("locked")).unwrap();
    let plain = sb.status(&[]).stdout;
    assert_eq!(hints(&plain), vec![hint_for(1, &p).as_str()], "{plain}");
}

/// §8.2: the hint describes the repos the filters keep, counting only unlocked ones, under
/// `--dirty` and `--top` as well as `--stale`.
#[test]
fn breaker_the_locked_count_holds_under_dirty_and_top() {
    let sb = Sb::new();
    let p = sb.repo("p");
    let (a, _) = sb.missing(&p, "a");
    sb.missing(&p, "b");
    sb.lock(&p, &a);
    fs::write(p.join("untracked"), "x").unwrap(); // `--dirty` keeps p
    sb.repo("c");
    for extra in [
        &["--dirty"][..],
        &["--top", "1"],
        &["--dirty", "--top", "1"],
    ] {
        let plain = sb.status(extra).stdout;
        assert_eq!(
            hints(&plain),
            vec![hint_for(1, &p).as_str()],
            "{extra:?}:\n{plain}"
        );
    }
}

/// S11-12: "the worker process and the cache carry it": an answer from the cache (`--no-scan`)
/// prints the same hint as the live scan.
#[test]
fn breaker_no_scan_from_the_cache_keeps_the_lock() {
    let sb = Sb::new();
    let p = sb.repo("p");
    let (a, _) = sb.missing(&p, "a");
    sb.missing(&p, "b");
    sb.lock(&p, &a);
    let live = sb.status(&[]);
    assert_eq!(hints(&live.stdout), vec![hint_for(1, &p).as_str()]);
    let cached = sb.status(&["--no-scan"]);
    assert_eq!(cached.code, live.code, "{}", cached.stderr);
    assert_eq!(cached.stdout, live.stdout);
}

/// S11-12: `#[serde(default)]` keeps older cache rows readable: a cache whose rows lack the
/// `locked` key still answers `--no-scan`, byte-identically in JSON (which never shows it).
#[test]
fn breaker_an_old_cache_without_locked_still_answers_no_scan() {
    let sb = Sb::new();
    let p = sb.repo("p");
    let (a, _) = sb.missing(&p, "a");
    sb.missing(&p, "b");
    sb.lock(&p, &a);
    assert_eq!(sb.status(&[]).code, Some(0));
    let json = sb.status(&["--no-scan", "--json"]);
    assert_eq!(json.code, Some(0), "{}", json.stderr);

    let conn = rusqlite::Connection::open(sb.cache_db()).unwrap();
    let rows: Vec<(i64, String)> = {
        let mut st = conn.prepare("SELECT rowid, repo FROM root_repos").unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let mut stripped = 0;
    for (rowid, repo) in rows {
        let old = repo
            .replace(",\"locked\":true", "")
            .replace(",\"locked\":false", "");
        stripped += repo.matches(",\"locked\":").count();
        conn.execute(
            "UPDATE root_repos SET repo = ?1 WHERE rowid = ?2",
            rusqlite::params![old, rowid],
        )
        .unwrap();
    }
    drop(conn);
    assert!(
        stripped >= 3,
        "the cache rows carried no `locked` key to strip"
    );

    let cached = sb.status(&["--no-scan", "--json"]);
    assert_eq!(cached.code, Some(0), "{}", cached.stderr);
    assert_eq!(cached.stdout, json.stdout);
}

// ---- C. lenient flags --------------------------------------------------------------------------

fn two_repos() -> Sb {
    let sb = Sb::new();
    sb.repo("a");
    sb.repo("b");
    sb
}

/// §3.1: repeated `status` flags take the last value; `--json` is a `status` flag.
#[test]
fn breaker_a_repeated_json_is_json() {
    let sb = two_repos();
    let once = sb.status(&["--json"]);
    let twice = sb.status(&["--json", "--json"]);
    assert_eq!(twice.code, Some(0), "{}", twice.stderr);
    assert_eq!(twice.stdout, once.stdout);
}

/// §8.2: filters are validated then ignored with `--json`; the last `--top` is what is
/// validated, and a huge one is "all repos".
#[test]
fn breaker_json_with_repeated_and_huge_top_is_plain_json() {
    let sb = two_repos();
    let json = sb.status(&["--json"]);
    for extra in [
        &["--top", "1", "--json", "--top", "99999999999999999999999"][..],
        &["--json", "--top", "18446744073709551616", "--top", "1"],
    ] {
        let run = sb.status(extra);
        assert_eq!(run.code, Some(0), "{extra:?}: {}", run.stderr);
        assert_eq!(run.stdout, json.stdout, "{extra:?}");
    }
}

/// S11-8 / §3.1: the last value decides, and a last `--top 0` is still a usage error.
#[test]
fn breaker_a_last_top_zero_still_exits_2() {
    let sb = two_repos();
    for extra in [
        &["--top", "2", "--top", "0"][..],
        &["--top", "2", "--top", "-1"],
    ] {
        let run = sb.status(extra);
        assert_eq!(run.code, Some(2), "{extra:?}: {}", run.stdout);
        assert!(run.stdout.is_empty(), "{extra:?}: {}", run.stdout);
    }
}

/// §8.2: N is an integer ≥ 1; `00` is zero, `0x10` is not an integer, empty is not an integer.
#[test]
fn breaker_top_zero_spellings_and_non_integers_exit_2() {
    let sb = two_repos();
    for v in ["00", "0x10", "", " 5", "5 ", "1e3", "1_000"] {
        let run = sb.status(&[&format!("--top={v}")]);
        assert_eq!(run.code, Some(2), "--top={v:?}: {}", run.stdout);
    }
    // Control: leading zeros of a positive integer are that integer.
    let one = sb.status(&["--top", "1"]);
    assert_eq!(sb.status(&["--top", "001"]).stdout, one.stdout);
    assert_ne!(one.stdout, sb.status(&[]).stdout);
}

/// `[PATH]` is one positional, not a flag: two PATHs stay a usage error (args_override_self
/// must not turn it into "last path wins").
#[test]
fn breaker_two_paths_stay_a_usage_error() {
    let sb = two_repos();
    let a = sb.root.join("a");
    let run = sb.rollcall(&["status", sb.root.to_str().unwrap(), a.to_str().unwrap()]);
    assert_eq!(run.code, Some(2), "{}", run.stdout);
}

/// §8.2: "N is an integer ≥ 1": `+1` is that integer, so it equals `--top 1` (the doc comment
/// on `parse_top`, "a sign … is an error", is stale: only a minus sign is rejected).
#[test]
fn breaker_top_with_a_plus_sign_is_that_integer() {
    let sb = two_repos();
    let one = sb.status(&["--top", "1"]);
    let run = sb.status(&["--top", "+1"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout, one.stdout);
    assert_ne!(one.stdout, sb.status(&[]).stdout);
}

/// §3.1 rev 16: "a `status` flag given more than once takes its last value instead of exiting
/// `2`". Read literally, `--top 0 --top 2` is `--top 2`; the code validates every occurrence
/// and exits 2 (likewise `--color bogus --color never`).
#[test]
#[ignore = "AMBIGUOUS: spec silent on whether an overridden earlier value is still validated (`--top 0 --top 2` exits 2)"]
fn breaker_an_overridden_invalid_top_is_not_validated() {
    let sb = two_repos();
    let last = sb.status(&["--top", "2"]);
    let run = sb.status(&["--top", "0", "--top", "2"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.stdout, last.stdout);
}

/// `args_override_self` covers the shared flags flattened into `status` too: `status --yes
/// --yes` / `--no-scan --no-scan` now exit 0 while `scan --yes --yes` and the piped
/// `rollcall <root> --no-scan --no-scan` (which "behaves as status", §3.1) still exit 2.
#[test]
#[ignore = "AMBIGUOUS: are --yes/--no-scan/--submodules `status` flags under rev 16? status accepts repeats, scan and piped `rollcall <root>` reject them"]
fn breaker_repeated_shared_flags_behave_alike_across_status_forms() {
    let sb = two_repos();
    let root = sb.root.to_str().unwrap();
    let status = sb
        .rollcall(&["status", root, "--no-scan", "--no-scan"])
        .code;
    let piped = sb.rollcall(&[root, "--no-scan", "--no-scan"]).code;
    let scan = sb.rollcall(&["scan", root, "--yes", "--yes"]).code;
    assert_eq!(status, piped, "status vs piped `rollcall <root>`");
    assert_eq!(status, scan, "status vs scan");
}

// ---- B. width ------------------------------------------------------------------------------------

fn cols(text: &str) -> usize {
    text.chars().count()
}

/// §8.2: the width is stdout's own terminal's; "if the width cannot be read, or is 0, nothing
/// is truncated". stdout is terminal A with 0 columns, the controlling terminal B has 40: no
/// line is cut, although B is narrow.
#[test]
fn breaker_a_zero_column_stdout_terminal_cuts_nothing_whatever_the_controlling_one() {
    let sb = Sb::new();
    let long = "a-repository-name-that-is-long-enough-to-pass-forty-columns";
    sb.repo(long);
    let full = sb.status(&["--color", "never"]).stdout;
    assert!(full.lines().any(|l| cols(l) > 40), "{full}");

    let mut a = Session::spawn({
        let mut c = Command::new("/bin/sh");
        c.args(["-c", "tty; exec sleep 30"]);
        c
    })
    .unwrap();
    a.set_expect_timeout(Some(DEADLINE));
    let tty = a.expect(expectrl::Regex("/dev/[a-z0-9/]+")).unwrap();
    let tty = String::from_utf8(tty.get(0).unwrap().to_vec()).unwrap();
    a.get_process_mut().set_window_size(0, 0).unwrap();
    let script = format!(
        "exec {} status {} --color never > {tty}",
        env!("CARGO_BIN_EXE_rollcall"),
        sb.root.display()
    );
    let mut b = Session::spawn({
        let mut sh = Command::new("/bin/sh");
        sh.args(["-c", &script]).env_clear().envs(sb.envs());
        sh
    })
    .unwrap();
    b.set_expect_timeout(Some(DEADLINE));
    b.get_process_mut().set_window_size(40, 24).unwrap();
    b.expect(Eof).unwrap();
    let got = a.expect(expectrl::Regex("  main[^\r\n]*\r\n")).unwrap();
    let all = String::from_utf8_lossy(got.before()).into_owned()
        + &String::from_utf8_lossy(got.get(0).unwrap());
    let shown: Vec<&str> = all
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.is_empty() && !l.starts_with("/dev/"))
        .collect();
    let want: Vec<&str> = full.lines().collect();
    assert_eq!(shown, want, "a 0-column stdout terminal cut output");
}
