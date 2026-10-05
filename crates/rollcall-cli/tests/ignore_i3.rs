//! Acceptance tests for I3: config `ignore` paths (spec rev 16 §3.2, §4.2, §10.2). Every test
//! drives the real binary on a sandbox of its own: `$R` holds the repos `a`, `app`, `app2`,
//! `grp/x`, `grp/y` and `z`; the config ignores `$R/grp`, `$R/app` and `~/work/z`.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
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
        let sub = |name: &str| {
            let p = base.join(name);
            fs::create_dir_all(&p).unwrap();
            p
        };
        Sb {
            home: sub("home"),
            xdg_config: sub("xdg/config"),
            xdg_cache: sub("xdg/cache"),
            root: sub("home/work"),
            _dir: dir,
            base,
        }
    }

    fn write_config(&self, text: &str) {
        let dir = self.xdg_config.join("rollcall");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), text).unwrap();
    }

    /// The §10.2 config: a parent directory, a repo's own tree, and a `~/` entry.
    fn ignore_config(&self) -> String {
        format!(
            "ignore = [\"{r}/grp\", \"{r}/app\", \"~/work/z\"]\n",
            r = self.root.display()
        )
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

    /// `status --json` on `root` plus `flags`.
    fn status_json(&self, root: &Path, flags: &[&str]) -> (Option<i32>, Value) {
        let mut args = vec!["status", "--json"];
        args.extend_from_slice(flags);
        args.push(root.to_str().unwrap());
        let run = self.rollcall(&args);
        let doc = serde_json::from_str(&run.stdout)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}\nstderr: {}", run.stdout, run.stderr));
        (run.code, doc)
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

    /// A clean repo with one commit.
    fn repo(&self, path: &Path) {
        fs::create_dir_all(path).unwrap();
        self.git(path, &["init", "-q", "-b", "main"]);
        self.git(path, &["commit", "-q", "--allow-empty", "-m", "init"]);
    }

    /// The I3-3..I3-9 fixture, without a config file.
    fn fixture() -> Sb {
        let sb = Sb::new();
        for name in ["a", "app", "app2", "grp/x", "grp/y", "z"] {
            sb.repo(&sb.root.join(name));
        }
        sb
    }

    /// The fixture with the §10.2 config in place.
    fn ignoring() -> Sb {
        let sb = Sb::fixture();
        sb.write_config(&sb.ignore_config());
        sb
    }

    fn root_str(&self) -> &str {
        self.root.to_str().unwrap()
    }
}

fn names(doc: &Value) -> String {
    let mut names: Vec<&str> = doc["repos"]
        .as_array()
        .expect("status has repos")
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    names.sort();
    names.join(",")
}

/// A `.git` file naming a gitdir that does not exist: one `Repo.error` row.
fn bad_repo(path: &Path) {
    fs::remove_dir_all(path.join(".git")).unwrap();
    fs::write(path.join(".git"), "gitdir: ../does-not-exist\n").unwrap();
}

#[test]
fn i3_1_ignore_is_a_known_key_and_warns_nothing() {
    let sb = Sb::ignoring();
    let run = sb.rollcall(&["status", sb.root_str()]);
    assert!(!run.stderr.contains("unknown"), "stderr: {}", run.stderr);
    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr);
}

#[test]
fn i3_2_bad_ignore_config_exits_2() {
    for bad in [
        "ignore = \"x\"\n",
        "ignore = [\"rel/x\"]\n",
        "ignore = [\"~bob/x\"]\n",
    ] {
        let sb = Sb::fixture();
        sb.write_config(bad);
        let run = sb.rollcall(&["status", sb.root_str()]);
        assert_eq!(run.code, Some(2), "{bad}: stderr {}", run.stderr);
        assert!(run.stdout.is_empty(), "{bad}: stdout {}", run.stdout);
    }
}

#[test]
fn i3_3_status_json_hides_ignored_repos_but_not_a_string_prefix_sibling() {
    let sb = Sb::ignoring();
    let (code, doc) = sb.status_json(&sb.root, &[]);
    assert_eq!(code, Some(0));
    assert_eq!(names(&doc), "a,app2");
    // Control: without the config all six repos show, so the check above can fail.
    let control = Sb::fixture();
    assert_eq!(
        names(&control.status_json(&control.root, &[]).1),
        "a,app,app2,x,y,z"
    );
}

#[test]
fn i3_4_plain_status_and_doctor_count_only_the_visible_repos() {
    let sb = Sb::ignoring();
    let run = sb.rollcall(&["status", sb.root_str()]);
    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr);
    // A repo line starts in column 0; worktree lines are indented.
    let repo_lines: Vec<&str> = run
        .stdout
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with(' '))
        .collect();
    assert_eq!(repo_lines.len(), 2, "{}", run.stdout);
    assert!(repo_lines[0].starts_with("a ") && repo_lines[1].starts_with("app2 "));
    let doctor = sb.rollcall(&["doctor", sb.root_str(), "--json"]);
    let doc: Value = serde_json::from_str(&doctor.stdout).unwrap();
    assert_eq!(
        doc["repos_by_kind"]["normal"],
        json!(2),
        "{}",
        doctor.stdout
    );
}

