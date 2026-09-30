use super::support::*;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const WATCHDOG_DEADLINE: Duration = Duration::from_millis(500);
const RSS_ALLOC_BYTES: u64 = 64 * 1024 * 1024;
const PAGE_STEP: u64 = 4096;

#[test]
fn watchdog() {
    let mut cmd = Command::new("sleep");
    cmd.arg("30");
    let start = Instant::now();
    let res = spawn_watchdog(cmd, WATCHDOG_DEADLINE);
    let took = start.elapsed();
    assert!(
        res.is_err(),
        "a 30 s sleep must not finish under a 500 ms deadline"
    );
    assert!(
        took < WATCHDOG_DEADLINE + Duration::from_secs(1),
        "watchdog took {took:?}"
    );
    // A child that finishes in time must come back intact.
    let mut quick = Command::new("echo");
    quick.arg("hi");
    let out = spawn_watchdog(quick, Duration::from_secs(30)).expect("echo finishes");
    assert_eq!(out.stdout, b"hi\n");
}

#[test]
fn manifest_parses() {
    let text = std::fs::read_to_string(manifest_path()).expect("read MANIFEST.toml");
    let m = parse_manifest(&text).expect("MANIFEST.toml parses with no unknown keys");
    assert!(!m.repo.is_empty() && !m.claude.is_empty());
    assert!(m.expect_counts.f_repos > 0);
    let bad = format!("{text}\n[meta_extra]\nx = 1\n");
    assert!(
        parse_manifest(&bad).is_err(),
        "an unknown table must be rejected"
    );
}

#[test]
fn peak_rss() {
    let Some(python) = require_tool("python3") else {
        return;
    };
    if require_tool("/usr/bin/time").is_none() {
        return;
    }
    let script = format!(
        "b = bytearray({RSS_ALLOC_BYTES})\nfor i in range(0, {RSS_ALLOC_BYTES}, {PAGE_STEP}): b[i] = 1"
    );
    let mut cmd = Command::new(python);
    cmd.args(["-c", &script]);
    let (out, rss) = super::support::peak_rss(cmd, Duration::from_secs(60));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(rss >= RSS_ALLOC_BYTES, "peak rss {rss} < {RSS_ALLOC_BYTES}");
    assert!(
        rss < 4 * RSS_ALLOC_BYTES,
        "peak rss {rss} is implausibly large (unit mix-up?)"
    );
}

#[test]
fn tokens_and_expected() {
    use std::os::unix::fs::FileTypeExt;
    let tmp = tempfile::tempdir().unwrap();
    let mut tokens = Tokens::new(tmp.path().join("fifos"));
    tokens.add("$H", "/h".into());
    tokens.add("$HOME", "/home".into());
    tokens.add("$F", "/f".into());
    assert_eq!(
        tokens.subst("$HOME/a $H/b $F/c $Zed"),
        "/home/a /h/b /f/c $Zed"
    );
    let fifo = tokens.subst("$FIFO:probe-1/x");
    assert_eq!(
        fifo,
        format!("{}/x", tmp.path().join("fifos/probe-1").display())
    );
    let made = std::fs::metadata(tmp.path().join("fifos/probe-1")).unwrap();
    assert!(made.file_type().is_fifo(), "a FIFO is created");
    let v = expected_from(
        r#"{"_note":"x","p":"$F/r","l":[{"_c":1,"q":"$H"}]}"#,
        &tokens,
    );
    assert_eq!(v, serde_json::json!({"p":"/f/r","l":[{"q":"/h"}]}));
}

#[test]
fn error_prefix() {
    use serde_json::json;
    assert_error_prefix("ok", &json!("open failed: boom"), &json!("open failed: "));
    assert_error_prefix("ok", &json!(null), &json!(null));
    for (actual, prefix) in [
        (json!("timed out"), json!("open failed: ")),
        (json!(null), json!("open failed: ")),
        (json!("open failed: x"), json!(null)),
    ] {
        let r = std::panic::catch_unwind(|| assert_error_prefix("bad", &actual, &prefix));
        assert!(r.is_err(), "{actual} vs {prefix} must fail");
    }
}

#[test]
fn validate_schemas() {
    let read = |name: &str| -> serde_json::Value {
        let path = repo_root().join("schema/examples").join(name);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    };
    // status.valid.json reaches `rollcall:status.json#/$defs/...` refs, so this fails if the registry is wrong.
    assert_eq!(validate("status", &read("status.valid.json")), Ok(()));
    assert!(validate("status", &read("status.invalid-extra-key.json")).is_err());
}

