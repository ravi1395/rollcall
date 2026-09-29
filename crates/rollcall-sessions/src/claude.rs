use std::path::Path;

use rollcall_core::{SourceScan, SourceState, TextRef};

pub fn scan(_projects_dir: &Path, _prior: &SourceState) -> SourceScan {
    unimplemented!()
}

pub fn resolve(_projects_dir: &Path, _r: &TextRef) -> Option<String> {
    unimplemented!()
}
