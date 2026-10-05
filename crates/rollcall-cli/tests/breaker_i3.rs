//! Breaker (adversarial QA) tests for I3: config `ignore` paths (spec rev 16 §3.2, §4.2,
//! §10.2). Every test drives the real binary on a sandbox of its own.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

const NOW: &str = "2026-09-29T12:00:00Z";
const DEADLINE: Duration = Duration::from_secs(60);

struct Sb {
    _dir: TempDir,
    base: PathBuf,
    home: PathBuf,
    xdg_config: PathBuf,
    xdg_cache: PathBuf,
    root: PathBuf,
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Sb {
    fn new() -> Sb {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        Sb::at(dir, base.clone(), base)
    }

    /// `home` (and so `root`) are spelled through `alias`, a symlink to `real`: every path the
    /// binary sees from the environment and the command line is non-canonical.
    fn aliased() -> Sb {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        fs::create_dir(base.join("real")).unwrap();
        symlink(base.join("real"), base.join("alias")).unwrap();
        Sb::at(dir, base.clone(), base.join("alias"))
    }

    fn at(dir: TempDir, base: PathBuf, home_parent: PathBuf) -> Sb {
        let sub = |at: &Path, name: &str| {
            let p = at.join(name);
            fs::create_dir_all(&p).unwrap();
            p
        };
        Sb {
            home: sub(&home_parent, "home"),
            xdg_config: sub(&base, "xdg/config"),
            xdg_cache: sub(&base, "xdg/cache"),
            root: sub(&home_parent, "home/work"),
            _dir: dir,
            base,
        }
    }

    fn write_config(&self, text: &str) {
        let dir = self.xdg_config.join("rollcall");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), text).unwrap();
    }

    fn ignore(&self, entries: &[String]) {
        let list: Vec<String> = entries.iter().map(|e| format!("{e:?}")).collect();
        self.write_config(&format!("ignore = [{}]\n", list.join(", ")));
    }

    fn rollcall_with(&self, args: &[&str], stdin: Option<&str>) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rollcall"));
        cmd.env_clear()
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.xdg_config)
            .env("XDG_CACHE_HOME", &self.xdg_cache)
            .env("CLAUDE_CONFIG_DIR", self.base.join("claude"))
            .env("CODEX_HOME", self.base.join("codex"))
            .env("ROLLCALL_NOW", NOW)
            .env("PATH", "/usr/bin:/bin")
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        if let Some(text) = stdin {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        }
        let id = child.id();
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });
        let out = rx.recv_timeout(DEADLINE).unwrap_or_else(|_| {
            let _ = Command::new("/bin/kill").arg(id.to_string()).status();
            panic!("rollcall {args:?} outlived {DEADLINE:?}")
        });
        let out = out.unwrap();
        Run {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn rollcall(&self, args: &[&str]) -> Run {
        self.rollcall_with(args, None)
    }

    fn json(&self, args: &[&str]) -> (Option<i32>, Value) {
        let run = self.rollcall(args);
        let doc = serde_json::from_str(&run.stdout)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}\nstderr: {}", run.stdout, run.stderr));
        (run.code, doc)
    }

    fn status_json(&self, root: &Path, flags: &[&str]) -> (Option<i32>, Value) {
        let mut args = vec!["status", "--json"];
        args.extend_from_slice(flags);
        args.push(root.to_str().unwrap());
        self.json(&args)
    }

    fn git(&self, dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_COMMITTER_DATE", "2026-09-20T10:00:00Z")
            .env("GIT_AUTHOR_DATE", "2026-09-20T10:00:00Z")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    fn repo(&self, path: &Path) {
        fs::create_dir_all(path).unwrap();
        self.git(path, &["init", "-q", "-b", "main"]);
        self.git(path, &["commit", "-q", "--allow-empty", "-m", "init"]);
    }

    /// The I3 fixture: `a`, `app`, `app2`, `grp/x`, `grp/y`, `z` under `root`.
    fn fixture(self) -> Sb {
        for name in ["a", "app", "app2", "grp/x", "grp/y", "z"] {
            self.repo(&self.root.join(name));
        }
        self
    }

    fn r(&self, rel: &str) -> String {
        format!("{}/{rel}", self.root.display())
    }
}

