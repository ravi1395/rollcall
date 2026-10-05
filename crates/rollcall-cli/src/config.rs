use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use rollcall_core::Timestamp;
use serde::Deserialize;

use crate::cli::CommonFlags;
use crate::scan::canonical_lenient;

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
    /// §3.2 `ignore`: absolute, realpath'd by `load` (`~` expanded); empty until then.
    pub ignore: Vec<PathBuf>,
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
            ignore: Vec::new(),
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

/// Every key a config file may set; flag-only fields of `Config` are deliberately absent.
const CONFIG_KEYS: [&str; 10] = [
    "roots",
    "skip_dirs",
    "ignore",
    "stale_days",
    "redact_prompts",
    "redact_emails",
    "walk_budget_entries",
    "walk_budget_secs",
    "repo_budget_secs",
    "clipboard",
];

/// `$XDG_CONFIG_HOME/rollcall/config.toml`, else `~/.config/rollcall/config.toml`. Like the
/// cache directory, a relative `XDG_CONFIG_HOME` is invalid per the XDG spec and ignored.
fn config_path(env: &EnvPaths) -> Option<PathBuf> {
    let base = match env.xdg_config_home.as_ref().filter(|p| p.is_absolute()) {
        Some(xdg) => xdg.clone(),
        None => env.home.as_ref()?.join(".config"),
    };
    Some(base.join("rollcall/config.toml"))
}

fn unknown_keys(table: &toml::Table) -> Vec<&str> {
    table
        .keys()
        .map(String::as_str)
        .filter(|k| !CONFIG_KEYS.contains(k))
        .collect()
}

/// §3.2: finite, > 0, and representable as a `Duration` (rejects `nan`, `inf`, `1e30`).
fn check_repo_budget(secs: f64) -> Result<(), UsageError> {
    if secs > 0.0 && std::time::Duration::try_from_secs_f64(secs).is_ok() {
        return Ok(());
    }
    Err(UsageError(format!(
        "config: repo_budget_secs must be a finite number > 0 that fits a duration, got {secs}"
    )))
}

/// §3.2 `ignore`: `~` or `~/…` is HOME, anything else must already be absolute (`~user` and
/// relative entries are config errors); then realpath where it exists, else lexically normalised.
fn resolve_ignore(entries: &[PathBuf], home: &Path) -> Result<Vec<PathBuf>, UsageError> {
    let resolve = |entry: &PathBuf| {
        let expanded = entry
            .strip_prefix("~")
            .map_or_else(|_| entry.clone(), |rest| home.join(rest));
        if expanded.is_absolute() {
            return Ok(canonical_lenient(&expanded));
        }
        Err(UsageError(format!(
            "config: ignore entry {entry:?} must be an absolute path or start with ~/"
        )))
    };
    entries.iter().map(resolve).collect()
}

fn read_config(env: &EnvPaths) -> Result<Config, UsageError> {
    let path = config_path(env)
        .ok_or_else(|| UsageError("cannot locate the config file: HOME is not set".to_string()))?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(UsageError(format!("cannot read {}: {e}", path.display()))),
    };
    let invalid =
        |e: toml::de::Error| UsageError(format!("invalid config {}: {e}", path.display()));
    let table: toml::Table = text.parse().map_err(invalid)?;
    for key in unknown_keys(&table) {
        eprintln!(
            "rollcall: warning: unknown config key `{key}` in {}",
            path.display()
        );
    }
    let config: Config = table.try_into().map_err(invalid)?;
    check_repo_budget(config.repo_budget_secs)?;
    Ok(config)
}

fn resolve_now(env: &EnvPaths) -> Result<Timestamp, UsageError> {
    let Some(raw) = &env.rollcall_now else {
        return Ok(Timestamp::now());
    };
    raw.to_str().and_then(|s| s.parse().ok()).ok_or_else(|| {
        UsageError(format!(
            "ROLLCALL_NOW is not an RFC 3339 timestamp: {raw:?}"
        ))
    })
}

