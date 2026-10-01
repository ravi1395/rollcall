//! §12.1 #8 (P35): hostile repos. The positive controls run first on their own `fresh(H)` builds; one
//! `rollcall scan H` on one more pristine `fresh(H)` then backs every row, count, time, memory and marker assertion.
use super::support::{
    RUN_DEADLINE, Repo, Which, World, fresh, git_path, manifest, peak_rss, require_tool,
    run_rollcall_with, spawn_watchdog,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const TIME_BIN: &str = "/usr/bin/time";
const SCAN_WALL_LIMIT: Duration = Duration::from_secs(10);
/// §10.2 "512 MB" is 512,000,000 bytes (≈ 488 MiB, §4.4), the stricter reading.
const SCAN_RSS_LIMIT: u64 = 512_000_000;
/// Watchdog only: the spec bound is asserted on the measured wall time after the run.
const SCAN_WATCHDOG: Duration = Duration::from_secs(180);
const MARKER_PREFIX: &str = "marker-";
/// Each fake binary writes `marker-path-<name>` under the scan root when something runs it.
const FAKE_TOOLS: [&str; 4] = ["git", "git-lfs", "claude", "codex"];
const UNTRUSTED_GIT_DIR: &str = "untrusted git dir";
const UNTRUSTED_WORKTREE: &str = "untrusted worktree path";
const TREE_TOO_LARGE: &str = "git error: HEAD tree too large";

/// The §10.2 global gitconfig (`filter.lfs.*`, `safe.directory=*`) in the world's sandbox `HOME`, and the fake
/// `git`, `git-lfs`, `claude`, `codex` first on the returned `PATH`.
fn arm(w: &World) -> String {
    let root = w.root.display();
    // Double-quoted: an unquoted `;` starts a comment in a gitconfig value. Git prefers `process` over
    // clean/smudge; the stand-in fails its handshake, but it has touched the marker by then.
    let touch = |name: &str| format!("\"sh -c 'touch {root}/{name}; cat'\"");
    let config = format!(
        "[safe]\n\tdirectory = *\n[filter \"lfs\"]\n\tclean = {}\n\tsmudge = {}\n\tprocess = {}\n",
        touch("marker-filter-lfs"),
        touch("marker-filter-lfs"),
        touch("marker-filter-lfs"),
    );
    std::fs::write(w.sb.home.join(".gitconfig"), config).expect("write global gitconfig");
    let fakes = w.base.join("fakes");
    std::fs::create_dir_all(&fakes).expect("create fakes dir");
    for name in FAKE_TOOLS {
        let path = fakes.join(name);
        std::fs::write(
            &path,
            format!("#!/bin/sh\ntouch {root}/marker-path-{name}\n"),
        )
        .expect("write fake tool");
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .expect("chmod fake tool");
    }
    let git_dir = git_path().parent().expect("git has a parent directory");
    std::env::join_paths([
        fakes.as_path(),
        git_dir,
        Path::new("/usr/bin"),
        Path::new("/bin"),
    ])
    .expect("PATH entries contain no ':'")
    .into_string()
    .expect("PATH is UTF-8")
}

fn markers_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("entry in {}: {e}", dir.display()));
        let path = entry.path();
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(MARKER_PREFIX)
        {
            out.push(path.clone());
        }
        // `file_type` does not follow symlinks, so the 4 GiB sparse targets are never entered.
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            markers_under(&path, out);
        }
    }
}

fn markers(w: &World) -> Vec<PathBuf> {
    let mut out = Vec::new();
    markers_under(&w.root, &mut out);
    out.sort();
    out
}

// ------------------------------------------------------------ positive controls

/// Runs the real git (absolute path) with `args` in `<H>/<dir>` and asserts `marker` appears. The precondition
/// (no marker yet) and the fresh build keep the control from touching the tree under test.
fn real_git_fires(dir: &str, args: &[&str], marker: &str) {
    let w = fresh(Which::H);
    arm(&w);
    let marker_path = w.root.join(marker);
    assert!(
        !marker_path.exists(),
        "{marker} exists before the control ran"
    );
    let mut cmd = w.sb.command(git_path());
    cmd.current_dir(w.root.join(dir)).args(args);
    let out = spawn_watchdog(cmd, RUN_DEADLINE).expect("real git status");
    assert!(
        marker_path.exists(),
        "real git {args:?} in {dir} did not create {marker}: the control proves nothing\nstderr: {}\nmarkers: {:?}",
        String::from_utf8_lossy(&out.stderr),
        markers(&w)
    );
}