fn names(doc: &Value) -> String {
    let mut names: Vec<&str> = doc["repos"]
        .as_array()
        .unwrap_or_else(|| panic!("status has repos: {doc}"))
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    names.sort();
    names.join(",")
}

fn paths(doc: &Value) -> Vec<String> {
    let mut out: Vec<String> = doc["repos"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| {
            std::iter::once(r["path"].as_str().unwrap().to_owned()).chain(
                r["worktrees"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|w| w["path"].as_str().unwrap().to_owned()),
            )
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn is_root_user() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|o| o.stdout == b"0\n")
}

struct Unlock(PathBuf);
impl Drop for Unlock {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
    }
}

// ---- entry resolution (§3.2: realpath where the entry exists, else lexical) ----

/// §3.2: an existing entry resolves by realpath. `$R/link/..` is `$R/deep` to the kernel
/// (link -> deep/inner), so `$R/link/../grp` names `$R/deep/grp`, not `$R/grp`.
#[test]
fn breaker_i3_an_existing_entry_through_a_symlink_and_dotdot_resolves_by_realpath() {
    let sb = Sb::new();
    fs::create_dir_all(sb.root.join("deep/inner")).unwrap();
    sb.repo(&sb.root.join("deep/grp/x"));
    sb.repo(&sb.root.join("grp/y"));
    symlink(sb.root.join("deep/inner"), sb.root.join("link")).unwrap();
    let entry = sb.r("link/../grp");
    // Control: the OS says the entry exists and is `$R/deep/grp`.
    assert_eq!(
        fs::canonicalize(&entry).unwrap(),
        sb.root.join("deep/grp"),
        "control: realpath of the entry"
    );
    let (_, doc) = sb.status_json(&sb.root, &[]);
    assert_eq!(names(&doc), "x,y", "control: both repos without the config");
    sb.ignore(&[entry]);
    let (code, doc) = sb.status_json(&sb.root, &[]);
    assert_eq!(code, Some(0));
    assert_eq!(
        names(&doc),
        "y",
        "the realpath'd entry $R/deep/grp must hide x and leave $R/grp/y"
    );
}

/// Trailing slashes, doubled slashes, `.` segments and a `..` that stays inside real dirs all
/// name the same directory as the plain spelling.
#[test]
fn breaker_i3_entry_spellings_resolve_to_the_same_directory() {
    let sb = Sb::new().fixture();
    sb.ignore(&[
        format!("{}//grp/", sb.root.display()),
        sb.r("./app/"),
        sb.r("a/../z"),
    ]);
    let (code, doc) = sb.status_json(&sb.root, &[]);
    assert_eq!(code, Some(0));
    assert_eq!(names(&doc), "a,app2");
}

/// `~` alone and `~/` are HOME itself: a root under HOME shows nothing.
#[test]
fn breaker_i3_tilde_alone_and_tilde_slash_hide_everything_under_home() {
    for entry in ["~", "~/"] {
        let sb = Sb::new().fixture();
        let (_, doc) = sb.status_json(&sb.root, &[]);
        assert_eq!(names(&doc), "a,app,app2,x,y,z", "control");
        sb.ignore(&[entry.to_owned()]);
        let (code, doc) = sb.status_json(&sb.root, &[]);
        assert_eq!(code, Some(0), "{entry}");
        assert_eq!(doc["repos"], json!([]), "{entry}: {doc}");
    }
}

