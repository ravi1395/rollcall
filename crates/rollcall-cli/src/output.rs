use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use rollcall_core::{Index, Session, SourceKind, StatusDoc, Uuid};
use rollcall_sessions::SessionSource;

use crate::config::Ctx;
use crate::scan::ScanOutcome;

/// What log-derived text may be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPolicy {
    /// Show prompt-derived text: `last_prompt`, and titles of kind `prompt` (MCP `--prompts`;
    /// always true for the CLI).
    pub prompts: bool,
    /// `redact_prompts`: hide all title and prompt text; overrides `prompts`.
    pub redact: bool,
}

/// What `R` and the `rc` function resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeTarget {
    pub source: SourceKind,
    pub id: Uuid,
    pub cwd: PathBuf,
}

/// The §8.1 `Y` line: `cd -- '<cwd>' && claude --resume <id>`.
pub fn display_line(_target: &ResumeTarget) -> String {
    unimplemented!()
}

pub fn render_session(
    _session: &Session,
    _source: &dyn SessionSource,
    _policy: TextPolicy,
) -> rollcall_core::SessionOut {
    unimplemented!()
}

pub fn build_status(
    _ctx: &Ctx,
    _index: &dyn Index,
    _root: &Path,
    _stale: bool,
    _policy: TextPolicy,
) -> StatusDoc {
    unimplemented!()
}

pub fn status(
    _ctx: &Ctx,
    _outcome: &ScanOutcome,
    _root: &Path,
    _json: bool,
    _out: &mut dyn Write,
) -> ExitCode {
    unimplemented!()
}

pub fn doctor(
    _ctx: &Ctx,
    _outcome: &ScanOutcome,
    _root: &Path,
    _json: bool,
    _out: &mut dyn Write,
) -> ExitCode {
    unimplemented!()
}

/// Prints the one-line `scanned <root>: …` summary (§3.1) to `err`.
pub fn scan_summary(
    _ctx: &Ctx,
    _outcome: &ScanOutcome,
    _root: &Path,
    _elapsed: Duration,
    _err: &mut dyn Write,
) -> ExitCode {
    unimplemented!()
}

pub fn cache_clear(_ctx: &Ctx, _out: &mut dyn Write) -> ExitCode {
    unimplemented!()
}
