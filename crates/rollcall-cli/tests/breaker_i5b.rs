//! Breaker (adversarial QA) tests for I5b: colour and terminal-width truncation in plain
//! `status` (spec rev 16 §3.1, §8.2), through channels `color_i5b.rs` does not drive: display
//! columns (not chars) of wide and combining text, a width sweep down to 1 column, colour vs no
//! colour under truncation, `--json` on a terminal, a 0-column terminal, and the hint/risk lines.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use expectrl::{Eof, Expect, Session};
use ratatui::text::Span;
use rollcall_core::text::graphemes;
use serde_json::json;
use tempfile::TempDir;

const NOW: &str = "2026-09-29T12:00:00Z";
const DATE: &str = "2026-09-20T10:00:00Z";
const DEADLINE: Duration = Duration::from_secs(60);
const ESC: char = '\x1b';
const RESET: &str = "\x1b[0m";

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

    /// `rollcall status <root> <extra…>`, stdout a pipe.
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
        run.stdout
    }

    /// `rollcall status <root> <extra…>` on a PTY of `cols` columns.
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

    /// A Claude session in `cwd` titled `title`.
    fn session(&self, id: &str, cwd: &Path, title: &str) {
        let records = [
            json!({"type": "user", "sessionId": id, "timestamp": "2026-09-28T10:00:00.000Z",
                "cwd": cwd.to_str().unwrap(), "gitBranch": "main", "isSidechain": false,
                "message": {"role": "user", "content": "p"}}),
            json!({"type": "custom-title", "sessionId": id, "customTitle": title}),
        ];
        let text: String = records.iter().map(|r| format!("{r}\n")).collect();
        fs::write(
            self.base.join(format!("claude/projects/p/{id}.jsonl")),
            text,
        )
        .unwrap();
    }

    /// `w`: a session titled in wide CJK; `a`: a session titled in decomposed `é`; `d`: dirty
    /// (a risk line); `x`: a missing linked worktree (a hint line naming the long sandbox path).
    fn wide() -> Sb {
        let sb = Sb::new();
        let w = sb.repo("w");
        sb.session(
            "0f8fad5b-d9cb-469f-a165-70867728950e",
            &w,
            &format!("ab{}", "漢字".repeat(30)),
        );
        let a = sb.repo("a");
        sb.session(
            "1f8fad5b-d9cb-469f-a165-70867728950e",
            &a,
            &"e\u{301}".repeat(70),
        );
        let d = sb.repo("d");
        fs::write(d.join("f"), "x").unwrap();
        let x = sb.repo("x");
        let linked = sb.base.join("linked/x-feat");
        fs::create_dir_all(linked.parent().unwrap()).unwrap();
        sb.git(
            &x,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat",
                linked.to_str().unwrap(),
            ],
        );
        fs::remove_dir_all(&linked).unwrap();
        sb
    }
}

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

fn cols(text: &str) -> usize {
    Span::raw(text).width()
}

/// §8.2 width truncation as a property of one line pair (terminal, piped), both without SGR.
fn check_line(width: usize, shown: &str, full: &str) {
    assert!(
        cols(shown) <= width,
        "width {width}: {shown:?} is {} columns",
        cols(shown)
    );
    if cols(full) <= width {
        assert_eq!(shown, full, "width {width}: a line that fits was changed");
        return;
    }
    let kept = shown
        .strip_suffix('…')
        .unwrap_or_else(|| panic!("width {width}: cut line {shown:?} has no trailing …"));
    assert!(
        full.starts_with(kept),
        "width {width}: {kept:?} is not a prefix of {full:?}"
    );
    // A grapheme boundary of the full line.
    let mut at = 0;
    let boundary = graphemes(full).any(|g| {
        at += g.len();
        at == kept.len()
    }) || kept.is_empty();
    assert!(
        boundary,
        "width {width}: {kept:?} splits a grapheme of {full:?}"
    );
}

const WIDTHS: [u16; 8] = [1, 2, 3, 4, 20, 59, 60, 61];

#[test]
fn breaker_wide_and_combining_text_fits_the_terminal_in_display_columns() {
    let sb = Sb::wide();
    let piped = sb.status(&["--color", "never"], &[]);
    let full: Vec<&str> = piped.lines().collect();
    assert!(
        full.iter().any(|l| l.contains('漢') && cols(l) > 61),
        "{piped}"
    );
    assert!(
        full.iter().any(|l| l.contains("e\u{301}") && cols(l) > 61),
        "{piped}"
    );
    assert!(full.iter().any(|l| l.starts_with("hint: ")), "{piped}");
    assert!(full.iter().any(|l| l.starts_with("at risk: ")), "{piped}");
    for width in WIDTHS {
        let raw = sb.pty(width, &["--color", "never"], &[]);
        assert!(!raw.contains(ESC), "{raw:?}");
        let shown: Vec<String> = raw.replace('\r', "").lines().map(str::to_owned).collect();
        assert_eq!(shown.len(), full.len(), "width {width}: {shown:?}\n{piped}");
        for (s, f) in shown.iter().zip(&full) {
            check_line(usize::from(width), s, f);
        }
    }
}