#[test]
fn control_real_git_runs_the_drivers_filter() {
    real_git_fires("drivers", &["status", "--porcelain"], "marker-filter-x");
}

#[test]
fn control_real_git_runs_the_submodule_lfs_filter() {
    real_git_fires(
        "submodule-lfs",
        &["status", "--porcelain", "--ignore-submodules=none"],
        "marker-filter-lfs",
    );
}

/// Control (2): gix opened with its default options runs the `drivers` filter, so rollcall's config
/// stripping, not gix's defaults, is what keeps the scan from running it.
#[test]
fn control_gix_defaults_run_the_drivers_filter() {
    let w = fresh(Which::H);
    arm(&w);
    let marker_path = w.root.join("marker-filter-x");
    assert!(
        !marker_path.exists(),
        "marker-filter-x exists before the control ran"
    );
    let drivers = w.root.join("drivers");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(rollcall_git::trust::status_with_gix_defaults(&drivers));
    });
    // The stand-in filter may fail gix's read of its output: only the marker matters.
    let result = rx
        .recv_timeout(RUN_DEADLINE)
        .expect("gix default status finished before the deadline");
    assert!(
        marker_path.exists(),
        "gix defaults in drivers did not create marker-filter-x: the control proves nothing\nresult: {result:?}\nmarkers: {:?}",
        markers(&w)
    );
}

// ------------------------------------------------------------------- the scan

struct Scan {
    /// Keeps the fixture directory alive, and names the root every row path is relative to.
    world: World,
    wall: Duration,
    rss: u64,
    code: i32,
    stderr: String,
    rows: Vec<Value>,
    markers: Vec<PathBuf>,
}

/// One `rollcall scan H` under `/usr/bin/time`, then the rows it stored (read back with `status --json
/// --no-scan`). `None` when GNU/BSD `time` is missing locally.
fn scan_h() -> Option<&'static Scan> {
    static SCAN: OnceLock<Option<Scan>> = OnceLock::new();
    SCAN.get_or_init(|| {
        require_tool(TIME_BIN)?;
        // `fresh`, not the shared `h()`: this run writes a gitconfig and a cache into its world.
        let w = fresh(Which::H);
        let path = arm(&w);
        let root = w.root.to_str().expect("UTF-8 root");
        let env = [("PATH", path.as_str())];
        let start = Instant::now();
        let (out, rss) = peak_rss(w.sb.rollcall(&["scan", root], &env), SCAN_WATCHDOG);
        let wall = start.elapsed();
        let code = out.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let status = run_rollcall_with(
            &w,
            &["status", "--json", "--no-scan", root],
            &env,
            RUN_DEADLINE,
        );
        let doc: Value = serde_json::from_str(&status.stdout)
            .unwrap_or_else(|e| panic!("status --json: {e}\n{}", status.stderr));
        let rows = doc["repos"].as_array().expect("repos array").clone();
        let markers = markers(&w);
        Some(Scan {
            world: w,
            wall,
            rss,
            code,
            stderr,
            rows,
            markers,
        })
    })
    .as_ref()
}

fn repo_spec(id: &str) -> &'static Repo {
    manifest()
        .repo
        .iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("no [[repo]] with id {id}"))
}

fn row<'a>(s: &'a Scan, rel: &str) -> &'a Value {
    let path = s.world.root.join(rel);
    s.rows
        .iter()
        .find(|r| r["path"].as_str() == path.to_str())
        .unwrap_or_else(|| {
            let paths: Vec<_> = s.rows.iter().map(|r| r["path"].as_str()).collect();
            panic!("no row for {}; rows: {paths:#?}", path.display())
        })
}

/// The repo-level error and every worktree's error of the row for manifest repo `id`.
fn errors(s: &Scan, id: &str) -> Vec<String> {
    let r = row(s, &repo_spec(id).path);
    std::iter::once(&r["error"])
        .chain(
            r["worktrees"]
                .as_array()
                .expect("worktrees")
                .iter()
                .map(|wt| &wt["error"]),
        )
        .filter_map(|e| e.as_str().map(str::to_owned))
        .collect()
}

