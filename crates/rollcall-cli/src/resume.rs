use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use crate::config::EnvPaths;
use crate::output::ResumeTarget;
use clap::ValueEnum;
use rollcall_core::SourceKind;

/// Exit code telling the `rc` function to read the resume file (spec §3.1).
const EXIT_RESUME_HANDOFF: u8 = 10;
const RESUME_FILE_MODE: u32 = 0o600;
const PERMISSION_BITS: u32 = 0o7777;
const ANY_EXEC_BIT: u32 = 0o111;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Debug, PartialEq, Eq)]
enum Plan {
    Exec {
        bin: PathBuf,
        args: Vec<OsString>,
        cwd: PathBuf,
    },
    File(PathBuf),
}

/// `Ok(code)` is returned only after the resume-file protocol (exit 10); the direct path
/// execs and does not return. `uid` is called only in the resume-file branch.
pub fn resume(
    target: &ResumeTarget,
    env: &EnvPaths,
    uid: &dyn Fn() -> rollcall_core::Result<u32>,
) -> Result<ExitCode, String> {
    match plan(target, env)? {
        Plan::File(path) => {
            write_resume_file(&path, target, uid)?;
            Ok(ExitCode::from(EXIT_RESUME_HANDOFF))
        }
        Plan::Exec { bin, args, cwd } => {
            // `exec` returns only on failure.
            let err = Command::new(&bin).args(args).current_dir(cwd).exec();
            Err(format!("cannot run {}: {err}", bin.display()))
        }
    }
}

/// Every refusal that can happen before the process changes directory or hands off.
fn plan(target: &ResumeTarget, env: &EnvPaths) -> Result<Plan, String> {
    if let Some(path) = &env.resume_file {
        return Ok(Plan::File(path.clone()));
    }
    let (name, subcommand) = match target.source {
        SourceKind::Claude => ("claude", "--resume"),
        SourceKind::Codex => ("codex", "resume"),
    };
    if !target.cwd.is_dir() {
        return Err(format!(
            "session directory no longer exists: {}",
            target.cwd.display()
        ));
    }
    let bin = env
        .path
        .as_deref()
        .and_then(|path| find_agent(path, name))
        .ok_or_else(|| format!("no absolute path to `{name}` found in PATH"))?;
    Ok(Plan::Exec {
        bin,
        args: vec![subcommand.into(), target.id.as_str().into()],
        cwd: target.cwd.clone(),
    })
}

/// Spec §8.1: open without following links or blocking, verify on the fd, only then truncate.
fn write_resume_file(
    path: &Path,
    target: &ResumeTarget,
    uid: &dyn Fn() -> rollcall_core::Result<u32>,
) -> Result<(), String> {
    let refuse = |why: &str| format!("resume file {} refused: {why}", path.display());
    // No O_TRUNC: nothing is modified until the checks below pass.
    let mut file = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| refuse(&e.to_string()))?;
    let meta = file.metadata().map_err(|e| refuse(&e.to_string()))?;
    let me = uid().map_err(|e| refuse(&format!("cannot determine the effective uid: {e}")))?;
    if !meta.is_file() {
        return Err(refuse("not a regular file"));
    }
    if meta.uid() != me {
        return Err(refuse("not owned by the current user"));
    }
    if meta.nlink() != 1 {
        return Err(refuse("has more than one link"));
    }
    if meta.permissions().mode() & PERMISSION_BITS != RESUME_FILE_MODE {
        return Err(refuse("mode is not 0600"));
    }
    let source = match target.source {
        SourceKind::Claude => "claude",
        SourceKind::Codex => "codex",
    };
    let mut bytes = target.cwd.as_os_str().as_bytes().to_vec();
    for field in [source, target.id.as_str()] {
        bytes.push(0);
        bytes.extend_from_slice(field.as_bytes());
    }
    bytes.push(0);
    file.set_len(0)
        .and_then(|()| file.write_all(&bytes))
        .map_err(|e| refuse(&e.to_string()))
}

/// Resolves `name` against the absolute entries of `path_var` only.
pub fn find_agent(path_var: &OsStr, name: &str) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & ANY_EXEC_BIT != 0)
        })
}

