//! §12.1 #11 (P39): shell safety and resume end-to-end. The `c` / `Y` fragments run under `sh -c`, the `rc`
//! function runs in bash, zsh and fish, and `R` is driven through a real PTY with stub agents that record their
//! argv and cwd. Every spawned process runs under a deadline with a scratch `HOME` and cache.
use crate::build_sessions::{CodexMode, build_copilot, build_corpora};
use crate::support::{
    Corpus, RUN_DEADLINE, Which, World, fresh, git_path, require_tool, run_rollcall, spawn_watchdog,
};
use expectrl::session::OsSession;
use expectrl::{Eof, Expect, Session};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rollcall_cli::app::{Action, handle_key};
use rollcall_cli::config::EnvPaths;
use rollcall_cli::tui::Model;
use rollcall_core::StatusDoc;
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

const ROLLCALL_BIN: &str = env!("CARGO_BIN_EXE_rollcall");
const PTY_DEADLINE: Duration = Duration::from_secs(30);
const PTY_COLS: u16 = 120;
const PTY_ROWS: u16 = 32;
const MARKER_PREFIX: &str = "marker-";
/// The main worktree of `f-shell` and its newest session (`c1a00006-…08`, branch `$(touch marker-rc)`).
const MAIN_SESSION: (&str, &str) = ("claude", "c1a00006-0000-4000-8000-000000000008");
/// Records `<name> <cwd> <argv…>` NUL-separated, so a newline in the path cannot blur the fields.
const STUB: &str = "#!/bin/sh\nprintf '%s\\0' \"${0##*/}\" \"$PWD\" \"$@\" > \"$STUB_RECORD\"\n";
/// `mktemp` that always lands under `$TMPDIR` (bare macOS `mktemp` ignores it) and records the path it made.
const MKTEMP_STUB: &str = "#!/bin/sh\nf=$(/usr/bin/mktemp \"$TMPDIR/tmp.XXXXXXXX\") || exit 1\nprintf '%s' \"$f\" > \"$TMPDIR/mktemp-made\"\nprintf '%s\\n' \"$f\"\n";
const PLANTED: &str = "#!/bin/sh\nprintf 'planted\\0' > \"$STUB_RECORD\"\n";

/// (worktree dir under `$F`, agent of its newest session, that session's id): the six hostile paths. The
/// Codex thread is newer than the Claude session in `shell wt space`, so it wins there.
const HOSTILE: [(&str, &str, &str); 6] = [
    (
        "shell wt space",
        "codex",
        "c0d00004-0000-4000-8000-000000000001",
    ),
    (
        "shell-wt$dollar",
        "claude",
        "c1a00006-0000-4000-8000-000000000002",
    ),
    (
        "shell-wt`tick",
        "claude",
        "c1a00006-0000-4000-8000-000000000003",
    ),
    (
        "shell-wt;semi",
        "claude",
        "c1a00006-0000-4000-8000-000000000004",
    ),
    (
        "shell-wt'quote",
        "claude",
        "c1a00006-0000-4000-8000-000000000005",
    ),
    (
        "shell-wt\nnewline",
        "claude",
        "c1a00006-0000-4000-8000-000000000006",
    ),
];

// ------------------------------------------------------------------- rig

/// One `F` world with the `shell` corpus, built once; tests scan it only through their own scratch cache.
struct Rig {
    w: World,
    doc: StatusDoc,
    /// Holds `rollcall` (what the `rc` function calls through `command`).
    bin: PathBuf,
    /// Holds the recording `claude` and `codex` stubs.
    stubs: PathBuf,
}

fn rig() -> &'static Rig {
    static RIG: OnceLock<Rig> = OnceLock::new();
    RIG.get_or_init(|| {
        make_rig(
            |w| build_corpora(w, &[Corpus::Shell], CodexMode::Db),
            &["claude", "codex"],
        )
    })
}

/// An `F` world whose only agent data is the manifest's Copilot session (cwd `$F/merged`), with a recording
/// `copilot` stub. Copilot sessions get their own rig so the `shell` corpus keeps its newest-session assertions.
fn copilot_rig() -> &'static Rig {
    static RIG: OnceLock<Rig> = OnceLock::new();
    RIG.get_or_init(|| make_rig(|w| build_copilot(w, &w.sb.copilot_home), &["copilot"]))
}

