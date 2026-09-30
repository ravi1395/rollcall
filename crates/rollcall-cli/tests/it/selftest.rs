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
const SPECIAL_COUNT: usize = 31;

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

const FLOOD_FILES: usize = 10_001;
const DEEP_TREE_BYTES: usize = 26 << 20;
const OFS_LINKS: usize = 8_000_000;
const EVIL_PACK_FILE_LEN: u64 = 256 << 20;
const PACK_HEADER_LEN: usize = 12;
const OBJECT_ID_LEN: usize = 20;
const EVIL_TREE_NODES: usize = 1025;

fn count_occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}

fn config_of(gitdir: &Path) -> String {
    std::fs::read_to_string(gitdir.join("config")).unwrap()
}

/// The repo-local config carries the gitoxide key(s); the hostile file each one names exists.
fn assert_gitoxide_keys(w: &World, r: &Repo, path: &Path, gitdir: &Path, python: &Path) {
    let cfg = config_of(gitdir);
    match r.target.as_deref().unwrap() {
        "shallow_dev_zero" => assert!(cfg.contains("shallowFile = \"/dev/zero\""), "{cfg}"),
        "shallow_fifo" => {
            let fifo = gitdir.join("shallow-fifo");
            assert!(
                cfg.contains(&format!("shallowFile = \"{}\"", fifo.display())),
                "{cfg}"
            );
            assert!(file_type(&fifo).is_fifo());
        }
        "index_file_chain" => {
            let evil = gitdir.join("evil");
            assert!(
                cfg.contains(&format!("indexFile = \"{}\"", evil.display())),
                "{cfg}"
            );
            let bytes = std::fs::read(&evil).unwrap();
            assert_eq!(&bytes[..4], b"DIRC");
            assert!(index_trailer_is_valid(python, &evil), "evil index checksum");
            // One TREE node per level: every node but the last has one child.
            assert_eq!(count_occurrences(&bytes, b"\0-1 1\n"), EVIL_TREE_NODES - 1);
            assert_eq!(count_occurrences(&bytes, b"\0-1 0\n"), 1);
            // The repo's own index is the ordinary clean one (git ignores the gitoxide key).
            assert_eq!(git_ok(w, path, &["ls-files"]).trim(), "README.md");
            assert!(index_trailer_is_valid(python, &gitdir.join("index")));
        }
        "replace_ref_base" => {
            assert!(cfg.contains("useReplaceRefs = \"false\""), "{cfg}");
            assert!(cfg.contains("replaceRefBase = \"objects/pack/\""), "{cfg}");
            let evil = std::fs::metadata(gitdir.join("objects/pack/evil")).unwrap();
            assert!(evil.is_file());
            assert_eq!(evil.len(), EVIL_PACK_FILE_LEN);
        }
        other => panic!("gitoxide_keys target {other}"),
    }
}

