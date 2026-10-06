//! §4.3 refused-root guard: `/` and `$HOME` are not scanned without confirmation, and the
//! refusal names a few repo-bearing subdirectories to scan instead. Nothing here opens a repo.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rollcall_core::WalkOptions;

use crate::RootCheck;
use crate::classify::{SKIP_DIR_NAMES, metadata_is_dataless, path_skips};

/// §4.3: the suggestion peek gives up after this long.
const PEEK_BUDGET: Duration = Duration::from_secs(2);
/// §4.3: a refusal lists at most this many `rollcall <dir>` suggestions.
const MAX_SUGGESTIONS: usize = 5;
/// §4.3: on macOS, reading these directly under `$HOME` raises privacy prompts.
const PRIVACY_DIRS: [&str; 6] = [
    "Desktop",
    "Documents",
    "Downloads",
    "Pictures",
    "Movies",
    "Music",
];

pub fn check_root(root: &Path, opts: &WalkOptions) -> RootCheck {
    if opts.confirmed {
        return RootCheck::Allowed;
    }
    // An unreadable root is the walk's to report, not a refusal.
    let Ok(root_md) = fs::metadata(root) else {
        return RootCheck::Allowed;
    };
    let is_root_dir = |other: &Path| {
        fs::metadata(other).is_ok_and(|md| md.dev() == root_md.dev() && md.ino() == root_md.ino())
    };
    if !is_root_dir(Path::new("/")) && !is_root_dir(&opts.home) {
        return RootCheck::Allowed;
    }
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let suggestions = peek(&root, opts, Instant::now() + PEEK_BUDGET, &mut 0);
    RootCheck::Refused { suggestions }
}

/// Immediate subdirectories of `root` that contain a `.git` (dir or file) at `d/.git` or
/// `d/x/.git`, in path order, at most [`MAX_SUGGESTIONS`]. `entries` counts every dirent read.
fn peek(root: &Path, opts: &WalkOptions, deadline: Instant, entries: &mut u64) -> Vec<PathBuf> {
    let Ok(root_md) = fs::symlink_metadata(root) else {
        return Vec::new();
    };
    let home = fs::canonicalize(&opts.home).unwrap_or_else(|_| opts.home.clone());
    let peek = Peek {
        home: &home,
        root_dev: root_md.dev(),
        skip_names: SKIP_DIR_NAMES
            .iter()
            .copied()
            .chain(opts.skip_dirs.iter().map(String::as_str))
            .collect(),
        // Config `ignore` hides a repo from suggestions as from the walk (§4.3).
        skip_paths: path_skips(&home)
            .into_iter()
            .chain(opts.ignore.iter().cloned())
            .collect(),
        deadline,
    };
    let mut suggestions = Vec::new();
    for dir in peek.subdirs(root, entries) {
        if suggestions.len() == MAX_SUGGESTIONS || peek.expired() {
            break;
        }
        if has_git(&dir) || peek.subdirs(&dir, entries).iter().any(|d| has_git(d)) {
            suggestions.push(dir);
        }
    }
    suggestions
}

struct Peek<'a> {
    home: &'a Path,
    root_dev: u64,
    skip_names: Vec<&'a str>,
    skip_paths: Vec<PathBuf>,
    deadline: Instant,
}

impl Peek<'_> {
    fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }

    /// The directories directly inside `dir` worth looking into, in path order. `lstat`
    /// only: a symlink to a directory is not a directory here.
    fn subdirs(&self, dir: &Path, entries: &mut u64) -> Vec<PathBuf> {
        let Ok(read) = fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for entry in read.flatten() {
            if self.expired() {
                break;
            }
            *entries += 1;
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) && self.admits(&entry.path()) {
                found.push(entry.path());
            }
        }
        found.sort();
        found
    }

    /// §4.2 skips (config `ignore` included), the macOS privacy directories, `SF_DATALESS`, and
    /// the same-device rule.
    fn admits(&self, path: &Path) -> bool {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let privacy_dir = cfg!(target_os = "macos")
            && PRIVACY_DIRS.contains(&&*name)
            && path.parent() == Some(self.home);
        if privacy_dir
            || self.skip_names.contains(&&*name)
            || self.skip_paths.iter().any(|skip| path.starts_with(skip))
        {
            return false;
        }
        fs::symlink_metadata(path)
            .is_ok_and(|md| md.dev() == self.root_dev && !metadata_is_dataless(&md))
    }
}