/// An entry that is a symlink to a directory hides the repos under its target.
#[test]
fn breaker_i3_a_symlinked_entry_hides_the_repos_under_its_target() {
    let sb = Sb::new().fixture();
    symlink(sb.root.join("grp"), sb.base.join("grplink")).unwrap();
    sb.ignore(&[sb.base.join("grplink").display().to_string()]);
    let (code, doc) = sb.status_json(&sb.root, &[]);
    assert_eq!(code, Some(0));
    assert_eq!(names(&doc), "a,app,app2,z");
}

/// HOME, the root and an absolute entry all spelled through a symlinked ancestor: matching is on
/// canonical paths, so the ignore still binds.
#[test]
fn breaker_i3_non_canonical_home_root_and_entries_still_match() {
    let sb = Sb::aliased().fixture();
    assert_ne!(
        fs::canonicalize(&sb.root).unwrap(),
        sb.root,
        "premise: root is non-canonical"
    );
    sb.ignore(&["~/work/grp".to_owned(), sb.r("app"), "~/work/z".to_owned()]);
    let (code, doc) = sb.status_json(&sb.root, &[]);
    assert_eq!(code, Some(0));
    assert_eq!(names(&doc), "a,app2");
    // A root under an ignored path, spelled non-canonically.
    let (_, doc) = sb.status_json(&sb.root.join("grp"), &[]);
    assert_eq!(doc["repos"], json!([]), "{doc}");
}

// ---- repo-level rule (§4.2) ----

/// The mirror of I3-12: an ignored repo (`grp/m`) with a linked worktree outside the ignored path
/// is dropped whole (§4.2: an `external_main` repo included), on a fresh scan and from the cache.
#[test]
fn breaker_i3_an_ignored_repo_takes_its_outside_worktree_with_it() {
    let sb = Sb::new();
    sb.repo(&sb.root.join("a"));
    let main = sb.root.join("grp/m");
    sb.repo(&main);
    let wt = sb.root.join("wt");
    sb.git(
        &main,
        &["worktree", "add", "-q", "-b", "topic", wt.to_str().unwrap()],
    );
    let (_, doc) = sb.status_json(&sb.root, &[]);
    assert!(
        paths(&doc).contains(&wt.display().to_string()),
        "control: {doc}"
    );
    sb.ignore(&[sb.r("grp")]);
    for flags in [&["--no-scan"][..], &[]] {
        let (code, doc) = sb.status_json(&sb.root, flags);
        assert_eq!(code, Some(0), "{flags:?}");
        assert_eq!(names(&doc), "a", "{flags:?}: {doc}");
        assert!(
            !paths(&doc).contains(&wt.display().to_string()),
            "{flags:?}: {doc}"
        );
    }
    let (_, doctor) = sb.json(&["doctor", "--json", sb.root.to_str().unwrap()]);
    assert_eq!(
        doctor["repos_by_kind"]["external_main"],
        json!(0),
        "{doctor}"
    );
    assert_eq!(doctor["worktrees"]["total"], json!(1), "{doctor}");
}

/// Case B (§4.1) with the main repo under an ignored path outside the root: no `external_main`
/// row and no worktree from it.
#[test]
fn breaker_i3_an_external_main_under_an_ignored_path_outside_the_root_is_dropped() {
    let sb = Sb::new();
    let main = sb.base.join("elsewhere/ign/main");
    sb.repo(&main);
    sb.repo(&sb.root.join("a"));
    let wt = sb.root.join("wt");
    sb.git(
        &main,
        &["worktree", "add", "-q", "-b", "topic", wt.to_str().unwrap()],
    );
    let (_, doctor) = sb.json(&["doctor", "--json", sb.root.to_str().unwrap()]);
    assert_eq!(
        doctor["repos_by_kind"]["external_main"],
        json!(1),
        "control: {doctor}"
    );
    sb.ignore(&[sb.base.join("elsewhere/ign").display().to_string()]);
    let (code, doc) = sb.status_json(&sb.root, &[]);
    assert_eq!(code, Some(0));
    assert_eq!(names(&doc), "a", "{doc}");
    let (_, doctor) = sb.json(&["doctor", "--json", sb.root.to_str().unwrap()]);
    assert_eq!(
        doctor["repos_by_kind"]["external_main"],
        json!(0),
        "{doctor}"
    );
}

