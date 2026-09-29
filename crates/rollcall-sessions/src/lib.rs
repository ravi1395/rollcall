use std::path::PathBuf;

use rollcall_core::{SourceKind, SourceScan, SourceState, TextRef};

pub mod claude;
pub mod codex_db;
pub mod codex_rollout;
pub mod jsonl;

/// Bumped whenever extraction changes; a mismatch forces a full re-read (§6.3).
pub const PARSER_VERSION: u32 = 1;

pub trait SessionSource {
    fn kind(&self) -> SourceKind;
    /// Owns incremental merge (§6.3).
    fn scan(&self, prior: &SourceState) -> SourceScan;
    /// Raw text; the caller sanitises, scrubs and truncates.
    fn resolve(&self, r: &TextRef) -> Option<String>;
}

pub struct Claude {
    projects_dir: PathBuf,
}

impl Claude {
    pub fn new(projects_dir: PathBuf) -> Claude {
        Claude { projects_dir }
    }
}

impl SessionSource for Claude {
    fn kind(&self) -> SourceKind {
        SourceKind::Claude
    }

    fn scan(&self, prior: &SourceState) -> SourceScan {
        claude::scan(&self.projects_dir, prior)
    }

    fn resolve(&self, r: &TextRef) -> Option<String> {
        claude::resolve(&self.projects_dir, r)
    }
}

pub struct Codex {
    codex_home: PathBuf,
}

impl Codex {
    pub fn new(codex_home: PathBuf) -> Codex {
        Codex { codex_home }
    }
}

impl SessionSource for Codex {
    fn kind(&self) -> SourceKind {
        SourceKind::Codex
    }

    fn scan(&self, prior: &SourceState) -> SourceScan {
        codex_rollout::scan(&self.codex_home, prior)
    }

    fn resolve(&self, r: &TextRef) -> Option<String> {
        codex_rollout::resolve(&self.codex_home, r)
    }
}