/// A `.git` directory or file, by `lstat`; a symlink or FIFO named `.git` does not count.
fn has_git(dir: &Path) -> bool {
    fs::symlink_metadata(dir.join(".git")).is_ok_and(|md| md.is_dir() || md.is_file())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use rollcall_core::WalkOptions;

    use super::{PEEK_BUDGET, check_root, peek};
    use crate::RootCheck;
    use crate::testgit::{mkfifo, tempdir_real, within};

    fn opts(home: &Path) -> WalkOptions {
        WalkOptions {
            home: home.to_path_buf(),
            confirmed: false,
            follow_symlinks: false,
            submodules: false,
            skip_dirs: Vec::new(),
            ignore: Vec::new(),
            budget_entries: 100_000,
            budget: Duration::from_secs(30),
        }
    }

    /// A directory with a `.git` directory inside it.
    fn repo(path: &Path) {
        std::fs::create_dir_all(path.join(".git")).unwrap();
    }

    fn suggestions(check: RootCheck) -> Vec<PathBuf> {
        match check {
            RootCheck::Refused { suggestions } => suggestions,
            RootCheck::Allowed => panic!("expected Refused"),
        }
    }

    /// Seven repos at depth 1 to 3, one under a skip name, one under `Documents`.
    fn seven_repo_home(home: &Path) {
        repo(&home.join("alpha"));
        std::fs::create_dir(home.join("bravo")).unwrap();
        std::fs::write(home.join("bravo/.git"), "gitdir: /elsewhere\n").unwrap();
        repo(&home.join("charlie/app"));
        repo(&home.join("delta/app"));
        repo(&home.join("echo/app"));
        repo(&home.join("foxtrot/app"));
        repo(&home.join("golf/one/two"));
        repo(&home.join(".cache/tool"));
        repo(&home.join("Documents/x"));
    }

    #[test]
    fn a_home_root_lists_the_capped_depth_two_suggestions_in_path_order() {
        let (_dir, home) = tempdir_real();
        seven_repo_home(&home);
        // golf is depth 3 and `.cache` is a skip name; `Documents` (byte-order first) is
        // a privacy directory on macOS only, and foxtrot falls off the 5-entry cap.
        let mut expected = vec!["alpha", "bravo", "charlie", "delta", "echo"];
        if cfg!(not(target_os = "macos")) {
            expected.insert(0, "Documents");
            expected.pop();
        }
        let expected: Vec<PathBuf> = expected.iter().map(|name| home.join(name)).collect();
        assert_eq!(suggestions(check_root(&home, &opts(&home))), expected);
    }

    #[test]
    fn a_confirmed_root_is_allowed() {
        let (_dir, home) = tempdir_real();
        seven_repo_home(&home);
        let confirmed = WalkOptions {
            confirmed: true,
            ..opts(&home)
        };
        assert_eq!(check_root(&home, &confirmed), RootCheck::Allowed);
        assert_eq!(check_root(Path::new("/"), &confirmed), RootCheck::Allowed);
    }

    #[test]
    fn a_root_that_is_neither_slash_nor_home_is_allowed_without_a_peek() {
        let (_dir, home) = tempdir_real();
        repo(&home.join("work/app"));
        assert_eq!(
            check_root(&home.join("work"), &opts(&home)),
            RootCheck::Allowed
        );
        assert_eq!(
            check_root(&home.join("missing"), &opts(&home)),
            RootCheck::Allowed
        );
    }

    #[test]
    fn a_symlink_to_home_is_still_refused() {
        let (_dir, home) = tempdir_real();
        let (_link_dir, link_parent) = tempdir_real();
        let link = link_parent.join("home-link");
        std::os::unix::fs::symlink(&home, &link).unwrap();
        repo(&home.join("alpha"));
        assert_eq!(
            suggestions(check_root(&link, &opts(&home))),
            vec![home.join("alpha")]
        );
    }

    #[test]
    fn the_slash_peek_is_refused_and_stays_within_two_levels() {
        let (_dir, home) = tempdir_real();
        let started = Instant::now();
        let (check, entries) = within(move || {
            let mut entries = 0;
            let deadline = Instant::now() + PEEK_BUDGET;
            let found = peek(Path::new("/"), &opts(&home), deadline, &mut entries);
            (found, entries)
        });
        assert!(started.elapsed() < PEEK_BUDGET + Duration::from_secs(3));

        // Everything a 2-level peek could possibly read: `/` and each of its children.
        let count = |dir: &Path| std::fs::read_dir(dir).map_or(0, |read| read.count() as u64);
        let bound = count(Path::new("/"))
            + std::fs::read_dir("/")
                .unwrap()
                .flatten()
                .map(|entry| count(&entry.path()))
                .sum::<u64>();
        assert!(entries > 0, "the peek read nothing");
        assert!(
            entries <= bound + 1_000,
            "{entries} entries > bound {bound}"
        );
        assert!(entries < 50_000, "{entries} entries looks like a full walk");
        assert!(check.len() <= 5);

        let (_dir, home) = tempdir_real();
        assert!(matches!(
            check_root(Path::new("/"), &opts(&home)),
            RootCheck::Refused { .. }
        ));
    }

    #[test]
    fn configured_skip_names_are_not_suggested() {
        let (_dir, home) = tempdir_real();
        repo(&home.join("vendor/app"));
        repo(&home.join("kept/app"));
        let skipping = WalkOptions {
            skip_dirs: vec!["vendor".to_string()],
            ignore: Vec::new(),
            ..opts(&home)
        };
        assert_eq!(
            suggestions(check_root(&home, &skipping)),
            vec![home.join("kept")]
        );
        assert_eq!(
            suggestions(check_root(&home, &opts(&home))),
            vec![home.join("kept"), home.join("vendor")]
        );
    }

    /// Author decision 2026-10-06 (session 12): the peek applies config `ignore` as the walk does.
    #[test]
    fn ignored_paths_are_not_suggested() {
        let (_dir, home) = tempdir_real();
        repo(&home.join("vendor/app"));
        repo(&home.join("kept/app"));
        repo(&home.join("deep/one"));
        let ignoring = WalkOptions {
            ignore: vec![home.join("vendor"), home.join("deep/one")],
            ..opts(&home)
        };
        assert_eq!(
            suggestions(check_root(&home, &ignoring)),
            vec![home.join("kept")]
        );
        assert_eq!(
            suggestions(check_root(&home, &opts(&home))),
            vec![home.join("deep"), home.join("kept"), home.join("vendor")]
        );
    }

    #[test]
    fn symlinks_and_fifos_do_not_count_as_repos() {
        let (_dir, home) = tempdir_real();
        repo(&home.join("real"));
        std::os::unix::fs::symlink(home.join("real"), home.join("link")).unwrap();
        std::fs::create_dir(home.join("fifo")).unwrap();
        mkfifo(&home.join("fifo/.git"));
        std::fs::create_dir(home.join("linked-git")).unwrap();
        std::os::unix::fs::symlink(home.join("real/.git"), home.join("linked-git/.git")).unwrap();
        assert_eq!(
            suggestions(check_root(&home, &opts(&home))),
            vec![home.join("real")]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_library_path_skip_applies_to_the_peek() {
        let (_dir, home) = tempdir_real();
        repo(&home.join("Library/x"));
        repo(&home.join("alpha"));
        assert_eq!(
            suggestions(check_root(&home, &opts(&home))),
            vec![home.join("alpha")]
        );
    }

    #[test]
    fn an_expired_deadline_stops_the_peek_with_nothing_read() {
        let (_dir, home) = tempdir_real();
        repo(&home.join("alpha"));
        let mut entries = 0;
        let found = peek(&home, &opts(&home), Instant::now(), &mut entries);
        assert!(found.is_empty());
        assert_eq!(entries, 0);
    }
}