/// `rollcall mcp --root $R` answering one tool call.
fn mcp_call(sb: &Sb, tool: &str) -> Value {
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": tool, "arguments": {"limit": 50}}});
    let run = sb.rollcall_with(
        &["mcp", "--root", sb.root_str()],
        Some(&format!("{request}\n")),
    );
    assert_eq!(run.code, Some(0), "stderr: {}", run.stderr);
    let reply: Value = serde_json::from_str(run.stdout.lines().next().unwrap()).unwrap();
    let text = reply["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("{reply}"));
    serde_json::from_str(text).unwrap()
}

#[test]
fn i3_5_mcp_tools_hide_ignored_repos() {
    let sb = Sb::fixture();
    // `grp/x` gets a linked worktree that is then deleted: `list_stale` lists it (missing).
    let wt = sb.root.join("wt-x");
    sb.git(
        &sb.root.join("grp/x"),
        &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()],
    );
    fs::remove_dir_all(&wt).unwrap();
    let repo_names = |doc: &Value| -> Vec<String> {
        doc["repos"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let stale = mcp_call(&sb, "list_stale");
    let stale_repos = |doc: &Value| -> Vec<String> {
        doc["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["repo_name"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(stale_repos(&stale), ["x"], "control: {stale}");

    sb.write_config(&sb.ignore_config());
    let stale = mcp_call(&sb, "list_stale");
    assert!(stale_repos(&stale).is_empty(), "{stale}");
    let seen = repo_names(&mcp_call(&sb, "where_was_i"));
    assert_eq!(
        seen.iter()
            .filter(|n| ["app", "x", "y", "z"].contains(&n.as_str()))
            .count(),
        0,
        "{seen:?}"
    );
    assert!(
        seen.contains(&"a".to_owned()) && seen.contains(&"app2".to_owned()),
        "{seen:?}"
    );
}

#[test]
fn i3_7_a_cached_scan_is_filtered_on_read() {
    let sb = Sb::fixture();
    let (code, doc) = sb.status_json(&sb.root, &[]);
    assert_eq!((code, names(&doc).as_str()), (Some(0), "a,app,app2,x,y,z"));
    sb.write_config(&sb.ignore_config());
    let (code, doc) = sb.status_json(&sb.root, &["--no-scan"]);
    assert_eq!(code, Some(0));
    assert_eq!(names(&doc), "a,app2");
    assert!(
        doc["scanned_at"].is_string(),
        "answered from the cache: {doc}"
    );
}

#[test]
fn i3_8_an_ignored_error_row_does_not_change_the_exit_code() {
    let sb = Sb::fixture();
    bad_repo(&sb.root.join("grp/x"));
    let (code, _) = sb.status_json(&sb.root, &[]);
    assert_eq!(
        code,
        Some(4),
        "control: the error row exits 4 without the ignore"
    );
    sb.write_config(&sb.ignore_config());
    // From the cache, which still holds the error row ...
    let (code, doc) = sb.status_json(&sb.root, &["--no-scan"]);
    assert_eq!((code, names(&doc).as_str()), (Some(0), "a,app2"));
    // ... and from a fresh scan.
    let (code, doc) = sb.status_json(&sb.root, &[]);
    assert_eq!((code, names(&doc).as_str()), (Some(0), "a,app2"));
}

/// Restores a directory's mode on drop so the tempdir can be cleaned up.
struct Unlock(PathBuf);
impl Drop for Unlock {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
    }
}

#[test]
fn i3_9_the_walk_never_enters_an_ignored_directory() {
    // Root can read a mode-000 directory, so the control would not hold.
    if Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|o| o.stdout == b"0\n")
    {
        return;
    }
    let sb = Sb::fixture();
    let locked = sb.root.join("grp/locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let _unlock = Unlock(locked);
    let unreadable = |sb: &Sb| -> Value {
        let run = sb.rollcall(&["doctor", sb.root_str(), "--json"]);
        let doc: Value = serde_json::from_str(&run.stdout).unwrap();
        doc["walk"]["unreadable_dirs"].clone()
    };
    assert_eq!(
        unreadable(&sb),
        json!(1),
        "control: without the config the walk meets `locked`"
    );
    sb.write_config(&sb.ignore_config());
    assert_eq!(unreadable(&sb), json!(0));
}

#[test]
fn i3_10_an_ignored_enclosing_repo_is_not_listed() {
    let sb = Sb::new();
    let enclosing = sb.root.join("e");
    sb.repo(&enclosing);
    let root = enclosing.join("sub");
    sb.repo(&root.join("inner"));
    let (_, doc) = sb.status_json(&root, &[]);
    assert!(
        doc["repos"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["contains_root"] == json!(true)),
        "control: {doc}"
    );
    sb.write_config(&format!("ignore = [\"{}\"]\n", enclosing.display()));
    let (code, doc) = sb.status_json(&root, &[]);
    assert_eq!(code, Some(0));
    assert!(
        doc["repos"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["contains_root"] != json!(true)),
        "{doc}"
    );
}

#[test]
fn i3_11_a_root_under_an_ignored_path_lists_nothing() {
    let sb = Sb::ignoring();
    let (code, doc) = sb.status_json(&sb.root.join("grp"), &[]);
    assert_eq!(code, Some(0));
    assert_eq!(doc["repos"], json!([]), "{doc}");
}

#[test]
fn i3_12_a_linked_worktree_under_an_ignored_path_stays_with_its_repo() {
    let sb = Sb::ignoring();
    let main = sb.root.join("m");
    sb.repo(&main);
    let wt = sb.root.join("grp/wt");
    sb.git(
        &main,
        &["worktree", "add", "-q", "-b", "topic", wt.to_str().unwrap()],
    );
    let (code, doc) = sb.status_json(&sb.root, &[]);
    assert_eq!(code, Some(0));
    assert_eq!(names(&doc), "a,app2,m");
    let m = doc["repos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "m")
        .unwrap();
    let paths: Vec<&str> = m["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["path"].as_str().unwrap())
        .collect();
    assert!(
        paths.contains(&main.to_str().unwrap()) && paths.contains(&wt.to_str().unwrap()),
        "{paths:?}"
    );
}