fn assert_carries(s: &Scan, id: &str, prefix: &str) {
    let errs = errors(s, id);
    assert!(
        errs.iter().any(|e| e.starts_with(prefix)),
        "{id}: expected an error starting {prefix:?}, got {errs:?}"
    );
}

fn assert_no_error(s: &Scan, id: &str) {
    let errs = errors(s, id);
    assert!(errs.is_empty(), "{id}: expected no error, got {errs:?}");
}

fn ids_covering(tag: &str) -> Vec<&'static str> {
    let ids: Vec<_> = manifest()
        .repo
        .iter()
        .filter(|r| r.root == super::support::RootId::H && r.covers.iter().any(|c| c == tag))
        .filter(|r| r.kind != super::support::Kind::Submodule)
        .map(|r| r.id.as_str())
        .collect();
    assert!(!ids.is_empty(), "no H repo covers {tag:?}");
    ids
}

fn assert_all_carry(s: &Scan, tag: &str, prefix: &str) {
    for id in ids_covering(tag) {
        assert_carries(s, id, prefix);
    }
}

fn first_worktree_count(s: &Scan, id: &str, key: &str) -> i64 {
    row(s, &repo_spec(id).path)["worktrees"][0][key]
        .as_i64()
        .unwrap_or_else(|| panic!("{id}: worktrees[0].{key} is not an integer"))
}

#[test]
fn scan_h_stays_inside_its_time_and_memory_bounds() {
    let Some(s) = scan_h() else { return };
    eprintln!(
        "scan H: wall {:?}, peak RSS {} bytes ({:.1} MiB), exit {}",
        s.wall,
        s.rss,
        s.rss as f64 / (1 << 20) as f64,
        s.code
    );
    for r in &s.rows {
        let errs: Vec<_> = std::iter::once(&r["error"])
            .chain(
                r["worktrees"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|wt| &wt["error"]),
            )
            .filter_map(Value::as_str)
            .collect();
        eprintln!("  {} {errs:?}", r["path"]);
    }
    assert!(
        s.wall < SCAN_WALL_LIMIT,
        "wall {:?}; stderr: {}",
        s.wall,
        s.stderr
    );
    assert!(s.rss < SCAN_RSS_LIMIT, "peak RSS {} bytes", s.rss);
    // Error rows make the scan exit 1 (§3.1); anything else means it did not finish the way §8 needs.
    assert_eq!(s.code, 1, "stderr: {}", s.stderr);
}

#[test]
fn scan_h_runs_no_driver_hook_or_fake_binary() {
    let Some(s) = scan_h() else { return };
    assert!(s.markers.is_empty(), "markers created: {:?}", s.markers);
    // Vacuity guard: status did run on the repo whose filter would have fired.
    assert_no_error(s, "h-drivers");
    assert!(first_worktree_count(s, "h-drivers", "dirty_count") >= 1);
}

#[test]
fn scan_h_refuses_symlinked_files_and_hang_probes() {
    let Some(s) = scan_h() else { return };
    assert_all_carry(s, "hostile: symlinked-files", UNTRUSTED_GIT_DIR);
    assert_all_carry(s, "hostile: hang-probe", UNTRUSTED_GIT_DIR);
}

#[test]
fn scan_h_strips_repo_local_excludes_file_and_keeps_the_row_clean() {
    let Some(s) = scan_h() else { return };
    assert_no_error(s, "h-excludes-file");
}

#[test]
fn scan_h_refuses_redirected_worktrees() {
    let Some(s) = scan_h() else { return };
    assert_carries(s, "h-worktree-escape", UNTRUSTED_WORKTREE);
    assert_carries(s, "h-commondir-escape", UNTRUSTED_WORKTREE);
}

#[test]
fn scan_h_bounds_the_head_tree() {
    let Some(s) = scan_h() else { return };
    assert_carries(s, "h-tree-bomb", TREE_TOO_LARGE);
    assert_carries(s, "h-tree-bomb-long", TREE_TOO_LARGE);
    // Revision 13: only the tree-bytes counter can trip on this one.
    assert_carries(s, "h-tree-deep-big", TREE_TOO_LARGE);
    // §10.2 lists no error for `big-object`; it must still finish with a row.
    let _ = row(s, &repo_spec("h-big-object").path);
}

