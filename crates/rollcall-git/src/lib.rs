use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{
    Arc,
    mpsc::{self, RecvTimeoutError},
};
#[cfg(any(test, feature = "test-hooks"))]
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use rollcall_core::{
    DiscoveredRepo, Discovery, GitOptions, LinkedWorktree, RepoState, WalkOptions, WorktreeState,
};

pub mod classify;
pub mod open;
pub mod root;
pub mod state;
#[cfg(test)]
pub mod testgit;
pub mod trust;
pub mod walk;

/// Outcome of the §4.3 refused-root guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootCheck {
    Allowed,
    /// Up to 5 `rollcall <dir>` suggestions.
    Refused {
        suggestions: Vec<PathBuf>,
    },
}

pub fn check_root(root: &Path, opts: &WalkOptions) -> RootCheck {
    root::check_root(root, opts)
}

/// Walks `root` (§4.1); every repo it finds is then read with [`repo_state`].
pub fn discover(root: &Path, opts: &WalkOptions) -> Discovery {
    walk::walk(root, opts)
}

/// Stack for a repo worker: margin over gix's recursive `TREE` decode at the 1,024-level cap
/// the index validation allows (§4.4).
const WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;
const TIMED_OUT: &str = "timed out";
/// Stands in for a deadline too far out to add to an `Instant`.
const FAR_FUTURE: Duration = Duration::from_secs(10 * 365 * 24 * 60 * 60);

/// Git state of one repo and its worktrees (§3.3), under the per-repo deadline (§4.4): the work
/// runs on a detached worker thread, and a result that is not back by the deadline is discarded
/// (`timed out`, with `pending` telling when the abandoned worker finally ends).
pub fn repo_state(repo: &DiscoveredRepo, opts: &GitOptions) -> RepoState {
    state_with(repo, opts, || {})
}

/// [`repo_state`] plus a hook the worker runs as its very last act (tests await it).
fn state_with(
    repo: &DiscoveredRepo,
    opts: &GitOptions,
    on_finish: impl FnOnce() + Send + 'static,
) -> RepoState {
    // Started before the pre-open checks, which run on the worker.
    let start = Instant::now();
    let deadline = start
        .checked_add(opts.repo_budget)
        .unwrap_or(start + FAR_FUTURE);
    let interrupt = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let worker = {
        let (repo, opts) = (repo.clone(), opts.clone());
        let (interrupt, finished) = (Arc::clone(&interrupt), Arc::clone(&finished));
        move || {
            let state = compute(&repo, &opts, deadline, &interrupt);
            #[cfg(any(test, feature = "test-hooks"))]
            thread::sleep(seam_delay(&repo.git_dir));
            // A send fails once the caller gave up at the deadline: that is the discard.
            let _ = tx.send(state);
            finished.store(true, Ordering::Release);
            on_finish();
        }
    };
    if let Err(e) = thread::Builder::new()
        .stack_size(WORKER_STACK_BYTES)
        .spawn(worker)
    {
        return error_state(format!("git error: cannot start worker: {e}"));
    }
    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(state) if Instant::now() < deadline => state,
        Err(RecvTimeoutError::Disconnected) => error_state("git error: worker failed".into()),
        // Timed out, or the result came in at or after the deadline.
        _ => {
            interrupt.store(true, Ordering::Relaxed);
            RepoState {
                pending: Some(finished),
                ..error_state(TIMED_OUT.into())
            }
        }
    }
}

fn error_state(error: String) -> RepoState {
    RepoState {
        default_branch: None,
        has_remote: false,
        error: Some(error),
        worktrees: Vec::new(),
        pending: None,
    }
}

/// A worktree row that failed before any git field could be read (§4.1: fields `null` / `0`).
fn error_row(path: &Path, is_main: bool, error: String) -> WorktreeState {
    WorktreeState {
        path: path.to_path_buf(),
        is_main,
        branch: None,
        detached: false,
        head: None,
        last_commit_at: None,
        upstream: None,
        ahead: None,
        behind: None,
        unpushed: None,
        dirty_count: 0,
        untracked_count: 0,
        missing: false,
        merged: false,
        error: Some(error),
    }
}

