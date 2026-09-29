use std::fs::{DirBuilder, OpenOptions, Permissions, TryLockError};
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use rollcall_core::Result;

use crate::ScanLock;

pub(crate) const DB_FILE: &str = "cache.sqlite";

const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

pub fn cache_dir(home: &Path, xdg_cache_home: Option<&Path>) -> PathBuf {
    if cfg!(target_os = "macos") {
        return home.join("Library/Caches/rollcall");
    }
    // The XDG spec says a relative base directory is invalid and must be ignored.
    match xdg_cache_home.filter(|p| p.is_absolute()) {
        Some(xdg) => xdg.join("rollcall"),
        None => home.join(".cache/rollcall"),
    }
}

/// Creates `dir` (and parents) as `0700`, and tightens a pre-existing `dir` to `0700`; the
/// explicit `set_permissions` makes the result independent of the umask.
pub fn ensure_cache_dir(dir: &Path) -> Result<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(dir)?;
    std::fs::set_permissions(dir, Permissions::from_mode(DIR_MODE))?;
    Ok(())
}

/// Takes the per-root scan lock, or `None` when another open file description holds it.
/// `root` is hashed as given: the caller passes the canonical root.
pub fn try_scan_lock(cache_dir: &Path, root: &Path) -> Result<Option<ScanLock>> {
    let mut hasher = gix_hash::hasher(gix_hash::Kind::Sha1);
    hasher.update(root.as_os_str().as_bytes());
    let digest = hasher.try_finalize().map_err(|e| e.to_string())?;
    let path = cache_dir.join(format!("scan-{}.lock", digest.to_hex()));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(FILE_MODE)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(ScanLock { _file: file })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e.into()),
    }
}

