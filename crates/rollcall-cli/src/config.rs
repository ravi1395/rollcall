use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

use rollcall_core::Timestamp;
use serde::Deserialize;

use crate::cli::CommonFlags;

/// Every environment variable rollcall reads, captured once in `run`. Code never reads
/// the process environment directly, and tests build one of these instead of calling `set_var`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvPaths {
    pub home: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    pub xdg_cache_home: Option<PathBuf>,
    pub claude_config_dir: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
    /// Raw `PATH`; `resume::find_agent` keeps only its absolute entries.
    pub path: Option<OsString>,
    pub visual: Option<OsString>,
    pub editor: Option<OsString>,
    /// `NO_COLOR` set to a non-empty value.
    pub no_color: bool,
    pub resume_file: Option<PathBuf>,
    /// Raw `ROLLCALL_NOW`; `load` parses it as RFC 3339 and rejects anything else.
    pub rollcall_now: Option<OsString>,
}

impl EnvPaths {
    pub fn from_env() -> EnvPaths {
        // An empty value counts as unset, as the XDG spec says for its variables.
        let nonempty = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
        let path = |name: &str| nonempty(name).map(PathBuf::from);
        EnvPaths {
            home: path("HOME"),
            xdg_config_home: path("XDG_CONFIG_HOME"),
            xdg_cache_home: path("XDG_CACHE_HOME"),
            claude_config_dir: path("CLAUDE_CONFIG_DIR"),
            codex_home: path("CODEX_HOME"),
            path: std::env::var_os("PATH"),
            visual: nonempty("VISUAL"),
            editor: nonempty("EDITOR"),
            no_color: nonempty("NO_COLOR").is_some(),
            resume_file: path("ROLLCALL_RESUME_FILE"),
            rollcall_now: std::env::var_os("ROLLCALL_NOW"),
        }
    }
}

/// Where `copy` actions write (§3.2 `clipboard`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Clipboard {
    #[default]
    System,
    Osc52,
}

/// §3.2 keys with their defaults, then the command-line flags that override or extend them.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    pub roots: Vec<PathBuf>,
    pub skip_dirs: Vec<String>,
    pub stale_days: u32,
    pub redact_prompts: bool,
    pub redact_emails: bool,
    pub walk_budget_entries: u64,
    pub walk_budget_secs: u64,
    pub repo_budget_secs: f64,
    pub clipboard: Clipboard,
    /// `--submodules` (flags are not config keys, so never read from TOML).
    #[serde(skip)]
    pub submodules: bool,
    /// `--follow-symlinks`.
    #[serde(skip)]
    pub follow_symlinks: bool,
    /// `--yes`: allow a refused root (§4.3); config `roots` also count as confirmed.
    #[serde(skip)]
    pub yes: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            roots: Vec::new(),
            skip_dirs: Vec::new(),
            stale_days: 14,
            redact_prompts: false,
            redact_emails: false,
            walk_budget_entries: 500_000,
            walk_budget_secs: 20,
            repo_budget_secs: 5.0,
            clipboard: Clipboard::System,
            submodules: false,
            follow_symlinks: false,
            yes: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Ctx {
    pub config: Config,
    pub now: Timestamp,
    pub env: EnvPaths,
    pub cache_dir: PathBuf,
}

/// A usage or config error: printed to stderr, exit 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageError(pub String);

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

/// Reads the config file, applies `flags` over it, resolves "now" and the cache directory.
pub fn load(_flags: &CommonFlags, _env: EnvPaths) -> Result<Ctx, UsageError> {
    unimplemented!()
}
