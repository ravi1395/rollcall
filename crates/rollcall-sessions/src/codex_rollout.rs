//! Codex source dispatch: the thread DB (`codex_db`) when present, else rollout files.

use std::path::Path;

use rollcall_core::{SourceScan, SourceState, TextRef};

pub fn scan(_codex_home: &Path, _prior: &SourceState) -> SourceScan {
    unimplemented!()
}

pub fn resolve(_codex_home: &Path, _r: &TextRef) -> Option<String> {
    unimplemented!()
}
