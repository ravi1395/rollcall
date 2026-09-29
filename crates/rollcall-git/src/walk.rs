//! §4.1–§4.3 parallel directory walk. Discovery only: nothing here opens a repo with gix.

use std::collections::HashSet;
use std::fs::{self, DirEntry, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::Instant;

use rollcall_core::{Discovery, WalkDiag, WalkOptions};

use crate::classify::{
    DirShape, Found, SKIP_DIR_NAMES, assemble, classify_dot_git, enclosing_repo,
    metadata_is_dataless, path_skips,
};

/// Directory reads are I/O bound; more threads than this only add contention.
const MAX_WORKERS: usize = 8;

/// Directories still to read, and how many workers are reading one right now.
#[derive(Default)]
struct Queue {
    stack: Vec<PathBuf>,
    active: usize,
}

/// What workers report; touched rarely, so one lock is enough.
#[derive(Default)]
struct Collected {
    found: Vec<Found>,
    unreadable_dirs: u32,
    skipped_dirs: u32,
    skipped_mounts: Vec<String>,
    stopped_at: Option<String>,
}

struct Walker<'a> {
    root: &'a Path,
    root_dev: u64,
    follow_symlinks: bool,
    skip_names: HashSet<String>,
    skip_paths: Vec<PathBuf>,
    budget_entries: u64,
    deadline: Option<Instant>,
    queue: Mutex<Queue>,
    wake: Condvar,
    entries: AtomicU64,
    stop: AtomicBool,
    visited: Mutex<HashSet<(u64, u64)>>,
    out: Mutex<Collected>,
}

/// Walks `root` and classifies every repo found (§4.1), within the §4.3 guards.
pub fn walk(root: &Path, opts: &WalkOptions) -> Discovery {
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let home = fs::canonicalize(&opts.home).unwrap_or_else(|_| opts.home.clone());
    let Ok(root_md) = fs::symlink_metadata(&root) else {
        let walk = WalkDiag {
            unreadable_dirs: 1,
            ..WalkDiag::default()
        };
        return Discovery {
            walk,
            ..Discovery::default()
        };
    };

    // An explicit root under a skipped path is allowed; the skip binds only from outside.
    let skip_paths = path_skips(&home)
        .into_iter()
        .filter(|skip| !root.starts_with(skip))
        .collect();
    let skip_names = SKIP_DIR_NAMES
        .iter()
        .map(|name| name.to_string())
        .chain(opts.skip_dirs.iter().cloned())
        .collect();
    let walker = Walker {
        root: &root,
        root_dev: root_md.dev(),
        follow_symlinks: opts.follow_symlinks,
        skip_names,
        skip_paths,
        budget_entries: opts.budget_entries,
        deadline: Instant::now().checked_add(opts.budget),
        queue: Mutex::new(Queue {
            stack: vec![root.clone()],
            active: 0,
        }),
        wake: Condvar::new(),
        entries: AtomicU64::new(0),
        stop: AtomicBool::new(false),
        visited: Mutex::new(HashSet::from([(root_md.dev(), root_md.ino())])),
        out: Mutex::new(Collected::default()),
    };

    let workers = thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(MAX_WORKERS);
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| walker.work());
        }
    });

    let entries = walker.entries.load(Ordering::SeqCst);
    let mut collected = walker.out.into_inner().unwrap();
    collected.found.extend(enclosing_repo(&root, &home));
    let (repos, notes) = assemble(collected.found, opts.submodules);
    collected.skipped_mounts.sort();
    let budget_hit = collected.stopped_at.is_some();
    Discovery {
        repos,
        walk: WalkDiag {
            entries,
            budget_hit,
            stopped_at: collected.stopped_at,
            unreadable_dirs: collected.unreadable_dirs,
            skipped_dirs: collected.skipped_dirs,
            skipped_mounts: collected.skipped_mounts,
        },
        partial: budget_hit,
        notes,
    }
}

