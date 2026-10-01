//! §12.1 #12 (P35): idempotent rescan, the scan lock (`stale: true`), rebuild on a `user_version` mismatch.
//! Crash-safety is satisfied by the P26 unit test `a_panic_between_inserts_leaves_the_prior_scan_intact`
//! (`crates/rollcall-store/src/lib.rs`), which a guard below keeps from being renamed away.
//!
//! Each test reads the shared read-only `F_clean` through a cache of its own (`HOME` and `XDG_CACHE_HOME`
//! point at a scratch directory), so no other test's scan shares its database or locks.
use super::support::{RUN_DEADLINE, Run, World, f_clean, lock_when_free, run_rollcall_with};
use serde_json::Value;
use std::path::PathBuf;

struct Cache {
    home: String,
    xdg: String,
    dir: PathBuf,
    // Removes the scratch directory on drop.
    _scratch: tempfile::TempDir,
}

impl Cache {
    fn new(w: &World) -> Cache {
        let scratch = w.scratch();
        let home = scratch.path().join("home");
        let xdg = scratch.path().join("xdg-cache");
        std::fs::create_dir_all(&home).expect("create scratch HOME");
        std::fs::create_dir_all(&xdg).expect("create scratch XDG_CACHE_HOME");
        Cache {
            dir: rollcall_store::cache_dir(&home, Some(&xdg)),
            home: home.to_str().expect("UTF-8").to_owned(),
            xdg: xdg.to_str().expect("UTF-8").to_owned(),
            _scratch: scratch,
        }
    }

    fn status(&self, extra: &[&str]) -> (Run, Value) {
        let w = f_clean();
        let root = w.root.to_str().expect("UTF-8 root");
        let mut args = vec!["status", "--json"];
        args.extend_from_slice(extra);
        args.push(root);
        let env = [
            ("HOME", self.home.as_str()),
            ("XDG_CACHE_HOME", self.xdg.as_str()),
        ];
        let run = run_rollcall_with(w, &args, &env, RUN_DEADLINE);
        let doc = serde_json::from_str(&run.stdout)
            .unwrap_or_else(|e| panic!("status --json: {e}\n{}", run.stderr));
        (run, doc)
    }
}

fn repos(doc: &Value) -> &Vec<Value> {
    doc["repos"].as_array().expect("repos array")
}

#[test]
fn two_scans_of_one_root_print_identical_status_json() {
    let cache = Cache::new(f_clean());
    let (first, _) = cache.status(&[]);
    let (second, doc) = cache.status(&[]);
    assert_eq!(first.code, second.code, "{}", second.stderr);
    assert_eq!(first.stdout, second.stdout);
    assert!(!repos(&doc).is_empty());
    assert_eq!(doc["stale"], false);
}

#[test]
fn a_held_scan_lock_answers_from_the_cache_as_stale() {
    let w = f_clean();
    let cache = Cache::new(w);
    let (_, fresh) = cache.status(&[]);
    assert_eq!(fresh["stale"], false);

    // The harness is the other scanner; rollcall keys the lock by the realpath of the root.
    let lock = lock_when_free(&cache.dir, &w.root);
    let (_, held) = cache.status(&[]);
    assert_eq!(held["stale"], true, "{held}");
    assert_eq!(
        repos(&held),
        repos(&fresh),
        "the stale answer is the cached one"
    );

    drop(lock);
    drop(lock_when_free(&cache.dir, &w.root));
    let (_, released) = cache.status(&[]);
    assert_eq!(released["stale"], false);
}

#[test]
fn a_user_version_mismatch_rebuilds_the_cache_empty_then_a_scan_refills_it() {
    let cache = Cache::new(f_clean());
    let (_, fresh) = cache.status(&[]);
    let expected = repos(&fresh).clone();
    assert!(!expected.is_empty());

    let db = cache.dir.join("cache.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open the cache");
    let before: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .expect("read user_version");
    conn.pragma_update(None, "user_version", before + 1000)
        .expect("bump user_version");
    drop(conn);

    // `--no-scan` cannot refill it, so an empty answer proves the old rows were dropped.
    let (run, empty) = cache.status(&["--no-scan"]);
    assert!(repos(&empty).is_empty(), "{empty}");
    assert!(empty["scanned_at"].is_null());
    assert_eq!(run.code, 1, "no scan ever completed: exit 1 (§3.1)");

    let (_, refilled) = cache.status(&[]);
    assert_eq!(repos(&refilled), &expected);
    let conn = rusqlite::Connection::open(&db).expect("reopen the cache");
    let after: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .expect("read user_version");
    assert_eq!(
        after, before,
        "the rebuilt database carries the current version"
    );
}

#[test]
fn crash_safety_is_the_p26_unit_test() {
    let source = std::fs::read_to_string(
        super::support::repo_root().join("crates/rollcall-store/src/lib.rs"),
    )
    .expect("read the store crate");
    assert!(
        source.contains("fn a_panic_between_inserts_leaves_the_prior_scan_intact"),
        "the P26 crash-safety test this gate cites is gone"
    );
}