/// Deletes the whole cache directory (`rollcall cache clear`); a missing directory is fine.
pub fn clear(cache_dir: &Path) -> Result<()> {
    match std::fs::remove_dir_all(cache_dir) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// The uid new files in `cache_dir` are owned by: create one, read its owner, remove it.
pub fn effective_uid(cache_dir: &Path) -> Result<u32> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let (path, file) = loop {
        let name = format!(
            ".uid-probe-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let path = cache_dir.join(name);
        // A leftover from a crashed run of the same pid is skipped, not reused.
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(FILE_MODE)
            .open(&path)
        {
            Ok(file) => break (path, file),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    };
    let uid = file.metadata().map(|m| m.uid());
    std::fs::remove_file(&path)?;
    Ok(uid?)
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};
    use std::sync::{Mutex, MutexGuard};
    use std::time::{Duration, Instant};

    use rollcall_core::{Index, SourceKind};

    use super::*;
    use crate::SqliteIndex;

    const CHILD_DIR_ENV: &str = "ROLLCALL_TEST_CACHE_DIR";
    const CHILD_ROOT_ENV: &str = "ROLLCALL_TEST_LOCK_ROOT";
    const CHILD_EXPECT_ENV: &str = "ROLLCALL_TEST_EXPECT_LOCK";
    const CHILD_DEADLINE: Duration = Duration::from_secs(60);

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    /// Runs an `#[ignore]`d test of this binary in a child (optionally under `umask 000`),
    /// kills it on the watchdog deadline, and returns its stdout. Fails unless it exited 0
    /// having actually run exactly one test.
    fn run_child(test: &str, umask_prefix: &str, envs: &[(&str, &Path)]) -> String {
        let exe = std::env::current_exe().unwrap();
        let script = format!("{umask_prefix}exec \"$0\" --ignored --exact {test}");
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(script)
            .arg(exe)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        let start = Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if start.elapsed() > CHILD_DEADLINE {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("child {test} exceeded its deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let out = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "child {test} failed:\n{stdout}\n{stderr}"
        );
        assert!(
            stdout.contains("1 passed"),
            "child {test} did not run exactly one test:\n{stdout}"
        );
        stdout
    }

    /// A child forked while another test holds a lock file briefly shares that lock until it
    /// execs, so tests that spawn or assert on locks must not overlap.
    fn serial() -> MutexGuard<'static, ()> {
        static SERIAL: Mutex<()> = Mutex::new(());
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn child_dir() -> PathBuf {
        PathBuf::from(std::env::var_os(CHILD_DIR_ENV).expect("run via its parent test"))
    }

    #[test]
    fn cache_dir_follows_the_platform_convention() {
        let home = Path::new("/home/u");
        let xdg = Path::new("/xdg");
        if cfg!(target_os = "macos") {
            let want = home.join("Library/Caches/rollcall");
            assert_eq!(cache_dir(home, Some(xdg)), want);
            assert_eq!(cache_dir(home, None), want);
        } else {
            assert_eq!(cache_dir(home, Some(xdg)), xdg.join("rollcall"));
            assert_eq!(cache_dir(home, None), home.join(".cache/rollcall"));
            assert_eq!(
                cache_dir(home, Some(Path::new("relative"))),
                home.join(".cache/rollcall"),
                "a relative XDG_CACHE_HOME is ignored"
            );
        }
    }

    #[test]
    fn ensure_cache_dir_creates_and_tightens_to_0700() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a/b/cache");
        ensure_cache_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_cache_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700, "an existing dir is tightened");
    }

    #[test]
    #[ignore = "child of modes_are_explicit_under_umask_000"]
    fn modes_child() {
        let dir = child_dir();
        let mut ix = SqliteIndex::open(&dir).unwrap();
        // A write, so -wal and -shm exist while the connection is still open.
        ix.commit_scan(
            Path::new("/A"),
            crate::tests::claude_scan("/A", false, vec![]),
        )
        .unwrap();
        assert!(ix.scanned_at(Path::new("/A")).is_some());
        assert_eq!(mode(&dir), 0o700, "dir");
        for name in ["cache.sqlite", "cache.sqlite-wal", "cache.sqlite-shm"] {
            assert!(dir.join(name).exists(), "{name} must exist while open");
            assert_eq!(mode(&dir.join(name)), 0o600, "{name}");
        }
        assert_eq!(ix.source_state(SourceKind::Claude).files.len(), 3);
    }

    #[test]
    fn modes_are_explicit_under_umask_000() {
        let _serial = serial();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cache");
        run_child(
            "fs::tests::modes_child",
            "umask 000; ",
            &[(CHILD_DIR_ENV, &dir)],
        );
    }

    #[test]
    #[ignore = "child of scan_lock_is_exclusive_across_processes"]
    fn lock_child() {
        let dir = child_dir();
        let root = PathBuf::from(std::env::var_os(CHILD_ROOT_ENV).unwrap());
        let other = root.join("other");
        let expect = std::env::var(CHILD_EXPECT_ENV).unwrap();
        let mine = try_scan_lock(&dir, &root).unwrap();
        match expect.as_str() {
            "held" => assert!(mine.is_none(), "lock is held by the parent"),
            "free" => assert!(mine.is_some(), "lock was released"),
            bad => panic!("bad expectation {bad}"),
        }
        assert!(
            try_scan_lock(&dir, &other).unwrap().is_some(),
            "another root is independent"
        );
    }

    #[test]
    fn scan_lock_is_exclusive_across_processes() {
        let _serial = serial();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let root = tmp.path().join("root");
        let child = |expect: &str| {
            run_child(
                "fs::tests::lock_child",
                "",
                &[
                    (CHILD_DIR_ENV, &dir),
                    (CHILD_ROOT_ENV, &root),
                    (CHILD_EXPECT_ENV, Path::new(expect)),
                ],
            )
        };

        let held = try_scan_lock(&dir, &root).unwrap().expect("first lock");
        child("held");
        drop(held);
        child("free");
    }

    #[test]
    fn scan_lock_conflicts_in_process_and_releases_on_drop() {
        let _serial = serial();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let a = try_scan_lock(dir, Path::new("/root/a")).unwrap();
        assert!(a.is_some());
        assert!(try_scan_lock(dir, Path::new("/root/a")).unwrap().is_none());
        assert!(try_scan_lock(dir, Path::new("/root/b")).unwrap().is_some());
        drop(a);
        assert!(try_scan_lock(dir, Path::new("/root/a")).unwrap().is_some());
    }

    #[test]
    fn scan_lock_file_is_named_by_the_sha1_of_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let _lock = try_scan_lock(tmp.path(), Path::new("abc"))
            .unwrap()
            .unwrap();
        // SHA-1("abc") is the FIPS 180 test vector.
        let name = "scan-a9993e364706816aba3e25717850c26c9cd0d89d.lock";
        assert!(tmp.path().join(name).exists());
        assert_eq!(mode(&tmp.path().join(name)), 0o600);
    }

    #[test]
    fn scan_lock_in_a_missing_dir_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(try_scan_lock(&tmp.path().join("nope"), Path::new("/r")).is_err());
    }

    #[test]
    fn clear_removes_the_cache_dir_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cache");
        let mut ix = SqliteIndex::open(&dir).unwrap();
        ix.commit_scan(
            Path::new("/A"),
            crate::tests::claude_scan("/A", false, vec![]),
        )
        .unwrap();
        drop(ix);
        assert!(dir.join(DB_FILE).exists());

        clear(&dir).unwrap();
        assert!(!dir.exists());
        clear(&dir).unwrap();

        let ix = SqliteIndex::open(&dir).unwrap();
        assert_eq!(ix.scanned_at(Path::new("/A")), None, "fresh after clear");
    }

    #[test]
    fn effective_uid_is_the_owner_of_new_files_and_leaves_no_trace() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cache");
        ensure_cache_dir(&dir).unwrap();
        let expected = std::fs::metadata(&dir).unwrap().uid();
        assert_eq!(effective_uid(&dir).unwrap(), expected);
        assert_eq!(effective_uid(&dir).unwrap(), expected, "repeatable");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        assert!(effective_uid(&tmp.path().join("missing")).is_err());
    }
}