// ------------------------------------------------------------ git builder (P27b)

const FSCK_DEADLINE: Duration = Duration::from_secs(60);
/// Fewer than this many F_clean repos means the row filter, not the builder, is broken.
const MIN_FSCK_REPOS: usize = 20;
const SPARSE_BIG_LEN: u64 = 4 << 30;
const BIG_TRACKED_LEN: u64 = 1 << 30;
/// Specials in the manifest's enum; a row for each must have been built.
const SPECIAL_COUNT: usize = 27;

fn git_in(w: &World, dir: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = w.sb.command(git_path());
    cmd.current_dir(dir).args(args);
    spawn_watchdog(cmd, FSCK_DEADLINE).unwrap_or_else(|e| panic!("git {args:?} in {dir:?}: {e}"))
}

fn git_ok(w: &World, dir: &Path, args: &[&str]) -> String {
    let out = git_in(w, dir, args);
    assert!(
        out.status.success(),
        "git {args:?} in {dir:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn row_path(w: &World, r: &Repo) -> PathBuf {
    let root = match r.root {
        RootId::F | RootId::H => w.root.clone(),
        RootId::Ext => w.ext.clone(),
        RootId::Home => w.sb.home.clone(),
        RootId::Budget => w.base.join("budget"),
        other => panic!("not a git-builder root: {other:?}"),
    };
    root.join(&r.path)
}

fn file_type(p: &Path) -> std::fs::FileType {
    std::fs::symlink_metadata(p)
        .unwrap_or_else(|e| panic!("lstat {}: {e}", p.display()))
        .file_type()
}

fn points_at_sparse(link: &Path) {
    assert!(
        file_type(link).is_symlink(),
        "{} is a symlink",
        link.display()
    );
    let len = std::fs::metadata(link)
        .unwrap_or_else(|e| panic!("stat through {}: {e}", link.display()))
        .len();
    assert_eq!(
        len,
        SPARSE_BIG_LEN,
        "{} targets the 4 GiB file",
        link.display()
    );
}

fn symlinks_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.map(Result::unwrap) {
        let ft = entry.file_type().unwrap();
        if ft.is_symlink() {
            out.push(entry.path());
        } else if ft.is_dir() {
            symlinks_under(&entry.path(), out);
        }
    }
}