impl Walker<'_> {
    fn out(&self) -> MutexGuard<'_, Collected> {
        self.out.lock().unwrap()
    }

    fn work(&self) {
        while let Some(dir) = self.next_dir() {
            self.visit(&dir);
            let mut queue = self.queue.lock().unwrap();
            queue.active -= 1;
            if queue.active == 0 {
                self.wake.notify_all();
            }
        }
    }

    /// Pops a directory, or waits for one; `None` once the walk is over.
    fn next_dir(&self) -> Option<PathBuf> {
        let mut queue = self.queue.lock().unwrap();
        loop {
            if self.stop.load(Ordering::SeqCst) {
                queue.stack.clear();
            }
            if let Some(dir) = queue.stack.pop() {
                queue.active += 1;
                return Some(dir);
            }
            if queue.active == 0 {
                self.wake.notify_all();
                return None;
            }
            queue = self.wake.wait(queue).unwrap();
        }
    }

    fn push_all(&self, dirs: Vec<PathBuf>) {
        if dirs.is_empty() {
            return;
        }
        self.queue.lock().unwrap().stack.extend(dirs);
        self.wake.notify_all();
    }

    fn visit(&self, dir: &Path) {
        let Ok(read) = fs::read_dir(dir) else {
            self.out().unreadable_dirs += 1;
            return;
        };
        let mut entries = read.peekable();
        let mut shape = DirShape::default();
        let mut children = Vec::new();
        loop {
            if self.stop.load(Ordering::SeqCst) {
                return;
            }
            if self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
                self.stop_at(dir);
                return;
            }
            // Peek first: the end of a directory must not spend a budget slot, or a worker
            // racing at the boundary would leave `entries` short of the budget it hit.
            match entries.peek() {
                None => break,
                Some(Err(_)) => {
                    self.out().unreadable_dirs += 1;
                    break;
                }
                Some(Ok(_)) => {}
            }
            if !self.claim_entry() {
                // A real dirent beyond the budget: the walk is incomplete.
                self.stop_at(dir);
                return;
            }
            if let Some(Ok(entry)) = entries.next() {
                self.handle(dir, &entry, &mut shape, &mut children);
            }
        }
        // A bare repo has no working tree; its `objects/` and `refs/` hold nothing to find.
        if shape.is_bare() {
            self.out().found.push(Found::Bare(dir.to_path_buf()));
            return;
        }
        self.push_all(children);
    }

    fn claim_entry(&self) -> bool {
        self.entries
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < self.budget_entries).then_some(n + 1)
            })
            .is_ok()
    }

    fn stop_at(&self, dir: &Path) {
        self.stop.store(true, Ordering::SeqCst);
        self.out()
            .stopped_at
            .get_or_insert_with(|| dir.display().to_string());
    }

    fn handle(
        &self,
        dir: &Path,
        entry: &DirEntry,
        shape: &mut DirShape,
        children: &mut Vec<PathBuf>,
    ) {
        let Ok(file_type) = entry.file_type() else {
            return;
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Never descended into; classified by lstat, so a FIFO here is never opened.
        if name == ".git" {
            // §4.2: a cloud placeholder is skipped without being opened, `.git` files included.
            if entry.metadata().is_ok_and(|md| metadata_is_dataless(&md)) {
                self.out().skipped_dirs += 1;
                return;
            }
            let result = classify_dot_git(dir, self.follow_symlinks, self.root);
            self.out().found.push(Found::DotGit {
                workdir: dir.to_path_buf(),
                result,
                enclosing: false,
            });
            return;
        }
        shape.observe(&name, file_type);
        if file_type.is_dir() {
            self.consider_dir(entry.path(), &name, entry.metadata(), children);
        } else if file_type.is_symlink() && self.follow_symlinks {
            self.consider_link(&entry.path(), children);
        }
    }

    fn consider_dir(
        &self,
        path: PathBuf,
        name: &str,
        metadata: io::Result<Metadata>,
        children: &mut Vec<PathBuf>,
    ) {
        if self.skip_names.contains(name) || self.under_skipped_path(&path) {
            self.out().skipped_dirs += 1;
            return;
        }
        match metadata {
            Ok(md) => self.admit(path, &md, children),
            Err(_) => self.out().unreadable_dirs += 1,
        }
    }

    /// §4.3: a link is followed only to a directory whose realpath is under the root and
    /// not yet visited. The walk then continues at the realpath, so a repo is reported at
    /// the same path whether its directory or a link to it is met first.
    fn consider_link(&self, link: &Path, children: &mut Vec<PathBuf>) {
        let Ok(target) = fs::canonicalize(link) else {
            return;
        };
        let Ok(relative) = target.strip_prefix(self.root) else {
            return;
        };
        let skipped_name = relative
            .components()
            .any(|c| self.skip_names.contains(&*c.as_os_str().to_string_lossy()));
        if skipped_name || self.under_skipped_path(&target) {
            self.out().skipped_dirs += 1;
            return;
        }
        if let Ok(md) = fs::symlink_metadata(&target)
            && md.is_dir()
        {
            self.admit(target, &md, children);
        }
    }

    /// Dataless, other-device and already-visited directories are not entered.
    fn admit(&self, path: PathBuf, md: &Metadata, children: &mut Vec<PathBuf>) {
        if metadata_is_dataless(md) {
            self.out().skipped_dirs += 1;
        } else if md.dev() != self.root_dev {
            self.out().skipped_mounts.push(path.display().to_string());
        } else if !self.follow_symlinks || self.visited.lock().unwrap().insert((md.dev(), md.ino()))
        {
            children.push(path);
        }
    }

    fn under_skipped_path(&self, path: &Path) -> bool {
        self.skip_paths.iter().any(|skip| path.starts_with(skip))
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::time::Duration;

    use rollcall_core::{DiscoveredRepo, Discovery, RepoKind, WalkOptions};

    use super::walk;
    use crate::testgit::{
        WATCHDOG, add_submodule, add_worktree, git, init_bare, init_repo, mkfifo, tempdir_real,
        within,
    };

    fn opts(home: &Path) -> WalkOptions {
        WalkOptions {
            home: home.to_path_buf(),
            confirmed: false,
            follow_symlinks: false,
            submodules: true,
            skip_dirs: Vec::new(),
            budget_entries: 100_000,
            budget: Duration::from_secs(30),
        }
    }

    /// The row whose working tree (or, for bare repos, git dir) is `path`.
    fn repo_at<'a>(d: &'a Discovery, path: &Path) -> &'a DiscoveredRepo {
        d.repos
            .iter()
            .find(|r| r.workdir.as_deref().unwrap_or(&r.git_dir) == path)
            .unwrap_or_else(|| panic!("no repo at {}: {:#?}", path.display(), d.repos))
    }

    fn mkdir(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
    }

    #[test]
    fn classifies_a_normal_repo() {
        let (_t, dir) = tempdir_real();
        init_repo(&dir.join("proj"));
        let d = walk(&dir, &opts(&dir));
        assert_eq!(d.repos.len(), 1, "{:#?}", d.repos);
        let repo = repo_at(&d, &dir.join("proj"));
        assert_eq!(repo.kind, RepoKind::Normal);
        assert_eq!(repo.git_dir, dir.join("proj/.git"));
        assert!(!repo.contains_root && repo.error.is_none() && repo.submodule_of.is_none());
        assert!(!d.partial && !d.walk.budget_hit && d.walk.entries > 0);
    }

    #[test]
    fn groups_a_linked_worktree_inside_the_root_under_its_main() {
        let (_t, dir) = tempdir_real();
        init_repo(&dir.join("main"));
        add_worktree(&dir.join("main"), &dir.join("wt"), "topic");
        let d = walk(&dir, &opts(&dir));
        assert_eq!(d.repos.len(), 1, "{:#?}", d.repos);
        let repo = repo_at(&d, &dir.join("main"));
        assert_eq!(repo.kind, RepoKind::Normal);
        assert_eq!(repo.linked.len(), 1);
        assert_eq!(repo.linked[0].path, dir.join("wt"));
        assert!(!repo.linked[0].missing);
    }

    #[test]
    fn case_a_lists_a_worktree_outside_the_root_under_its_main() {
        let (_t, dir) = tempdir_real();
        init_repo(&dir.join("root/main"));
        add_worktree(&dir.join("root/main"), &dir.join("ext/wt"), "topic");
        let d = walk(&dir.join("root"), &opts(&dir));
        assert_eq!(d.repos.len(), 1, "{:#?}", d.repos);
        assert_eq!(d.repos[0].linked[0].path, dir.join("ext/wt"));
    }

    #[test]
    fn case_b_reports_a_main_outside_the_root_as_external_main() {
        let (_t, dir) = tempdir_real();
        init_repo(&dir.join("ext/main"));
        add_worktree(&dir.join("ext/main"), &dir.join("root/wt"), "topic");
        add_worktree(&dir.join("ext/main"), &dir.join("ext/other"), "other");
        let d = walk(&dir.join("root"), &opts(&dir));
        assert_eq!(d.repos.len(), 1, "{:#?}", d.repos);
        let repo = repo_at(&d, &dir.join("ext/main"));
        assert_eq!(repo.kind, RepoKind::ExternalMain);
        assert_eq!(repo.git_dir, dir.join("ext/main/.git"));
        let paths: Vec<_> = repo.linked.iter().map(|w| w.path.clone()).collect();
        assert!(paths.contains(&dir.join("root/wt")), "{paths:?}");
        assert!(paths.contains(&dir.join("ext/other")), "{paths:?}");
    }

    #[test]
    fn flags_a_worktree_whose_directory_is_gone_as_missing() {
        let (_t, dir) = tempdir_real();
        init_repo(&dir.join("main"));
        add_worktree(&dir.join("main"), &dir.join("wt"), "topic");
        std::fs::remove_dir_all(dir.join("wt")).unwrap();
        let d = walk(&dir, &opts(&dir));
        let linked = &repo_at(&d, &dir.join("main")).linked;
        assert_eq!(linked.len(), 1);
        assert!(linked[0].missing);
    }

    #[test]
    fn classifies_a_bare_repo_without_a_workdir() {
        let (_t, dir) = tempdir_real();
        init_bare(&dir.join("fixtures/foo.git"));
        let d = walk(&dir, &opts(&dir));
        assert_eq!(d.repos.len(), 1, "{:#?}", d.repos);
        let repo = &d.repos[0];
        assert_eq!(repo.kind, RepoKind::Bare);
        assert_eq!(repo.workdir, None);
        assert_eq!(repo.git_dir, dir.join("fixtures/foo.git"));
    }

    #[test]
    fn a_repo_inside_a_working_tree_is_nested() {
        let (_t, dir) = tempdir_real();
        init_repo(&dir.join("outer"));
        init_repo(&dir.join("outer/vendor-copy/inner"));
        let d = walk(&dir, &opts(&dir));
        assert_eq!(d.repos.len(), 2, "{:#?}", d.repos);
        assert_eq!(repo_at(&d, &dir.join("outer")).kind, RepoKind::Normal);
        assert_eq!(
            repo_at(&d, &dir.join("outer/vendor-copy/inner")).kind,
            RepoKind::Nested
        );
    }

    #[test]
    fn groups_a_modern_submodule_under_its_parent_only_with_submodules() {
        let (_t, dir) = tempdir_real();
        init_repo(&dir.join("src-lib"));
        init_repo(&dir.join("sup"));
        add_submodule(&dir.join("sup"), &dir.join("src-lib"), "libs/sub");

        let with = walk(&dir.join("sup"), &opts(&dir));
        assert_eq!(with.repos.len(), 2, "{:#?}", with.repos);
        let parent = with
            .repos
            .iter()
            .position(|r| r.submodule_of.is_none())
            .unwrap();
        let sub = repo_at(&with, &dir.join("sup/libs/sub"));
        assert_eq!(sub.submodule_of, Some(parent));
        assert_eq!(
            with.repos[parent].workdir.as_deref(),
            Some(dir.join("sup").as_path())
        );

        let mut without_opts = opts(&dir);
        without_opts.submodules = false;
        let without = walk(&dir.join("sup"), &without_opts);
        assert_eq!(without.repos.len(), 1, "{:#?}", without.repos);
        assert_eq!(without.repos[0].submodule_of, None);
    }

    #[test]
    fn a_submodule_without_its_superproject_is_listed_on_its_own() {
        let (_t, dir) = tempdir_real();
        mkdir(&dir.join("store/.git/modules/orphan"));
        mkdir(&dir.join("orphan"));
        std::fs::write(
            dir.join("orphan/.git"),
            format!(
                "gitdir: {}\n",
                dir.join("store/.git/modules/orphan").display()
            ),
        )
        .unwrap();
        let mut o = opts(&dir);
        o.submodules = false;
        let d = walk(&dir.join("orphan"), &o);
        assert_eq!(d.repos.len(), 1, "{:#?}", d.repos);
        assert_eq!(d.repos[0].kind, RepoKind::Normal);
        assert_eq!(d.repos[0].submodule_of, None);
        assert!(
            d.notes.iter().any(|n| n.contains("outside the scan")),
            "{:?}",
            d.notes
        );
    }

    #[test]
    fn groups_an_old_style_submodule_listed_in_gitmodules() {
        let (_t, dir) = tempdir_real();
        init_repo(&dir.join("sup"));
        init_repo(&dir.join("sup/old"));
        init_repo(&dir.join("sup/unlisted"));
        std::fs::write(
            dir.join("sup/.gitmodules"),
            "[submodule \"old\"]\n\tpath = old\n\turl = ../old\n",
        )
        .unwrap();
        let d = walk(&dir, &opts(&dir));
        let parent = d
            .repos
            .iter()
            .position(|r| r.workdir.as_deref() == Some(dir.join("sup").as_path()))
            .unwrap();
        assert_eq!(repo_at(&d, &dir.join("sup/old")).submodule_of, Some(parent));
        assert_eq!(repo_at(&d, &dir.join("sup/old")).kind, RepoKind::Normal);
        // Not listed in `.gitmodules`: an ordinary nested repo.
        let unlisted = repo_at(&d, &dir.join("sup/unlisted"));
        assert_eq!(unlisted.submodule_of, None);
        assert_eq!(unlisted.kind, RepoKind::Nested);
    }

    #[test]
    fn a_gitdir_elsewhere_layout_is_opened_as_a_repo_and_noted() {
        let (_t, dir) = tempdir_real();
        mkdir(&dir.join("proj"));
        mkdir(&dir.join("gitdirs"));
        let elsewhere = dir.join("gitdirs/proj.git");
        git(
            &dir.join("proj"),
            &[
                "init",
                "-q",
                "--separate-git-dir",
                elsewhere.to_str().unwrap(),
            ],
        );
        let d = walk(&dir.join("proj"), &opts(&dir));
        assert_eq!(d.repos.len(), 1, "{:#?}", d.repos);
        assert_eq!(d.repos[0].git_dir, elsewhere);
        assert_eq!(
            d.repos[0].workdir.as_deref(),
            Some(dir.join("proj").as_path())
        );
        assert_eq!(d.notes.len(), 1, "{:?}", d.notes);
        assert!(d.notes[0].contains("unusual layout"), "{:?}", d.notes);
    }

    #[test]
    fn an_unusable_git_file_is_an_error_row() {
        let (_t, dir) = tempdir_real();
        mkdir(&dir.join("garbage"));
        std::fs::write(dir.join("garbage/.git"), "not a pointer").unwrap();
        mkdir(&dir.join("dangling"));
        std::fs::write(dir.join("dangling/.git"), "gitdir: /no/such/place\n").unwrap();
        let d = walk(&dir, &opts(&dir));
        assert_eq!(d.repos.len(), 2, "{:#?}", d.repos);
        for name in ["garbage", "dangling"] {
            let repo = repo_at(&d, &dir.join(name));
            let error = repo.error.as_deref().unwrap();
            assert!(error.starts_with("bad gitdir: "), "{name}: {error}");
        }
    }

    #[test]
    fn skips_node_modules_target_and_configured_names() {
        let (_t, dir) = tempdir_real();
        init_repo(&dir.join("visible"));
        init_repo(&dir.join("node_modules/x"));
        init_repo(&dir.join("target/x"));
        init_repo(&dir.join("deep/vendor/x"));
        let mut o = opts(&dir);
        o.skip_dirs = vec!["vendor".to_string()];
        let d = walk(&dir, &o);
        assert_eq!(d.repos.len(), 1, "{:#?}", d.repos);
        repo_at(&d, &dir.join("visible"));
        assert_eq!(d.walk.skipped_dirs, 3);
    }

    #[cfg(target_os = "macos")]
    const SKIPPED_UNDER_HOME: &str = "Library";
    #[cfg(target_os = "linux")]
    const SKIPPED_UNDER_HOME: &str = ".local/share/Trash";

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_skipped_home_path_is_pruned_when_the_root_is_outside_it() {
        let (_t, home) = tempdir_real();
        init_repo(&home.join(SKIPPED_UNDER_HOME).join("hidden"));
        init_repo(&home.join("proj"));
        let d = walk(&home, &opts(&home));
        assert_eq!(d.repos.len(), 1, "{:#?}", d.repos);
        repo_at(&d, &home.join("proj"));
        assert!(d.walk.skipped_dirs >= 1);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_root_under_a_skipped_home_path_is_walked() {
        let (_t, home) = tempdir_real();
        let inside = home.join(SKIPPED_UNDER_HOME);
        init_repo(&inside.join("hidden"));
        let d = walk(&inside, &opts(&home));
        assert_eq!(d.repos.len(), 1, "{:#?}", d.repos);
        repo_at(&d, &inside.join("hidden"));
    }

    fn fan_out_tree(dir: &Path, with_links: bool) {
        init_repo(&dir.join("proj"));
        mkdir(&dir.join("a/b/c"));
        if with_links {
            symlink(dir, dir.join("a/b/c/loop1")).unwrap();
            symlink(dir, dir.join("a/b/c/loop2")).unwrap();
        }
    }

    #[test]
    fn symlink_fan_out_is_bounded_by_the_visited_set() {
        let (_t, plain) = tempdir_real();
        fan_out_tree(&plain, false);
        let baseline = walk(&plain, &opts(&plain)).walk.entries;

        let (_t2, dir) = tempdir_real();
        fan_out_tree(&dir, true);
        let mut o = opts(&dir);
        o.follow_symlinks = true;
        let d = walk(&dir, &o);
        assert!(
            d.walk.entries <= 2 * baseline,
            "{} entries vs baseline {baseline}",
            d.walk.entries
        );
        assert!(!d.partial);
        assert_eq!(d.repos.len(), 1, "{:#?}", d.repos);
    }

    #[test]
    fn without_the_flag_symlinks_are_not_followed() {
        let (_t, plain) = tempdir_real();
        fan_out_tree(&plain, false);
        let baseline = walk(&plain, &opts(&plain)).walk.entries;

        let (_t2, dir) = tempdir_real();
        fan_out_tree(&dir, true);
        let d = walk(&dir, &opts(&dir));
        // The two loop links are dirents like any other, never entered.
        assert_eq!(d.walk.entries, baseline + 2);
        assert_eq!(d.repos.len(), 1, "{:#?}", d.repos);
    }

    #[test]
    fn a_followed_link_reports_the_real_path_once() {
        let (_t, dir) = tempdir_real();
        init_repo(&dir.join("zzz/proj"));
        symlink(dir.join("zzz"), dir.join("aaa-link")).unwrap();
        let mut o = opts(&dir);
        o.follow_symlinks = true;
        let d = walk(&dir, &o);
        assert_eq!(d.repos.len(), 1, "{:#?}", d.repos);
        repo_at(&d, &dir.join("zzz/proj"));
    }

    #[test]
    fn a_link_leaving_the_root_is_never_followed() {
        let (_t, dir) = tempdir_real();
        init_repo(&dir.join("outside/hidden"));
        mkdir(&dir.join("root"));
        symlink(dir.join("outside"), dir.join("root/out")).unwrap();
        let mut o = opts(&dir);
        o.follow_symlinks = true;
        let d = walk(&dir.join("root"), &o);
        assert!(d.repos.is_empty(), "{:#?}", d.repos);
    }

    #[test]
    fn a_git_symlink_needs_the_flag() {
        let (_t, dir) = tempdir_real();
        init_repo(&dir.join("real"));
        mkdir(&dir.join("linked"));
        symlink(dir.join("real/.git"), dir.join("linked/.git")).unwrap();
        let plain = walk(&dir.join("linked"), &opts(&dir));
        assert!(plain.repos.is_empty(), "{:#?}", plain.repos);
        assert_eq!(plain.notes.len(), 1, "{:?}", plain.notes);
        let mut o = opts(&dir);
        o.follow_symlinks = true;
        let followed = walk(&dir, &o);
        assert!(
            followed
                .repos
                .iter()
                .any(|r| r.workdir.as_deref() == Some(dir.join("linked").as_path()))
        );
    }

    #[test]
    fn the_entry_budget_stops_the_walk_exactly() {
        let (_t, dir) = tempdir_real();
        for i in 0..30 {
            mkdir(&dir.join(format!("d{i:02}")));
            std::fs::write(dir.join(format!("d{i:02}/f")), "x").unwrap();
        }
        let mut o = opts(&dir);
        o.budget_entries = 10;
        let d = walk(&dir, &o);
        assert!(d.partial && d.walk.budget_hit);
        assert_eq!(d.walk.entries, 10);
        assert!(d.walk.stopped_at.is_some());
    }

    #[test]
    fn a_tree_exactly_at_the_budget_is_not_partial() {
        let (_t, dir) = tempdir_real();
        for i in 0..5 {
            std::fs::write(dir.join(format!("f{i}")), "x").unwrap();
        }
        let mut o = opts(&dir);
        o.budget_entries = 5;
        let d = walk(&dir, &o);
        assert!(!d.partial && !d.walk.budget_hit);
        assert_eq!(d.walk.entries, 5);
    }

    #[test]
    fn an_expired_time_budget_is_partial() {
        let (_t, dir) = tempdir_real();
        init_repo(&dir.join("proj"));
        let mut o = opts(&dir);
        o.budget = Duration::ZERO;
        let d = walk(&dir, &o);
        assert!(d.partial && d.walk.budget_hit);
        assert!(d.repos.is_empty());
    }

    #[test]
    fn a_fifo_named_dot_git_is_never_opened() {
        let (_t, dir) = tempdir_real();
        mkdir(&dir.join("proj"));
        let fifo = dir.join("proj/.git");
        mkfifo(&fifo);

        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let writer_path = fifo.clone();
        let writer = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            // Blocks until something opens the FIFO for reading.
            let file = std::fs::OpenOptions::new().write(true).open(&writer_path);
            done_tx.send(file.is_ok()).unwrap();
        });
        started_rx.recv_timeout(WATCHDOG).unwrap();
        // Let the writer reach its blocking open before the walk runs.
        assert!(done_rx.recv_timeout(Duration::from_millis(200)).is_err());

        let (root, home) = (dir.clone(), dir.clone());
        let d = within(move || walk(&root, &opts(&home)));
        assert!(
            done_rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "the walk opened the FIFO"
        );
        assert_eq!(d.repos.len(), 1, "{:#?}", d.repos);
        assert!(
            d.repos[0]
                .error
                .as_deref()
                .unwrap()
                .starts_with("bad gitdir: ")
        );

        // Release the writer ourselves, then join under the watchdog.
        let _reader = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)
            .unwrap();
        assert!(done_rx.recv_timeout(WATCHDOG).unwrap());
        writer.join().unwrap();
    }

    #[test]
    fn home_dot_git_is_never_an_enclosing_repo() {
        let (_t, dir) = tempdir_real();
        let home = dir.join("home");
        mkdir(&home);
        git(&home, &["init", "-q"]);
        mkdir(&home.join("proj"));
        let d = walk(&home.join("proj"), &opts(&home));
        assert!(d.repos.is_empty(), "{:#?}", d.repos);
    }

    #[test]
    fn a_root_inside_a_repo_yields_the_enclosing_row() {
        let (_t, dir) = tempdir_real();
        init_repo(&dir.join("outer"));
        init_repo(&dir.join("outer/sub/inner"));
        let d = walk(&dir.join("outer/sub"), &opts(&dir.join("home")));
        assert_eq!(d.repos.len(), 2, "{:#?}", d.repos);
        let outer = repo_at(&d, &dir.join("outer"));
        assert!(outer.contains_root);
        assert_eq!(outer.kind, RepoKind::Normal);
        // The enclosing repo is not walked into, so it is not a nesting parent.
        let inner = repo_at(&d, &dir.join("outer/sub/inner"));
        assert!(!inner.contains_root);
        assert_eq!(inner.kind, RepoKind::Normal);
    }

    #[test]
    fn an_unreadable_directory_is_counted_and_skipped() {
        let (_t, dir) = tempdir_real();
        init_repo(&dir.join("proj"));
        let locked = dir.join("locked");
        mkdir(&locked);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable_anyway = std::fs::read_dir(&locked).is_ok(); // euid 0
        let d = walk(&dir, &opts(&dir));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if readable_anyway {
            return;
        }
        assert_eq!(d.walk.unreadable_dirs, 1);
        assert_eq!(d.repos.len(), 1);
    }

    #[test]
    fn repos_come_back_sorted_by_path() {
        let (_t, dir) = tempdir_real();
        for name in ["c", "a", "b"] {
            init_repo(&dir.join(name));
        }
        let d = walk(&dir, &opts(&dir));
        let paths: Vec<PathBuf> = d.repos.iter().filter_map(|r| r.workdir.clone()).collect();
        assert_eq!(paths, vec![dir.join("a"), dir.join("b"), dir.join("c")]);
    }
}
