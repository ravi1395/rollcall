use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use rollcall_core::text::shell_quote;
use rollcall_core::{StatusDoc, Uuid, Worktree};

use crate::config::{Clipboard, Ctx, EnvPaths};
use crate::output::{self, ResumeTarget, TextPolicy};
use crate::resume;
use crate::scan::{self, ScanMode};
use crate::tui::{self, Model, Pane, visible_repos};

/// How long the loop waits for a key before it checks for a finished scan.
const TICK: Duration = Duration::from_millis(100);

/// What the event loop must do after a key; `handle_key` itself performs no I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    Rescan,
    Copy(String),
    /// Run without a shell (`o`): `program` is `$VISUAL` / `$EDITOR`'s first word.
    Spawn {
        program: String,
        args: Vec<String>,
    },
    Resume(ResumeTarget),
}

/// Terminal setup, first paint from the cache, background scan, event loop.
pub fn run(ctx: Ctx, root: &Path) -> ExitCode {
    let doc = snapshot(&ctx, root, ScanMode::NoScan);
    let mut model = Model::new(doc, ctx.now, ctx.env.no_color);
    if model.doc.scanned_at.is_none() {
        // ponytail: `scan` reports no progress, so the count stays 0; add a callback when it does.
        model.scanning = Some(0);
    }
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut model, &ctx, root);
    ratatui::restore();
    match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("rollcall: {err}");
            ExitCode::FAILURE
        }
    }
}

/// A status document straight from `scan`, for `run`'s first paint and every background scan.
fn snapshot(ctx: &Ctx, root: &Path, mode: ScanMode) -> StatusDoc {
    let outcome = scan::scan(ctx, root, mode);
    let policy = TextPolicy {
        prompts: true,
        redact: ctx.config.redact_prompts,
    };
    output::build_status(ctx, &outcome.index, root, outcome.stale, policy)
}

/// Runs a full scan off-thread; the receiver gets the fresh document, or hangs up if the scan
/// thread died.
fn spawn_scan(ctx: &Ctx, root: &Path) -> Receiver<StatusDoc> {
    let (tx, rx): (Sender<StatusDoc>, _) = mpsc::channel();
    let (ctx, root) = (ctx.clone(), root.to_path_buf());
    thread::spawn(move || {
        // A send error means the UI already quit.
        let _ = tx.send(snapshot(&ctx, &root, ScanMode::Fresh));
    });
    rx
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    model: &mut Model,
    ctx: &Ctx,
    root: &Path,
) -> io::Result<ExitCode> {
    let mut scan_rx = Some(spawn_scan(ctx, root));
    loop {
        if let Some(rx) = &scan_rx {
            match rx.try_recv() {
                Ok(doc) => {
                    model.doc = doc;
                    model.scanning = None;
                    scan_rx = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    model.scanning = None;
                    model.status_message = Some("scan failed".to_owned());
                    scan_rx = None;
                }
            }
        }
        terminal.draw(|frame| tui::draw(frame, model))?;
        if !event::poll(TICK)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        match handle_key(model, &ctx.env, key) {
            Action::None => {}
            Action::Quit => return Ok(ExitCode::SUCCESS),
            Action::Rescan => {
                if scan_rx.is_none() {
                    scan_rx = Some(spawn_scan(ctx, root));
                }
            }
            Action::Copy(text) => {
                let clipboard = ctx.config.clipboard;
                apply_copy(model, &text, |t| write_clipboard(clipboard, t));
            }
            Action::Spawn { program, args } => {
                ratatui::restore();
                let result = Command::new(&program).args(&args).status();
                *terminal = ratatui::init();
                model.status_message = result.err().map(|e| format!("cannot run {program}: {e}"));
            }
            Action::Resume(target) => {
                ratatui::restore();
                let uid = || rollcall_store::effective_uid(&ctx.cache_dir);
                match resume::resume(&target, &ctx.env, &uid) {
                    Ok(code) => return Ok(code),
                    Err(message) => {
                        *terminal = ratatui::init();
                        model.status_message = Some(message);
                    }
                }
            }
        }
    }
}

