//! Acceptance tests for I5b: colour and terminal-width truncation in plain `status`, and
//! `--color` (spec rev 16 §3.1, §8.2). Every test drives the real binary on a sandbox of its
//! own; the PTY tests run it with a window of a fixed size.
//!
//! Colour fixture (`Sb::colors`): `u` `↑1`, `m` a dirty linked worktree, `s` a merged stale
//! linked worktree, `x` a missing worktree, `e` a worktree error, `n` with a nested repo, `b.git`
//! bare, `extwt` a worktree of a main repo outside the root, `nu` a remote but no upstream, and
//! `c` clean (attention 0, so its last activity reads `quiet …`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use expectrl::{Eof, Expect, Session};
use serde_json::json;
use tempfile::TempDir;

const NOW: &str = "2026-09-29T12:00:00Z";
const DATE: &str = "2026-09-20T10:00:00Z";
const OLD: &str = "2026-08-01T10:00:00Z";
const DEADLINE: Duration = Duration::from_secs(60);
const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
const ESC: char = '\x1b';
const RESET: &str = "\x1b[0m";
const LONG_TITLE: &str =
    "refactor the scanner so that every worktree line is far wider than sixty columns";

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
        for name in ["claude/projects/p", "codex"] {
            sub(name);
        }
        Sb {
            home: sub("home"),
            root: sub("home/work"),
            _dir: dir,
            base,
        }
    }

    fn command(&self, args: &[&str], env: &[(&str, &str)]) -> Command {
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
            .env("TERM", "xterm-256color")
            .envs(env.iter().copied());
        cmd
    }

    /// `rollcall <args>` with stdout a pipe.
    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Run {
        let mut cmd = self.command(args, env);
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

    /// `rollcall status <root> <extra…>`, stdout a pipe; exit codes 0 and 4 only.
    fn status(&self, extra: &[&str], env: &[(&str, &str)]) -> String {
        let mut args = vec!["status", self.root.to_str().unwrap()];
        args.extend_from_slice(extra);
        let run = self.run(&args, env);
        assert!(
            matches!(run.code, Some(0 | 4)),
            "{:?} {}",
            run.code,
            run.stderr
        );
        assert!(run.stderr.is_empty(), "stderr: {}", run.stderr);
        run.stdout
    }

    /// `rollcall status <root> <extra…>` on a PTY of `cols` columns; returns everything it wrote.
    /// The PTY expect timeout is the watchdog.
    fn pty(&self, cols: u16, extra: &[&str], env: &[(&str, &str)]) -> String {
        let mut args = vec!["status", self.root.to_str().unwrap()];
        args.extend_from_slice(extra);
        let mut p = Session::spawn(self.command(&args, env)).expect("spawn in a PTY");
        p.set_expect_timeout(Some(DEADLINE));
        p.get_process_mut()
            .set_window_size(cols, 24)
            .expect("set the PTY size");
        let found = p.expect(Eof).expect("rollcall never exited");
        String::from_utf8(found.as_bytes().to_vec()).expect("utf-8 output")
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

    /// A clean repo `name` under `parent` with one commit made at `date`, no remote.
    fn repo_in(&self, parent: &Path, name: &str, date: &str) -> PathBuf {
        let path = parent.join(name);
        fs::create_dir_all(&path).unwrap();
        self.git(&path, date, &["init", "-q", "-b", "main"]);
        self.git(&path, date, &["commit", "-q", "--allow-empty", "-m", "c"]);
        path
    }

    fn repo(&self, name: &str, date: &str) -> PathBuf {
        self.repo_in(&self.root, name, date)
    }

    /// A linked worktree `<repo>-feat` outside the root.
    fn linked(&self, repo: &Path, date: &str) -> PathBuf {
        let name = format!("{}-feat", repo.file_name().unwrap().to_str().unwrap());
        let linked = self.base.join("linked").join(name);
        fs::create_dir_all(linked.parent().unwrap()).unwrap();
        let add = ["worktree", "add", "-q", "-b", "feat"];
        self.git(
            repo,
            date,
            &[&add[..], &[linked.to_str().unwrap()]].concat(),
        );
        linked
    }

    /// A bare remote with `repo`'s `main` pushed (upstream set).
    fn remote(&self, repo: &Path, push: bool) {
        let remote = self.base.join("remotes").join(repo.file_name().unwrap());
        fs::create_dir_all(&remote).unwrap();
        self.git(&remote, DATE, &["init", "-q", "--bare"]);
        self.git(
            repo,
            DATE,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        if push {
            self.git(repo, DATE, &["push", "-q", "-u", "origin", "main"]);
        }
    }

    /// Every badge kind of §8.2's colour rule, one repo each.
    fn colors() -> Sb {
        use std::os::unix::fs::PermissionsExt;
        let sb = Sb::new();
        // u: one commit ahead of its upstream.
        let u = sb.repo("u", DATE);
        sb.remote(&u, true);
        sb.git(&u, DATE, &["commit", "-q", "--allow-empty", "-m", "c2"]);
        // m: a dirty linked worktree.
        let m = sb.repo("m", DATE);
        fs::write(sb.linked(&m, DATE).join("f"), "x").unwrap();
        // s: a merged linked worktree in a repo idle for weeks.
        let s = sb.repo("s", OLD);
        sb.linked(&s, OLD);
        // i: an unmerged linked worktree idle for weeks (`stale`, not `merged·stale`).
        let i = sb.repo("i", OLD);
        sb.git(
            &sb.linked(&i, OLD),
            OLD,
            &["commit", "-q", "--allow-empty", "-m", "f"],
        );
        // x: a linked worktree deleted from disk.
        let x = sb.repo("x", DATE);
        fs::remove_dir_all(sb.linked(&x, DATE)).unwrap();
        // e: a Worktree.error (an executable tracked file replaced by a FIFO).
        let e = sb.repo("e", DATE);
        sb.git(&e, DATE, &["config", "core.autocrlf", "true"]);
        fs::write(e.join("run.sh"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(e.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        sb.git(&e, DATE, &["add", "run.sh"]);
        sb.git(&e, DATE, &["commit", "-q", "-m", "exec"]);
        fs::remove_file(e.join("run.sh")).unwrap();
        assert!(
            Command::new("mkfifo")
                .arg(e.join("run.sh"))
                .status()
                .unwrap()
                .success()
        );
        // n: a repo with a nested repo inside it.
        let n = sb.repo("n", DATE);
        sb.repo_in(&n, "inner", DATE);
        // b.git: a bare repo.
        let b = sb.root.join("b.git");
        fs::create_dir_all(&b).unwrap();
        sb.git(&b, DATE, &["init", "-q", "--bare"]);
        // extwt: a worktree under the root of a main repo outside it.
        let main = sb.repo_in(&sb.base.join("extmain"), "main", DATE);
        let extwt = sb.root.join("extwt");
        let add = ["worktree", "add", "-q", "-b", "feat"];
        sb.git(
            &main,
            DATE,
            &[&add[..], &[extwt.to_str().unwrap()]].concat(),
        );
        // nu: a remote, no upstream for the branch.
        let nu = sb.repo("nu", DATE);
        sb.remote(&nu, false);
        // c: clean and idle: attention 0.
        sb.repo("c", DATE);
        sb
    }

    /// Repo `r` (clean) with a Claude session titled `LONG_TITLE`, and repo `d` (dirty).
    fn titled() -> Sb {
        let sb = Sb::new();
        let r = sb.repo("r", DATE);
        let d = sb.repo("d", DATE);
        fs::write(d.join("f"), "x").unwrap();
        let records = [
            json!({"type": "user", "sessionId": ID, "timestamp": "2026-09-28T10:00:00.000Z",
                "cwd": r.to_str().unwrap(), "gitBranch": "main", "isSidechain": false,
                "message": {"role": "user", "content": "p"}}),
            json!({"type": "custom-title", "sessionId": ID, "customTitle": LONG_TITLE}),
        ];
        let text: String = records.iter().map(|r| format!("{r}\n")).collect();
        fs::write(sb.base.join(format!("claude/projects/p/{ID}.jsonl")), text).unwrap();
        sb
    }
}

/// `text` without its `ESC [ … m` sequences.
fn strip_sgr(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("\x1b[") {
        out.push_str(&rest[..i]);
        let end = rest[i..].find('m').expect("an SGR sequence ends in m");
        rest = &rest[i + end + 1..];
    }
    out + rest
}

/// What a PTY printed, as lines: `\r` and SGR removed.
fn pty_lines(raw: &str) -> Vec<String> {
    strip_sgr(&raw.replace('\r', ""))
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Every text painted with SGR `code`: the text between `ESC[<code>m` and the next `ESC[0m`,
/// asserting that each is closed and holds no other escape.
fn painted(out: &str, code: &str) -> Vec<String> {
    let open = format!("\x1b[{code}m");
    let mut found = Vec::new();
    let mut rest = out;
    while let Some(i) = rest.find(&open) {
        rest = &rest[i + open.len()..];
        let end = rest
            .find(RESET)
            .unwrap_or_else(|| panic!("{open} never reset: {out}"));
        assert!(!rest[..end].contains(ESC), "{open} nests: {}", &rest[..end]);
        found.push(rest[..end].to_owned());
        rest = &rest[end + RESET.len()..];
    }
    found
}

fn count_tokens(plain: &str, token: &str) -> usize {
    // The risk line separates its entries with `;`.
    let tokens = plain.split_whitespace().map(|t| t.trim_end_matches(';'));
    tokens.filter(|t| *t == token).count()
}

#[test]
fn i5b_1_each_badge_kind_takes_its_palette_colour_and_is_reset() {
    let sb = Sb::colors();
    let out = sb.status(&["--color", "always"], &[]);
    let yellow = painted(&out, "33");
    assert!(
        yellow.iter().any(|t| t == "↑1") && yellow.iter().all(|t| t.starts_with('↑')),
        "{yellow:?}\n{out}"
    );
    let blue = painted(&out, "34");
    assert!(
        !blue.is_empty() && blue.iter().all(|t| t.starts_with('●')),
        "{blue:?}\n{out}"
    );
    let dim = painted(&out, "2");
    for want in ["missing", "stale", "1 stale", "merged·stale"] {
        assert!(
            dim.iter().any(|t| t == want),
            "no dim {want:?}: {dim:?}\n{out}"
        );
    }
    let red = painted(&out, "31");
    assert!(
        !red.is_empty() && red.iter().all(|t| t.starts_with("error: ")),
        "{red:?}\n{out}"
    );
}

#[test]
fn i5b_2_never_and_a_pipe_print_no_escape_byte() {
    let sb = Sb::colors();
    for extra in [&["--color", "never"][..], &[]] {
        let out = sb.status(extra, &[]);
        assert!(!out.contains(ESC), "{extra:?}: {out:?}");
        assert!(out.contains("missing"), "fixture lost its rows: {out}");
    }
}

#[test]
fn i5b_3_stripping_the_colour_from_always_gives_never_exactly() {
    let sb = Sb::colors();
    let always = sb.status(&["--color", "always"], &[]);
    let never = sb.status(&["--color", "never"], &[]);
    assert!(always.contains(ESC), "{always}");
    assert_eq!(strip_sgr(&always), never);
}

#[test]
fn i5b_4_a_terminal_colours_unless_no_color_and_always_beats_no_color() {
    let sb = Sb::colors();
    let auto = sb.pty(120, &[], &[]);
    assert!(auto.contains("\x1b["), "{auto:?}");
    let off = sb.pty(120, &[], &[("NO_COLOR", "1")]);
    assert!(!off.contains(ESC), "{off:?}");
    let forced = sb.pty(120, &["--color", "always"], &[("NO_COLOR", "1")]);
    assert!(forced.contains("\x1b["), "{forced:?}");
}

#[test]
fn i5b_4b_a_pipe_with_no_color_set_and_an_empty_no_color_behave() {
    let sb = Sb::colors();
    let piped = sb.status(&["--color", "always"], &[("NO_COLOR", "1")]);
    assert!(piped.contains(ESC), "always wins over NO_COLOR: {piped:?}");
    // An empty NO_COLOR counts as unset (§8.2): auto on a terminal still colours.
    let empty = sb.pty(120, &[], &[("NO_COLOR", "")]);
    assert!(empty.contains("\x1b["), "{empty:?}");
}

/// The `claude …` line of the titled fixture.
fn session_line(lines: &[String]) -> &String {
    lines
        .iter()
        .find(|l| l.contains("claude "))
        .expect("a session line")
}

#[test]
fn i5b_5_a_narrow_terminal_cuts_the_wide_line_and_keeps_every_row() {
    let sb = Sb::titled();
    let piped = sb.status(&[], &[]);
    let raw = sb.pty(60, &[], &[]);
    let lines = pty_lines(&raw);
    assert_eq!(lines.len(), piped.lines().count(), "{raw:?}\n{piped}");
    for line in &lines {
        assert!(
            line.chars().count() <= 60,
            "{line:?} is {} columns",
            line.chars().count()
        );
    }
    let cut = session_line(&lines);
    assert!(cut.ends_with('…'), "{cut:?}");
    assert!(cut.starts_with("  main"), "{cut:?}");
}

#[test]
fn i5b_6_piped_output_keeps_the_wide_line_whole() {
    let sb = Sb::titled();
    let piped = sb.status(&[], &[]);
    let lines: Vec<String> = piped.lines().map(str::to_owned).collect();
    let line = session_line(&lines);
    assert!(line.chars().count() > 60, "{line:?}");
    assert!(line.ends_with(&format!("{LONG_TITLE}\"")), "{line:?}");
    assert!(!piped.contains('…'), "{piped}");
}

#[test]
fn i5b_7_truncation_does_not_depend_on_colour() {
    let sb = Sb::titled();
    let raw = sb.pty(60, &["--color", "never"], &[]);
    assert!(!raw.contains(ESC), "{raw:?}");
    let lines = pty_lines(&raw);
    assert!(lines.iter().all(|l| l.chars().count() <= 60), "{lines:?}");
    assert!(session_line(&lines).ends_with('…'), "{lines:?}");
}

#[test]
fn i5b_5b_a_cut_coloured_line_closes_its_colour_after_the_ellipsis() {
    let sb = Sb::titled();
    // Colour on, narrow enough to cut inside the dirty repo's badge line too.
    let raw = sb.pty(60, &["--color", "always"], &[]);
    let line = raw
        .replace('\r', "")
        .lines()
        .find(|l| l.contains("claude "))
        .unwrap()
        .to_owned();
    assert!(line.ends_with(&format!("…{RESET}")), "{line:?}");
    assert!(pty_lines(&raw).iter().all(|l| l.chars().count() <= 60));
    // No colour is left open at the end of any line.
    for l in raw.replace('\r', "").lines() {
        let last_open = l.rfind("\x1b[").map(|i| &l[i..]);
        assert!(
            last_open.is_none_or(|s| s.starts_with(RESET)),
            "{l:?} ends in an open colour"
        );
    }
}

#[test]
fn i5b_10_a_bad_value_is_a_usage_error_and_json_ignores_the_flag() {
    let sb = Sb::colors();
    let bad = sb.run(
        &["status", sb.root.to_str().unwrap(), "--color", "bogus"],
        &[],
    );
    assert_eq!(bad.code, Some(2), "{}", bad.stderr);
    let plain = sb.status(&["--json"], &[]);
    let always = sb.status(&["--json", "--color", "always"], &[]);
    assert!(plain.starts_with('{'), "{plain}");
    assert_eq!(plain, always);
    assert!(!always.contains(ESC));
}

#[test]
fn i5b_12_the_dim_badges_of_the_tui_palette_are_dim_in_plain_status_too() {
    let sb = Sb::colors();
    let always = sb.status(&["--color", "always"], &[]);
    let never = sb.status(&["--color", "never"], &[]);
    let dim = painted(&always, "2");
    for word in ["nested", "bare", "ext", "no-upstream"] {
        let shown = count_tokens(&never, word);
        assert!(shown > 0, "the fixture has no {word} badge:\n{never}");
        assert_eq!(
            dim.iter().filter(|t| *t == word).count(),
            shown,
            "{word}: {dim:?}\n{always}"
        );
    }
    let quiet = never.matches("quiet ").count();
    assert!(quiet > 0, "no quiet cell:\n{never}");
    assert_eq!(
        dim.iter().filter(|t| t.starts_with("quiet ")).count(),
        quiet,
        "{dim:?}"
    );
}