/// The one pack and its `.idx`: header bytes, entry bytes and both SHA-1 trailers, checked independently
/// of the builder. Real git runs only on the ordinary objects of variant (ii): a self-referencing or
/// 8,000,000-link delta pack is exactly what makes a reader loop, so variants (i) and (iii) are never
/// given to git (`git verify-pack` on (i) would fail or hang, and is not run).
fn assert_pack_bomb(w: &World, r: &Repo, path: &Path, gitdir: &Path, python: &Path) {
    let target = r.target.as_deref().unwrap();
    let dir = gitdir.join("objects/pack");
    let mut names: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    names.sort();
    assert_eq!(names.len(), 2, "{names:?}");
    let (idx_path, pack_path) = (&names[0], &names[1]);
    assert_eq!(idx_path.extension().unwrap(), "idx");
    assert_eq!(pack_path.extension().unwrap(), "pack");
    assert_eq!(idx_path.file_stem(), pack_path.file_stem());
    let stem = pack_path.file_stem().unwrap().to_str().unwrap();
    assert!(stem.starts_with("pack-") && stem.len() == 5 + 40, "{stem}");

    let pack = std::fs::read(pack_path).unwrap();
    let idx = std::fs::read(idx_path).unwrap();
    assert_eq!(&pack[..4], b"PACK");
    assert_eq!(&pack[4..8], &2u32.to_be_bytes());
    assert_eq!(&idx[..8], b"\xfftOc\0\0\0\x02");
    assert!(index_trailer_is_valid(python, pack_path), "pack checksum");
    assert!(index_trailer_is_valid(python, idx_path), "idx checksum");
    assert_eq!(
        idx[idx.len() - 40..idx.len() - 20],
        pack[pack.len() - 20..],
        "idx names its pack"
    );

    let count = u32::from_be_bytes(pack[8..12].try_into().unwrap()) as usize;
    let id_at = PACK_HEADER_LEN + 1;
    match target {
        "ref_delta_self_head" | "ref_delta_self_attr" => {
            assert_eq!(count, 1);
            // Type 7 (REF_DELTA); the low size nibble is the declared inflated size: 0, or 2 for the delta header.
            let want = if target == "ref_delta_self_head" {
                0x70
            } else {
                0x72
            };
            assert_eq!(pack[PACK_HEADER_LEN], want);
            // The base id is the entry's own id, the only id the .idx lists.
            let own = &idx[8 + 256 * 4..8 + 256 * 4 + OBJECT_ID_LEN];
            assert_eq!(&pack[id_at..id_at + OBJECT_ID_LEN], own);
        }
        "ofs_delta_chain:8000000" => {
            assert_eq!(count, OFS_LINKS + 1);
            // One 9-byte blob, then OFS_DELTA links of two bytes each (type 6, size 0; distance 9 then 2).
            assert_eq!(pack.len(), PACK_HEADER_LEN + 9 + 2 * OFS_LINKS + 20);
            assert_eq!(
                &pack[PACK_HEADER_LEN + 9..PACK_HEADER_LEN + 13],
                b"\x60\x09\x60\x02"
            );
            assert_eq!(&pack[pack.len() - 22..pack.len() - 20], b"\x60\x02");
        }
        other => panic!("pack_bomb target {other}"),
    }
    if target == "ref_delta_self_attr" {
        assert_eq!(
            git_ok(w, path, &["cat-file", "-t", "HEAD"]).trim(),
            "commit"
        );
        assert_eq!(
            git_ok(w, path, &["cat-file", "-t", "HEAD^{tree}"]).trim(),
            "tree"
        );
        let staged = git_ok(w, path, &["ls-files", "-s"]);
        assert!(staged.contains("0\tsub/.gitattributes"), "{staged}");
    }
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
        Special::GitoxideKeys => assert_gitoxide_keys(w, r, &path, &gitdir, python),
        Special::PackBomb => assert_pack_bomb(w, r, &path, &gitdir, python),
        Special::ObjectsPackFlood => {
            let entries: Vec<_> = std::fs::read_dir(gitdir.join("objects/pack"))
                .unwrap()
                .map(|e| e.unwrap())
                .collect();
            assert_eq!(entries.len(), FLOOD_FILES);
            for e in entries {
                let meta = e.metadata().unwrap();
                assert!(meta.is_file() && meta.len() == 0, "{:?}", e.path());
            }
        }
        Special::TreeDeepBig => {
            // Git never parses a tree to report its size; the chain is five trees, the last entry a blob.
            for (levels, path_in_tree) in ["", "a", "a/a", "a/a/a", "a/a/a/a"].iter().enumerate() {
                let spec = format!("HEAD:{path_in_tree}");
                assert_eq!(git_ok(w, &path, &["cat-file", "-t", &spec]).trim(), "tree");
                let size = git_ok(w, &path, &["cat-file", "-s", &spec]);
                assert_eq!(size.trim(), DEEP_TREE_BYTES.to_string(), "level {levels}");
            }
            let bottom = git_ok(w, &path, &["cat-file", "-t", "HEAD:a/a/a/a/a"]);
            assert_eq!(bottom.trim(), "blob");
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

// ------------------------------------------------------- session builder (P28)

use super::build_sessions::{
    CodexMode, build_big_log, build_corpora, build_demo_root, build_mcp_200, build_perf_root,
    log_bytes,
};
use rusqlite::{Connection, OpenFlags};

const ALL_CORPORA: [Corpus; 7] = [
    Corpus::Matching,
    Corpus::ClaudeRules,
    Corpus::CodexRules,
    Corpus::Incremental,
    Corpus::Privacy,
    Corpus::Sanitise,
    Corpus::Shell,
];
const STATE_DB: &str = "state_5.sqlite";
const BIG_LOG_BYTES: u64 = 64 * 1024 * 1024;
const OVERSIZE_LINE: usize = 1 << 20;
/// Rollouts under `$CODEX_HOME` in fallback mode (`expect_counts`: 10 sessions) plus 2 children.
const FALLBACK_ROLLOUTS: usize = 10 + 2;
/// `threads` rows of matching + codex_rules (`expect_counts`: 10 sessions) plus 3 children.
const DB_THREADS: i64 = 10 + 3;
/// Claude files of claude_rules + matching + incremental (`expect_counts`: 43 sessions) plus 3 excluded.
const CLAUDE_FILES: usize = 43 + 3;

fn files_under(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.map(Result::unwrap) {
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            files_under(&path, ext, out);
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
}

fn jsonl_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    files_under(dir, "jsonl", &mut out);
    out
}

fn open_db(codex_home: &Path) -> Connection {
    Connection::open_with_flags(codex_home.join(STATE_DB), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open state_5.sqlite")
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).expect(sql)
}

#[test]
fn schemas() {
    let dir = repo_root().join("schema/examples");
    let (mut valid, mut invalid) = (0, 0);
    for entry in std::fs::read_dir(&dir).unwrap().map(Result::unwrap) {
        let name = entry.file_name().into_string().unwrap();
        let (schema, kind) = name
            .strip_suffix(".json")
            .and_then(|stem| stem.split_once('.'))
            .unwrap_or_else(|| panic!("example {name} is not <schema>.<kind>.json"));
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(entry.path()).unwrap()).unwrap();
        let result = validate(schema, &doc);
        if kind == "valid" {
            assert_eq!(result, Ok(()), "{name} must validate");
            valid += 1;
        } else if kind.starts_with("invalid-") {
            assert!(result.is_err(), "{name} must fail validation");
            invalid += 1;
        } else {
            panic!("example {name}: kind must be `valid` or `invalid-*`");
        }
    }
    assert!(
        valid >= 4 && invalid >= 4,
        "{valid} valid, {invalid} invalid examples"
    );
}

#[test]
fn line_tokens() {
    let tmp = tempfile::tempdir().unwrap();
    let mut tokens = Tokens::new(tmp.path().join("fifos"));
    tokens.add("$F", "/f".into());
    let lines = [
        r#"{"cwd":"$F/a","pad":"$PAD:5"}"#.to_string(),
        r#"{"partial":"x"}$NOEOL"#.to_string(),
    ];
    let out = String::from_utf8(log_bytes(&tokens, &lines)).unwrap();
    assert_eq!(
        out,
        "{\"cwd\":\"/f/a\",\"pad\":\"xxxxx\"}\n{\"partial\":\"x\"}"
    );
}

#[test]
fn build_all_sessions() {
    let m = manifest();
    let w = fresh(Which::H);
    build_corpora(&w, &ALL_CORPORA, CodexMode::Db);
    for r in &m.claude {
        let path = w.sb.claude_projects().join(&r.project_dir).join(&r.file);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("claude row {}: {}: {e}", r.id, path.display()));
        assert!(!text.contains("$PAD"), "{}", r.id);
        assert_eq!(text.lines().count(), r.lines.len(), "{}", r.id);
    }
    let oversize = m
        .claude
        .iter()
        .find(|r| r.lines.iter().any(|l| l.contains("$PAD:")))
        .expect("a row carries a $PAD line");
    let path =
        w.sb.claude_projects()
            .join(&oversize.project_dir)
            .join(&oversize.file);
    let longest = std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::len)
        .max();
    assert!(longest > Some(OVERSIZE_LINE), "the $PAD line is oversize");

    let conn = open_db(&w.sb.codex_home);
    let mut stmt = conn
        .prepare("SELECT name FROM pragma_table_info('threads') ORDER BY name")
        .unwrap();
    let columns: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut required: Vec<String> = rollcall_sessions::codex_db::REQUIRED_COLUMNS
        .map(String::from)
        .to_vec();
    required.sort();
    assert_eq!(columns, required, "threads columns drifted from §5.2");
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM threads"),
        m.codex_thread.len() as i64
    );
    let spawned = m
        .codex_thread
        .iter()
        .filter(|t| t.spawn_parent.is_some())
        .count() as i64;
    assert!(spawned >= 1);
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM thread_spawn_edges"),
        spawned
    );
    // A linked rollout exists, even the one outside $CODEX_HOME.
    let linked = m.codex_thread.iter().filter(|t| t.rollout_id.is_some());
    for t in linked {
        let path: String = conn
            .query_row(
                "SELECT rollout_path FROM threads WHERE id = ?1",
                [&t.id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(Path::new(&path).is_file(), "thread {}: {path}", t.id);
    }
}

#[test]
fn corpus_selection() {
    let m = manifest();
    let in_corpus = |c: Corpus| m.claude.iter().filter(|r| r.corpus == c).count();
    let w = fresh(Which::H);
    build_corpora(&w, &[Corpus::Matching, Corpus::CodexRules], CodexMode::Db);
    assert_eq!(
        count(&open_db(&w.sb.codex_home), "SELECT COUNT(*) FROM threads"),
        DB_THREADS
    );
    assert_eq!(
        jsonl_under(&w.sb.claude_projects()).len(),
        in_corpus(Corpus::Matching)
    );

    let fb = fresh(Which::H);
    let corpora = [Corpus::ClaudeRules, Corpus::CodexRules, Corpus::Incremental];
    build_corpora(&fb, &corpora, CodexMode::Fallback);
    assert!(
        !fb.sb.codex_home.join(STATE_DB).exists(),
        "fallback mode has no database"
    );
    assert_eq!(jsonl_under(&fb.sb.codex_home).len(), FALLBACK_ROLLOUTS);
    assert_eq!(
        jsonl_under(&fb.sb.claude_projects()).len(),
        in_corpus(Corpus::ClaudeRules) + in_corpus(Corpus::Incremental)
    );

    let all = fresh(Which::H);
    build_corpora(
        &all,
        &[Corpus::ClaudeRules, Corpus::Matching, Corpus::Incremental],
        CodexMode::Db,
    );
    assert_eq!(jsonl_under(&all.sb.claude_projects()).len(), CLAUDE_FILES);
}

#[test]
fn big_log_and_mcp_200() {
    let w = fresh(Which::H);
    build_big_log(&w);
    let logs = jsonl_under(&w.sb.claude_projects());
    assert_eq!(logs.len(), 1);
    let bytes = std::fs::read(&logs[0]).unwrap();
    assert_eq!(bytes.len() as u64, BIG_LOG_BYTES);
    assert!(!bytes.contains(&b'\n'), "one line, no newline");

    let w = fresh(Which::H);
    build_mcp_200(&w);
    let logs = jsonl_under(&w.sb.claude_projects());
    assert_eq!(logs.len(), manifest().mcp_200.sessions as usize);
    let ids: std::collections::BTreeSet<_> = logs.iter().map(|p| p.file_stem().unwrap()).collect();
    assert_eq!(ids.len(), logs.len(), "session ids are distinct");
}

#[test]
fn perf_builder_is_exact() {
    let spec = Perf {
        repos: 6,
        linked_worktrees: 2,
        logs: 10,
        bytes: 10 * 30_000 + 7,
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("perf");
    std::fs::create_dir_all(&root).unwrap();
    assert_eq!(build_perf_root(&root, &spec), spec.bytes);
    let logs = jsonl_under(&root.join(".agents"));
    assert_eq!(logs.len(), spec.logs as usize);
    let total: u64 = logs
        .iter()
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum();
    assert_eq!(total, spec.bytes);
    for log in &logs {
        for line in std::fs::read_to_string(log).unwrap().lines() {
            serde_json::from_str::<serde_json::Value>(line).expect("every log line is JSON");
        }
    }
    let repos: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join(".git").exists())
        .collect();
    assert_eq!(repos.len(), spec.repos as usize);
    let linked = repos.iter().filter(|p| p.join(".git").is_file()).count();
    assert_eq!(linked, spec.linked_worktrees as usize);
}

#[test]
fn demo_builder() {
    let m = manifest();
    let tmp = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(tmp.path()).unwrap();
    let root = base.join("demo");
    std::fs::create_dir_all(&root).unwrap();
    build_demo_root(&root);
    let sb = Sandbox::new(&base.join("sandbox"));
    let git = |dir: &str, args: &[&str]| -> String {
        let mut cmd = sb.command(git_path());
        cmd.current_dir(root.join(dir)).args(args);
        let out = spawn_watchdog(cmd, FSCK_DEADLINE).unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    for id in &m.demo.repos {
        let r = m.repo.iter().find(|r| &r.id == id).unwrap();
        assert!(root.join(&r.path).join(".git").exists(), "{id}");
        for wt in &r.worktrees {
            assert!(
                root.join(&wt.path).join(".git").is_file(),
                "{id} worktree {}",
                wt.path
            );
        }
    }
    let ahead_behind = git(
        "api-gateway",
        &["rev-list", "--left-right", "--count", "main...origin/main"],
    );
    assert_eq!(ahead_behind, "2\t1");
    let status = git("api-gateway", &["status", "--porcelain"]);
    assert!(
        status.contains("M README.md") && status.contains("?? scratch.md"),
        "{status}"
    );
    let logs = jsonl_under(&root.join(".agents"));
    assert_eq!(logs.len(), m.demo.claude.len());
    let root_str = root.to_str().unwrap();
    for log in logs {
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(
            !text.contains("$DEMO") && text.contains(root_str),
            "{}",
            log.display()
        );
    }
    let conn = open_db(&root.join(".agents/codex"));
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM threads"),
        m.demo.codex_thread.len() as i64
    );
    let cwd: String = conn
        .query_row(
            "SELECT cwd FROM threads WHERE id = ?1",
            [&m.demo.codex_thread[0].id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(cwd.starts_with(root_str), "{cwd}");
}