fn make_rig(build: impl FnOnce(&World), agents: &[&str]) -> Rig {
    let w = fresh(Which::F);
    build(&w);
    let bin = w.base.join("bin");
    let stubs = w.base.join("stubs");
    for dir in [&bin, &stubs] {
        std::fs::create_dir_all(dir).expect("create rig dir");
    }
    std::os::unix::fs::symlink(ROLLCALL_BIN, bin.join("rollcall")).expect("link rollcall");
    for name in agents {
        write_executable(&stubs.join(name), STUB);
    }
    write_executable(&stubs.join("mktemp"), MKTEMP_STUB);
    let out = run_rollcall(&w, &["status", w.root.to_str().unwrap(), "--json"]);
    let doc = serde_json::from_str(&out.stdout)
        .unwrap_or_else(|e| panic!("status --json: {e}\n{}", out.stderr));
    Rig { w, doc, bin, stubs }
}

fn write_executable(path: &Path, script: &str) {
    std::fs::write(path, script).expect("write stub");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

impl Rig {
    fn shell_dir(&self) -> PathBuf {
        self.w.root.join("shell")
    }

    /// A command in the sandbox with its own `HOME` and cache (`scratch`), `rollcall` and the stubs on an
    /// all-absolute `PATH`, and `STUB_RECORD` naming where a stub writes.
    fn command(&self, program: impl AsRef<OsStr>, scratch: &Path) -> Command {
        let path = std::env::join_paths([
            &self.bin,
            &self.stubs,
            git_path().parent().unwrap(),
            Path::new("/usr/bin"),
            Path::new("/bin"),
        ])
        .expect("PATH entries contain no ':'");
        let home = scratch.join("home");
        std::fs::create_dir_all(&home).expect("create scratch home");
        let mut cmd = self.w.sb.command(program);
        cmd.env("HOME", home)
            .env("XDG_CACHE_HOME", scratch.join("cache"))
            .env("TMPDIR", scratch)
            .env("STUB_RECORD", record_path(scratch))
            .env("TERM", "xterm-256color")
            .env("PATH", path);
        cmd
    }

    fn rollcall(&self, scratch: &Path, args: &[&str]) -> Command {
        let mut cmd = self.command(ROLLCALL_BIN, scratch);
        cmd.args(args).current_dir(self.shell_dir());
        cmd
    }

    /// Fills the scratch cache so a TUI's first paint already has the repos.
    fn warm_cache(&self, scratch: &Path) {
        let root = self.w.root.to_str().unwrap();
        let out = spawn_watchdog(self.rollcall(scratch, &["scan", root]), RUN_DEADLINE)
            .unwrap_or_else(|e| panic!("scan: {e}"));
        assert!(
            !out.stdout.is_empty() || !out.stderr.is_empty(),
            "scan printed nothing"
        );
    }

    fn model(&self) -> Model {
        let now = crate::support::manifest()
            .meta
            .now
            .parse()
            .expect("meta.now");
        Model::new(self.doc.clone(), now, false)
    }

    /// A model whose cursor sits on the worktree at `path`.
    fn model_at(&self, path: &Path) -> Model {
        let mut model = self.model();
        for (r, repo) in self.doc.repos.iter().enumerate() {
            if let Some(w) = repo
                .worktrees
                .iter()
                .position(|w| Path::new(&w.path) == path)
            {
                model.selected = r;
                model.selected_worktree = w;
                return model;
            }
        }
        let rows: Vec<_> = (self.doc.repos.iter())
            .filter(|r| r.name == "shell")
            .flat_map(|r| &r.worktrees)
            .map(|w| (&w.path, &w.error))
            .collect();
        panic!("no worktree at {path:?} in the status document; the shell repo has {rows:#?}");
    }
}

fn record_path(scratch: &Path) -> PathBuf {
    scratch.join("record")
}

fn read_record(scratch: &Path) -> Vec<String> {
    let path = record_path(scratch);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("the stub agent never ran ({}): {e}", path.display()));
    bytes
        .split(|b| *b == 0)
        .filter(|field| !field.is_empty())
        .map(|field| String::from_utf8(field.to_vec()).expect("record is UTF-8"))
        .collect()
}

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn press(model: &mut Model, c: char) -> Action {
    handle_key(model, &EnvPaths::default(), key(c))
}

