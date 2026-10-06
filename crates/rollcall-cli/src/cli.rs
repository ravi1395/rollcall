use std::ffi::OsString;
use std::io::{IsTerminal, Write};
use std::num::{IntErrorKind, NonZeroUsize};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::config::{self, Ctx, EnvPaths};
use crate::output::Render;
use crate::resume::Shell;
use crate::scan::{self, ScanMode};
use crate::{app, mcp, output, resume};

const EXIT_USAGE: u8 = 2;
const DEFAULT_ROOT: &str = ".";

#[derive(Parser, Debug)]
#[command(
    name = "rollcall",
    version,
    about = "Every repo, every worktree, and the agent session that left it dirty",
    // `rollcall [PATH] [flags]` is the TUI; those arguments do not mix with a subcommand.
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(flatten)]
    root: RootArgs,
    #[command(subcommand)]
    command: Option<Command>,
}

/// Flags shared by `--submodules` / `--follow-symlinks` commands (also `mcp`).
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct WalkFlags {
    /// Include submodules as repos.
    #[arg(long)]
    pub submodules: bool,
    /// Follow symlinks whose target is under the root.
    #[arg(long)]
    pub follow_symlinks: bool,
}

/// The shared flags of the TUI, `status`, `scan` and `doctor`; also what `config::load` takes.
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct CommonFlags {
    #[command(flatten)]
    pub walk: WalkFlags,
    /// Allow a refused root (`/` or `$HOME`).
    #[arg(long)]
    pub yes: bool,
    /// Answer from the cache only.
    #[arg(long)]
    pub no_scan: bool,
}

/// §8.2: the repo filters of plain `status` (accepted and ignored with `--json`).
/// §8.2 `--color`: when plain `status` paints its badges.
#[derive(ValueEnum, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ColorMode {
    /// Only when stdout is a terminal and `NO_COLOR` is unset or empty.
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusFilters {
    /// Only repos with a dirty, untracked or unpushed worktree.
    #[arg(long)]
    pub dirty: bool,
    /// Only repos with a stale worktree.
    #[arg(long)]
    pub stale: bool,
    /// Only the first N repos, after the other filters.
    #[arg(long, value_name = "N", value_parser = parse_top)]
    pub top: Option<NonZeroUsize>,
}