/// Keys edit the filter while `/` is active; everything else is a command.
pub fn handle_key(model: &mut Model, env: &EnvPaths, key: KeyEvent) -> Action {
    if key.kind != KeyEventKind::Press {
        return Action::None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        // Raw mode turns Ctrl-C into a key event, so honour it as quit.
        return match key.code {
            KeyCode::Char('c') => Action::Quit,
            _ => Action::None,
        };
    }
    model.status_message = None;
    if model.filter_editing {
        edit_filter(model, key.code);
        return Action::None;
    }
    match key.code {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('r') => Action::Rescan,
        KeyCode::Char('j') | KeyCode::Down => move_cursor(model, true),
        KeyCode::Char('k') | KeyCode::Up => move_cursor(model, false),
        KeyCode::Enter => {
            model.expanded = !model.expanded;
            Action::None
        }
        KeyCode::Tab => {
            model.pane = match model.pane {
                Pane::Repos => Pane::Worktrees,
                Pane::Worktrees => Pane::Repos,
            };
            Action::None
        }
        KeyCode::Char('/') => {
            model.filter.get_or_insert_with(String::new);
            model.filter_editing = true;
            Action::None
        }
        KeyCode::Esc => {
            model.filter = None;
            model.show_help = false;
            Action::None
        }
        KeyCode::Char('?') => {
            model.show_help = !model.show_help;
            Action::None
        }
        KeyCode::Char('c') => selected_worktree(model)
            .map(|w| Action::Copy(format!("cd -- {}", shell_quote(Path::new(&w.path)))))
            .unwrap_or(Action::None),
        KeyCode::Char('Y') => selected_worktree(model)
            .and_then(resume_target)
            .map(|t| Action::Copy(output::display_line(&t)))
            .unwrap_or(Action::None),
        KeyCode::Char('R') => selected_worktree(model)
            .and_then(resume_target)
            .map(Action::Resume)
            .unwrap_or(Action::None),
        KeyCode::Char('o') => match selected_worktree(model).map(|w| w.path.clone()) {
            Some(path) => editor_action(model, env, path),
            None => Action::None,
        },
        _ => Action::None,
    }
}

fn edit_filter(model: &mut Model, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            model.filter = None;
            model.filter_editing = false;
        }
        KeyCode::Enter => model.filter_editing = false,
        KeyCode::Backspace => {
            model.filter.get_or_insert_with(String::new).pop();
        }
        KeyCode::Char(c) => model.filter.get_or_insert_with(String::new).push(c),
        _ => return,
    }
    // The visible repos changed, so the old cursor may point at a different repo.
    model.selected = 0;
    model.selected_worktree = 0;
}

fn step(cursor: usize, len: usize, down: bool) -> usize {
    let last = len.saturating_sub(1);
    if down {
        (cursor + 1).min(last)
    } else {
        cursor.min(last).saturating_sub(1)
    }
}

fn move_cursor(model: &mut Model, down: bool) -> Action {
    match model.pane {
        Pane::Repos => {
            let next = step(model.selected, visible_repos(model).len(), down);
            if next != model.selected {
                model.selected = next;
                model.selected_worktree = 0;
                model.expanded = false;
            }
        }
        Pane::Worktrees => {
            let len = selected_repo_worktrees(model);
            model.selected_worktree = step(model.selected_worktree, len, down);
        }
    }
    Action::None
}

fn selected_repo_worktrees(model: &Model) -> usize {
    let repos = visible_repos(model);
    repos
        .get(model.selected.min(repos.len().saturating_sub(1)))
        .map_or(0, |r| r.worktrees.len())
}

/// The worktree under the cursor, clamped the same way `tui::draw` clamps it.
fn selected_worktree(model: &Model) -> Option<&Worktree> {
    let repos = visible_repos(model);
    let repo = repos.get(model.selected.min(repos.len().saturating_sub(1)))?;
    repo.worktrees.get(
        model
            .selected_worktree
            .min(repo.worktrees.len().saturating_sub(1)),
    )
}

fn resume_target(worktree: &Worktree) -> Option<ResumeTarget> {
    let session = worktree.last_session.as_ref()?;
    Some(ResumeTarget {
        source: session.source,
        id: Uuid::parse(&session.id)?,
        cwd: PathBuf::from(&session.cwd),
    })
}

/// `$VISUAL` before `$EDITOR`, split with shell-words rules, never run through a shell.
fn editor_action(model: &mut Model, env: &EnvPaths, path: String) -> Action {
    let Some(command) = env.visual.as_ref().or(env.editor.as_ref()) else {
        model.status_message = Some("set $VISUAL or $EDITOR to open a worktree".to_owned());
        return Action::None;
    };
    let words = match shell_words::split(&command.to_string_lossy()) {
        Ok(words) => words,
        Err(err) => {
            model.status_message = Some(format!("cannot parse the editor command: {err}"));
            return Action::None;
        }
    };
    let mut words = words.into_iter();
    let Some(program) = words.next() else {
        model.status_message = Some("the editor command is empty".to_owned());
        return Action::None;
    };
    Action::Spawn {
        program,
        args: words.chain([path]).collect(),
    }
}