#[test]
fn scan_h_rejects_index_bombs_and_accepts_the_deep_controls() {
    let Some(s) = scan_h() else { return };
    assert_all_carry(s, "hostile: index-bombs", UNTRUSTED_GIT_DIR);
    for id in ids_covering("hostile: index-deep-ok") {
        assert_no_error(s, id);
    }
}

#[test]
fn scan_h_nested_probe_counts_three_untracked_and_never_reads_the_probes() {
    let Some(s) = scan_h() else { return };
    assert_no_error(s, "h-nested-probe");
    assert_eq!(
        first_worktree_count(s, "h-nested-probe", "untracked_count"),
        3
    );
    let base = &repo_spec("h-nested-probe").path;
    for dir in ["a", "b", "c"] {
        let r = row(s, &format!("{base}/{dir}"));
        let err = r["error"].as_str().unwrap_or_default();
        assert!(err.starts_with(UNTRUSTED_GIT_DIR), "{base}/{dir}: {r}");
    }
}

#[test]
fn scan_h_flags_executable_files_replaced_by_non_regular_ones() {
    let Some(s) = scan_h() else { return };
    assert_all_carry(s, "hostile: exec-type", "git error: unsafe tracked file");
}

#[test]
fn scan_h_counts_the_big_tracked_file_without_reading_it() {
    let Some(s) = scan_h() else { return };
    assert_no_error(s, "h-big-tracked");
    assert_eq!(first_worktree_count(s, "h-big-tracked", "dirty_count"), 1);
}

#[test]
fn scan_h_refuses_oversized_pattern_blobs() {
    let Some(s) = scan_h() else { return };
    assert_all_carry(
        s,
        "hostile: pattern-blob-bomb",
        "git error: pattern files too large",
    );
}

#[test]
fn scan_h_rename_bomb_is_a_deletion_plus_an_addition() {
    let Some(s) = scan_h() else { return };
    assert_no_error(s, "h-rename-bomb");
    assert_eq!(first_worktree_count(s, "h-rename-bomb", "dirty_count"), 2);
}

#[test]
fn scan_h_overrides_the_gitoxide_path_keys() {
    let Some(s) = scan_h() else { return };
    for id in ids_covering("hostile: gitoxide-keys") {
        assert_no_error(s, id);
    }
}

#[test]
fn scan_h_pack_bombs_each_end_in_their_own_way() {
    let Some(s) = scan_h() else { return };
    for id in ids_covering("hostile: pack-bombs") {
        eprintln!("{id}: {:?}", errors(s, id));
    }
    // (i) the memory monitor fired on a growing delta cycle (the positive control for the monitor).
    assert_carries(
        s,
        "h-pack-bomb-ref-delta-self-head",
        "git error: memory limit",
    );
    // (ii) the worker process ended a flat-memory loop at its deadline.
    assert_carries(s, "h-pack-bomb-ref-delta-self-attr", "timed out");
    // (iii) whichever way the chain ends, it ends in a git error.
    assert_carries(s, "h-pack-bomb-ofs-delta-chain", "git error: ");
}

#[test]
fn scan_h_refuses_a_flooded_object_pack_directory() {
    let Some(s) = scan_h() else { return };
    assert_carries(s, "h-objects-pack-flood", UNTRUSTED_GIT_DIR);
}

#[test]
fn marker_walk_finds_nested_markers_and_ignores_symlink_targets() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let deep = tmp.path().join("a/b");
    std::fs::create_dir_all(&deep).expect("mkdir");
    std::fs::write(deep.join("marker-x"), "").expect("write marker");
    let outside = tempfile::tempdir().expect("tempdir");
    std::fs::write(outside.path().join("marker-y"), "").expect("write marker");
    std::os::unix::fs::symlink(outside.path(), tmp.path().join("link")).expect("symlink");
    let mut found = Vec::new();
    markers_under(tmp.path(), &mut found);
    assert_eq!(found, vec![deep.join("marker-x")]);
}
