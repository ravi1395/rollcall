use std::ffi::OsString;
use std::process::ExitCode;

use clap::Parser;

#[derive(Parser)]
#[command(name = "rollcall", version)]
struct Cli {}

/// P01 handles `--version` only; P04 adds every §3.1 command.
pub fn run<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    match Cli::try_parse_from(args) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = e.print();
            ExitCode::from(u8::try_from(e.exit_code()).unwrap_or(2))
        }
    }
}