/// A nested repo listed in `ignore` goes; the repo whose working tree holds it stays.
#[test]
fn breaker_i3_an_ignored_nested_repo_leaves_its_outer_repo() {
    let sb = Sb::new();
    sb.repo(&sb.root.join("outer"));
    sb.repo(&sb.root.join("outer/inner"));
    let (_, doc) = sb.status_json(&sb.root, &[]);
    assert_eq!(names(&doc), "inner,outer", "control");
    sb.ignore(&[sb.r("outer/inner")]);
    let (code, doc) = sb.status_json(&sb.root, &[]);
    assert_eq!(code, Some(0));
    assert_eq!(names(&doc), "outer");
}

/// With `--submodules`: ignoring a submodule's path drops it and keeps the parent; ignoring the
/// parent drops both. Fresh and cached.
#[test]
fn breaker_i3_submodules_follow_the_ignore() {
    let sb = Sb::new();
    let src = sb.base.join("src");
    sb.repo(&src);
    let p = sb.root.join("p");
    sb.repo(&p);
    sb.git(
        &p,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            src.to_str().unwrap(),
            "sub",
        ],
    );
    sb.git(&p, &["commit", "-q", "-m", "add sub"]);
    let subs = |doc: &Value| -> Vec<String> {
        doc["repos"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|r| r["submodules"].as_array().cloned().unwrap_or_default())
            .map(|s| s["path"].as_str().unwrap().to_owned())
            .collect()
    };
    let (_, doc) = sb.status_json(&sb.root, &["--submodules"]);
    assert_eq!(subs(&doc), [sb.r("p/sub")], "control: {doc}");

    sb.ignore(&[sb.r("p/sub")]);
    for flags in [&["--submodules", "--no-scan"][..], &["--submodules"]] {
        let (code, doc) = sb.status_json(&sb.root, flags);
        assert_eq!(code, Some(0), "{flags:?}");
        assert_eq!(names(&doc), "p", "{flags:?}: {doc}");
        assert!(subs(&doc).is_empty(), "{flags:?}: {doc}");
    }
    sb.ignore(&[sb.r("p")]);
    let (_, doc) = sb.status_json(&sb.root, &["--submodules"]);
    assert_eq!(doc["repos"], json!([]), "{doc}");
}

// ---- other channels ----

/// `--follow-symlinks` with a link inside the root pointing into the ignored dir: the walk does
/// not reach the ignored repos or the unreadable dir through the link.
#[test]
fn breaker_i3_a_followed_link_into_an_ignored_dir_is_not_walked() {
    if is_root_user() {
        return;
    }
    let sb = Sb::new().fixture();
    let locked = sb.root.join("grp/locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let _unlock = Unlock(locked);
    symlink(sb.root.join("grp"), sb.root.join("l")).unwrap();
    let root = sb.root.to_str().unwrap();
    let unreadable = |sb: &Sb| {
        let (_, doc) = sb.json(&["doctor", "--json", "--follow-symlinks", root]);
        doc["walk"]["unreadable_dirs"].clone()
    };
    assert_eq!(unreadable(&sb), json!(1), "control");
    sb.ignore(&[sb.r("grp")]);
    assert_eq!(unreadable(&sb), json!(0));
    let (code, doc) = sb.status_json(&sb.root, &["--follow-symlinks"]);
    assert_eq!(code, Some(0));
    assert_eq!(names(&doc), "a,app,app2,z");
}

/// `rollcall scan` prints its summary from the visible rows: 2 repos, 0 errors, exit 0 even with
/// an ignored error row.
#[test]
fn breaker_i3_scan_summary_counts_only_visible_repos() {
    let sb = Sb::new().fixture();
    let x = sb.root.join("grp/x");
    fs::remove_dir_all(x.join(".git")).unwrap();
    fs::write(x.join(".git"), "gitdir: ../does-not-exist\n").unwrap();
    sb.ignore(&[sb.r("grp"), sb.r("app"), sb.r("z")]);
    let run = sb.rollcall(&["scan", sb.root.to_str().unwrap()]);
    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr);
    assert!(
        run.stderr.contains(": 2 repos, 2 worktrees") && run.stderr.contains("(0 errors)"),
        "{}",
        run.stderr
    );
}

