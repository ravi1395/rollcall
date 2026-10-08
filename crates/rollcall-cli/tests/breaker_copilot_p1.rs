//! Breaker tests for plan phase P1 (`SourceKind::Copilot` end-to-end, inert). Every test drives
//! the real binary on a sandbox of its own; no real agent home is read.
#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use rollcall_core::{
    Backing, Index, ScanResult, Session, SkippedCounts, SourceDiagnostics, SourceKind, SourceMode,
    SourceScan, UnmatchedCounts, Uuid,
};
use rollcall_store::SqliteIndex;
use tempfile::TempDir;

const NOW: &str = "2026-09-29T12:00:00Z";
const ID: &str = "0a1b2c3d-0000-4000-8000-0123456789ab";
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

fn wait(mut cmd: Command, what: &str) -> Run {
    let child = cmd
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
        panic!("{what} outlived {DEADLINE:?}")
    });
    let out = out.unwrap();
    Run {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
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
            .env("COPILOT_HOME", self.base.join("copilot"))
            .env("ROLLCALL_NOW", NOW)
            .env("PATH", "/usr/bin:/bin");
        wait(cmd, &format!("rollcall {args:?}"))
    }

    fn cache_dir(&self) -> PathBuf {
        rollcall_store::cache_dir(&self.home, Some(&self.base.join("xdg/cache")))
    }
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("/usr/bin/git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_DATE", "2026-09-10T09:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-09-10T09:00:00Z")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A repo `crm` under the root, scanned once by the real binary so the cache holds its row.
fn scanned() -> (Sb, PathBuf) {
    let sb = Sb::new();
    let repo = sb.root.join("crm");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    fs::write(repo.join("f"), "x").unwrap();
    git(&repo, &["add", "f"]);
    git(&repo, &["commit", "-q", "-m", "c"]);
    let first = sb.rollcall(&["status", "--json", sb.root.to_str().unwrap()]);
    assert_eq!(first.code, Some(0), "{}", first.stderr);
    (sb, repo)
}

/// Re-commits the cached scan of the root with one untitled `kind` session matched to `repo`.
fn seed(sb: &Sb, repo: &Path, kind: SourceKind) {
    let mut ix = SqliteIndex::open(&sb.cache_dir()).unwrap();
    let repos = ix.repos_for_root(&sb.root);
    assert!(!repos.is_empty(), "the first scan left no repo row");
    let diagnostics = ix.last_diagnostics(&sb.root).unwrap();
    let ts = "2026-09-29T10:00:00Z".parse().unwrap();
    let id = Uuid::parse(ID).unwrap();
    let session = Session {
        source: kind,
        id: id.clone(),
        cwd: repo.to_path_buf(),
        branch: Some("main".to_owned()),
        started_at: ts,
        ended_at: ts,
        title: None,
        last_prompt: None,
        message_count: 0,
        backing: Backing::LogFile {
            dev: 1,
            inode: 2,
            path: sb.base.join("events.jsonl"),
        },
    };
    let scan = ScanResult {
        scanned_at: diagnostics.scanned_at.unwrap(),
        submodules: false,
        partial: false,
        repos,
        sources: vec![SourceScan {
            files: Vec::new(),
            sessions: vec![session],
            diag: SourceDiagnostics {
                source: kind,
                mode: SourceMode::Jsonl,
                db_version: None,
                files: 0,
                records: 0,
                skipped: SkippedCounts::default(),
                sessions: 1,
                excluded_children: 0,
                vanished: 0,
                matched: 1,
                unmatched: UnmatchedCounts::default(),
            },
            notes: Vec::new(),
        }],
        matches: vec![(kind, id, Some(repo.to_path_buf()))],
        diagnostics,
    };
    ix.commit_scan(&sb.root, scan).unwrap();
}

fn cached_status(kind: SourceKind) -> Run {
    let (sb, repo) = scanned();
    seed(&sb, &repo, kind);
    sb.rollcall(&["status", "--no-scan", sb.root.to_str().unwrap()])
}

/// Control: the seeding route shows a Claude last session, so it can show a Copilot one too.
#[test]
fn breaker_control_a_seeded_claude_session_shows_in_cached_status() {
    let run = cached_status(SourceKind::Claude);
    assert!(run.code.is_some_and(|c| c != 101), "{}", run.stderr);
    assert!(run.stdout.contains("claude "), "{}", run.stdout);
}