/// Stage boundary: the worker stops once the deadline passed or the caller gave up.
fn stage(deadline: Instant, interrupt: &AtomicBool) -> Result<(), String> {
    if Instant::now() >= deadline || interrupt.load(Ordering::Relaxed) {
        Err(TIMED_OUT.to_string())
    } else {
        Ok(())
    }
}

fn compute(
    repo: &DiscoveredRepo,
    opts: &GitOptions,
    deadline: Instant,
    interrupt: &AtomicBool,
) -> RepoState {
    build(repo, opts, deadline, interrupt).unwrap_or_else(error_state)
}

/// `Err` is a repo-level failure: the row carries the error and no worktrees (§4.1).
fn build(
    repo: &DiscoveredRepo,
    opts: &GitOptions,
    deadline: Instant,
    interrupt: &AtomicBool,
) -> Result<RepoState, String> {
    if let Some(error) = &repo.error {
        return Err(error.clone());
    }
    stage(deadline, interrupt)?;
    open::pre_open_checks(&repo.git_dir, repo.workdir.as_deref(), None)?;
    stage(deadline, interrupt)?;
    let opened = trust::open_hardened(&repo.git_dir, opts)?;
    let default = state::default_branch(&opened);
    let mut state = RepoState {
        default_branch: default.clone(),
        has_remote: state::has_remote(&opened),
        error: None,
        worktrees: Vec::new(),
        pending: None,
    };
    let trusted = trust::check_trust(&opened, repo.workdir.as_deref(), &repo.git_dir, None, opts);
    // A bare repo lists no worktrees, so a trust failure there is the repo's own.
    let Some(workdir) = repo.workdir.as_deref() else {
        return trusted.map(|()| state);
    };
    stage(deadline, interrupt)?;
    state.worktrees.push(match trusted {
        Ok(()) => state::worktree_state(
            &opened,
            workdir,
            true,
            default.as_deref(),
            deadline,
            interrupt,
        ),
        Err(error) => error_row(workdir, true, error),
    });
    for linked in &repo.linked {
        stage(deadline, interrupt)?;
        let row = linked_row(linked, repo, default.as_deref(), opts, deadline, interrupt);
        state.worktrees.push(row);
    }
    Ok(state)
}

/// One linked worktree, opened through the same pre-open checks and hardened open as the main.
fn linked_row(
    linked: &LinkedWorktree,
    repo: &DiscoveredRepo,
    default: Option<&str>,
    opts: &GitOptions,
    deadline: Instant,
    interrupt: &AtomicBool,
) -> WorktreeState {
    if let Some(error) = &linked.error {
        return error_row(&linked.path, false, error.clone());
    }
    if linked.missing {
        return state::missing_worktree_state(&linked.admin_dir, &linked.path);
    }
    let opened = open::pre_open_checks(&linked.admin_dir, Some(&linked.path), Some(&repo.git_dir))
        .and_then(|_| trust::open_hardened(&linked.admin_dir, opts))
        .and_then(|opened| {
            let admin = Some(linked.admin_dir.as_path());
            trust::check_trust(&opened, Some(&linked.path), &repo.git_dir, admin, opts)?;
            Ok(opened)
        });
    match opened {
        Ok(opened) => {
            state::worktree_state(&opened, &linked.path, false, default, deadline, interrupt)
        }
        Err(error) => error_row(&linked.path, false, error),
    }
}

/// Test seam (`GitOptions` has no field for it): how long the worker holds a finished result,
/// for repos under a path a test registered with [`set_state_delay`]. Keyed by path so tests
/// running in parallel do not slow each other.
#[cfg(any(test, feature = "test-hooks"))]
static STATE_DELAYS: Mutex<Vec<(PathBuf, Duration)>> = Mutex::new(Vec::new());