/// The `rc` function for `rollcall init <shell>`.
pub fn init_script(shell: Shell) -> &'static str {
    match shell {
        Shell::Bash | Shell::Zsh => POSIX_RC,
        Shell::Fish => FISH_RC,
    }
}

// `code`, not `status`: `status` is read-only in zsh. The UUID test is a glob, not a regex, so
// the same text parses in bash and zsh.
const POSIX_RC: &str = r#"rc() {
  local f code cwd src id bin
  f=$(mktemp) || return 1
  ROLLCALL_RESUME_FILE=$f command rollcall "$@"
  code=$?
  if [ "$code" -ne 10 ]; then
    rm -f -- "$f"
    return "$code"
  fi
  if ! { IFS= read -r -d '' cwd && IFS= read -r -d '' src && IFS= read -r -d '' id; } < "$f"; then
    rm -f -- "$f"
    echo "rc: malformed resume file" >&2
    return 1
  fi
  rm -f -- "$f"
  case $id in
    [[:xdigit:]][[:xdigit:]][[:xdigit:]][[:xdigit:]][[:xdigit:]][[:xdigit:]][[:xdigit:]][[:xdigit:]]-[[:xdigit:]][[:xdigit:]][[:xdigit:]][[:xdigit:]]-[[:xdigit:]][[:xdigit:]][[:xdigit:]][[:xdigit:]]-[[:xdigit:]][[:xdigit:]][[:xdigit:]][[:xdigit:]]-[[:xdigit:]][[:xdigit:]][[:xdigit:]][[:xdigit:]][[:xdigit:]][[:xdigit:]][[:xdigit:]][[:xdigit:]][[:xdigit:]][[:xdigit:]][[:xdigit:]][[:xdigit:]]) ;;
    *) echo "rc: invalid session id" >&2; return 1 ;;
  esac
  case $src in
    claude | codex) ;;
    *) echo "rc: unknown agent" >&2; return 1 ;;
  esac
  bin=$(command -v "$src")
  case $bin in
    /*) ;;
    *) echo "rc: no absolute path to $src" >&2; return 1 ;;
  esac
  if [ "$src" = claude ]; then
    cd -- "$cwd" && "$bin" --resume "$id"
  else
    cd -- "$cwd" && "$bin" resume "$id"
  fi
}
"#;

const FISH_RC: &str = r#"function rc
    set -l f (mktemp); or return 1
    ROLLCALL_RESUME_FILE=$f command rollcall $argv
    set -l code $status
    if test $code -ne 10
        rm -f -- $f
        return $code
    end
    set -l cwd
    set -l src
    set -l id
    begin
        read -z cwd; and read -z src; and read -z id
    end < $f
    set -l read_status $status
    rm -f -- $f
    if test $read_status -ne 0
        echo "rc: malformed resume file" >&2
        return 1
    end
    if not string match -qr '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$' -- $id
        echo "rc: invalid session id" >&2
        return 1
    end
    if test "$src" != claude -a "$src" != codex
        echo "rc: unknown agent" >&2
        return 1
    end
    set -l bin (command -v $src)
    if not string match -q '/*' -- $bin
        echo "rc: no absolute path to $src" >&2
        return 1
    end
    if test "$src" = claude
        cd -- $cwd; and $bin --resume $id
    else
        cd -- $cwd; and $bin resume $id
    end
end
"#;

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use std::process::Command;
    use std::sync::mpsc;
    use std::time::Duration;

    use rollcall_core::{SourceKind, Uuid};
    use tempfile::TempDir;

    use super::*;

    const ID: &str = "0a1b2c3d-0000-4000-8000-0123456789ab";
    const WATCHDOG: Duration = Duration::from_secs(10);
    const SHELLS: [(Shell, &str, &str); 3] = [
        (Shell::Bash, "bash", "-n"),
        (Shell::Zsh, "zsh", "-n"),
        (Shell::Fish, "fish", "--no-execute"),
    ];

    fn target(cwd: &Path, source: SourceKind) -> ResumeTarget {
        ResumeTarget {
            source,
            id: Uuid::parse(ID).unwrap(),
            cwd: cwd.to_path_buf(),
        }
    }

    fn me(dir: &Path) -> u32 {
        std::fs::metadata(dir).unwrap().uid()
    }

    fn resume_file(dir: &TempDir, mode: u32, bytes: &[u8]) -> PathBuf {
        let path = dir.path().join("resume");
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    fn env_with_file(path: &Path) -> EnvPaths {
        EnvPaths {
            resume_file: Some(path.to_path_buf()),
            ..EnvPaths::default()
        }
    }

    fn run_resume(tmp: &TempDir, path: &Path, uid: u32) -> Result<ExitCode, String> {
        resume(
            &target(tmp.path(), SourceKind::Claude),
            &env_with_file(path),
            &|| Ok(uid),
        )
    }

    fn plant(dir: &Path, name: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn a_private_regular_file_gets_three_nul_fields_and_exit_10() {
        let tmp = tempfile::tempdir().unwrap();
        let path = resume_file(&tmp, 0o600, &[b'x'; 200]);
        let code = run_resume(&tmp, &path, me(tmp.path())).unwrap();
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(10)));
        let expected = format!("{}\0claude\0{ID}\0", tmp.path().display());
        assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
    }

    #[test]
    fn the_codex_source_is_written_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let path = resume_file(&tmp, 0o600, b"");
        let uid = me(tmp.path());
        let got = resume(
            &target(tmp.path(), SourceKind::Codex),
            &env_with_file(&path),
            &|| Ok(uid),
        );
        assert!(got.is_ok());
        let written = std::fs::read(&path).unwrap();
        assert!(written.windows(7).any(|w| w == b"\0codex\0"), "{written:?}");
    }

    #[test]
    fn a_symlink_is_refused_and_its_target_is_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let real = resume_file(&tmp, 0o600, b"victim");
        let link = tmp.path().join("link");
        symlink(&real, &link).unwrap();
        assert!(run_resume(&tmp, &link, me(tmp.path())).is_err());
        assert_eq!(std::fs::read(&real).unwrap(), b"victim");
    }

    #[test]
    fn a_group_readable_file_is_refused_and_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let path = resume_file(&tmp, 0o644, b"keep me");
        assert!(run_resume(&tmp, &path, me(tmp.path())).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"keep me");
    }

    #[test]
    fn a_hard_linked_file_is_refused_and_both_names_are_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let path = resume_file(&tmp, 0o600, b"shared");
        let partner = tmp.path().join("partner");
        std::fs::hard_link(&path, &partner).unwrap();
        assert!(run_resume(&tmp, &path, me(tmp.path())).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"shared");
        assert_eq!(std::fs::read(&partner).unwrap(), b"shared");
    }

    #[test]
    fn a_file_owned_by_another_uid_is_refused_and_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let path = resume_file(&tmp, 0o600, b"not yours");
        let other = me(tmp.path()).wrapping_add(1);
        assert!(run_resume(&tmp, &path, other).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"not yours");
    }

    #[test]
    fn a_failing_uid_probe_refuses_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = resume_file(&tmp, 0o600, b"untouched");
        let got = resume(
            &target(tmp.path(), SourceKind::Claude),
            &env_with_file(&path),
            &|| Err("probe failed".into()),
        );
        assert!(got.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"untouched");
    }

    #[test]
    fn a_fifo_is_refused_without_blocking() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("fifo");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let uid = me(tmp.path());
        let cwd = tmp.path().to_path_buf();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let env = env_with_file(&fifo);
            let got = resume(&target(&cwd, SourceKind::Claude), &env, &|| Ok(uid));
            let _ = tx.send(got.is_err());
        });
        assert!(rx.recv_timeout(WATCHDOG).expect("resume blocked on a FIFO"));
    }

    #[test]
    fn a_missing_resume_file_is_refused_and_not_created() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("absent");
        assert!(run_resume(&tmp, &path, me(tmp.path())).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn find_agent_ignores_relative_and_empty_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let stub = tmp.path().join("abs-stub");
        let want = plant(&stub, "claude");
        let rel = PathBuf::from("relative/dir");
        plant(&tmp.path().join(&rel), "claude");
        let joined = std::env::join_paths([rel.as_path(), Path::new(""), stub.as_path()]).unwrap();
        assert_eq!(find_agent(&joined, "claude"), Some(want));
        let only_relative = std::env::join_paths([rel.as_path(), Path::new("")]).unwrap();
        assert_eq!(find_agent(&only_relative, "claude"), None);
    }

    #[test]
    fn find_agent_skips_non_executable_files_and_takes_the_first_hit() {
        let tmp = tempfile::tempdir().unwrap();
        let first = tmp.path().join("a");
        let second = tmp.path().join("b");
        let blocked = plant(&first, "codex");
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o644)).unwrap();
        let want = plant(&second, "codex");
        let joined = std::env::join_paths([first.as_path(), second.as_path()]).unwrap();
        assert_eq!(find_agent(&joined, "codex"), Some(want));
        assert_eq!(find_agent(&joined, "claude"), None);
    }

    #[test]
    fn direct_path_plans_the_agent_argv_before_chdir() {
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        let claude = plant(&bin_dir, "claude");
        let codex = plant(&bin_dir, "codex");
        let env = EnvPaths {
            path: Some(bin_dir.into_os_string()),
            ..EnvPaths::default()
        };
        let cwd = tmp.path();
        assert_eq!(
            plan(&target(cwd, SourceKind::Claude), &env).unwrap(),
            Plan::Exec {
                bin: claude,
                args: vec!["--resume".into(), ID.into()],
                cwd: cwd.to_path_buf(),
            }
        );
        assert_eq!(
            plan(&target(cwd, SourceKind::Codex), &env).unwrap(),
            Plan::Exec {
                bin: codex,
                args: vec!["resume".into(), ID.into()],
                cwd: cwd.to_path_buf(),
            }
        );
    }

    #[test]
    fn direct_path_refuses_a_missing_cwd_or_agent_without_calling_uid() {
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("bin");
        plant(&bin_dir, "claude");
        let calls = Cell::new(0);
        let uid = || {
            calls.set(calls.get() + 1);
            Ok(0)
        };
        let with_path = EnvPaths {
            path: Some(bin_dir.into_os_string()),
            ..EnvPaths::default()
        };
        let gone = tmp.path().join("gone");
        let err = resume(&target(&gone, SourceKind::Claude), &with_path, &uid).unwrap_err();
        assert!(err.contains("no longer exists"), "{err}");
        let no_path = EnvPaths::default();
        let err = resume(&target(tmp.path(), SourceKind::Claude), &no_path, &uid).unwrap_err();
        assert!(err.contains("claude"), "{err}");
        assert_eq!(calls.get(), 0, "uid probe ran on the direct path");
    }

    #[test]
    fn resume_file_mode_does_not_search_for_the_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = resume_file(&tmp, 0o600, b"");
        let got = plan(
            &target(tmp.path(), SourceKind::Claude),
            &env_with_file(&path),
        );
        assert_eq!(got.unwrap(), Plan::File(path));
    }

    #[test]
    fn init_scripts_contain_no_eval() {
        for (shell, name, _) in SHELLS {
            let script = init_script(shell);
            assert!(script.contains("ROLLCALL_RESUME_FILE"), "{name}");
            assert_eq!(script.matches("eval").count(), 0, "{name}");
        }
    }

    #[test]
    fn init_scripts_pass_their_shells_syntax_check() {
        for (shell, name, flag) in SHELLS {
            let Some(tool) = which(name) else { continue };
            let tmp = tempfile::tempdir().unwrap();
            let file = tmp.path().join("rc-init");
            std::fs::write(&file, init_script(shell)).unwrap();
            let out = Command::new(tool).arg(flag).arg(&file).output().unwrap();
            assert!(
                out.status.success(),
                "{name}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    /// A missing shell skips locally and fails under `ROLLCALL_REQUIRE_TOOLS=1` (R3 tool rule).
    fn which(name: &str) -> Option<PathBuf> {
        let found = std::env::var_os("PATH").and_then(|p| find_agent(&p, name));
        if found.is_none() {
            assert!(
                std::env::var_os("ROLLCALL_REQUIRE_TOOLS").is_none_or(|v| v != "1"),
                "required tool `{name}` is missing and ROLLCALL_REQUIRE_TOOLS=1"
            );
            eprintln!("skip: `{name}` is not installed");
        }
        found
    }
}