/// MCP `where_was_i` asked for an ignored repo by name or by path returns no repo.
#[test]
fn breaker_i3_mcp_where_was_i_for_an_ignored_repo_by_name_or_path() {
    let sb = Sb::new().fixture();
    sb.ignore(&[sb.r("grp")]);
    for repo in ["x".to_owned(), sb.r("grp/x")] {
        let request = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "where_was_i", "arguments": {"repo": repo}}});
        let run = sb.rollcall_with(
            &["mcp", "--root", sb.root.to_str().unwrap()],
            Some(&format!("{request}\n")),
        );
        assert_eq!(run.code, Some(0), "stderr: {}", run.stderr);
        let reply: Value = serde_json::from_str(run.stdout.lines().next().unwrap()).unwrap();
        let doc: Value =
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(doc["repos"], json!([]), "{repo}: {doc}");
    }
}

/// §4.2: `doctor --no-scan` after the ignore is added recounts from the visible rows, not the
/// counts of the scan made before it (author decision 2026-10-05: filter on read).
#[test]
fn breaker_i3_doctor_no_scan_counts_after_the_ignore_is_added() {
    let sb = Sb::new().fixture();
    let root = sb.root.to_str().unwrap();
    let (_, doc) = sb.json(&["doctor", "--json", root]);
    assert_eq!(doc["repos_by_kind"]["normal"], json!(6), "control");
    sb.ignore(&[sb.r("grp"), sb.r("app"), sb.r("z")]);
    let (_, doc) = sb.json(&["doctor", "--json", "--no-scan", root]);
    assert_eq!(doc["repos_by_kind"]["normal"], json!(2), "{doc}");
}

/// §4.2: each ignored directory the walk reaches counts once in `walk.skipped_dirs`
/// (author decision 2026-10-05).
#[test]
fn breaker_i3_an_ignored_dir_counts_once_in_skipped_dirs() {
    let sb = Sb::new().fixture();
    let root = sb.root.to_str().unwrap();
    let (_, doc) = sb.json(&["doctor", "--json", root]);
    let before = doc["walk"]["skipped_dirs"].as_u64().unwrap();
    sb.ignore(&[sb.r("grp")]);
    let (_, doc) = sb.json(&["doctor", "--json", root]);
    assert_eq!(doc["walk"]["skipped_dirs"], json!(before + 1), "{doc}");
}

/// §4.2: a walk-reachable repo under an ignored path is dropped before its git state is computed,
/// so a fresh scan's exit code never sees its error (the read-time filter alone would not stop it).
#[test]
fn breaker_i3_a_walk_reachable_ignored_error_repo_never_reaches_the_exit_code() {
    let sb = Sb::new();
    let main = sb.base.join("elsewhere/ign/main");
    sb.repo(&main);
    sb.repo(&sb.root.join("a"));
    let wt = sb.root.join("wt");
    sb.git(
        &main,
        &["worktree", "add", "-q", "-b", "topic", wt.to_str().unwrap()],
    );
    fs::write(main.join(".git/HEAD"), "garbage\n").unwrap();
    let (code, doc) = sb.status_json(&sb.root, &[]);
    assert_eq!(code, Some(4), "control: {doc}");
    sb.ignore(&[sb.base.join("elsewhere/ign").display().to_string()]);
    let (code, doc) = sb.status_json(&sb.root, &[]);
    assert_eq!((code, names(&doc)), (Some(0), "a".to_owned()), "{doc}");
}