/// `Some` makes every repo under `under` hold its result for `delay`; `None` removes the entry.
#[cfg(any(test, feature = "test-hooks"))]
pub fn set_state_delay(under: &Path, delay: Option<Duration>) {
    let mut delays = STATE_DELAYS.lock().unwrap_or_else(PoisonError::into_inner);
    delays.retain(|(path, _)| path != under);
    if let Some(delay) = delay {
        delays.push((under.to_path_buf(), delay));
    }
}

#[cfg(any(test, feature = "test-hooks"))]
fn seam_delay(git_dir: &Path) -> Duration {
    let delays = STATE_DELAYS.lock().unwrap_or_else(PoisonError::into_inner);
    delays
        .iter()
        .find(|(under, _)| git_dir.starts_with(under))
        .map_or(Duration::ZERO, |(_, delay)| *delay)
}

pub fn user_excludes_file(home: &Path, xdg_config_home: Option<&Path>) -> Option<PathBuf> {
    open::user_excludes_file(home, xdg_config_home)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;
    use crate::testgit::{
        WATCHDOG, add_submodule, add_worktree, init_bare, init_repo, tempdir_real, within,
    };

    const GENEROUS: Duration = Duration::from_secs(30);

    fn walk_opts(home: &Path) -> WalkOptions {
        WalkOptions {
            home: home.to_path_buf(),
            confirmed: false,
            follow_symlinks: false,
            submodules: true,
            skip_dirs: Vec::new(),
            budget_entries: 100_000,
            budget: GENEROUS,
        }
    }

    fn git_opts(root: &Path, budget: Duration) -> GitOptions {
        GitOptions {
            root: root.to_path_buf(),
            home: root.join("nowhere-home"),
            skip_paths: Vec::new(),
            repo_budget: budget,
            excludes_file: None,
        }
    }

    fn discovered(root: &Path) -> Discovery {
        discover(root, &walk_opts(&root.join("nowhere-home")))
    }

    /// The discovered repo whose working tree (bare: git dir) is `path`.
    fn repo_at(d: &Discovery, path: &Path) -> DiscoveredRepo {
        d.repos
            .iter()
            .find(|r| r.workdir.as_deref().unwrap_or(&r.git_dir) == path)
            .unwrap_or_else(|| panic!("no repo at {}: {:#?}", path.display(), d.repos))
            .clone()
    }

    /// A normal repo, a bare repo, and a main repo with one linked worktree under `root`.
    fn three_repos(root: &Path) -> Discovery {
        init_repo(&root.join("normal"));
        init_bare(&root.join("bare.git"));
        init_repo(&root.join("main"));
        add_worktree(&root.join("main"), &root.join("wt"), "topic");
        discovered(root)
    }

    #[test]
    fn discover_delegates_to_the_walk() {
        let (_t, root) = tempdir_real();
        let d = three_repos(&root);
        assert_eq!(d.repos.len(), 3, "{:#?}", d.repos);
        assert_eq!(d, walk::walk(&root, &walk_opts(&root.join("nowhere-home"))));
    }

    #[test]
    fn a_microsecond_budget_times_out_normal_bare_and_linked_rows() {
        let (_t, root) = tempdir_real();
        let d = three_repos(&root);
        for repo in &d.repos {
            let fast = repo_state(repo, &git_opts(&root, Duration::from_micros(1)));
            assert_eq!(fast.error.as_deref(), Some("timed out"), "{repo:#?}");
            assert!(fast.worktrees.is_empty() && fast.pending.is_some());
            // Control: the same repo with a generous budget is not an error row.
            let ok = repo_state(repo, &git_opts(&root, GENEROUS));
            assert_eq!(ok.error, None, "{repo:#?}");
            assert!(ok.pending.is_none());
        }
    }

    #[test]
    fn states_cover_main_linked_and_bare_repos() {
        let (_t, root) = tempdir_real();
        let d = three_repos(&root);
        let opts = git_opts(&root, GENEROUS);

        let main = repo_state(&repo_at(&d, &root.join("main")), &opts);
        let rows: Vec<_> = main
            .worktrees
            .iter()
            .map(|w| (w.path.clone(), w.is_main, w.branch.clone(), w.error.clone()))
            .collect();
        assert_eq!(
            rows,
            vec![
                (root.join("main"), true, Some("main".into()), None),
                (root.join("wt"), false, Some("topic".into()), None),
            ]
        );
        assert_eq!(main.default_branch.as_deref(), Some("main"));
        assert!(!main.has_remote);

        let bare = repo_state(&repo_at(&d, &root.join("bare.git")), &opts);
        assert!(bare.worktrees.is_empty() && bare.error.is_none());
    }

    #[test]
    fn a_missing_linked_worktree_is_a_missing_row() {
        let (_t, root) = tempdir_real();
        init_repo(&root.join("main"));
        add_worktree(&root.join("main"), &root.join("wt"), "topic");
        fs::remove_dir_all(root.join("wt")).unwrap();
        let d = discovered(&root);
        let state = repo_state(&repo_at(&d, &root.join("main")), &git_opts(&root, GENEROUS));
        assert_eq!(state.worktrees.len(), 2);
        assert!(state.worktrees[1].missing);
        assert_eq!(state.worktrees[1].branch.as_deref(), Some("topic"));
    }

    #[test]
    fn a_submodule_repo_is_opened_through_the_same_path() {
        let (_t, root) = tempdir_real();
        init_repo(&root.join("source"));
        init_repo(&root.join("super"));
        add_submodule(&root.join("super"), &root.join("source"), "sub");
        let d = discovered(&root);
        let sub = d
            .repos
            .iter()
            .find(|r| r.submodule_of.is_some())
            .expect("a submodule row")
            .clone();
        let state = repo_state(&sub, &git_opts(&root, GENEROUS));
        assert_eq!(state.error, None);
        assert_eq!(state.worktrees.len(), 1);
        assert_eq!(state.worktrees[0].error, None);
        assert!(state.worktrees[0].head.is_some());
    }

    #[test]
    fn a_discovery_error_becomes_the_repo_error_with_no_worktrees() {
        let (_t, root) = tempdir_real();
        init_repo(&root.join("a"));
        let mut repo = repo_at(&discovered(&root), &root.join("a"));
        repo.error = Some("bad gitdir: nope".into());
        let state = repo_state(&repo, &git_opts(&root, GENEROUS));
        assert_eq!(state.error.as_deref(), Some("bad gitdir: nope"));
        assert!(state.worktrees.is_empty());
    }

    #[test]
    fn a_late_result_is_discarded_and_pending_flips_when_the_worker_finishes() {
        let (_t, root) = tempdir_real();
        init_repo(&root.join("slow"));
        let repo = repo_at(&discovered(&root), &root.join("slow"));
        set_state_delay(&root, Some(Duration::from_millis(200)));
        let (done_tx, done_rx) = mpsc::channel();
        let opts = git_opts(&root, Duration::from_millis(50));
        let state = within(move || {
            state_with(&repo, &opts, move || {
                let _ = done_tx.send(());
            })
        });
        // The worker computed a good state, but only after the deadline.
        assert_eq!(state.error.as_deref(), Some("timed out"));
        assert!(state.worktrees.is_empty());
        let pending = state.pending.expect("abandoned worker has a pending flag");
        assert!(!pending.load(Ordering::Acquire), "worker still sleeping");
        done_rx.recv_timeout(WATCHDOG).expect("worker finished");
        assert!(pending.load(Ordering::Acquire));
        set_state_delay(&root, None);
    }

    #[test]
    fn the_delay_seam_only_touches_repos_under_its_path() {
        let (_t, root) = tempdir_real();
        init_repo(&root.join("fast"));
        let repo = repo_at(&discovered(&root), &root.join("fast"));
        let (_other, elsewhere) = tempdir_real();
        set_state_delay(&elsewhere, Some(Duration::from_secs(60)));
        let state = within(move || repo_state(&repo, &git_opts(&root, Duration::from_secs(5))));
        assert_eq!(state.error, None);
        set_state_delay(&elsewhere, None);
    }

    #[test]
    fn an_unsupported_repository_format_is_an_open_failure_and_siblings_are_fine() {
        let (_t, root) = tempdir_real();
        init_repo(&root.join("good"));
        init_repo(&root.join("bad"));
        let config = root.join("bad/.git/config");
        let text = fs::read_to_string(&config).unwrap();
        let edited = text.replace("repositoryformatversion = 0", "repositoryformatversion = 7");
        assert_ne!(text, edited, "config carried no format version to edit");
        fs::write(&config, edited).unwrap();
        let d = discovered(&root);
        let opts = git_opts(&root, GENEROUS);
        let bad = repo_state(&repo_at(&d, &root.join("bad")), &opts);
        let message = bad.error.expect("open failure");
        assert!(message.starts_with("open failed: "), "{message}");
        assert!(bad.worktrees.is_empty());
        let good = repo_state(&repo_at(&d, &root.join("good")), &opts);
        assert_eq!(good.error, None);
        assert_eq!(good.worktrees.len(), 1);
    }

    #[test]
    fn a_pre_open_refusal_is_an_untrusted_git_dir_row() {
        let (_t, root) = tempdir_real();
        init_repo(&root.join("evil"));
        let config = root.join("evil/.git/config");
        let stash = root.join("config.real");
        fs::rename(&config, &stash).unwrap();
        std::os::unix::fs::symlink(&stash, &config).unwrap();
        let repo = repo_at(&discovered(&root), &root.join("evil"));
        let state = repo_state(&repo, &git_opts(&root, GENEROUS));
        let message = state.error.expect("refused");
        assert!(message.starts_with("untrusted git dir"), "{message}");
        assert!(state.worktrees.is_empty());
    }

    // ---- the 1,024-level TREE index -----------------------------------------------------------

    /// An index with no entries and one `TREE` extension `depth` levels deep, checksummed.
    fn tree_chain_index(depth: usize) -> Vec<u8> {
        let mut tree: Vec<u8> = Vec::new();
        for level in 0..depth {
            let children = usize::from(level + 1 < depth);
            tree.extend(if level == 0 { &b""[..] } else { &b"d"[..] });
            tree.extend(format!("\0-1 {children}\n").as_bytes());
        }
        let mut bytes = b"DIRC".to_vec();
        bytes.extend(2u32.to_be_bytes());
        bytes.extend(0u32.to_be_bytes());
        bytes.extend(b"TREE");
        bytes.extend((tree.len() as u32).to_be_bytes());
        bytes.extend(tree);
        let mut hasher = gix::hash::hasher(gix::hash::Kind::Sha1);
        hasher.update(&bytes);
        bytes.extend_from_slice(hasher.try_finalize().expect("sha1").as_slice());
        bytes
    }

    fn state_with_tree_chain(depth: usize) -> RepoState {
        let (_t, root) = tempdir_real();
        init_repo(&root.join("deep"));
        fs::write(root.join("deep/.git/index"), tree_chain_index(depth)).unwrap();
        let repo = repo_at(&discovered(&root), &root.join("deep"));
        within(move || repo_state(&repo, &git_opts(&root, GENEROUS)))
    }

    #[test]
    fn an_index_tree_chain_of_1024_levels_decodes_through_repo_state() {
        let state = state_with_tree_chain(1_024);
        assert_eq!(state.error, None);
        let worktree = &state.worktrees[0];
        assert_eq!(worktree.error, None);
        // The empty index stages the deletion of README: proof the index was read.
        assert_eq!(worktree.dirty_count, 1);
    }

    #[test]
    fn an_index_tree_chain_of_1025_levels_is_refused_before_gix_decodes_it() {
        let state = state_with_tree_chain(1_025);
        let message = state.error.expect("refused");
        assert!(message.starts_with("untrusted git dir"), "{message}");
        assert!(state.worktrees.is_empty());
    }
}
