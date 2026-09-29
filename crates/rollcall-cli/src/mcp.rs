use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use crate::config::Ctx;

/// Serves JSON-RPC 2.0 over `input` / `output` until `input` closes (§8.3).
pub fn serve(
    _ctx: &Ctx,
    _roots: Vec<PathBuf>,
    _prompts: bool,
    _input: impl BufRead,
    _output: impl Write,
) -> ExitCode {
    unimplemented!()
}
