use std::fs::File;
use std::path::{Path, PathBuf};

use rollcall_core::{
    Index, RepoRow, Result, ScanDiagnostics, ScanResult, Session, SourceKind, SourceState,
    Timestamp,
};

pub mod fs;

/// The SQLite-backed `Index` (§6). P26 owns the body.
#[derive(Debug)]
pub struct SqliteIndex {
    _private: (),
}

impl SqliteIndex {
    pub fn open(_cache_dir: &Path) -> Result<SqliteIndex> {
        unimplemented!()
    }
}

impl Index for SqliteIndex {
    fn source_state(&self, _s: SourceKind) -> SourceState {
        unimplemented!()
    }

    fn repos_for_root(&self, _root: &Path) -> Vec<RepoRow> {
        unimplemented!()
    }

    fn sessions_for(&self, _root: &Path, _worktree: &Path) -> Vec<Session> {
        unimplemented!()
    }

    fn scanned_at(&self, _root: &Path) -> Option<Timestamp> {
        unimplemented!()
    }

    fn scanned_submodules(&self, _root: &Path) -> Option<bool> {
        unimplemented!()
    }

    fn last_diagnostics(&self, _root: &Path) -> Option<ScanDiagnostics> {
        unimplemented!()
    }

    fn commit_scan(&mut self, _root: &Path, _scan: ScanResult) -> Result<()> {
        unimplemented!()
    }
}

/// Holds the per-root scan lock until dropped.
#[derive(Debug)]
pub struct ScanLock {
    _file: File,
}

pub fn cache_dir(home: &Path, xdg_cache_home: Option<&Path>) -> PathBuf {
    fs::cache_dir(home, xdg_cache_home)
}

pub fn ensure_cache_dir(dir: &Path) -> Result<()> {
    fs::ensure_cache_dir(dir)
}

pub fn try_scan_lock(cache_dir: &Path, root: &Path) -> Result<Option<ScanLock>> {
    fs::try_scan_lock(cache_dir, root)
}

pub fn clear(cache_dir: &Path) -> Result<()> {
    fs::clear(cache_dir)
}

pub fn effective_uid(cache_dir: &Path) -> Result<u32> {
    fs::effective_uid(cache_dir)
}
