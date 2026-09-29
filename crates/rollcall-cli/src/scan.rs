use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use rollcall_store::SqliteIndex;

use crate::config::Ctx;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    /// Scan, and wait for it.
    Fresh,
    /// Answer from the cache only.
    NoScan,
    /// Scan, but answer stale from the cache after this long (MCP, §6.4).
    WaitAtMost(Duration),
}

pub struct ScanOutcome {
    pub index: SqliteIndex,
    /// The answer did not come from a scan completed by this invocation.
    pub stale: bool,
    pub exit: ExitCode,
}

pub fn scan(_ctx: &Ctx, _root: &Path, _mode: ScanMode) -> ScanOutcome {
    unimplemented!()
}