/// Every `marker-*` file in `dirs`, so a missing marker proves the command never ran there.
fn markers(dirs: &[&Path]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for dir in dirs {
        for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let entry = entry.expect("dir entry");
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(MARKER_PREFIX)
            {
                found.push(entry.path());
            }
        }
    }
    found
}

fn assert_no_marker(r: &Rig, scratch: &Path) {
    let shell = r.shell_dir();
    let found = markers(&[scratch, &shell, &r.w.root]);
    assert!(found.is_empty(), "a hostile branch name ran: {found:?}");
}

// ------------------------------------------------------- `c` and `Y` fragments

fn assert_c_fragment(mut model: Model, raw: &Path) {
    let Action::Copy(text) = press(&mut model, 'c') else {
        panic!("`c` on {raw:?} did not copy");
    };
    let quoted = text
        .strip_prefix("cd -- ")
        .unwrap_or_else(|| panic!("not a cd fragment: {text:?}"));
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(format!("printf '%s' {quoted}"));
    let out = spawn_watchdog(cmd, RUN_DEADLINE).unwrap_or_else(|e| panic!("{raw:?}: {e}"));
    assert_eq!(
        out.stdout,
        raw.as_os_str().as_encoded_bytes(),
        "`c` fragment for {raw:?} mangled the path ({text:?})"
    );
}

fn assert_y_line(mut model: Model, raw: &Path, agent: &str, id: &str) {
    let r = rig();
    let Action::Copy(line) = press(&mut model, 'Y') else {
        panic!("`Y` on {raw:?} did not copy");
    };
    let scratch = r.w.scratch();
    let mut cmd = r.command("sh", scratch.path());
    cmd.arg("-c").arg(&line);
    let out = spawn_watchdog(cmd, RUN_DEADLINE).unwrap_or_else(|e| panic!("{raw:?}: {e}"));
    assert!(
        out.status.success(),
        "{line:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let subcommand = if agent == "codex" {
        "resume"
    } else {
        "--resume"
    };
    assert_eq!(
        read_record(scratch.path()),
        [agent, raw.to_str().unwrap(), subcommand, id],
        "`Y` line {line:?}"
    );
}

/// One `c` test and one `Y` test per hostile worktree, each on the row the status document gives it.
macro_rules! hostile_path_tests {
    ($($module:ident: $case:expr;)*) => {$(
        mod $module {
            use super::*;

            #[test]
            fn c_fragment_prints_the_raw_path() {
                let (dir, _, _) = HOSTILE[$case];
                let raw = rig().w.root.join(dir);
                assert_c_fragment(rig().model_at(&raw), &raw);
            }

            #[test]
            fn y_line_records_the_session_cwd_and_argv() {
                let (dir, agent, id) = HOSTILE[$case];
                let raw = rig().w.root.join(dir);
                assert_y_line(rig().model_at(&raw), &raw, agent, id);
            }
        }
    )*};
}

hostile_path_tests! {
    space: 0;
    dollar: 1;
    backtick: 2;
    semicolon: 3;
    single_quote: 4;
    newline: 5;
}

/// `git worktree add` records a newline path in `<admin>/gitdir` across two lines, so the scan may not
/// surface it as a row (see the `newline` tests above). This checks the quoting contract on a row built
/// by hand from the main worktree, so a scan defect cannot hide a quoting one.
#[test]
fn newline_path_quoting_holds_on_a_hand_built_row() {
    let r = rig();
    let (dir, agent, id) = HOSTILE[5];
    let raw = r.w.root.join(dir);
    assert!(raw.is_dir(), "the fixture must have created {raw:?}");
    let mut model = r.model();
    let at = model
        .doc
        .repos
        .iter()
        .position(|repo| repo.name == "shell")
        .unwrap();
    let mut row = model.doc.repos[at].worktrees[0].clone();
    row.path = raw.to_str().unwrap().to_owned();
    let session = row.last_session.as_mut().unwrap();
    session.cwd = row.path.clone();
    session.id = id.to_owned();
    model.doc.repos[at].worktrees = vec![row];
    model.selected = at;
    model.selected_worktree = 0;
    assert_c_fragment(model.clone(), &raw);
    assert_y_line(model, &raw, agent, id);
}