/// Reads the config file, applies `flags` over it, resolves "now" and the cache directory.
pub fn load(flags: &CommonFlags, env: EnvPaths) -> Result<Ctx, UsageError> {
    let mut config = read_config(&env)?;
    config.submodules = flags.walk.submodules;
    config.follow_symlinks = flags.walk.follow_symlinks;
    config.yes = flags.yes;
    let now = resolve_now(&env)?;
    // The config file can sit under an absolute XDG_CONFIG_HOME, but the cache needs HOME.
    let home = env
        .home
        .as_deref()
        .ok_or_else(|| UsageError("HOME is not set".to_string()))?;
    config.ignore = resolve_ignore(&config.ignore, home)?;
    let cache_dir = rollcall_store::cache_dir(home, env.xdg_cache_home.as_deref());
    Ok(Ctx {
        config,
        now,
        env,
        cache_dir,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;
    use crate::cli::WalkFlags;

    /// An env whose HOME is a tempdir, with `toml` (if any) at the default config path.
    fn env_with(toml: Option<&str>) -> (TempDir, EnvPaths) {
        let home = TempDir::new().unwrap();
        let env = EnvPaths {
            home: Some(home.path().to_path_buf()),
            ..EnvPaths::default()
        };
        if let Some(text) = toml {
            write_config(&env, text);
        }
        (home, env)
    }

    fn write_config(env: &EnvPaths, text: &str) {
        let path = config_path(env).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn load_toml(text: &str) -> Result<Ctx, UsageError> {
        let (_home, env) = env_with(Some(text));
        load(&CommonFlags::default(), env)
    }

    #[test]
    fn missing_file_gives_the_defaults() {
        let (_home, env) = env_with(None);
        let ctx = load(&CommonFlags::default(), env).unwrap();
        assert_eq!(ctx.config, Config::default());
    }

    #[test]
    fn every_key_is_read() {
        let ctx = load_toml(
            r#"
roots = ["/a", "/b"]
skip_dirs = ["vendor"]
stale_days = 30
redact_prompts = true
redact_emails = true
walk_budget_entries = 10
walk_budget_secs = 3
repo_budget_secs = 0.5
clipboard = "osc52"
"#,
        )
        .unwrap();
        let want = Config {
            roots: vec!["/a".into(), "/b".into()],
            skip_dirs: vec!["vendor".into()],
            stale_days: 30,
            redact_prompts: true,
            redact_emails: true,
            walk_budget_entries: 10,
            walk_budget_secs: 3,
            repo_budget_secs: 0.5,
            clipboard: Clipboard::Osc52,
            ..Config::default()
        };
        assert_eq!(ctx.config, want);
    }

    #[test]
    fn unknown_keys_warn_and_the_run_continues() {
        let text = "stale_days = 3\nbogus = 1\nsubmodules = true\n";
        let table: toml::Table = text.parse().unwrap();
        assert_eq!(unknown_keys(&table), ["bogus", "submodules"]);
        let ctx = load_toml(text).unwrap();
        assert_eq!(ctx.config.stale_days, 3);
        assert!(
            !ctx.config.submodules,
            "a flag-only field is never read from TOML"
        );
    }

    #[test]
    fn ignore_is_a_known_key() {
        let table: toml::Table = "ignore = [\"/a\"]\n".parse().unwrap();
        assert!(unknown_keys(&table).is_empty());
    }

    #[test]
    fn ignore_entries_expand_tilde_and_resolve_to_realpaths() {
        let (home, env) = env_with(None);
        let real_home = std::fs::canonicalize(home.path()).unwrap();
        std::fs::create_dir(real_home.join("real")).unwrap();
        std::os::unix::fs::symlink(real_home.join("real"), real_home.join("link")).unwrap();
        write_config(
            &env,
            &format!(
                "ignore = [\"~\", \"~/gone\", \"{h}/link\", \"/no-such-rollcall-dir/x/../y\", \"{h}/real/\"]",
                h = home.path().display()
            ),
        );
        let ignore = load(&CommonFlags::default(), env).unwrap().config.ignore;
        assert_eq!(
            ignore,
            [
                real_home.clone(),
                real_home.join("gone"),
                real_home.join("real"),
                PathBuf::from("/no-such-rollcall-dir/y"),
                real_home.join("real"),
            ]
        );
    }

    #[test]
    fn ignore_entries_that_are_not_absolute_after_expansion_are_usage_errors() {
        for bad in [
            "ignore = \"x\"",
            "ignore = [1]",
            "ignore = [\"rel/x\"]",
            "ignore = [\"\"]",
            "ignore = [\"~bob/x\"]",
            "ignore = [\"~bob\"]",
            "ignore = [\"./x\"]",
        ] {
            assert!(load_toml(bad).is_err(), "accepted: {bad}");
        }
        let err = load_toml("ignore = [\"~bob/x\"]").unwrap_err();
        assert!(
            err.0.contains("ignore") && err.0.contains("~bob/x"),
            "{err}"
        );
    }

    #[test]
    fn invalid_toml_and_wrong_types_are_usage_errors() {
        for text in [
            "stale_days = ",
            "stale_days = \"7\"",
            "stale_days = -1",
            "roots = \"/a\"",
            "redact_prompts = 1",
            "clipboard = \"paste\"",
            "repo_budget_secs = \"5\"",
        ] {
            assert!(load_toml(text).is_err(), "accepted: {text}");
        }
    }

    #[test]
    fn repo_budget_must_be_positive_finite_and_a_duration() {
        for bad in ["0", "0.0", "-1.5", "nan", "inf", "-inf", "1e30"] {
            let err = load_toml(&format!("repo_budget_secs = {bad}")).unwrap_err();
            assert!(err.0.contains("repo_budget_secs"), "{bad}: {err}");
        }
        for (good, want) in [("0.25", 0.25), ("5", 5.0), ("3600.0", 3600.0)] {
            let ctx = load_toml(&format!("repo_budget_secs = {good}")).unwrap();
            assert_eq!(ctx.config.repo_budget_secs, want);
        }
    }

    #[test]
    fn rollcall_now_must_be_rfc_3339() {
        let (_home, mut env) = env_with(None);
        env.rollcall_now = Some("2026-09-30T12:00:00Z".into());
        let ctx = load(&CommonFlags::default(), env.clone()).unwrap();
        assert_eq!(
            ctx.now,
            "2026-09-30T12:00:00Z".parse::<Timestamp>().unwrap()
        );
        for bad in ["yesterday", "2026-09-30", "1700000000", ""] {
            env.rollcall_now = Some(bad.into());
            assert!(
                load(&CommonFlags::default(), env.clone()).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn unset_rollcall_now_uses_the_clock() {
        let (_home, env) = env_with(None);
        let before = Timestamp::now();
        let ctx = load(&CommonFlags::default(), env).unwrap();
        assert!(ctx.now >= before);
    }

    #[test]
    fn flags_are_applied_and_config_roots_are_kept() {
        let (_home, env) = env_with(Some("roots = [\"/r\"]"));
        let flags = CommonFlags {
            walk: WalkFlags {
                submodules: true,
                follow_symlinks: true,
            },
            yes: true,
            no_scan: false,
        };
        let cfg = load(&flags, env).unwrap().config;
        assert!(cfg.submodules && cfg.follow_symlinks && cfg.yes);
        assert_eq!(cfg.roots, [PathBuf::from("/r")]);
    }

    #[test]
    fn config_path_prefers_an_absolute_xdg_config_home() {
        let env = EnvPaths {
            home: Some("/home/u".into()),
            xdg_config_home: Some("/xdg".into()),
            ..EnvPaths::default()
        };
        assert_eq!(
            config_path(&env).unwrap(),
            Path::new("/xdg/rollcall/config.toml")
        );
        let default = Path::new("/home/u/.config/rollcall/config.toml");
        let unset = EnvPaths {
            xdg_config_home: None,
            ..env.clone()
        };
        assert_eq!(config_path(&unset).unwrap(), default);
        let relative = EnvPaths {
            xdg_config_home: Some("rel".into()),
            ..env
        };
        assert_eq!(config_path(&relative).unwrap(), default);
    }

    #[test]
    fn the_file_is_read_from_xdg_config_home() {
        let (xdg, mut env) = env_with(None);
        env.xdg_config_home = Some(xdg.path().join("cfg"));
        write_config(&env, "stale_days = 99");
        let ctx = load(&CommonFlags::default(), env).unwrap();
        assert_eq!(ctx.config.stale_days, 99);
    }

    #[test]
    fn cache_dir_comes_from_the_store() {
        let (home, mut env) = env_with(None);
        env.xdg_cache_home = Some(home.path().join("xc"));
        let want = rollcall_store::cache_dir(home.path(), env.xdg_cache_home.as_deref());
        assert_eq!(load(&CommonFlags::default(), env).unwrap().cache_dir, want);
    }

    #[test]
    fn no_home_is_a_usage_error() {
        assert!(load(&CommonFlags::default(), EnvPaths::default()).is_err());
    }

    #[test]
    fn an_unreadable_config_path_is_a_usage_error_not_defaults() {
        let (_home, env) = env_with(None);
        let path = config_path(&env).unwrap();
        // A directory where the file should be.
        std::fs::create_dir_all(&path).unwrap();
        assert!(load(&CommonFlags::default(), env).is_err());
    }
}