/// `--top N` past the largest count keeps every repo; `0`, a sign or a non-number is an error.
fn parse_top(s: &str) -> Result<NonZeroUsize, String> {
    match s.parse::<NonZeroUsize>() {
        Ok(n) => Ok(n),
        Err(e) if *e.kind() == IntErrorKind::PosOverflow => Ok(NonZeroUsize::MAX),
        Err(e) => Err(e.to_string()),
    }
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct RootArgs {
    /// Root directory to scan [default: the current directory].
    pub path: Option<PathBuf>,
    #[command(flatten)]
    pub flags: CommonFlags,
}

impl RootArgs {
    fn root(&self) -> PathBuf {
        self.path
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_ROOT))
    }
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
pub enum Command {
    /// Print repos, worktrees and their last sessions.
    // A repeated `status` flag takes its last value; scoped here so `mcp --root` still collects.
    #[command(args_override_self = true)]
    Status {
        #[command(flatten)]
        root: RootArgs,
        /// Print the StatusDoc as JSON.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        filters: StatusFilters,
        /// Colour the badges (accepted and ignored with `--json`).
        #[arg(long, value_enum, default_value_t)]
        color: ColorMode,
    },
    /// Scan and print one summary line to stderr.
    Scan {
        #[command(flatten)]
        root: RootArgs,
    },
    /// Print diagnostics.
    Doctor {
        #[command(flatten)]
        root: RootArgs,
        /// Print the DoctorDoc as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Serve read-only tools over stdio MCP.
    Mcp {
        /// A root to serve (repeatable); defaults to the config `roots`.
        #[arg(long = "root", value_name = "PATH")]
        roots: Vec<PathBuf>,
        /// Also return prompt-derived text.
        #[arg(long)]
        prompts: bool,
        #[command(flatten)]
        walk: WalkFlags,
    },
    /// Print the optional `rc` shell function.
    Init {
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Manage the cache.
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
    /// Internal: one repo's state for the scan's worker process (§4.4). Reads JSON on stdin,
    /// writes JSON on stdout; not a user command.
    #[command(name = "__worker", hide = true)]
    Worker,
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
pub enum CacheAction {
    /// Delete the cache directory.
    Clear,
}

/// What a parsed command line asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Target<'a> {
    Tui,
    Status,
    Subcommand(&'a Command),
}

/// §3.1: with no subcommand, `rollcall [PATH]` is the TUI, or `status` when stdout is not a TTY.
pub fn dispatch_target(stdout_is_tty: bool, command: Option<&Command>) -> Target<'_> {
    match command {
        Some(command) => Target::Subcommand(command),
        None if stdout_is_tty => Target::Tui,
        None => Target::Status,
    }
}

pub fn run<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    match Cli::try_parse_from(args) {
        Ok(cli) => dispatch(cli, EnvPaths::from_env(), std::io::stdout().is_terminal()),
        Err(e) => {
            // `--help` / `--version` exit 0; every other clap error is a usage error.
            let _ = e.print();
            ExitCode::from(u8::try_from(e.exit_code()).unwrap_or(EXIT_USAGE))
        }
    }
}

fn dispatch(cli: Cli, env: EnvPaths, stdout_is_tty: bool) -> ExitCode {
    match dispatch_target(stdout_is_tty, cli.command.as_ref()) {
        Target::Tui => with_ctx(&cli.root.flags, env, |ctx| app::run(ctx, &cli.root.root())),
        Target::Status => run_status(
            &cli.root,
            false,
            &StatusFilters::default(),
            ColorMode::Auto,
            stdout_is_tty,
            env,
        ),
        Target::Subcommand(command) => run_command(command, env, stdout_is_tty),
    }
}

fn run_command(command: &Command, env: EnvPaths, stdout_is_tty: bool) -> ExitCode {
    match command {
        // Before any config or context load: the worker reads nothing but its stdin.
        Command::Worker => scan::worker_main(),
        Command::Status {
            root,
            json,
            filters,
            color,
        } => run_status(root, *json, filters, *color, stdout_is_tty, env),
        Command::Scan { root } => with_ctx(&root.flags, env, |ctx| {
            let started = Instant::now();
            let outcome = scan::scan(&ctx, &root.root(), scan_mode(&root.flags));
            let elapsed = started.elapsed();
            output::scan_summary(
                &ctx,
                &outcome,
                &root.root(),
                elapsed,
                &mut std::io::stderr().lock(),
            )
        }),
        Command::Doctor { root, json } => with_ctx(&root.flags, env, |ctx| {
            let outcome = scan::scan(&ctx, &root.root(), scan_mode(&root.flags));
            output::doctor(
                &ctx,
                &outcome,
                &root.root(),
                *json,
                &mut std::io::stdout().lock(),
            )
        }),
        Command::Mcp {
            roots,
            prompts,
            walk,
        } => {
            let flags = CommonFlags {
                walk: walk.clone(),
                ..CommonFlags::default()
            };
            with_ctx(&flags, env, |ctx| {
                let roots = if roots.is_empty() {
                    ctx.config.roots.clone()
                } else {
                    roots.clone()
                };
                if roots.is_empty() {
                    eprintln!(
                        "rollcall: mcp needs a root: pass --root PATH or set `roots` in the config"
                    );
                    return ExitCode::from(EXIT_USAGE);
                }
                mcp::serve(
                    &ctx,
                    roots,
                    *prompts,
                    std::io::stdin().lock(),
                    std::io::stdout().lock(),
                )
            })
        }
        Command::Init { shell } => {
            match std::io::stdout().write_all(resume::init_script(*shell).as_bytes()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("rollcall: cannot write the init script: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Cache {
            action: CacheAction::Clear,
        } => with_ctx(&CommonFlags::default(), env, |ctx| {
            output::cache_clear(&ctx, &mut std::io::stderr().lock())
        }),
    }
}

fn run_status(
    root: &RootArgs,
    json: bool,
    filters: &StatusFilters,
    color: ColorMode,
    stdout_is_tty: bool,
    env: EnvPaths,
) -> ExitCode {
    with_ctx(&root.flags, env, |mut ctx| {
        ctx.status_filters = filters.clone();
        let outcome = scan::scan(&ctx, &root.root(), scan_mode(&root.flags));
        ctx.render = Render {
            color: match color {
                ColorMode::Always => true,
                ColorMode::Never => false,
                ColorMode::Auto => stdout_is_tty && !ctx.env.no_color,
            },
            // Read after the scan, which outlasts a resize, from stdout's own terminal (§8.2).
            width: if stdout_is_tty {
                rustix::termios::tcgetwinsize(std::io::stdout())
                    .map_or(0, |ws| usize::from(ws.ws_col))
            } else {
                0
            },
        };
        output::status(
            &ctx,
            &outcome,
            &root.root(),
            json,
            &mut std::io::stdout().lock(),
        )
    })
}

fn scan_mode(flags: &CommonFlags) -> ScanMode {
    if flags.no_scan {
        ScanMode::NoScan
    } else {
        ScanMode::Fresh
    }
}

/// Loads config and context, or reports the usage error (exit 2).
fn with_ctx(flags: &CommonFlags, env: EnvPaths, f: impl FnOnce(Ctx) -> ExitCode) -> ExitCode {
    match config::load(flags, env) {
        Ok(ctx) => f(ctx),
        Err(e) => {
            eprintln!("rollcall: {e}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;

    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("rollcall").chain(args.iter().copied()))
    }

    #[test]
    fn no_subcommand_on_a_tty_is_the_tui() {
        assert_eq!(dispatch_target(true, None), Target::Tui);
    }

    #[test]
    fn no_subcommand_off_a_tty_is_status() {
        assert_eq!(dispatch_target(false, None), Target::Status);
    }

    #[test]
    fn a_subcommand_is_dispatched_as_itself_on_or_off_a_tty() {
        let cli = parse(&["scan", "."]).unwrap();
        let command = cli.command.as_ref().unwrap();
        for tty in [true, false] {
            assert_eq!(
                dispatch_target(tty, Some(command)),
                Target::Subcommand(command)
            );
        }
    }

    #[test]
    fn unknown_flag_is_a_usage_error_with_exit_2() {
        let e = parse(&["status", "--frobnicate"]).expect_err("must fail");
        assert_eq!(e.kind(), ErrorKind::UnknownArgument);
        assert_eq!(e.exit_code(), 2);
        // Through `run` too, so the mapping to `ExitCode` is covered.
        assert_eq!(
            format!("{:?}", run(["rollcall", "status", "--frobnicate"])),
            format!("{:?}", ExitCode::from(2))
        );
    }

    #[test]
    fn help_and_version_exit_0() {
        for args in [&["--help"][..], &["--version"], &["status", "--help"]] {
            assert_eq!(parse(args).expect_err("displays").exit_code(), 0);
        }
    }

    #[test]
    fn every_shared_flag_parses_on_status() {
        let cli = parse(&[
            "status",
            "--json",
            "--submodules",
            "--follow-symlinks",
            "--yes",
            "--no-scan",
            "some/dir",
        ])
        .unwrap();
        let Some(Command::Status { root, json, .. }) = cli.command else {
            panic!("expected status");
        };
        assert!(json);
        assert_eq!(root.path, Some(PathBuf::from("some/dir")));
        assert_eq!(
            root.flags,
            CommonFlags {
                walk: WalkFlags {
                    submodules: true,
                    follow_symlinks: true
                },
                yes: true,
                no_scan: true,
            }
        );
    }

    #[test]
    fn color_defaults_to_auto_takes_three_values_and_rejects_the_rest() {
        let color_of = |args: &[&str]| match parse(args).unwrap().command {
            Some(Command::Status { color, .. }) => color,
            other => panic!("expected status: {other:?}"),
        };
        assert_eq!(color_of(&["status"]), ColorMode::Auto);
        assert_eq!(
            color_of(&["status", "--color", "always"]),
            ColorMode::Always
        );
        assert_eq!(color_of(&["status", "--color", "never"]), ColorMode::Never);
        assert_eq!(
            color_of(&["status", "--json", "--color=auto"]),
            ColorMode::Auto
        );
        let e = parse(&["status", "--color", "bogus"]).expect_err("must fail");
        assert_eq!(e.kind(), ErrorKind::InvalidValue);
        assert_eq!(e.exit_code(), 2);
        // Not a flag of the other commands.
        assert!(parse(&["scan", "--color", "always"]).is_err());
    }

    #[test]
    fn bare_word_is_the_root_path_not_a_command() {
        let cli = parse(&["projects", "--yes"]).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.root.path, Some(PathBuf::from("projects")));
        assert!(cli.root.flags.yes);
    }

    #[test]
    fn mcp_takes_repeatable_roots_and_no_yes_flag() {
        let cli = parse(&["mcp", "--root", "a", "--root", "b", "--prompts"]).unwrap();
        let Some(Command::Mcp { roots, prompts, .. }) = cli.command else {
            panic!("expected mcp");
        };
        assert_eq!(roots, [PathBuf::from("a"), PathBuf::from("b")]);
        assert!(prompts);
        assert!(parse(&["mcp", "--yes"]).is_err());
    }

    #[test]
    fn init_accepts_only_the_three_shells() {
        for shell in ["bash", "zsh", "fish"] {
            assert!(parse(&["init", shell]).is_ok(), "{shell}");
        }
        assert!(parse(&["init", "tcsh"]).is_err());
        assert!(parse(&["init"]).is_err());
    }

    #[test]
    fn the_worker_subcommand_parses_but_is_hidden_from_help() {
        assert_eq!(parse(&["__worker"]).unwrap().command, Some(Command::Worker));
        let help = parse(&["--help"]).expect_err("displays").to_string();
        assert!(help.contains("status"), "{help}");
        assert!(!help.contains("__worker"), "{help}");
    }

    #[test]
    fn cache_clear_parses() {
        let cli = parse(&["cache", "clear"]).unwrap();
        assert_eq!(
            cli.command,
            Some(Command::Cache {
                action: CacheAction::Clear
            })
        );
    }
}