#[test]
fn y_and_c_do_nothing_without_a_session_or_worktree_selection() {
    let r = rig();
    let mut model = r.model();
    model.doc.repos.clear();
    assert_eq!(press(&mut model, 'c'), Action::None);
    assert_eq!(press(&mut model, 'Y'), Action::None);
    assert_eq!(press(&mut model, 'R'), Action::None);
}

// ------------------------------------------------------------ the marker detector

#[test]
fn the_marker_check_sees_a_real_command_substitution() {
    let r = rig();
    let scratch = r.w.scratch();
    let mut cmd = Command::new("sh");
    cmd.current_dir(scratch.path())
        .arg("-c")
        .arg("echo \"$(touch marker-rc)\"");
    spawn_watchdog(cmd, RUN_DEADLINE).expect("sh");
    assert_eq!(markers(&[scratch.path()]).len(), 1);
}

// --------------------------------------------------------------- rc in a shell

#[derive(Clone, Copy)]
enum Sh {
    Bash,
    Zsh,
    Fish,
}

impl Sh {
    fn name(self) -> &'static str {
        match self {
            Sh::Bash => "bash",
            Sh::Zsh => "zsh",
            Sh::Fish => "fish",
        }
    }

    /// The shell with `rollcall init <shell>` sourced, then `rc <args>`; the init file is `$1` / `$argv[1]`.
    fn rc_command(self, r: &Rig, tool: &Path, scratch: &Path, args: &[&str]) -> Command {
        let init = scratch.join("rc-init");
        let script = spawn_watchdog(r.rollcall(scratch, &["init", self.name()]), RUN_DEADLINE)
            .unwrap_or_else(|e| panic!("init: {e}"));
        std::fs::write(&init, script.stdout).expect("write init script");
        let mut cmd = r.command(tool, scratch);
        cmd.current_dir(r.shell_dir());
        match self {
            Sh::Bash => cmd.args(["--noprofile", "--norc", "-c", POSIX_DRIVER, "bash"]),
            Sh::Zsh => cmd.args(["-f", "-c", POSIX_DRIVER, "zsh"]),
            Sh::Fish => cmd.args(["--no-config", "-c", FISH_DRIVER]),
        };
        cmd.arg(init).args(args);
        cmd
    }
}

const POSIX_DRIVER: &str = "source \"$1\"; shift; rc \"$@\"";
const FISH_DRIVER: &str = "source $argv[1]; rc $argv[2..-1]";

/// What `rollcall <args>` prints when run without the function.
fn direct(r: &Rig, scratch: &Path, args: &[&str]) -> crate::support::Run {
    spawn_watchdog(r.rollcall(scratch, args), RUN_DEADLINE)
        .unwrap_or_else(|e| panic!("rollcall {args:?}: {e}"))
        .into()
}

/// `rc <args>` must behave exactly like `rollcall <args>` and never evaluate a branch name.
fn rc_passes_through(sh: Sh, args: &[&str]) -> crate::support::Run {
    let r = rig();
    let scratch = r.w.scratch();
    let expected = direct(r, scratch.path(), args);
    let Some(tool) = require_tool(sh.name()) else {
        return expected;
    };
    let cmd = sh.rc_command(r, &tool, scratch.path(), args);
    let got: crate::support::Run = spawn_watchdog(cmd, RUN_DEADLINE)
        .unwrap_or_else(|e| panic!("{} rc {args:?}: {e}", sh.name()))
        .into();
    assert_eq!(got.code, expected.code, "{}: {}", sh.name(), got.stderr);
    assert_eq!(got.stdout, expected.stdout, "{} rc {args:?}", sh.name());
    assert_no_marker(r, scratch.path());
    // `rc` called the `mktemp` stub (it records the file it made) and removed that file on every non-10 exit.
    let made = std::fs::read_to_string(scratch.path().join("mktemp-made"))
        .unwrap_or_else(|e| panic!("{}: rc never called the mktemp stub: {e}", sh.name()));
    assert!(
        Path::new(&made).starts_with(scratch.path()),
        "{}: {made}",
        sh.name()
    );
    assert!(
        !Path::new(&made).exists(),
        "{}: resume temp file left: {made}",
        sh.name()
    );
    got
}

fn rc_status(sh: Sh) {
    let root = rig().w.root.to_str().unwrap().to_owned();
    let got = rc_passes_through(sh, &["status", &root]);
    // `F` holds error rows: `rc` returns exit 4 unchanged (only 10 is its own).
    assert_eq!(got.code, 4, "{}: {}", sh.name(), got.stderr);
    if require_tool(sh.name()).is_some() {
        // The hostile branch was rendered, not run.
        assert!(got.stdout.contains("$(true>marker-rc)"), "{}", got.stdout);
    }
}

