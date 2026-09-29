use ratatui::Frame;
use rollcall_core::{StatusDoc, Timestamp};

/// Which pane has focus (`tab` switches).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Pane {
    #[default]
    Repos,
    Worktrees,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub doc: StatusDoc,
    /// Cursor in the Repos pane: index into the repos that pass `filter`.
    pub selected: usize,
    /// Cursor in the worktrees pane: index into the selected repo's worktrees.
    pub selected_worktree: usize,
    /// `enter`: the selected worktree's older sessions are expanded.
    pub expanded: bool,
    pub pane: Pane,
    /// Case-insensitive substring over repo names and branches.
    pub filter: Option<String>,
    /// `/` was pressed and keys currently edit `filter`.
    pub filter_editing: bool,
    pub status_message: Option<String>,
    pub show_help: bool,
    pub no_color: bool,
    pub now: Timestamp,
    /// Repos found so far during a first scan; `None` once rows are available.
    pub scanning: Option<usize>,
}

impl Model {
    pub fn new(doc: StatusDoc, now: Timestamp, no_color: bool) -> Model {
        Model {
            doc,
            selected: 0,
            selected_worktree: 0,
            expanded: false,
            pane: Pane::Repos,
            filter: None,
            filter_editing: false,
            status_message: None,
            show_help: false,
            no_color,
            now,
            scanning: None,
        }
    }
}

pub fn draw(_frame: &mut Frame, _model: &Model) {
    unimplemented!()
}
