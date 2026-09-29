use std::path::{Path, PathBuf};

use rollcall_core::Result;

use crate::ScanLock;

pub fn cache_dir(_home: &Path, _xdg_cache_home: Option<&Path>) -> PathBuf {
    unimplemented!()
}

pub fn ensure_cache_dir(_dir: &Path) -> Result<()> {
    unimplemented!()
}

pub fn try_scan_lock(_cache_dir: &Path, _root: &Path) -> Result<Option<ScanLock>> {
    unimplemented!()
}

pub fn clear(_cache_dir: &Path) -> Result<()> {
    unimplemented!()
}

pub fn effective_uid(_cache_dir: &Path) -> Result<u32> {
    unimplemented!()
}
