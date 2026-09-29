use std::path::Path;
use std::process::ExitCode;

use ratatui::crossterm::event::KeyEvent;

use crate::config::{Ctx, EnvPaths};
use crate::output::ResumeTarget;
use crate::tui::Model;

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
pub fn run(_ctx: Ctx, _root: &Path) -> ExitCode {
    unimplemented!()
}

pub fn handle_key(_model: &mut Model, _env: &EnvPaths, _key: KeyEvent) -> Action {
    unimplemented!()
}

/// Runs `write`; on failure sets `model.status_message`.
pub fn apply_copy(
    _model: &mut Model,
    _text: &str,
    _write: impl FnOnce(&str) -> Result<(), String>,
) {
    unimplemented!()
}