fn rc_status_json(sh: Sh) {
    let root = rig().w.root.to_str().unwrap().to_owned();
    let got = rc_passes_through(sh, &["status", &root, "--json"]);
    if require_tool(sh.name()).is_some() {
        assert!(got.stdout.contains("$(touch marker-rc)"), "{}", got.stdout);
    }
}

fn rc_help(sh: Sh) {
    let got = rc_passes_through(sh, &["--help"]);
    if require_tool(sh.name()).is_some() {
        assert_eq!(got.code, 0);
        assert!(got.stdout.contains("Usage"), "{}", got.stdout);
    }
}

// -------------------------------------------------------------------- the PTY

fn spawn_pty(cmd: Command) -> OsSession {
    let mut p = Session::spawn(cmd).expect("spawn in a PTY");
    p.set_expect_timeout(Some(PTY_DEADLINE));
    p.get_process_mut()
        .set_window_size(PTY_COLS, PTY_ROWS)
        .expect("set the PTY size");
    p
}

/// Waits for the first frame, filters to the `shell` repo (the only one matching), and presses `R`: the
/// cursor is then on the main worktree. Returns when the PTY child has exited.
fn resume_main_worktree(p: &mut OsSession) {
    resume_filtered(p, "/shell");
}

/// As above for the repo `filter` (`/name`) selects.
fn resume_filtered(p: &mut OsSession, filter: &str) {
    p.expect("rollcall").expect("the TUI never drew");
    p.send(filter).expect("type the filter");
    p.send("\r").expect("end the filter");
    p.send("R").expect("press R");
    p.expect(Eof).expect("the process never exited after R");
}

fn assert_main_session_record(r: &Rig, scratch: &Path) {
    let (agent, id) = MAIN_SESSION;
    assert_eq!(
        read_record(scratch),
        [agent, r.shell_dir().to_str().unwrap(), "--resume", id]
    );
}

#[test]
fn the_fixture_has_exactly_one_repo_matching_the_shell_filter() {
    let r = rig();
    let mut model = r.model();
    press(&mut model, '/');
    for c in "shell".chars() {
        press(&mut model, c);
    }
    let visible = rollcall_cli::tui::visible_repos(&model);
    assert_eq!(
        visible.len(),
        1,
        "{:?}",
        visible.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
    let main = &visible[0].worktrees[0];
    assert_eq!(Path::new(&main.path), r.shell_dir());
    let session = main
        .last_session
        .as_ref()
        .expect("main worktree has a session");
    assert_eq!(
        (
            format!("{:?}", session.source).to_lowercase().as_str(),
            session.id.as_str()
        ),
        MAIN_SESSION
    );
}

#[test]
fn tui_r_execs_the_absolute_stub_in_the_session_cwd_and_ignores_a_relative_path_entry() {
    let r = rig();
    let scratch = r.w.scratch();
    r.warm_cache(scratch.path());
    // `relative-bin/claude` is first on PATH and would win if relative entries were searched.
    let planted = scratch.path().join("relative-bin");
    std::fs::create_dir_all(&planted).unwrap();
    write_executable(&planted.join("claude"), PLANTED);
    let mut cmd = r.command(ROLLCALL_BIN, scratch.path());
    let absolute = cmd
        .get_envs()
        .find(|(k, _)| *k == "PATH")
        .unwrap()
        .1
        .unwrap()
        .to_owned();
    cmd.arg(r.w.root.to_str().unwrap())
        .current_dir(scratch.path())
        .env(
            "PATH",
            format!("relative-bin:{}", absolute.to_string_lossy()),
        );
    let mut p = spawn_pty(cmd);
    resume_main_worktree(&mut p);
    assert_main_session_record(r, scratch.path());
    assert_no_marker(r, scratch.path());
}

/// The repo filter that leaves only `merged`, whose main worktree holds the Copilot session.
const COPILOT_FILTER: &str = "/merged";

fn copilot_session() -> &'static crate::support::CopilotSession {
    &crate::support::manifest().copilot[0]
}