#[test]
fn breaker_colour_under_truncation_strips_to_the_uncoloured_cut() {
    let sb = Sb::wide();
    for width in WIDTHS {
        let never = sb.pty(width, &["--color", "never"], &[]).replace('\r', "");
        let always = sb.pty(width, &["--color", "always"], &[]).replace('\r', "");
        assert!(always.contains(ESC), "width {width}: {always:?}");
        assert_eq!(strip_sgr(&always), never, "width {width}");
        for (line, plain) in always.lines().zip(never.lines()) {
            if plain.ends_with('…') {
                assert!(
                    line.ends_with(&format!("…{RESET}")),
                    "width {width}: {line:?}"
                );
            }
            let last_open = line.rfind("\x1b[").map(|i| &line[i..]);
            assert!(
                last_open.is_none_or(|s| s.starts_with(RESET)),
                "width {width}: {line:?} ends in an open colour"
            );
        }
    }
}

#[test]
fn breaker_json_on_a_narrow_terminal_is_neither_cut_nor_coloured() {
    let sb = Sb::wide();
    let piped = sb.status(&["--json"], &[]);
    for extra in [&["--json"][..], &["--json", "--color", "always"]] {
        let raw = sb.pty(20, extra, &[]).replace('\r', "");
        assert!(!raw.contains(ESC), "{extra:?}: {raw:?}");
        assert!(
            !raw.contains('…') || piped.contains('…'),
            "{extra:?}: {raw}"
        );
        assert_eq!(raw, piped, "{extra:?}");
    }
}

#[test]
fn breaker_a_zero_column_terminal_cuts_nothing() {
    let sb = Sb::wide();
    let piped = sb.status(&["--color", "never"], &[]);
    let raw = sb.pty(0, &["--color", "never"], &[]).replace('\r', "");
    assert_eq!(raw, piped);
}

#[test]
fn breaker_no_color_never_beats_always_on_a_pipe_and_never_stays_plain() {
    let sb = Sb::wide();
    let forced = sb.status(&["--color", "always"], &[("NO_COLOR", "1")]);
    assert!(forced.contains("\x1b["), "{forced:?}");
    let never = sb.pty(120, &["--color", "never"], &[("NO_COLOR", "")]);
    assert!(!never.contains(ESC), "{never:?}");
    let auto = sb.pty(120, &["--color", "auto"], &[("NO_COLOR", "1")]);
    assert!(!auto.contains(ESC), "{auto:?}");
}

/// Regression test for the author decision of 2026-10-06 (spec §8.2 names `tcgetwinsize` on
/// stdout): stdout is terminal A (40 columns), the controlling terminal is B (60 columns), and
/// "every output line wider than the terminal's width" is stdout's, not `/dev/tty`'s.
#[test]
fn breaker_width_is_that_of_the_stdout_terminal_not_the_controlling_one() {
    let sb = Sb::wide();
    let mut a = Session::spawn({
        let mut c = Command::new("/bin/sh");
        c.args(["-c", "tty; exec sleep 30"]);
        c
    })
    .unwrap();
    a.set_expect_timeout(Some(DEADLINE));
    a.get_process_mut().set_window_size(40, 24).unwrap();
    let tty = a.expect(expectrl::Regex("/dev/[a-z0-9/]+")).unwrap();
    let tty = String::from_utf8(tty.get(0).unwrap().to_vec()).unwrap();
    let script = format!(
        "exec {} status {} --color never > {tty}",
        env!("CARGO_BIN_EXE_rollcall"),
        sb.root.display()
    );
    let mut b = Session::spawn({
        let mut c = sb.command(&[], &[]);
        c.env("SH_SCRIPT", &script);
        let mut sh = Command::new("/bin/sh");
        sh.args(["-c", &script]).env_clear();
        for (k, v) in c.get_envs() {
            if let Some(v) = v {
                sh.env(k, v);
            }
        }
        sh
    })
    .unwrap();
    b.set_expect_timeout(Some(DEADLINE));
    b.get_process_mut().set_window_size(60, 24).unwrap();
    b.expect(Eof).unwrap();
    let got = a.expect(expectrl::Regex("hint: [^\r\n]*\r\n")).unwrap();
    let all = String::from_utf8_lossy(got.before()).into_owned()
        + &String::from_utf8_lossy(got.get(0).unwrap());
    let shown: Vec<&str> = all
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.is_empty() && !l.starts_with("/dev/"))
        .collect();
    let widest = shown.iter().map(|l| cols(l)).max().unwrap();
    assert!(
        widest <= 40,
        "widest line {widest} columns on a 40-column stdout (controlling terminal 60): {shown:#?}"
    );
}