fn python_out(python: &Path, script: &str, arg: &Path) -> String {
    let mut cmd = Command::new(python);
    cmd.args(["-c", script]).arg(arg);
    let out = spawn_watchdog(cmd, FSCK_DEADLINE).expect("python3 finishes");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Independent of the builder: SHA-1 of everything before the last 20 bytes equals those bytes.
fn index_trailer_is_valid(python: &Path, index: &Path) -> bool {
    let script = "import hashlib,sys\nd=open(sys.argv[1],'rb').read()\nprint(hashlib.sha1(d[:-20]).digest()==d[-20:])";
    python_out(python, script, index) == "True"
}

fn only_admin_dir(gitdir: &Path) -> PathBuf {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(gitdir.join("worktrees"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs.remove(0)
}

/// One assertion block per special: the builder must have produced the hostile shape, not just a directory.
fn assert_special(w: &World, r: &Repo, special: Special, python: &Path) {
    let path = row_path(w, r);
    let gitdir = path.join(".git");
    match special {
        Special::FifoGit => assert!(file_type(&gitdir).is_fifo()),
        Special::SymlinkFanout => {
            for name in ["loop-a", "loop-b"] {
                let link = path.join("d1/d2").join(name);
                assert_eq!(std::fs::read_link(&link).unwrap(), w.root, "{name}");
            }
        }
        Special::SymlinkOutOfRoot => {
            assert_eq!(
                std::fs::read_link(&path).unwrap(),
                w.ext.join("outside-repo")
            )
        }
        Special::UnreadableDir => {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0);
        }
        Special::BadGitdir => assert_eq!(
            std::fs::read_to_string(&gitdir).unwrap(),
            "gitdir: ../does-not-exist\n"
        ),
        Special::Corrupted => assert!(
            !git_in(w, &path, &["fsck"]).status.success(),
            "fsck must fail on the corrupted repo"
        ),
        Special::UntrustedPath => {
            let text = std::fs::read_to_string(only_admin_dir(&gitdir).join("gitdir")).unwrap();
            assert_eq!(text.trim_end(), format!("{}/.git", w.sb.home.display()));
        }
        Special::CommondirEscape => {
            let text = std::fs::read_to_string(only_admin_dir(&gitdir).join("commondir")).unwrap();
            assert_eq!(
                text.trim_end(),
                format!("{}/commondir-victim/.git", w.root.display())
            );
        }
        Special::WorktreeEscape => {
            let cfg = std::fs::read_to_string(gitdir.join("config")).unwrap();
            assert!(cfg.contains(&w.sb.home.display().to_string()), "{cfg}");
        }
        Special::SymlinkedFiles | Special::ExcludesFile | Special::WorktreeConfigSymlink => {
            let mut links = Vec::new();
            symlinks_under(&path, &mut links);
            // The second-hop alternates symlink lives in a helper directory outside the repo.
            symlinks_under(&w.base.join("helpers").join(&r.id), &mut links);
            assert!(!links.is_empty(), "{} has no symlink", r.id);
            links.iter().for_each(|l| points_at_sparse(l));
        }
        Special::HangProbe => assert!(file_type(&gitdir.join("packed-refs")).is_fifo()),
        Special::HangProbeWorktreeConfig => {
            assert!(file_type(&gitdir.join("config.worktree")).is_fifo())
        }
        Special::IndexBomb | Special::IndexDeepOk => {
            let index = gitdir.join("index");
            let head = std::fs::read(&index).unwrap();
            assert_eq!(&head[..4], b"DIRC");
            assert!(
                index_trailer_is_valid(python, &index),
                "{} index checksum",
                r.id
            );
            if special == Special::IndexDeepOk {
                // Real git reading the index proves the bytes are a well-formed index.
                assert_eq!(git_ok(w, &path, &["ls-files"]).trim(), "README.md");
            }
        }
        Special::NestedProbe => {
            assert!(file_type(&path.join("a/.git/HEAD")).is_fifo());
            points_at_sparse(&path.join("b/.git/HEAD"));
            assert_eq!(
                std::fs::read_to_string(path.join("c/.git")).unwrap(),
                "gitdir: ../d\n"
            );
            assert!(file_type(&path.join("d/HEAD")).is_fifo());
        }
        Special::ExecType => {
            let run = path.join("run.sh");
            if r.target.as_deref() == Some("run.sh:fifo") {
                assert!(file_type(&run).is_fifo());
            } else {
                points_at_sparse(&run);
            }
        }
        Special::BigTracked => {
            let len = std::fs::metadata(path.join("data.txt")).unwrap().len();
            assert_eq!(len, BIG_TRACKED_LEN);
        }
        Special::PatternBlobBomb => {
            let listing = git_ok(w, &path, &["ls-files", "-s", "-t"]);
            let big: Vec<&str> = listing.lines().filter(|l| l.contains("/.git")).collect();
            let want = if r.id.ends_with("ignore") { 8 } else { 1 };
            assert_eq!(big.len(), want, "{listing}");
            for line in big {
                let id = line.split_whitespace().nth(2).unwrap();
                let size: u64 = git_ok(w, &path, &["cat-file", "-s", id])
                    .trim()
                    .parse()
                    .unwrap();
                assert!(size == 307_200 || size == 204_800, "blob size {size}");
            }
            if r.id.ends_with("ignore") {
                let skipped = listing.lines().filter(|l| l.starts_with("S ")).count();
                assert_eq!(skipped, 8, "{listing}");
            }
        }
        Special::RenameBomb => {
            let status = git_ok(w, &path, &["status", "--porcelain"]);
            assert!(status.contains("big-a.txt -> big-b.txt"), "{status}");
        }
        Special::TreeBomb | Special::TreeBombLong => {
            let tree = git_ok(w, &path, &["cat-file", "-p", "HEAD^{tree}"]);
            let want = if special == Special::TreeBomb {
                201
            } else {
                1000
            };
            assert_eq!(tree.lines().count(), want);
        }
        Special::BigObject => {
            // Reading the corrupted tree is an error for git too, so take its id from the commit.
            let commit = git_ok(w, &path, &["cat-file", "-p", "HEAD"]);
            let tree = commit.strip_prefix("tree ").unwrap()[..40].to_string();
            let tree = tree.as_str();
            let obj = gitdir.join("objects").join(&tree[..2]).join(&tree[2..]);
            let script =
                "import sys,zlib\nprint(zlib.decompress(open(sys.argv[1],'rb').read())[:16])";
            assert_eq!(python_out(python, script, &obj), "b'tree 1073741824\\x00'");
        }
        Special::EnclosingRoot => assert!(path.join("sub/dir/keep.txt").is_file()),
        Special::Drivers => {
            let cfg = std::fs::read_to_string(gitdir.join("config")).unwrap();
            assert!(
                cfg.contains("marker-filter-x") && cfg.contains("marker-fsmonitor"),
                "{cfg}"
            );
            assert!(path.join(".hooks/pre-commit").is_file());
        }
        Special::SubmoduleLfs => assert!(path.join(".gitmodules").is_file()),
    }
}

fn assert_world(w: &World, rows: &[&Repo], python: &Path) {
    for r in rows {
        let path = row_path(w, r);
        assert!(
            std::fs::symlink_metadata(&path).is_ok(),
            "{} ({}) was not built at {}",
            r.id,
            r.path,
            path.display()
        );
        match (r.kind, r.special) {
            (Kind::Special, Some(special)) => assert_special(w, r, special, python),
            (Kind::Bare, _) => {
                assert!(path.join("HEAD").is_file() && path.join("objects").is_dir())
            }
            (_, _) => assert!(path.join(".git").exists(), "{} has no .git", r.id),
        }
    }
}

#[test]
fn f_clean_fsck() {
    let w = f_clean();
    let m = manifest();
    let mut checked = 0;
    let in_f_clean = |r: &&Repo| matches!(r.root, RootId::F | RootId::Ext) && r.in_clean();
    for r in m.repo.iter().filter(in_f_clean) {
        let path = row_path(w, r);
        let is_repo = r.kind == Kind::Bare || path.join(".git").exists();
        if !is_repo {
            continue; // directory fixtures: fan-out, symlink out of root
        }
        let out = git_in(w, &path, &["fsck", "--strict"]);
        assert!(
            out.status.success(),
            "git fsck failed on {} ({}): {}",
            r.id,
            path.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        checked += 1;
    }
    assert!(
        checked >= MIN_FSCK_REPOS,
        "only {checked} repos were fsck'd"
    );

    // The same check must be able to fail: the corrupted repo lives in F, not F_clean.
    let corrupted = m
        .repo
        .iter()
        .find(|r| r.special == Some(Special::Corrupted))
        .unwrap();
    let out = git_in(f(), &row_path(f(), corrupted), &["fsck"]);
    assert!(
        !out.status.success(),
        "fsck must fail on the corrupted repo"
    );
}

#[test]
fn build_all_git() {
    let python = require_tool("python3").expect("python3 is required to build fixtures");
    let m = manifest();
    let (wf, wh) = (f(), h());
    let rows = |roots: &[RootId]| -> Vec<&Repo> {
        m.repo.iter().filter(|r| roots.contains(&r.root)).collect()
    };
    assert_world(
        wf,
        &rows(&[RootId::F, RootId::Ext, RootId::Home, RootId::Budget]),
        &python,
    );
    assert_world(wh, &rows(&[RootId::H]), &python);

    // P09 assumption (1): $EXT sorts before $F by path.
    assert!(
        wf.ext < wf.root,
        "{:?} must sort before {:?}",
        wf.ext,
        wf.root
    );
    // [home] files land in the sandboxed HOME.
    for (rel, content) in &m.home.files {
        let text = std::fs::read_to_string(wf.sb.home.join(rel)).unwrap();
        assert_eq!(text, wf.tokens.subst(content), "{rel}");
    }
    // Every special the manifest names was exercised by some row.
    let specials: std::collections::BTreeSet<String> = m
        .repo
        .iter()
        .filter_map(|r| r.special)
        .map(|s| format!("{s:?}"))
        .collect();
    assert_eq!(specials.len(), SPECIAL_COUNT, "{specials:?}");
    // Fixed dates: identical history in two repos gives identical commit ids (P09: shared `init`).
    let find = |id: &str| m.repo.iter().find(|r| r.id == id).unwrap();
    let first = |r: &Repo| {
        let p = row_path(wf, r);
        git_ok(wf, &p, &["rev-list", "--max-parents=0", "main"])
            .trim()
            .to_string()
    };
    assert_eq!(first(find("f-ahead-behind")), first(find("x-origin-b")));
}