fn assert_copilot_session_record(r: &Rig, scratch: &Path) {
    assert_eq!(
        read_record(scratch),
        [
            "copilot",
            r.w.root.join("merged").to_str().unwrap(),
            "--resume",
            copilot_session().id.as_str()
        ]
    );
}

#[test]
fn the_copilot_fixture_has_exactly_one_repo_matching_its_filter() {
    let r = copilot_rig();
    let mut model = r.model();
    press(&mut model, '/');
    for c in COPILOT_FILTER.chars().skip(1) {
        press(&mut model, c);
    }
    let visible = rollcall_cli::tui::visible_repos(&model);
    assert_eq!(
        visible.iter().map(|v| &v.name).collect::<Vec<_>>(),
        ["merged"]
    );
    let main = &visible[0].worktrees[0];
    assert_eq!(Path::new(&main.path), r.w.root.join("merged"));
    let session = main.last_session.as_ref().expect("a Copilot session");
    assert_eq!(
        (session.source, session.id.as_str()),
        (
            rollcall_core::SourceKind::Copilot,
            copilot_session().id.as_str()
        )
    );
}

/// P3-5: `R` on a Copilot session with `ROLLCALL_RESUME_FILE` writes `<cwd>\0copilot\0<id>\0` and exits 10.
#[test]
fn tui_r_on_a_copilot_session_writes_the_resume_file_and_exits_10() {
    let r = copilot_rig();
    let scratch = r.w.scratch();
    r.warm_cache(scratch.path());
    // The `rc` function hands over a private file (mode 0600) it made with `mktemp`.
    let file = scratch.path().join("resume-file");
    std::fs::write(&file, b"").expect("create resume file");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    let mut cmd = r.command(ROLLCALL_BIN, scratch.path());
    cmd.arg(r.w.root.to_str().unwrap())
        .current_dir(scratch.path())
        .env("ROLLCALL_RESUME_FILE", &file);
    let mut p = spawn_pty(cmd);
    resume_filtered(&mut p, COPILOT_FILTER);
    let status = format!("{:?}", p.get_process_mut().wait().expect("wait"));
    assert!(
        status.starts_with("Exited(") && status.ends_with(", 10)"),
        "{status}"
    );
    let want = format!(
        "{}\0copilot\0{}\0",
        r.w.root.join("merged").display(),
        copilot_session().id
    );
    assert_eq!(
        std::fs::read(&file).expect("read resume file"),
        want.as_bytes()
    );
    assert!(
        !record_path(scratch.path()).exists(),
        "the stub ran: R must only write the file"
    );
}

macro_rules! rc_shell_tests {
    ($module:ident, $sh:expr) => {
        mod $module {
            use super::*;

            #[test]
            fn rc_status_runs_no_branch_name() {
                rc_status($sh);
            }

            #[test]
            fn rc_status_json_runs_no_branch_name() {
                rc_status_json($sh);
            }

            #[test]
            fn rc_help_runs_no_branch_name() {
                rc_help($sh);
            }

            #[test]
            fn rc_r_lands_in_the_session_cwd_with_the_stub_argv() {
                let r = rig();
                let Some(tool) = require_tool($sh.name()) else {
                    return;
                };
                let scratch = r.w.scratch();
                r.warm_cache(scratch.path());
                let cmd = $sh.rc_command(r, &tool, scratch.path(), &[r.w.root.to_str().unwrap()]);
                let mut p = spawn_pty(cmd);
                resume_main_worktree(&mut p);
                assert_main_session_record(r, scratch.path());
                assert_no_marker(r, scratch.path());
            }

            #[test]
            fn rc_r_lands_in_the_copilot_session_cwd_with_resume_argv() {
                let r = copilot_rig();
                let Some(tool) = require_tool($sh.name()) else {
                    return;
                };
                let scratch = r.w.scratch();
                r.warm_cache(scratch.path());
                let cmd = $sh.rc_command(r, &tool, scratch.path(), &[r.w.root.to_str().unwrap()]);
                let mut p = spawn_pty(cmd);
                resume_filtered(&mut p, COPILOT_FILTER);
                assert_copilot_session_record(r, scratch.path());
                assert_no_marker(r, scratch.path());
            }
        }
    };
}

rc_shell_tests!(bash, Sh::Bash);
rc_shell_tests!(zsh, Sh::Zsh);
rc_shell_tests!(fish, Sh::Fish);