/// P1-8: plain `status` for a worktree whose last session is a Copilot session prints a cell
/// starting `copilot `. Through the real binary the cached Copilot row reaches
/// `output.rs` `Builder::session`, whose two-entry resolver list has no Copilot source.
#[test]
fn breaker_cached_status_shows_a_copilot_last_session_without_panicking() {
    let run = cached_status(SourceKind::Copilot);
    assert_ne!(run.code, Some(101), "rollcall panicked: {}", run.stderr);
    assert!(run.stdout.contains("copilot "), "{}", run.stdout);
}

fn which(name: &str) -> Option<PathBuf> {
    let found = ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"]
        .iter()
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file());
    if found.is_none() && std::env::var_os("ROLLCALL_REQUIRE_TOOLS").is_some_and(|v| v == "1") {
        panic!("{name} is required under ROLLCALL_REQUIRE_TOOLS=1");
    }
    found
}

/// P1-4 through `rollcall init <shell>` (the real binary's stdout, not `init_script`): a stub
/// `rollcall` writes `<cwd>\0<agent>\0<id>\0` and exits 10; the agent stub records cwd + argv.
fn rc_via_binary(shell: &str, tool: &Path, agent: &str) -> (Run, Option<Vec<u8>>, PathBuf) {
    let sb = Sb::new();
    let script = sb.rollcall(&["init", shell]);
    assert_eq!(script.code, Some(0), "{}", script.stderr);
    let (bin, cwd) = (sb.base.join("bin"), sb.base.join("cwd"));
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    let payload = sb.base.join("payload");
    fs::write(&payload, format!("{}\0{agent}\0{ID}\0", cwd.display())).unwrap();
    let record = sb.base.join("record");
    let stub = |name: &str, body: String| {
        let path = bin.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    stub(
        "rollcall",
        format!(
            "cat '{}' > \"$ROLLCALL_RESUME_FILE\"\nexit 10",
            payload.display()
        ),
    );
    for name in ["claude", "codex", "copilot"] {
        stub(
            name,
            format!(
                "printf '%s\\0' {name} \"$(pwd -P)\" \"$@\" > '{}'",
                record.display()
            ),
        );
    }
    let init = sb.base.join("rc-init");
    fs::write(&init, script.stdout).unwrap();
    let call = if shell == "fish" {
        "source $argv[1]; rc"
    } else {
        ". \"$0\"; rc"
    };
    let mut cmd = Command::new(tool);
    cmd.args(["-c", call])
        .arg(&init)
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HOME", &sb.home);
    let run = wait(cmd, &format!("{shell} rc"));
    (run, fs::read(&record).ok(), cwd)
}

#[test]
fn breaker_rc_from_the_binary_runs_each_agent_with_its_own_argv_in_each_shell() {
    for shell in ["bash", "zsh", "fish"] {
        let Some(tool) = which(shell) else { continue };
        for (agent, verb) in [
            ("claude", "--resume"),
            ("codex", "resume"),
            ("copilot", "--resume"),
        ] {
            let (run, record, cwd) = rc_via_binary(shell, &tool, agent);
            assert_eq!(run.code, Some(0), "{shell} {agent}: {}", run.stderr);
            let want = format!("{agent}\0{}\0{verb}\0{ID}\0", cwd.display());
            assert_eq!(
                record.as_deref().map(String::from_utf8_lossy),
                Some(want.as_str().into()),
                "{shell} {agent}"
            );
        }
    }
}

#[test]
fn breaker_rc_from_the_binary_refuses_near_miss_agent_names_in_each_shell() {
    for shell in ["bash", "zsh", "fish"] {
        let Some(tool) = which(shell) else { continue };
        for agent in ["copilt", "Copilot", "copilot ", "copilot-cli", ""] {
            let (run, record, _) = rc_via_binary(shell, &tool, agent);
            assert_ne!(run.code, Some(0), "{shell} {agent:?}");
            assert!(
                run.stderr.contains("rc: unknown agent"),
                "{shell} {agent:?}: {}",
                run.stderr
            );
            assert_eq!(record, None, "{shell} {agent:?}: an agent stub ran");
        }
    }
}