/// Runs `write`; on failure sets `model.status_message`.
pub fn apply_copy(model: &mut Model, text: &str, write: impl FnOnce(&str) -> Result<(), String>) {
    model.status_message = Some(match write(text) {
        Ok(()) => "copied".to_owned(),
        Err(err) => format!("copy failed: {err}"),
    });
}

fn write_clipboard(clipboard: Clipboard, text: &str) -> Result<(), String> {
    match clipboard {
        Clipboard::System => arboard::Clipboard::new()
            .and_then(|mut board| board.set_text(text))
            .map_err(|e| e.to_string()),
        Clipboard::Osc52 => {
            let mut out = io::stdout();
            out.write_all(osc52(text).as_bytes())
                .and_then(|()| out.flush())
                .map_err(|e| e.to_string())
        }
    }
}

/// OSC 52: ask the terminal to set the clipboard (`c`) to the base64 payload.
fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard padded base64; the only use is OSC 52, so it is not worth a dependency.
fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bits = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, &b)| acc | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(
                    BASE64_ALPHABET[(bits >> (18 - 6 * i)) as usize & 63],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use ratatui::crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
    use rollcall_core::{
        Repo, RepoKind, SessionOut, SourceKind, StatusDoc, Timestamp, TitleKind, Uuid, Worktree,
    };

    use super::*;
    use crate::tui::{Pane, visible_repos};

    const SESSION_ID: &str = "0f6b1c1e-4c6e-4a4e-9d3a-2b1f0c9d8e7f";

    fn now() -> Timestamp {
        Timestamp::from_second(1_800_000_000).unwrap()
    }

    fn worktree(path: &str, branch: &str) -> Worktree {
        Worktree {
            path: path.to_owned(),
            is_main: false,
            external: false,
            branch: Some(branch.to_owned()),
            detached: false,
            head: None,
            last_commit_at: None,
            upstream: None,
            ahead: None,
            behind: None,
            unpushed: None,
            dirty_count: 0,
            untracked_count: 0,
            stale_reason: None,
            attention: 0,
            error: None,
            last_session: None,
            older_sessions: 0,
        }
    }

    fn session(cwd: &str, id: &str) -> SessionOut {
        SessionOut {
            source: SourceKind::Claude,
            id: id.to_owned(),
            branch: None,
            cwd: cwd.to_owned(),
            started_at: now(),
            ended_at: now(),
            title: Some("t".to_owned()),
            title_kind: Some(TitleKind::Prompt),
            last_prompt: None,
            message_count: 1,
            resume: None,
        }
    }

    fn repo(name: &str, worktrees: Vec<Worktree>) -> Repo {
        Repo {
            name: name.to_owned(),
            path: format!("/r/{name}"),
            kind: RepoKind::Normal,
            contains_root: false,
            default_branch: None,
            has_remote: true,
            attention: 0,
            last_activity_at: None,
            error: None,
            worktrees,
            submodules: None,
        }
    }

    /// CRM (main, feat/x), sutra (main), agentrec (trunk).
    fn model() -> Model {
        let doc = StatusDoc {
            schema_version: 1,
            root: "/r".to_owned(),
            scanned_at: Some(now()),
            stale: false,
            partial: false,
            repos: vec![
                repo(
                    "CRM",
                    vec![worktree("/r/CRM", "main"), worktree("/r/CRM-x", "feat/x")],
                ),
                repo("sutra", vec![worktree("/r/sutra", "main")]),
                repo("agentrec", vec![worktree("/r/agentrec", "trunk")]),
            ],
        };
        Model::new(doc, now(), false)
    }

    fn with_session(mut m: Model, id: &str) -> Model {
        m.doc.repos[0].worktrees[0].last_session = Some(session("/r/CRM", id));
        m
    }

    fn press(m: &mut Model, code: KeyCode) -> Action {
        handle_key(
            m,
            &EnvPaths::default(),
            KeyEvent::new(code, KeyModifiers::NONE),
        )
    }

    fn ch(m: &mut Model, c: char) -> Action {
        press(m, KeyCode::Char(c))
    }

    fn env_editor(visual: Option<&str>, editor: Option<&str>) -> EnvPaths {
        EnvPaths {
            visual: visual.map(OsString::from),
            editor: editor.map(OsString::from),
            ..EnvPaths::default()
        }
    }

    fn open(m: &mut Model, env: &EnvPaths) -> Action {
        handle_key(
            m,
            env,
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE),
        )
    }

    #[test]
    fn jk_and_arrows_move_the_repo_cursor_and_clamp() {
        let mut m = model();
        ch(&mut m, 'j');
        press(&mut m, KeyCode::Down);
        assert_eq!(m.selected, 2);
        ch(&mut m, 'j');
        assert_eq!(m.selected, 2, "clamped at the last repo");
        ch(&mut m, 'k');
        press(&mut m, KeyCode::Up);
        press(&mut m, KeyCode::Up);
        assert_eq!(m.selected, 0, "clamped at the first repo");
    }

    #[test]
    fn changing_repo_resets_worktree_cursor_and_collapses() {
        let mut m = model();
        m.selected_worktree = 1;
        m.expanded = true;
        ch(&mut m, 'j');
        assert_eq!((m.selected, m.selected_worktree, m.expanded), (1, 0, false));
    }

    #[test]
    fn tab_switches_pane_and_jk_then_move_the_worktree_cursor() {
        let mut m = model();
        press(&mut m, KeyCode::Tab);
        assert_eq!(m.pane, Pane::Worktrees);
        ch(&mut m, 'j');
        ch(&mut m, 'j');
        assert_eq!(
            (m.selected, m.selected_worktree),
            (0, 1),
            "clamped to 2 worktrees"
        );
        press(&mut m, KeyCode::Tab);
        assert_eq!(m.pane, Pane::Repos);
    }

    #[test]
    fn enter_toggles_expanded() {
        let mut m = model();
        press(&mut m, KeyCode::Enter);
        assert!(m.expanded);
        press(&mut m, KeyCode::Enter);
        assert!(!m.expanded);
    }

    #[test]
    fn slash_filters_case_insensitively_over_repo_names_and_branches() {
        let mut m = model();
        ch(&mut m, '/');
        assert!(m.filter_editing);
        for c in "SUT".chars() {
            ch(&mut m, c);
        }
        assert_eq!(visible_repos(&m).len(), 1);
        // A branch match: "FEAT" only appears in CRM's branch.
        press(&mut m, KeyCode::Esc);
        ch(&mut m, '/');
        for c in "FEAT".chars() {
            ch(&mut m, c);
        }
        let names: Vec<_> = visible_repos(&m).iter().map(|r| r.name.clone()).collect();
        assert_eq!(names, ["CRM"]);
        press(&mut m, KeyCode::Backspace);
        assert_eq!(m.filter.as_deref(), Some("FEA"));
    }

    #[test]
    fn filter_editing_consumes_command_keys_and_enter_keeps_the_filter() {
        let mut m = model();
        ch(&mut m, '/');
        assert_eq!(ch(&mut m, 'q'), Action::None, "q is text while editing");
        assert_eq!(m.filter.as_deref(), Some("q"));
        press(&mut m, KeyCode::Enter);
        assert!(!m.filter_editing);
        assert_eq!(m.filter.as_deref(), Some("q"));
        assert_eq!(ch(&mut m, 'q'), Action::Quit);
    }

    #[test]
    fn esc_clears_the_filter_and_editing_and_help() {
        let mut m = model();
        ch(&mut m, '/');
        ch(&mut m, 'x');
        press(&mut m, KeyCode::Esc);
        assert_eq!((m.filter.clone(), m.filter_editing), (None, false));
        m.filter = Some("kept".to_owned());
        m.show_help = true;
        press(&mut m, KeyCode::Esc);
        assert_eq!((m.filter.clone(), m.show_help), (None, false));
    }

    #[test]
    fn r_rescans_question_mark_toggles_help_q_and_ctrl_c_quit() {
        let mut m = model();
        assert_eq!(ch(&mut m, 'r'), Action::Rescan);
        ch(&mut m, '?');
        assert!(m.show_help);
        ch(&mut m, '?');
        assert!(!m.show_help);
        assert_eq!(ch(&mut m, 'q'), Action::Quit);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(
            handle_key(&mut m, &EnvPaths::default(), ctrl_c),
            Action::Quit
        );
    }

    #[test]
    fn key_release_events_are_ignored() {
        let mut m = model();
        let mut release = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert_eq!(
            handle_key(&mut m, &EnvPaths::default(), release),
            Action::None
        );
    }

    #[test]
    fn c_copies_a_shell_quoted_cd_for_the_selected_worktree() {
        let mut m = model();
        m.doc.repos[0].worktrees[1].path = "/r/it's here".to_owned();
        m.selected_worktree = 1;
        assert_eq!(
            ch(&mut m, 'c'),
            Action::Copy(r"cd -- '/r/it'\''s here'".to_owned())
        );
    }

    #[test]
    fn c_with_no_repos_does_nothing() {
        let mut m = model();
        m.doc.repos.clear();
        assert_eq!(ch(&mut m, 'c'), Action::None);
    }

    #[test]
    fn apply_copy_failure_sets_the_status_message_and_success_reports_it() {
        let mut m = model();
        apply_copy(&mut m, "x", |_| Err("no clipboard".to_owned()));
        assert_eq!(
            m.status_message.as_deref(),
            Some("copy failed: no clipboard")
        );
        let mut written = String::new();
        apply_copy(&mut m, "cd -- 'x'", |t| {
            written = t.to_owned();
            Ok(())
        });
        assert_eq!(written, "cd -- 'x'");
        assert_eq!(m.status_message.as_deref(), Some("copied"));
    }

    #[test]
    fn o_splits_visual_without_a_shell_and_appends_the_path() {
        let mut m = model();
        assert_eq!(
            open(&mut m, &env_editor(Some("code --wait"), None)),
            Action::Spawn {
                program: "code".to_owned(),
                args: vec!["--wait".to_owned(), "/r/CRM".to_owned()],
            }
        );
    }

    #[test]
    fn o_prefers_visual_over_editor_and_falls_back_to_editor() {
        let mut m = model();
        let both = open(&mut m, &env_editor(Some("code"), Some("vim")));
        assert!(matches!(both, Action::Spawn { ref program, .. } if program == "code"));
        let only_editor = open(&mut m, &env_editor(None, Some("vim -p")));
        assert_eq!(
            only_editor,
            Action::Spawn {
                program: "vim".to_owned(),
                args: vec!["-p".to_owned(), "/r/CRM".to_owned()],
            }
        );
    }

    #[test]
    fn o_with_no_editor_or_an_unparsable_one_sets_a_status_message() {
        let mut m = model();
        assert_eq!(open(&mut m, &env_editor(None, None)), Action::None);
        assert!(m.status_message.is_some());
        m.status_message = None;
        assert_eq!(
            open(&mut m, &env_editor(Some("vim 'oops"), None)),
            Action::None
        );
        assert!(m.status_message.is_some());
    }

    #[test]
    fn a_keypress_clears_the_previous_status_message() {
        let mut m = model();
        m.status_message = Some("old".to_owned());
        ch(&mut m, 'j');
        assert_eq!(m.status_message, None);
    }

    #[test]
    fn capital_r_resumes_the_selected_worktrees_last_session() {
        let mut m = with_session(model(), SESSION_ID);
        assert_eq!(
            ch(&mut m, 'R'),
            Action::Resume(ResumeTarget {
                source: SourceKind::Claude,
                id: Uuid::parse(SESSION_ID).unwrap(),
                cwd: PathBuf::from("/r/CRM"),
            })
        );
    }

    #[test]
    fn r_and_y_do_nothing_without_a_session_or_with_a_malformed_id() {
        let mut m = model();
        assert_eq!(ch(&mut m, 'R'), Action::None);
        assert_eq!(ch(&mut m, 'Y'), Action::None);
        let mut bad = with_session(model(), "not-a-uuid");
        assert_eq!(ch(&mut bad, 'R'), Action::None);
        assert_eq!(ch(&mut bad, 'Y'), Action::None);
    }

    #[test]
    fn capital_y_copies_the_display_resume_line() {
        let mut m = with_session(model(), SESSION_ID);
        assert_eq!(
            ch(&mut m, 'Y'),
            Action::Copy(format!("cd -- '/r/CRM' && claude --resume {SESSION_ID}"))
        );
    }

    #[test]
    fn base64_matches_the_rfc_4648_vectors() {
        for (raw, enc) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(raw.as_bytes()), enc, "{raw:?}");
        }
    }

    #[test]
    fn osc52_wraps_the_base64_payload_for_the_clipboard_selection() {
        assert_eq!(osc52("foo"), "\x1b]52;c;Zm9v\x07");
    }
}
