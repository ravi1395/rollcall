use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::config::EnvPaths;
use crate::output::ResumeTarget;
use clap::ValueEnum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

/// `Ok(code)` is returned only after the resume-file protocol (exit 10); the direct path
/// execs and does not return. `uid` is called only in the resume-file branch.
pub fn resume(
    _target: &ResumeTarget,
    _env: &EnvPaths,
    _uid: &dyn Fn() -> rollcall_core::Result<u32>,
) -> Result<ExitCode, String> {
    unimplemented!()
}

/// Resolves `name` against the absolute entries of `path_var` only.
pub fn find_agent(_path_var: &OsStr, _name: &str) -> Option<PathBuf> {
    unimplemented!()
}

/// The `rc` function for `rollcall init <shell>`.
pub fn init_script(_shell: Shell) -> &'static str {
    unimplemented!()
}
