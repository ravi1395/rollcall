//! §3.3 per-worktree git state, under the §4.4 guards (HEAD tree bound, status call shape,
//! pre-status checks).

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use gix::bstr::{BStr, BString, ByteSlice};
use gix::diff::index::Action as IndexAction;
use gix::dir::walk::EmissionMode;
use gix::index::entry::{Flags, Mode, Stage};
use gix::status::index_worktree::Options as StatusOptions;
use gix::status::plumbing::index_as_worktree::EntryStatus;
use gix::status::plumbing::index_as_worktree::traits::{
    CompareBlobs, FastEq, ReadData, SubmoduleStatus,
};
use gix::status::plumbing::index_as_worktree_with_renames::{Entry, VisitEntry};
use gix::status::tree_index::TrackRenames;
use gix::traverse::tree::Visit;
use gix::traverse::tree::depthfirst::State as TreeWalkState;
use rollcall_core::{Timestamp, WorktreeState};

use crate::classify::read_small_file;

#[cfg(test)]
thread_local! {
    /// Times `tree_index_status` was reached on this thread (the HEAD tree bound must stop first).
    static TREE_INDEX_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Commits pulled from rev-walks on this thread.
    static COMMITS_WALKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Times rollcall's `CompareBlobs` deferred to gix (which may read the file whole).
    static COMPARE_DEFERRALS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn bump(counter: &'static std::thread::LocalKey<std::cell::Cell<usize>>) {
    counter.with(|c| c.set(c.get() + 1));
}

const MAX_TREE_ENTRIES: u64 = 1_000_000;
const MAX_TREE_PATH_BYTES: u64 = 128 * 1024 * 1024;
/// Sum of the sizes of every tree the walk descends into (repeats counted). gix's depth-first walk
/// keeps each open level's whole decoded tree, which no other cap sees (P15b U-14).
const MAX_TREE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_TREE_DEPTH: usize = 4_096;
/// Deadline cadence of the HEAD tree walk and of the pre-status entry scan.
const ENTRY_DEADLINE_STRIDE: u64 = 1_024;
/// `ahead` / `behind` / `unpushed` saturate here.
const COMMIT_SATURATION: u32 = 10_000;
const COMMIT_DEADLINE_STRIDE: u32 = 256;
/// A tracked file larger than this is reported modified without being read (§4.4).
const COMPARE_BLOBS_CAP: u64 = 64 * 1024 * 1024;
const PATTERN_BLOB_CAP: u64 = 256 * 1024;
const PATTERN_BLOBS_TOTAL_CAP: u64 = 1024 * 1024;
const RENAME_SIMILARITY: f32 = 0.5;
const RENAME_LIMIT: usize = 1_000;
/// `HEAD` of a missing worktree's admin dir (§3.3).
const ADMIN_HEAD_CAP: u64 = 4 * 1024;

#[derive(Debug, PartialEq, Eq)]
enum Cap {
    Entries,
    PathBytes,
    Depth,
    TreeBytes,
}

#[derive(Debug, PartialEq, Eq)]
enum Stop {
    TimedOut,
    TreeTooLarge(Cap),
    UnsafeTrackedFile,
    PatternFilesTooLarge,
    Git(String),
}

impl Stop {
    /// The fixed §4.1 row error.
    fn message(&self) -> String {
        match self {
            Stop::TimedOut => "timed out".to_string(),
            Stop::TreeTooLarge(_) => "git error: HEAD tree too large".to_string(),
            Stop::UnsafeTrackedFile => "git error: unsafe tracked file".to_string(),
            Stop::PatternFilesTooLarge => "git error: pattern files too large".to_string(),
            Stop::Git(detail) => format!("git error: {detail}"),
        }
    }
}

fn git_err(error: impl std::fmt::Display) -> Stop {
    Stop::Git(error.to_string())
}

fn expired(deadline: Instant, interrupt: &AtomicBool) -> bool {
    Instant::now() >= deadline || interrupt.load(Ordering::Relaxed)
}

fn check(deadline: Instant, interrupt: &AtomicBool) -> Result<(), Stop> {
    if expired(deadline, interrupt) {
        Err(Stop::TimedOut)
    } else {
        Ok(())
    }
}

/// A gix failure after the deadline or interrupt is the timeout, not a git error.
fn classify(error: impl std::fmt::Display, deadline: Instant, interrupt: &AtomicBool) -> Stop {
    if expired(deadline, interrupt) {
        Stop::TimedOut
    } else {
        git_err(error)
    }
}

// ---- §4.4 HEAD tree bound ---------------------------------------------------------------------

/// Counts expanded entries, their full-path bytes and the sizes of the trees descended into,
/// without keeping any path: a hostile chain of long names would make gix's own `Recorder` hold a
/// path copy per level. `push_path_component` is always followed by `pop_path_component` around
/// one visit, so one saved slot is enough to undo it.
struct TreeBound<'a> {
    repo: &'a gix::Repository,
    entries: u64,
    path_bytes: u64,
    tree_bytes: u64,
    /// Byte length and component count of the path being visited.
    current: (u64, usize),
    saved: (u64, usize),
    /// Mirrors the depth-first walk's queue of tree paths.
    queue: VecDeque<(u64, usize)>,
    deadline: Instant,
    interrupt: &'a AtomicBool,
    stop: Option<Stop>,
}

impl TreeBound<'_> {
    fn extend(&mut self, component: &BStr) {
        if component.is_empty() {
            return;
        }
        let (len, depth) = self.current;
        let separator = u64::from(depth > 0);
        self.current = (len + separator + component.len() as u64, depth + 1);
    }

    /// Adds the size of tree `id` (header only, the body is not read here).
    fn add_tree(&mut self, id: &gix::oid) -> Result<(), Stop> {
        self.tree_bytes += self.repo.find_header(id).map_err(git_err)?.size();
        if self.tree_bytes > MAX_TREE_BYTES {
            return Err(Stop::TreeTooLarge(Cap::TreeBytes));
        }
        Ok(())
    }

    fn entry_over_cap(&self) -> Option<Cap> {
        if self.entries > MAX_TREE_ENTRIES {
            Some(Cap::Entries)
        } else if self.path_bytes > MAX_TREE_PATH_BYTES {
            Some(Cap::PathBytes)
        } else if self.current.1 > MAX_TREE_DEPTH {
            Some(Cap::Depth)
        } else {
            None
        }
    }

    /// One visited entry; `descend` is the tree to walk into, if it is one.
    fn see(&mut self, descend: Option<&gix::oid>) -> ControlFlow<(), bool> {
        self.entries += 1;
        self.path_bytes += self.current.0;
        let verdict = match self.entry_over_cap() {
            Some(cap) => Err(Stop::TreeTooLarge(cap)),
            None if self.entries.is_multiple_of(ENTRY_DEADLINE_STRIDE) => {
                check(self.deadline, self.interrupt)
            }
            None => Ok(()),
        }
        .and_then(|()| descend.map_or(Ok(()), |id| self.add_tree(id)));
        match verdict {
            Ok(()) => ControlFlow::Continue(true),
            Err(stop) => {
                self.stop = Some(stop);
                ControlFlow::Break(())
            }
        }
    }
}

impl Visit for TreeBound<'_> {
    fn pop_back_tracked_path_and_set_current(&mut self) {
        self.current = self.queue.pop_back().unwrap_or_default();
    }

    // Only the breadth-first walk calls this; the depth-first one used here never does.
    fn pop_front_tracked_path_and_set_current(&mut self) {}

    fn push_back_tracked_path_component(&mut self, component: &BStr) {
        self.extend(component);
        self.queue.push_back(self.current);
    }

    fn push_path_component(&mut self, component: &BStr) {
        self.saved = self.current;
        self.extend(component);
    }

    fn pop_path_component(&mut self) {
        self.current = self.saved;
    }

    fn visit_tree(&mut self, entry: &gix::objs::tree::EntryRef<'_>) -> ControlFlow<(), bool> {
        self.see(Some(entry.oid))
    }

    fn visit_nontree(&mut self, _entry: &gix::objs::tree::EntryRef<'_>) -> ControlFlow<(), bool> {
        self.see(None)
    }
}

/// Walks the HEAD tree iteratively, refusing one that would expand past the caps (§4.4).
fn head_tree_bound(
    repo: &gix::Repository,
    tree_id: gix::ObjectId,
    deadline: Instant,
    interrupt: &AtomicBool,
) -> Result<(), Stop> {
    let mut bound = TreeBound {
        repo,
        entries: 0,
        path_bytes: 0,
        tree_bytes: 0,
        current: (0, 0),
        saved: (0, 0),
        queue: VecDeque::new(),
        deadline,
        interrupt,
        stop: None,
    };
    bound.add_tree(&tree_id)?;
    let walked = gix::traverse::tree::depthfirst(
        tree_id,
        TreeWalkState::default(),
        &repo.objects,
        &mut bound,
    );
    match bound.stop {
        Some(stop) => Err(stop),
        None => walked.map_err(|e| classify(e, deadline, interrupt)),
    }
}

// ---- §4.4 status call shape -------------------------------------------------------------------

/// `FastEq` behind the 64 MiB cap: gix-filter reads a whole filtered file into memory, so a larger
/// file is reported modified without either side being read.
#[derive(Clone)]
struct CappedEq;

impl CompareBlobs for CappedEq {
    type Output = ();

    fn compare_blobs<'a, 'b>(
        &mut self,
        entry: &gix::index::Entry,
        worktree_blob_size: u64,
        data: impl ReadData<'a>,
        buf: &mut Vec<u8>,
    ) -> gix::ExnResult<Option<()>> {
        if worktree_blob_size > COMPARE_BLOBS_CAP {
            return Ok(Some(()));
        }
        #[cfg(test)]
        bump(&COMPARE_DEFERRALS);
        FastEq.compare_blobs(entry, worktree_blob_size, data, buf)
    }
}

/// §4.4: gix never opens a submodule during status.
#[derive(Clone)]
struct NoSubmodules;

impl SubmoduleStatus for NoSubmodules {
    type Output = ();

    fn status(
        &mut self,
        _entry: &gix::index::Entry,
        _rela_path: &BStr,
    ) -> gix::ExnResult<Option<()>> {
        Ok(None)
    }
}

/// Collects the index↔worktree half of `dirty_count` and the §3.3 `untracked_count`.
struct Tally<'w> {
    workdir: &'w Path,
    /// Index paths of mode 160000: nothing at or under them is ever counted.
    gitlinks: HashSet<BString>,
    dirty: HashSet<BString>,
    /// Directories already counted as one nested repository.
    nested: HashSet<BString>,
    has_dot_git: HashMap<BString, bool>,
    untracked: u32,
}

impl Tally<'_> {
    /// Whether `dir` holds a `.git` entry of any kind: `lstat` only, never opened.
    fn holds_dot_git(&mut self, dir: &BStr) -> bool {
        if let Some(known) = self.has_dot_git.get(dir) {
            return *known;
        }
        let found = self
            .workdir
            .join(gix::path::from_bstr(dir))
            .join(".git")
            .symlink_metadata()
            .is_ok();
        self.has_dot_git.insert(dir.to_owned(), found);
        found
    }

    /// Counts the untracked file `rela_path`, or (`is_dir`) the directory itself. The shallowest
    /// ancestor that is a gitlink hides everything below it; the shallowest that holds a `.git`
    /// entry counts once as a nested repository.
    fn count_untracked(&mut self, rela_path: &BStr, is_dir: bool) {
        let mut dirs: Vec<&BStr> = rela_path
            .iter()
            .enumerate()
            .filter(|&(_, &byte)| byte == b'/')
            .map(|(slash, _)| rela_path[..slash].as_bstr())
            .collect();
        if is_dir {
            dirs.push(rela_path);
        }
        for dir in dirs {
            if self.gitlinks.contains(dir) {
                return;
            }
            if self.holds_dot_git(dir) {
                if self.nested.insert(dir.to_owned()) {
                    self.untracked += 1;
                }
                return;
            }
        }
        if !is_dir {
            self.untracked += 1;
        }
    }
}

impl<'index> VisitEntry<'index> for Tally<'_> {
    type ContentChange = ();
    type SubmoduleStatus = ();

    fn visit_entry(&mut self, entry: Entry<'index, (), ()>) {
        match entry {
            Entry::Modification {
                rela_path, status, ..
            } => match status {
                EntryStatus::Conflict { .. }
                | EntryStatus::Change(_)
                | EntryStatus::IntentToAdd => {
                    self.dirty.insert(rela_path.to_owned());
                }
                // Only stat data differs: the content is unchanged.
                EntryStatus::NeedsUpdate(_) => {}
            },
            Entry::DirectoryContents { entry, .. } => {
                // With `recurse_repositories(true)` the dirwalk never reads a nested repository's
                // `.git`; it reports the entry as pruned, which is how a directory holding nothing
                // else (`a/.git/HEAD` only) is still seen.
                if entry.property == Some(gix::dir::entry::Property::DotGit) {
                    if let Some(parent_end) = entry.rela_path.rfind_byte(b'/') {
                        self.count_untracked(entry.rela_path[..parent_end].as_bstr(), true);
                    }
                } else if entry.status == gix::dir::entry::Status::Untracked
                    && matches!(
                        entry.disk_kind,
                        Some(gix::dir::entry::Kind::File | gix::dir::entry::Kind::Symlink)
                    )
                {
                    self.count_untracked(entry.rela_path.as_ref(), false);
                }
            }
            // Index↔worktree rename tracking is off (§3.3), so nothing is paired.
            Entry::Rewrite { .. } => {}
        }
    }
}

/// §4.4 pre-status checks (a) and (b), before either status call.
fn pre_status_checks(
    repo: &gix::Repository,
    index: &gix::index::State,
    workdir: &Path,
    deadline: Instant,
    interrupt: &AtomicBool,
) -> Result<(), Stop> {
    let mut pattern_bytes = 0u64;
    for (position, entry) in index.entries().iter().enumerate() {
        if (position as u64).is_multiple_of(ENTRY_DEADLINE_STRIDE) {
            check(deadline, interrupt)?;
        }
        let path = entry.path(index);
        let skip_worktree = entry.flags.contains(Flags::SKIP_WORKTREE);
        // (a) status opens a 100755 file with `File::open`: a FIFO blocks, a symlink is followed.
        if entry.mode == Mode::FILE_EXECUTABLE
            && !skip_worktree
            && workdir
                .join(gix::path::from_bstr(path))
                .symlink_metadata()
                .is_ok_and(|md| !md.is_file())
        {
            return Err(Stop::UnsafeTrackedFile);
        }
        // (b) blobs gix parses as pattern files retain 40x to 70x their size.
        if is_pattern_blob(path, entry, skip_worktree)
            && let Some(header) = repo.try_find_header(entry.id).map_err(git_err)?
        {
            pattern_bytes += header.size();
            if header.size() > PATTERN_BLOB_CAP || pattern_bytes > PATTERN_BLOBS_TOTAL_CAP {
                return Err(Stop::PatternFilesTooLarge);
            }
        }
    }
    Ok(())
}

fn is_pattern_blob(path: &BStr, entry: &gix::index::Entry, skip_worktree: bool) -> bool {
    let name = path.rsplit(|&b| b == b'/').next().unwrap_or_default();
    entry.mode == Mode::FILE
        && match name {
            b".gitattributes" => matches!(entry.stage(), Stage::Unconflicted | Stage::Ours),
            b".gitignore" => skip_worktree,
            _ => false,
        }
}

/// `(dirty_count, untracked_count)` of the non-bare worktree `repo` is open on.
fn status_counts(
    repo: &gix::Repository,
    workdir: &Path,
    head_tree: Option<gix::ObjectId>,
    deadline: Instant,
    interrupt: &AtomicBool,
) -> Result<(u32, u32), Stop> {
    if let Some(tree) = head_tree {
        head_tree_bound(repo, tree, deadline, interrupt)?;
    }
    let index = repo.index_or_empty().map_err(git_err)?;
    pre_status_checks(repo, &index, workdir, deadline, interrupt)?;
    check(deadline, interrupt)?;

    let mut tally = Tally {
        workdir,
        gitlinks: index
            .entries()
            .iter()
            .filter(|e| e.mode == Mode::COMMIT)
            .map(|e| e.path(&index).to_owned())
            .collect(),
        dirty: HashSet::new(),
        nested: HashSet::new(),
        has_dot_git: HashMap::new(),
        untracked: 0,
    };
    let tree = head_tree.unwrap_or_else(|| gix::ObjectId::empty_tree(repo.object_hash()));
    let rewrites = gix::diff::Rewrites {
        copies: None,
        percentage: Some(RENAME_SIMILARITY),
        limit: RENAME_LIMIT,
        ..Default::default()
    };
    #[cfg(test)]
    bump(&TREE_INDEX_CALLS);
    let mut staged = HashSet::new();
    repo.tree_index_status(
        &tree,
        &index,
        None,
        TrackRenames::Given(rewrites),
        |change, _, _| {
            // A rewrite's location is its destination, so a staged rename is keyed by it.
            staged.insert(change.location().to_owned());
            Ok(if interrupt.load(Ordering::Relaxed) {
                IndexAction::Break(())
            } else {
                IndexAction::Continue(())
            })
        },
    )
    .map_err(|e| classify(e, deadline, interrupt))?;
    check(deadline, interrupt)?;

    let dirwalk = repo
        .dirwalk_options()
        .map_err(git_err)?
        .recurse_repositories(true)
        .classify_untracked_bare_repositories(false)
        .emit_untracked(EmissionMode::Matching)
        .emit_pruned(true);
    repo.index_worktree_status(
        &index,
        Vec::<BString>::new(),
        &mut tally,
        CappedEq,
        NoSubmodules,
        &mut gix::progress::Discard,
        interrupt,
        StatusOptions {
            sorting: None,
            dirwalk_options: Some(dirwalk),
            rewrites: None,
            thread_limit: Some(1),
        },
    )
    .map_err(|e| classify(e, deadline, interrupt))?;

    tally.dirty.extend(staged);
    Ok((tally.dirty.len() as u32, tally.untracked))
}

// ---- refs: default branch, upstream, ahead / behind / unpushed, merged ------------------------

/// Commits reachable from `tip` and not from `hidden`, saturating at `limit`. The commit-graph is
/// never used (§4.4).
fn count_commits(
    repo: &gix::Repository,
    tip: gix::ObjectId,
    hidden: Vec<gix::ObjectId>,
    limit: u32,
    deadline: Instant,
    interrupt: &AtomicBool,
) -> Result<u32, Stop> {
    let walk = repo
        .rev_walk([tip])
        .with_hidden(hidden)
        .use_commit_graph(false)
        .all()
        .map_err(git_err)?;
    let mut count = 0;
    for commit in walk {
        commit.map_err(git_err)?;
        #[cfg(test)]
        bump(&COMMITS_WALKED);
        count += 1;
        if count >= limit {
            break;
        }
        if count % COMMIT_DEADLINE_STRIDE == 0 {
            check(deadline, interrupt)?;
        }
    }
    Ok(count)
}

/// A repo-local config value, only if gix would trust its source (RISKS P19: plain snapshot
/// getters ignore trust, the filtered accessors do not).
fn trusted_config(repo: &gix::Repository, key: &str) -> Option<String> {
    repo.config_snapshot()
        .plumbing()
        .string_filter(key, &mut gix::config::section::is_trusted)
        .map(|value| value.to_string())
}

/// Whether a trusted `[remote "<name>"]` section exists. Config, not `refs/remotes/*`: at
/// reduced trust repo-local remotes do not count (§3.3 "untrusted" upstream rule).
pub fn has_remote(repo: &gix::Repository) -> bool {
    repo.config_snapshot()
        .plumbing()
        .sections_by_name("remote")
        .is_some_and(|mut sections| {
            sections.any(|section| {
                section.header().subsection_name().is_some()
                    && gix::config::section::is_trusted(section.meta())
            })
        })
}

fn resolve_ref(repo: &gix::Repository, name: &str) -> Result<Option<gix::ObjectId>, Stop> {
    // A name gix refuses (`..` in a remote name, say) cannot be a ref: unresolvable, not an error.
    let Ok(full_name) = gix::refs::FullName::try_from(name) else {
        return Ok(None);
    };
    match repo
        .try_find_reference(full_name.as_ref())
        .map_err(git_err)?
    {
        Some(reference) => Ok(Some(
            reference.into_fully_peeled_id().map_err(git_err)?.detach(),
        )),
        None => Ok(None),
    }
}

/// `(<remote>/<branch>, tip)` of `branch`'s upstream when configured (trusted), and resolvable.
/// ponytail: the tracking ref is `refs/remotes/<remote>/<branch>`; a custom fetch refspec is not
/// consulted, add it if a fixture needs one.
fn upstream_of(
    repo: &gix::Repository,
    branch: &str,
) -> Result<Option<(String, gix::ObjectId)>, Stop> {
    let (Some(remote), Some(merge)) = (
        trusted_config(repo, &format!("branch.{branch}.remote")),
        trusted_config(repo, &format!("branch.{branch}.merge")),
    ) else {
        return Ok(None);
    };
    let Some(upstream_branch) = merge.strip_prefix("refs/heads/") else {
        return Ok(None);
    };
    if remote == "." {
        return Ok(None);
    }
    let tip = resolve_ref(repo, &format!("refs/remotes/{remote}/{upstream_branch}"))?;
    Ok(tip.map(|tip| (format!("{remote}/{upstream_branch}"), tip)))
}

fn remote_tips(repo: &gix::Repository) -> Result<Vec<gix::ObjectId>, Stop> {
    let platform = repo.references().map_err(git_err)?;
    let mut tips = Vec::new();
    for reference in platform.prefixed("refs/remotes/").map_err(git_err)? {
        // A symbolic `origin/HEAD` has no id of its own; its target is listed on its own.
        if let Some(id) = reference.map_err(git_err)?.try_id() {
            tips.push(id.detach());
        }
    }
    Ok(tips)
}

#[derive(Default)]
struct Relation {
    upstream: Option<String>,
    ahead: Option<u32>,
    behind: Option<u32>,
    unpushed: Option<u32>,
}

fn branch_relation(
    repo: &gix::Repository,
    branch: &str,
    tip: gix::ObjectId,
    deadline: Instant,
    interrupt: &AtomicBool,
) -> Result<Relation, Stop> {
    let walk =
        |from, hidden| count_commits(repo, from, hidden, COMMIT_SATURATION, deadline, interrupt);
    if let Some((upstream, upstream_tip)) = upstream_of(repo, branch)? {
        let ahead = walk(tip, vec![upstream_tip])?;
        let behind = walk(upstream_tip, vec![tip])?;
        return Ok(Relation {
            upstream: Some(upstream),
            ahead: Some(ahead),
            behind: Some(behind),
            unpushed: Some(ahead),
        });
    }
    if has_remote(repo) {
        let unpushed = walk(tip, remote_tips(repo)?)?;
        return Ok(Relation {
            unpushed: Some(unpushed),
            ..Relation::default()
        });
    }
    Ok(Relation::default())
}

/// §3.3 default branch. Errors read as "none": a repo too broken to read refs fails the
/// worktree state on its own.
pub fn default_branch(repo: &gix::Repository) -> Option<String> {
    if let Ok(Some(origin_head)) = repo.try_find_reference("refs/remotes/origin/HEAD")
        && let gix::refs::TargetRef::Symbolic(target) = origin_head.target()
        && let Some(local) = target.as_bstr().strip_prefix(b"refs/remotes/origin/")
    {
        return Some(local.to_str_lossy().into_owned());
    }
    ["main", "master"].into_iter().find_map(|name| {
        let exists = repo
            .try_find_reference(format!("refs/heads/{name}").as_str())
            .is_ok_and(|found| found.is_some());
        exists.then(|| name.to_string())
    })
}

// ---- the entry points -------------------------------------------------------------------------

/// Every git field `null` / `0`: the shape of an error row (§4.1) and the start of a real one.
fn blank_state(path: &Path, is_main: bool) -> WorktreeState {
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
        error: None,
        locked: false,
    }
}

/// The state of the worktree `repo` is open on (`wt_path` is only its reported path). Any failure
/// is a row with `error` set and every git field `null` / `0`.
pub fn worktree_state(
    repo: &gix::Repository,
    wt_path: &Path,
    is_main: bool,
    default_branch: Option<&str>,
    deadline: Instant,
    interrupt: &AtomicBool,
) -> WorktreeState {
    let mut state = blank_state(wt_path, is_main);
    match check(deadline, interrupt)
        .and_then(|()| fill_state(&mut state, repo, default_branch, deadline, interrupt))
    {
        Ok(()) => state,
        Err(stop) => WorktreeState {
            error: Some(stop.message()),
            locked: false,
            ..blank_state(wt_path, is_main)
        },
    }
}

fn fill_state(
    state: &mut WorktreeState,
    repo: &gix::Repository,
    default_branch: Option<&str>,
    deadline: Instant,
    interrupt: &AtomicBool,
) -> Result<(), Stop> {
    let head = repo.head().map_err(git_err)?;
    let tip = head.id().map(|id| id.detach());
    let commit = tip
        .map(|id| repo.find_commit(id).map_err(git_err))
        .transpose()?;
    if let Some(workdir) = repo.workdir() {
        let head_tree = commit
            .as_ref()
            .map(|c| c.tree_id().map(|id| id.detach()).map_err(git_err))
            .transpose()?;
        (state.dirty_count, state.untracked_count) =
            status_counts(repo, workdir, head_tree, deadline, interrupt)?;
    }

    state.detached = head.is_detached();
    state.branch = head
        .referent_name()
        .map(|name| name.shorten().to_str_lossy().into_owned());
    state.head = tip.map(|id| id.to_string());
    if let Some(commit) = &commit {
        let seconds = commit.time().map_err(git_err)?.seconds;
        state.last_commit_at = Some(Timestamp::from_second(seconds).map_err(git_err)?);
    }
    let (Some(branch), Some(tip)) = (state.branch.as_deref(), tip) else {
        return Ok(());
    };
    let relation = branch_relation(repo, branch, tip, deadline, interrupt)?;
    state.upstream = relation.upstream;
    state.ahead = relation.ahead;
    state.behind = relation.behind;
    state.unpushed = relation.unpushed;
    if !state.is_main
        && let Some(default) = default_branch.filter(|default| *default != branch)
        && let Some(default_tip) = resolve_ref(repo, &format!("refs/heads/{default}"))?
    {
        // Merged iff no commit of the branch lies outside the default branch.
        state.merged = count_commits(repo, tip, vec![default_tip], 1, deadline, interrupt)? == 0;
    }
    Ok(())
}

/// A worktree registered under `.git/worktrees/<name>` whose directory is gone: only its branch is
/// read, from the admin dir's `HEAD` (§3.3).
pub fn missing_worktree_state(admin_dir: &Path, wt_path: &Path) -> WorktreeState {
    let branch = read_small_file(&admin_dir.join("HEAD"), ADMIN_HEAD_CAP)
        .ok()
        .and_then(|content| branch_of_head_file(&content));
    WorktreeState {
        branch,
        missing: true,
        // Existence only (§4.4): `locked` may be a FIFO or unreadable, so it is never opened.
        locked: std::fs::symlink_metadata(admin_dir.join("locked")).is_ok(),
        ..blank_state(wt_path, false)
    }
}

fn branch_of_head_file(content: &[u8]) -> Option<String> {
    let first_line = content.lines().next()?;
    let name = first_line.trim().strip_prefix(b"ref: refs/heads/")?;
    (!name.is_empty())
        .then(|| std::str::from_utf8(name).ok().map(str::to_string))
        .flatten()
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs::Permissions;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::{Command, Output, Stdio};
    use std::time::Duration;

    use gix::sec::Trust;
    use rollcall_core::{GitOptions, Timestamp};

    use super::*;
    use crate::testgit::{
        add_submodule, add_worktree, commit_file, git, init_bare, init_repo, mkfifo, tempdir_real,
        within,
    };
    use crate::trust::{open_hardened, open_hardened_with_trust};

    /// 2026-01-01T00:00:00Z, the committer date `testgit` gives every commit.
    const FIXED_COMMIT_SECONDS: i64 = 1_767_225_600;
    const SATURATION: u32 = 10_000;

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    fn options(base: &Path, excludes_file: Option<PathBuf>) -> GitOptions {
        GitOptions {
            root: base.to_path_buf(),
            home: base.join("nowhere-home"),
            skip_paths: Vec::new(),
            repo_budget: Duration::from_secs(5),
            excludes_file,
        }
    }

    // ---- git plumbing the shared helpers lack (stdin, tolerated failure) -------------------

    fn git_raw(dir: &Path, args: &[&str], input: &[u8]) -> Output {
        let path = std::env::var_os("PATH").expect("PATH is set");
        let git_bin = std::env::split_paths(&path)
            .map(|d| d.join("git"))
            .find(|c| c.is_file())
            .expect("a git binary on PATH");
        let date = format!("{FIXED_COMMIT_SECONDS} +0000");
        let mut child = Command::new(git_bin)
            .current_dir(dir)
            .args([
                "-c",
                "init.defaultBranch=main",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env_clear()
            .env("PATH", path)
            .env("HOME", "/nonexistent-rollcall-test-home")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_DATE", &date)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn git");
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(input)
            .expect("write git stdin");
        child.wait_with_output().expect("wait for git")
    }

    /// Runs git with `input` on stdin, panics on failure, returns trimmed stdout.
    fn git_in(dir: &Path, args: &[&str], input: &str) -> String {
        let out = git_raw(dir, args, input.as_bytes());
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn fast_import(dir: &Path, stream: &str) {
        git_in(dir, &["fast-import", "--quiet"], stream);
    }

    fn rev_parse(dir: &Path, rev: &str) -> String {
        git(dir, &["rev-parse", rev]).trim().to_string()
    }

    fn rev_list_count(dir: &Path, args: &[&str]) -> u32 {
        let mut full = vec!["rev-list", "--count"];
        full.extend(args);
        git(dir, &full).trim().parse().expect("a count")
    }

    /// `git status --porcelain=v2 --branch --untracked-files=all`, reduced to what §3.3 defines.
    #[derive(Debug, PartialEq)]
    struct Porcelain {
        dirty: u32,
        untracked: u32,
        upstream: Option<String>,
        ahead_behind: Option<(u32, u32)>,
    }

    fn porcelain(dir: &Path, excludes: Option<&Path>) -> Porcelain {
        let excludes_config = excludes.map(|p| format!("core.excludesFile={}", p.display()));
        let mut args: Vec<&str> = Vec::new();
        if let Some(config) = &excludes_config {
            args.extend(["-c", config]);
        }
        args.extend([
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=all",
        ]);
        let mut parsed = Porcelain {
            dirty: 0,
            untracked: 0,
            upstream: None,
            ahead_behind: None,
        };
        for line in git(dir, &args).lines() {
            if let Some(name) = line.strip_prefix("# branch.upstream ") {
                parsed.upstream = Some(name.to_string());
            } else if let Some(counts) = line.strip_prefix("# branch.ab ") {
                let (ahead, behind) = counts.split_once(' ').expect("+A -B");
                parsed.ahead_behind = Some((
                    ahead.trim_start_matches('+').parse().expect("ahead"),
                    behind.trim_start_matches('-').parse().expect("behind"),
                ));
            } else if line.starts_with("1 ") || line.starts_with("2 ") || line.starts_with("u ") {
                parsed.dirty += 1;
            } else if line.starts_with("? ") {
                parsed.untracked += 1;
            }
        }
        parsed
    }

    fn assert_agrees_with_git(dir: &Path, state: &WorktreeState, excludes: Option<&Path>) {
        let expected = porcelain(dir, excludes);
        assert_eq!(state.dirty_count, expected.dirty, "dirty_count vs git");
        assert_eq!(
            state.untracked_count, expected.untracked,
            "untracked vs git"
        );
        assert_eq!(state.upstream, expected.upstream, "upstream vs git");
        assert_eq!(
            state.ahead.zip(state.behind),
            expected.ahead_behind,
            "ahead/behind vs git"
        );
    }

    // ---- running the code under test ----------------------------------------------------------

    struct Run {
        state: WorktreeState,
        tree_index_calls: usize,
        commits_walked: usize,
        compare_deferrals: usize,
    }

    /// Opens `git_dir` hardened (optionally at a forced trust level) and computes the state of
    /// `workdir` on a watchdog thread, resolving the default branch in the same repo.
    fn run_at(
        git_dir: &Path,
        workdir: &Path,
        is_main: bool,
        base: &Path,
        excludes: Option<PathBuf>,
        trust: Option<Trust>,
    ) -> Run {
        let (git_dir, workdir) = (git_dir.to_path_buf(), workdir.to_path_buf());
        let opts = options(base, excludes);
        within(move || {
            let repo = match trust {
                Some(level) => open_hardened_with_trust(&git_dir, &opts, level),
                None => open_hardened(&git_dir, &opts),
            }
            .expect("hardened open");
            let default = default_branch(&repo);
            let state = worktree_state(
                &repo,
                &workdir,
                is_main,
                default.as_deref(),
                far(),
                &AtomicBool::new(false),
            );
            Run {
                state,
                tree_index_calls: TREE_INDEX_CALLS.with(Cell::get),
                commits_walked: COMMITS_WALKED.with(Cell::get),
                compare_deferrals: COMPARE_DEFERRALS.with(Cell::get),
            }
        })
    }

    fn run_main(repo: &Path, base: &Path) -> Run {
        run_at(&repo.join(".git"), repo, true, base, None, None)
    }

    fn write(dir: &Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).expect("write file");
    }

    /// `main` pushed to a bare `origin.git` with upstream tracking set.
    fn repo_with_origin(base: &Path) -> PathBuf {
        let origin = base.join("origin.git");
        init_bare(&origin);
        let main = base.join("main");
        init_repo(&main);
        git(
            &main,
            &["remote", "add", "origin", origin.to_str().expect("utf-8")],
        );
        git(&main, &["push", "-q", "-u", "origin", "main"]);
        main
    }

    // ---- §10.2 "Git state" ---------------------------------------------------------------------

    #[test]
    fn dirty_staged_change_counts_one() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        write(&repo, "README", "changed\n");
        git(&repo, &["add", "README"]);

        let run = run_main(&repo, &base);

        assert_eq!(run.state.error, None);
        assert_eq!(run.state.dirty_count, 1);
        assert_eq!(run.state.untracked_count, 0);
        assert_agrees_with_git(&repo, &run.state, None);
    }

    #[test]
    fn dirty_unstaged_change_counts_one() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        write(&repo, "README", "changed\n");

        let run = run_main(&repo, &base);

        assert_eq!(run.state.dirty_count, 1);
        assert_eq!(run.state.untracked_count, 0);
        assert_agrees_with_git(&repo, &run.state, None);
    }

    #[test]
    fn staged_change_plus_unstaged_edit_of_the_same_path_counts_once() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        write(&repo, "README", "staged\n");
        git(&repo, &["add", "README"]);
        write(&repo, "README", "staged and then edited\n");

        let run = run_main(&repo, &base);

        assert_eq!(run.state.dirty_count, 1);
        assert_agrees_with_git(&repo, &run.state, None);
    }

    #[test]
    fn staged_rename_counts_once() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        git(&repo, &["mv", "README", "MOVED"]);

        let run = run_main(&repo, &base);

        assert_eq!(run.state.dirty_count, 1);
        assert_eq!(run.state.untracked_count, 0);
        assert_agrees_with_git(&repo, &run.state, None);
    }

    #[test]
    fn staged_rename_plus_unstaged_edit_of_the_destination_counts_once() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        git(&repo, &["mv", "README", "MOVED"]);
        write(&repo, "MOVED", "hello, edited after the move\n");

        let run = run_main(&repo, &base);

        assert_eq!(run.state.dirty_count, 1);
        assert_agrees_with_git(&repo, &run.state, None);
    }

    #[test]
    fn unstaged_move_is_one_deletion_and_one_untracked_file() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        std::fs::rename(repo.join("README"), repo.join("MOVED")).expect("mv");

        let run = run_main(&repo, &base);

        assert_eq!(run.state.dirty_count, 1);
        assert_eq!(run.state.untracked_count, 1);
        assert_agrees_with_git(&repo, &run.state, None);
    }

    #[test]
    fn untracked_counts_files_not_directories_and_skips_ignored() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        std::fs::create_dir_all(repo.join("nested/deeper")).expect("mkdir");
        write(&repo, "nested/deeper/one.txt", "1\n");
        write(&repo, "two.txt", "2\n");
        std::fs::create_dir_all(repo.join("empty-dir")).expect("mkdir");
        write(&repo, "ignored.txt", "x\n");
        write(&repo, ".git/info/exclude", "ignored.txt\n");

        let run = run_main(&repo, &base);

        assert_eq!(run.state.untracked_count, 2);
        assert_eq!(run.state.dirty_count, 0);
        assert_agrees_with_git(&repo, &run.state, None);
    }

    #[test]
    fn user_excludes_file_is_honoured_through_the_open_override() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        let excludes = base.join("user-excludes");
        std::fs::write(&excludes, "*.log\n").expect("write excludes");
        write(&repo, "noise.log", "x\n");
        write(&repo, "kept.txt", "y\n");

        let with = run_at(
            &repo.join(".git"),
            &repo,
            true,
            &base,
            Some(excludes.clone()),
            None,
        );
        let without = run_main(&repo, &base);

        assert_eq!(with.state.untracked_count, 1);
        assert_agrees_with_git(&repo, &with.state, Some(&excludes));
        // Control: without the override the same file is counted, so the 1 above is the rule.
        assert_eq!(without.state.untracked_count, 2);
    }

    #[test]
    fn ahead_and_behind_an_upstream() {
        let (_t, base) = tempdir_real();
        let main = repo_with_origin(&base);
        let other = base.join("other");
        git(&base, &["clone", "-q", "origin.git", "other"]);
        commit_file(&other, "theirs", "t\n", "theirs");
        git(&other, &["push", "-q", "origin", "main"]);
        commit_file(&main, "mine", "m\n", "mine");
        git(&main, &["fetch", "-q", "origin"]);

        let run = run_main(&main, &base);

        assert_eq!(run.state.upstream.as_deref(), Some("origin/main"));
        assert_eq!(run.state.ahead, Some(1));
        assert_eq!(run.state.behind, Some(1));
        assert_eq!(run.state.unpushed, Some(1));
        assert_eq!(
            rev_list_count(
                &main,
                &["refs/heads/main", "--not", "refs/remotes/origin/main"]
            ),
            1
        );
        assert_agrees_with_git(&main, &run.state, None);
    }

    #[test]
    fn no_upstream_with_a_remote_counts_commits_on_no_remote_branch() {
        let (_t, base) = tempdir_real();
        let main = repo_with_origin(&base);
        git(&main, &["checkout", "-q", "-b", "feature"]);
        commit_file(&main, "one", "1\n", "one");
        commit_file(&main, "two", "2\n", "two");

        let run = run_main(&main, &base);

        assert_eq!(run.state.upstream, None);
        assert_eq!(run.state.ahead, None);
        assert_eq!(run.state.behind, None);
        assert_eq!(run.state.unpushed, Some(2));
        assert_eq!(rev_list_count(&main, &["feature", "--not", "--remotes"]), 2);
        assert_agrees_with_git(&main, &run.state, None);
    }

    #[test]
    fn no_remote_leaves_upstream_and_unpushed_null() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        commit_file(&repo, "more", "x\n", "more");

        let run = run_main(&repo, &base);
        let has_remote = within({
            let git_dir = repo.join(".git");
            let opts = options(&base, None);
            move || has_remote(&open_hardened(&git_dir, &opts).expect("open"))
        });

        assert_eq!(run.state.upstream, None);
        assert_eq!(run.state.ahead, None);
        assert_eq!(run.state.unpushed, None);
        assert!(!has_remote);
        assert_agrees_with_git(&repo, &run.state, None);
    }

    #[test]
    fn detached_head_has_no_branch_and_a_full_hex_head() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        git(&repo, &["checkout", "-q", "--detach"]);
        let sha = rev_parse(&repo, "HEAD");

        let run = run_main(&repo, &base);

        assert!(run.state.detached);
        assert_eq!(run.state.branch, None);
        assert_eq!(run.state.head.as_deref(), Some(sha.as_str()));
        assert_eq!(sha.len(), 40);
        assert_eq!(run.state.error, None);
    }

    /// A linked worktree `wt` on branch `feat` off a repo whose default branch is `main`.
    fn repo_with_linked_feat(base: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let main = base.join("main");
        init_repo(&main);
        let wt = base.join("wt");
        add_worktree(&main, &wt, "feat");
        let admin = main.join(".git/worktrees/wt");
        (main, wt, admin)
    }

    #[test]
    fn linked_worktree_whose_tip_is_in_the_default_branch_is_merged() {
        let (_t, base) = tempdir_real();
        let (main, wt, admin) = repo_with_linked_feat(&base);
        commit_file(&wt, "feature", "f\n", "feature work");

        let unmerged = run_at(&admin, &wt, false, &base, None, None);
        git(
            &main,
            &["merge", "-q", "--no-ff", "-m", "merge feat", "feat"],
        );
        let merged = run_at(&admin, &wt, false, &base, None, None);

        assert_eq!(unmerged.state.branch.as_deref(), Some("feat"));
        assert!(!unmerged.state.merged, "feat has a commit main lacks");
        assert!(merged.state.merged);
        assert_eq!(merged.state.error, None);
        assert_eq!(
            rev_list_count(&main, &["refs/heads/feat", "--not", "refs/heads/main"]),
            0
        );
    }

    #[test]
    fn merged_is_false_for_the_main_worktree_and_for_the_default_branch() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);

        let as_main = run_at(&repo.join(".git"), &repo, true, &base, None, None);
        // Same repo treated as a linked worktree: its branch is the default branch.
        let on_default = run_at(&repo.join(".git"), &repo, false, &base, None, None);

        assert!(!as_main.state.merged);
        assert!(!on_default.state.merged);
    }

    #[test]
    fn squash_merged_branch_is_not_reported_merged() {
        let (_t, base) = tempdir_real();
        let (main, wt, admin) = repo_with_linked_feat(&base);
        commit_file(&wt, "feature", "f\n", "feature work");
        git(&main, &["merge", "-q", "--squash", "feat"]);
        git(&main, &["commit", "-q", "-m", "squashed feat"]);

        let run = run_at(&admin, &wt, false, &base, None, None);

        assert!(!run.state.merged, "squash merges are not detected (§3.3)");
        assert_eq!(run.state.error, None);
    }

    #[test]
    fn linked_worktree_state_is_read_from_its_own_workdir() {
        let (_t, base) = tempdir_real();
        let (main, wt, admin) = repo_with_linked_feat(&base);
        write(&wt, "README", "edited in the linked worktree\n");
        write(&wt, "new.txt", "n\n");

        let linked = run_at(&admin, &wt, false, &base, None, None);
        let primary = run_main(&main, &base);

        assert_eq!(linked.state.path, wt);
        assert_eq!(linked.state.dirty_count, 1);
        assert_eq!(linked.state.untracked_count, 1);
        assert_agrees_with_git(&wt, &linked.state, None);
        assert_eq!(primary.state.dirty_count, 0);
        assert_eq!(primary.state.untracked_count, 0);
    }

    #[test]
    fn old_clean_commit_yields_the_inactive_shape() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        let sha = rev_parse(&repo, "HEAD");

        let run = run_main(&repo, &base);

        assert_eq!(
            run.state,
            WorktreeState {
                path: repo.clone(),
                is_main: true,
                branch: Some("main".to_string()),
                detached: false,
                head: Some(sha),
                last_commit_at: Some(
                    Timestamp::from_second(FIXED_COMMIT_SECONDS).expect("timestamp")
                ),
                upstream: None,
                ahead: None,
                behind: None,
                unpushed: None,
                dirty_count: 0,
                untracked_count: 0,
                missing: false,
                merged: false,
                error: None,
                locked: false,
            }
        );
    }

    #[test]
    fn default_branch_prefers_origin_head_then_main_then_master() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        let default_of = |repo: &Path| {
            let (git_dir, opts) = (repo.join(".git"), options(repo, None));
            within(move || default_branch(&open_hardened(&git_dir, &opts).expect("open")))
        };

        assert_eq!(default_of(&repo).as_deref(), Some("main"));

        git(&repo, &["branch", "master"]);
        assert_eq!(
            default_of(&repo).as_deref(),
            Some("main"),
            "main beats master"
        );

        git(&repo, &["checkout", "-q", "master"]);
        git(&repo, &["branch", "-D", "main"]);
        assert_eq!(default_of(&repo).as_deref(), Some("master"));

        git(
            &repo,
            &["update-ref", "refs/remotes/origin/develop", "HEAD"],
        );
        git(
            &repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/develop",
            ],
        );
        assert_eq!(
            default_of(&repo).as_deref(),
            Some("develop"),
            "origin/HEAD wins"
        );
    }

    #[test]
    fn no_default_branch_reports_none_and_never_merged() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        std::fs::create_dir_all(&repo).expect("mkdir");
        git(&repo, &["init", "-q", "--initial-branch=trunk"]);
        commit_file(&repo, "f", "x\n", "first");

        let run = run_at(&repo.join(".git"), &repo, false, &base, None, None);
        let default = within({
            let (git_dir, opts) = (repo.join(".git"), options(&base, None));
            move || default_branch(&open_hardened(&git_dir, &opts).expect("open"))
        });

        assert_eq!(default, None);
        assert_eq!(run.state.branch.as_deref(), Some("trunk"));
        assert!(!run.state.merged);
        assert_eq!(run.state.error, None);
    }

    #[test]
    fn unborn_head_reports_the_branch_and_counts_staged_files() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        std::fs::create_dir_all(&repo).expect("mkdir");
        git(&repo, &["init", "-q"]);
        write(&repo, "first", "1\n");
        git(&repo, &["add", "first"]);

        let run = run_main(&repo, &base);

        assert_eq!(run.state.error, None);
        assert_eq!(run.state.branch.as_deref(), Some("main"));
        assert_eq!(run.state.head, None);
        assert_eq!(run.state.last_commit_at, None);
        assert_eq!(run.state.dirty_count, 1);
        assert_agrees_with_git(&repo, &run.state, None);
    }

    #[test]
    fn merge_conflict_counts_the_conflicted_path_once() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        git(&repo, &["checkout", "-q", "-b", "other"]);
        commit_file(&repo, "README", "other side\n", "other");
        git(&repo, &["checkout", "-q", "main"]);
        commit_file(&repo, "README", "main side\n", "main");
        let merge = git_raw(&repo, &["merge", "other"], b"");
        assert!(!merge.status.success(), "the merge must conflict");

        let run = run_main(&repo, &base);

        assert_eq!(run.state.error, None);
        assert_eq!(run.state.dirty_count, 1);
        assert_agrees_with_git(&repo, &run.state, None);
    }

    #[test]
    fn intent_to_add_counts_as_dirty_not_untracked() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        write(&repo, "planned", "later\n");
        git(&repo, &["add", "-N", "planned"]);

        let run = run_main(&repo, &base);

        assert_eq!(run.state.dirty_count, 1);
        assert_eq!(run.state.untracked_count, 0);
        assert_agrees_with_git(&repo, &run.state, None);
    }

    #[test]
    fn bare_repository_reports_refs_only() {
        let (_t, base) = tempdir_real();
        let main = base.join("main");
        init_repo(&main);
        git(&base, &["clone", "-q", "--bare", "main", "bare.git"]);
        let bare = base.join("bare.git");

        let run = run_at(&bare, &bare, true, &base, None, None);

        assert_eq!(run.state.error, None);
        assert_eq!(run.state.branch.as_deref(), Some("main"));
        assert_eq!(run.state.dirty_count, 0);
        assert_eq!(run.state.untracked_count, 0);
        assert_eq!(
            run.state.head.as_deref(),
            Some(rev_parse(&main, "HEAD").as_str())
        );
    }

    // ---- errors, deadline ------------------------------------------------------------------

    #[test]
    fn a_missing_head_tree_object_becomes_a_git_error_with_null_fields() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        write(&repo, "README", "dirty\n");
        let tree = rev_parse(&repo, "HEAD^{tree}");
        let (fan_out, rest) = tree.split_at(2);
        std::fs::remove_file(repo.join(".git/objects").join(fan_out).join(rest))
            .expect("remove the loose tree object");

        let run = run_main(&repo, &base);

        let error = run.state.error.expect("an error row");
        assert!(error.starts_with("git error: "), "{error}");
        assert_eq!(run.state.branch, None);
        assert_eq!(run.state.head, None);
        assert_eq!(run.state.dirty_count, 0);
        assert!(run.state.path.ends_with("r"));
        assert!(run.state.is_main);
    }

    #[test]
    fn an_expired_deadline_or_a_set_interrupt_is_a_timeout() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        write(&repo, "README", "dirty\n");
        let (git_dir, opts) = (repo.join(".git"), options(&base, None));

        let (expired, interrupted) = within({
            let repo_dir = repo.clone();
            move || {
                let gix_repo = open_hardened(&git_dir, &opts).expect("open");
                let expired = worktree_state(
                    &gix_repo,
                    &repo_dir,
                    true,
                    Some("main"),
                    Instant::now(),
                    &AtomicBool::new(false),
                );
                let flag = AtomicBool::new(true);
                let interrupted =
                    worktree_state(&gix_repo, &repo_dir, true, Some("main"), far(), &flag);
                (expired, interrupted)
            }
        });

        for state in [expired, interrupted] {
            assert_eq!(state.error.as_deref(), Some("timed out"));
            assert_eq!(state.dirty_count, 0);
            assert_eq!(state.branch, None);
        }
    }

    // ---- §4.4 HEAD tree bound --------------------------------------------------------------

    fn init_empty(base: &Path, name: &str) -> PathBuf {
        let dir = base.join(name);
        std::fs::create_dir_all(&dir).expect("mkdir");
        git(&dir, &["init", "-q"]);
        dir
    }

    fn head_tree(repo: &gix::Repository) -> gix::ObjectId {
        repo.head_commit()
            .expect("head commit")
            .tree_id()
            .expect("tree id")
            .detach()
    }

    /// `git mktree` lines for `count` entries named by `name(i)`, all pointing at `target`.
    fn mktree(
        dir: &Path,
        mode_and_kind: &str,
        target: &str,
        names: impl Iterator<Item = String>,
    ) -> String {
        let listing: String = names
            .map(|name| format!("{mode_and_kind} {target}\t{name}\n"))
            .collect();
        git_in(dir, &["mktree"], &listing)
    }

    fn commit_tree_as_main(dir: &Path, tree: &str) {
        let commit = git_in(dir, &["commit-tree", tree, "-m", "bomb"], "");
        git(dir, &["update-ref", "refs/heads/main", &commit]);
    }

    /// Three nested levels of 200 entries, each pointing at the same subtree: 8,040,200
    /// expanded paths from four tiny objects.
    fn tree_bomb(base: &Path) -> PathBuf {
        let dir = init_empty(base, "bomb");
        let blob = git_in(&dir, &["hash-object", "-w", "--stdin"], "x");
        let names = || (0..200).map(|i| format!("e{i:03}"));
        let leaf = mktree(&dir, "100644 blob", &blob, names());
        let middle = mktree(&dir, "040000 tree", &leaf, names());
        let root = mktree(&dir, "040000 tree", &middle, names());
        commit_tree_as_main(&dir, &root);
        dir
    }

    /// 999 top-level and 1,000 second-level entries, all with 500-byte names: 999,999 entries
    /// (under the entry cap) but about a gigabyte of path bytes.
    fn long_name_bomb(base: &Path) -> PathBuf {
        let dir = init_empty(base, "long-bomb");
        let blob = git_in(&dir, &["hash-object", "-w", "--stdin"], "x");
        let name = |i: usize| format!("{i:0>500}");
        let leaf = mktree(&dir, "100644 blob", &blob, (0..1000).map(name));
        let root = mktree(&dir, "040000 tree", &leaf, (0..999).map(name));
        commit_tree_as_main(&dir, &root);
        dir
    }

    /// One commit whose only file sits `depth` path components deep.
    fn deep_chain(base: &Path, name: &str, depth: usize) -> PathBuf {
        let dir = init_empty(base, name);
        let path = "d/".repeat(depth - 1) + "f";
        fast_import(
            &dir,
            &format!(
                "commit refs/heads/main\ncommitter Test <test@example.com> {FIXED_COMMIT_SECONDS} +0000\n\
                 data 2\nc\nM 100644 inline {path}\ndata 2\nf\n"
            ),
        );
        dir
    }

    fn bound_of(dir: &Path, base: &Path) -> Result<(), Stop> {
        let (git_dir, opts) = (dir.join(".git"), options(base, None));
        within(move || {
            let repo = open_hardened(&git_dir, &opts).expect("open");
            head_tree_bound(&repo, head_tree(&repo), far(), &AtomicBool::new(false))
        })
    }

    #[test]
    fn tree_bomb_stops_at_the_entry_cap_before_tree_index_status() {
        let (_t, base) = tempdir_real();
        let dir = tree_bomb(&base);

        let bound = bound_of(&dir, &base);
        let run = run_main(&dir, &base);

        assert_eq!(bound, Err(Stop::TreeTooLarge(Cap::Entries)));
        assert_eq!(
            run.state.error.as_deref(),
            Some("git error: HEAD tree too large")
        );
        assert_eq!(run.tree_index_calls, 0, "tree_index_status must not run");
        assert_eq!(run.state.branch, None);
        assert_eq!(run.state.dirty_count, 0);
    }

    #[test]
    fn long_name_bomb_stops_at_the_path_byte_cap_before_tree_index_status() {
        let (_t, base) = tempdir_real();
        let dir = long_name_bomb(&base);

        let bound = bound_of(&dir, &base);
        let run = run_main(&dir, &base);

        assert_eq!(bound, Err(Stop::TreeTooLarge(Cap::PathBytes)));
        assert_eq!(
            run.state.error.as_deref(),
            Some("git error: HEAD tree too large")
        );
        assert_eq!(run.tree_index_calls, 0, "tree_index_status must not run");
    }

    #[test]
    fn deep_chain_stops_at_the_depth_cap_without_overflowing_the_stack() {
        let (_t, base) = tempdir_real();
        let too_deep = deep_chain(&base, "chain-5000", 5_000);
        let exactly_at_cap = deep_chain(&base, "chain-4096", 4_096);
        let one_over = deep_chain(&base, "chain-4097", 4_097);

        assert_eq!(
            bound_of(&too_deep, &base),
            Err(Stop::TreeTooLarge(Cap::Depth))
        );
        assert_eq!(
            bound_of(&one_over, &base),
            Err(Stop::TreeTooLarge(Cap::Depth))
        );
        assert_eq!(bound_of(&exactly_at_cap, &base), Ok(()));
        let run = run_main(&too_deep, &base);
        assert_eq!(
            run.state.error.as_deref(),
            Some("git error: HEAD tree too large")
        );
        assert_eq!(run.tree_index_calls, 0);
    }

    // ---- §4.4 HEAD tree bound: tree bytes (P15b U-14) ---------------------------------------------

    const TREE_BYTES_CAP: usize = 128 * 1024 * 1024;
    const FILLER_NAME_LEN: usize = 300;

    /// Raw bytes of one tree object of exactly `size` bytes: an entry `a` naming `child` (when
    /// there is one), then filler blob entries with 300-byte names, the last padded to fit. The
    /// filler comes after the subtree, so a depth-first walk descends into `a` before it has
    /// visited any of it: only the tree-bytes count sees those levels pile up.
    fn padded_tree(child: Option<&str>, size: usize) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(size);
        if let Some(child) = child {
            bytes.extend(b"40000 a\0");
            for pair in child.as_bytes().chunks(2) {
                let hex = std::str::from_utf8(pair).expect("hex");
                bytes.push(u8::from_str_radix(hex, 16).expect("hex digit pair"));
            }
        }
        let entry_len = b"100644 ".len() + FILLER_NAME_LEN + 1 + 20;
        let count = (size - bytes.len()) / entry_len;
        let slack = size - bytes.len() - count * entry_len;
        for i in 0..count {
            let width = FILLER_NAME_LEN + if i + 1 == count { slack } else { 0 };
            bytes.extend(b"100644 ");
            bytes.extend(format!("{i:0>width$}").as_bytes());
            bytes.push(0);
            bytes.extend([0xab; 20]);
        }
        assert_eq!(bytes.len(), size);
        bytes
    }

    /// A HEAD tree that is a chain of trees of the given sizes (root first).
    fn tree_chain(base: &Path, name: &str, sizes: &[usize]) -> PathBuf {
        let dir = init_empty(base, name);
        let mut child: Option<String> = None;
        for &size in sizes.iter().rev() {
            let raw = padded_tree(child.as_deref(), size);
            let out = git_raw(
                &dir,
                &["hash-object", "-w", "-t", "tree", "--literally", "--stdin"],
                &raw,
            );
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            child = Some(
                String::from_utf8(out.stdout)
                    .expect("utf-8 id")
                    .trim()
                    .to_string(),
            );
        }
        commit_tree_as_main(&dir, child.as_deref().expect("at least one tree"));
        dir
    }

    #[test]
    fn a_chain_of_large_trees_trips_the_tree_bytes_cap_before_any_other() {
        let (_t, base) = tempdir_real();
        // 5 x 30 MiB = 150 MiB, each level listing its subtree first: only 5 entries are visited
        // before the total passes 128 MiB, so entries, path bytes and depth are all far under.
        let dir = tree_chain(&base, "fat-chain", &[30 * MIB as usize; 5]);

        let bound = bound_of(&dir, &base);
        let run = run_main(&dir, &base);

        assert_eq!(bound, Err(Stop::TreeTooLarge(Cap::TreeBytes)));
        assert_eq!(
            run.state.error.as_deref(),
            Some("git error: HEAD tree too large")
        );
        assert_eq!(run.tree_index_calls, 0, "tree_index_status must not run");
    }

    #[test]
    fn tree_bytes_are_capped_at_exactly_128_mib_root_included() {
        let (_t, base) = tempdir_real();
        let quarter = TREE_BYTES_CAP / 4;
        let at_cap = tree_chain(&base, "at-cap", &[quarter; 4]);
        let one_over = tree_chain(&base, "one-over", &[quarter, quarter, quarter, quarter + 1]);

        assert_eq!(bound_of(&at_cap, &base), Ok(()));
        assert_eq!(
            bound_of(&one_over, &base),
            Err(Stop::TreeTooLarge(Cap::TreeBytes))
        );
    }

    #[test]
    fn an_ordinary_tree_passes_the_bound_and_reaches_tree_index_status() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);

        let run = run_main(&repo, &base);

        assert_eq!(bound_of(&repo, &base), Ok(()));
        assert_eq!(run.tree_index_calls, 1);
    }

    #[test]
    fn the_bound_reports_a_timeout_once_the_deadline_has_passed() {
        let (_t, base) = tempdir_real();
        let dir = tree_bomb(&base);
        let (git_dir, opts) = (dir.join(".git"), options(&base, None));

        let bound = within(move || {
            let repo = open_hardened(&git_dir, &opts).expect("open");
            head_tree_bound(
                &repo,
                head_tree(&repo),
                Instant::now(),
                &AtomicBool::new(false),
            )
        });

        assert_eq!(bound, Err(Stop::TimedOut));
    }

    // ---- trust, commit graph, saturation ---------------------------------------------------

    #[test]
    fn reduced_trust_nulls_upstream_ahead_behind_and_remote_detection() {
        let (_t, base) = tempdir_real();
        let main = repo_with_origin(&base);
        commit_file(&main, "mine", "m\n", "mine");
        let git_dir = main.join(".git");

        let full = run_at(&git_dir, &main, true, &base, None, None);
        let reduced = run_at(&git_dir, &main, true, &base, None, Some(Trust::Reduced));
        let remote_detection = |trust| {
            let (git_dir, opts) = (git_dir.clone(), options(&base, None));
            within(move || {
                has_remote(&open_hardened_with_trust(&git_dir, &opts, trust).expect("open"))
            })
        };

        // Control: at full trust the same repo has all three.
        assert_eq!(full.state.upstream.as_deref(), Some("origin/main"));
        assert_eq!(full.state.ahead, Some(1));
        assert_eq!(full.state.behind, Some(0));
        assert!(remote_detection(Trust::Full));
        assert_eq!(reduced.state.error, None);
        assert_eq!(reduced.state.upstream, None);
        assert_eq!(reduced.state.ahead, None);
        assert_eq!(reduced.state.behind, None);
        assert_eq!(reduced.state.unpushed, None);
        assert!(!remote_detection(Trust::Reduced));
    }

    /// Rewrites the parent of `commit` in the commit-graph file to "none", so a walk that trusts
    /// the graph sees a shorter history than the object database holds.
    fn sever_parent_in_commit_graph(path: &Path, commit: &str) {
        const PARENT_NONE: [u8; 4] = 0x7000_0000u32.to_be_bytes();
        const OID_LEN: usize = 20;
        const COMMIT_ROW_LEN: usize = 36;
        let mut graph = std::fs::read(path).expect("read commit-graph");
        let chunk_count = usize::from(graph[6]);
        let chunk_offset = |id: &[u8; 4]| -> usize {
            (0..chunk_count)
                .map(|n| 8 + n * 12)
                .find(|&at| &graph[at..at + 4] == id)
                .map(|at| {
                    u64::from_be_bytes(graph[at + 4..at + 12].try_into().expect("8 bytes")) as usize
                })
                .expect("chunk present")
        };
        let (oid_list, commit_data) = (chunk_offset(b"OIDL"), chunk_offset(b"CDAT"));
        let wanted: Vec<u8> = (0..OID_LEN)
            .map(|n| u8::from_str_radix(&commit[2 * n..2 * n + 2], 16).expect("hex"))
            .collect();
        let row = (0..)
            .find(|n| graph[oid_list + n * OID_LEN..][..OID_LEN] == wanted[..])
            .expect("commit in graph");
        let parent = commit_data + row * COMMIT_ROW_LEN + OID_LEN;
        graph[parent..parent + 4].copy_from_slice(&PARENT_NONE);
        // git writes the graph read-only.
        std::fs::remove_file(path).expect("remove commit-graph");
        std::fs::write(path, graph).expect("write commit-graph");
    }

    /// The graph file `git commit-graph write` produced: the single file, or the one layer of a
    /// split chain (named by the chain file).
    fn written_graph_file(repo: &Path, split: bool) -> PathBuf {
        let info = repo.join(".git/objects/info");
        if !split {
            return info.join("commit-graph");
        }
        let chain =
            std::fs::read_to_string(info.join("commit-graphs/commit-graph-chain")).expect("chain");
        info.join(format!(
            "commit-graphs/graph-{}.graph",
            chain.lines().next().expect("one layer")
        ))
    }

    #[test]
    fn a_doctored_commit_graph_does_not_change_ahead_or_behind() {
        for split in [false, true] {
            let (_t, base) = tempdir_real();
            let main = repo_with_origin(&base);
            commit_file(&main, "one", "1\n", "one");
            commit_file(&main, "two", "2\n", "two");
            let tip = rev_parse(&main, "HEAD");
            let mut write = vec!["commit-graph", "write", "--reachable"];
            if split {
                write.push("--split");
            }
            git(&main, &write);
            sever_parent_in_commit_graph(&written_graph_file(&main, split), &tip);
            let (git_dir, opts) = (main.join(".git"), options(&base, None));

            // Control: a walk that does use the graph is fooled by the doctoring (1 instead of 2).
            let through_graph = within(move || {
                let repo = open_hardened(&git_dir, &opts).expect("open");
                let tip = repo.head_id().expect("head").detach();
                let origin = repo
                    .find_reference("refs/remotes/origin/main")
                    .expect("ref")
                    .id()
                    .detach();
                repo.rev_walk([tip])
                    .with_hidden([origin])
                    .use_commit_graph(true)
                    .all()
                    .expect("walk")
                    .count()
            });
            let run = run_main(&main, &base);

            assert_eq!(
                through_graph, 1,
                "split={split}: the doctored graph must be able to change the count"
            );
            assert_eq!(run.state.error, None, "split={split}");
            assert_eq!(run.state.ahead, Some(2), "split={split}");
            assert_eq!(run.state.unpushed, Some(2), "split={split}");
            assert_eq!(run.state.behind, Some(0), "split={split}");
            let oracle = [
                "-c",
                "core.commitGraph=false",
                "rev-list",
                "--count",
                "main",
                "--not",
                "origin/main",
            ];
            assert_eq!(git(&main, &oracle).trim(), "2", "split={split}");
        }
    }

    #[test]
    fn an_unreadable_commit_graph_chain_does_not_change_ahead_or_behind() {
        let (_t, base) = tempdir_real();
        let main = repo_with_origin(&base);
        commit_file(&main, "mine", "m\n", "mine");
        git(&main, &["commit-graph", "write", "--reachable", "--split"]);
        let chain = main.join(".git/objects/info/commit-graphs/commit-graph-chain");
        let before = run_main(&main, &base);
        std::fs::remove_file(&chain).expect("remove chain");
        std::fs::write(&chain, "not-a-graph-hash\n").expect("corrupt chain");

        let after = run_main(&main, &base);

        assert_eq!(before.state.ahead, Some(1));
        assert_eq!(after.state.error, None);
        assert_eq!((after.state.ahead, after.state.behind), (Some(1), Some(0)));
    }

    #[test]
    fn ahead_saturates_at_ten_thousand_and_the_walk_stops_there() {
        let (_t, base) = tempdir_real();
        let repo = init_empty(&base, "long");
        let commits = SATURATION as usize + 1;
        let mut stream = String::new();
        for i in 0..commits {
            let when = FIXED_COMMIT_SECONDS + i as i64;
            stream.push_str(&format!(
                "commit refs/heads/main\ncommitter Test <test@example.com> {when} +0000\ndata 2\nc\n"
            ));
            if i == 0 {
                stream.push_str("M 100644 inline f\ndata 2\nf\n");
            }
            stream.push('\n');
        }
        fast_import(&repo, &stream);
        git(&repo, &["reset", "-q", "--hard", "main"]);
        // An unrelated upstream tip: every one of the 10,001 commits is ahead, one is behind.
        let empty_tree = git_in(&repo, &["mktree"], "");
        let unrelated = git_in(&repo, &["commit-tree", &empty_tree, "-m", "elsewhere"], "");
        git(
            &repo,
            &["remote", "add", "origin", "/nonexistent/origin.git"],
        );
        git(
            &repo,
            &["update-ref", "refs/remotes/origin/main", &unrelated],
        );
        git(&repo, &["config", "branch.main.remote", "origin"]);
        git(&repo, &["config", "branch.main.merge", "refs/heads/main"]);
        assert_eq!(
            rev_list_count(
                &repo,
                &["refs/heads/main", "--not", "refs/remotes/origin/main"]
            ),
            SATURATION + 1,
            "git counts every commit; only rollcall saturates"
        );

        let run = run_main(&repo, &base);

        assert_eq!(run.state.error, None);
        assert_eq!(run.state.ahead, Some(SATURATION));
        assert_eq!(run.state.unpushed, Some(SATURATION));
        assert_eq!(run.state.behind, Some(1));
        assert!(
            run.commits_walked <= SATURATION as usize + 1,
            "walked {} commits",
            run.commits_walked
        );
    }

    // ---- missing worktrees ------------------------------------------------------------------

    #[test]
    fn missing_worktree_reads_only_the_branch_from_the_admin_head() {
        let (_t, base) = tempdir_real();
        let (_main, wt, admin) = repo_with_linked_feat(&base);
        std::fs::remove_dir_all(&wt).expect("remove the worktree");

        let state = missing_worktree_state(&admin, &wt);

        assert_eq!(
            state,
            WorktreeState {
                path: wt,
                is_main: false,
                branch: Some("feat".to_string()),
                detached: false,
                head: None,
                last_commit_at: None,
                upstream: None,
                ahead: None,
                behind: None,
                unpushed: None,
                dirty_count: 0,
                untracked_count: 0,
                missing: true,
                merged: false,
                error: None,
                locked: false,
            }
        );
    }

    #[test]
    fn missing_worktree_with_an_unusable_head_has_no_branch() {
        let (_t, base) = tempdir_real();
        let (_main, wt, admin) = repo_with_linked_feat(&base);
        let head = admin.join("HEAD");

        let branch_after = |content: &str| {
            std::fs::write(&head, content).expect("write HEAD");
            let state = missing_worktree_state(&admin, &wt);
            assert!(state.missing);
            assert_eq!(state.error, None);
            state.branch
        };

        assert_eq!(
            branch_after(&format!("ref: refs/heads/{}\n", "x".repeat(5_000))),
            None
        );
        assert_eq!(branch_after(&format!("{}\n", "a".repeat(40))), None);
        assert_eq!(branch_after(""), None);
        std::fs::remove_file(&head).expect("remove HEAD");
        assert_eq!(missing_worktree_state(&admin, &wt).branch, None);
    }

    /// S11-13: `locked` is the existence of `<admin>/locked`, whatever it is; never opened.
    #[test]
    fn missing_worktree_is_locked_iff_the_lock_entry_exists() {
        use std::os::unix::fs::symlink;

        let (_t, base) = tempdir_real();
        let (_main, wt, admin) = repo_with_linked_feat(&base);
        let lock = admin.join("locked");
        let locked_now = |admin: &Path, wt: &Path| {
            let (admin, wt) = (admin.to_path_buf(), wt.to_path_buf());
            // R3: a FIFO opened for reading would block forever.
            let state = within(move || missing_worktree_state(&admin, &wt));
            assert!(state.missing);
            state.locked
        };

        assert!(!locked_now(&admin, &wt), "absent");
        std::fs::write(&lock, "").expect("empty lock");
        assert!(locked_now(&admin, &wt), "empty file");
        std::fs::write(&lock, "on a usb stick\n").expect("lock with reason");
        assert!(locked_now(&admin, &wt), "file with a reason");
        std::fs::set_permissions(&lock, Permissions::from_mode(0o000)).expect("chmod 000");
        assert!(locked_now(&admin, &wt), "mode 000");
        std::fs::remove_file(&lock).expect("remove lock");
        symlink(base.join("no-such-target"), &lock).expect("dangling symlink");
        assert!(locked_now(&admin, &wt), "dangling symlink");
        std::fs::remove_file(&lock).expect("remove lock");
        mkfifo(&lock);
        assert!(locked_now(&admin, &wt), "fifo");
        std::fs::remove_file(&lock).expect("remove lock");
        assert!(!locked_now(&admin, &wt), "absent again");
    }

    // ---- revision 12: nested repositories, status call shape ------------------------------------

    const MIB: u64 = 1024 * 1024;
    const FOUR_GIB: u64 = 4 * 1024 * MIB;

    fn sparse_file(path: &Path, len: u64) {
        std::fs::File::create(path)
            .expect("create sparse file")
            .set_len(len)
            .expect("set_len");
    }

    fn backdate(path: &Path) {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open for mtime")
            .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000))
            .expect("set mtime");
    }

    /// Makes `big` the only tracked file gix must compare by content: every other file gets an old
    /// mtime recorded in the index (so none is racy-clean, which would be compared too), then
    /// `big`'s mtime changes so its stat data no longer matches (size and content do not).
    fn leave_only_this_file_for_content_comparison(repo: &Path, big: &str) {
        for name in git(repo, &["ls-files"]).lines().filter(|name| *name != big) {
            backdate(&repo.join(name));
        }
        git(repo, &["update-index", "-q", "--really-refresh"]);
        backdate(&repo.join(big));
    }

    #[test]
    fn nested_probe_counts_a_b_and_c_once_each_and_never_opens_their_heads() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        let huge = base.join("huge");
        sparse_file(&huge, FOUR_GIB);
        // a: `.git/HEAD` is a FIFO (opening it for reading would block).
        std::fs::create_dir_all(repo.join("a/.git")).expect("mkdir");
        mkfifo(&repo.join("a/.git/HEAD"));
        // b: `.git/HEAD` is a symlink to a sparse 4 GiB file (reading it would exhaust memory).
        std::fs::create_dir_all(repo.join("b/.git")).expect("mkdir");
        std::os::unix::fs::symlink(&huge, repo.join("b/.git/HEAD")).expect("symlink");
        // c: a `.git` file naming `../d`, whose `HEAD` is a FIFO.
        std::fs::create_dir_all(repo.join("c")).expect("mkdir");
        write(&repo, "c/.git", "gitdir: ../d\n");
        std::fs::create_dir_all(repo.join("d")).expect("mkdir");
        mkfifo(&repo.join("d/HEAD"));

        // No git oracle: real git blocks on these FIFOs.
        let run = run_main(&repo, &base);

        assert_eq!(run.state.error, None);
        assert_eq!(
            run.state.untracked_count, 3,
            "a, b and c once each; d holds no file"
        );
        assert_eq!(run.state.dirty_count, 0);
    }

    #[test]
    fn untracked_nested_repo_counts_once_however_many_files_it_holds() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        let inner = repo.join("inner");
        init_repo(&inner);
        write(&inner, "untracked-in-inner.txt", "u\n");
        write(&repo, "outer-untracked.txt", "o\n");

        let run = run_main(&repo, &base);

        assert_eq!(run.state.error, None);
        assert_eq!(
            run.state.untracked_count, 2,
            "`inner/` once, plus the outer file"
        );
        assert_eq!(run.state.dirty_count, 0);
        assert_agrees_with_git(&repo, &run.state, None);
    }

    #[test]
    fn submodule_worktree_files_are_never_counted() {
        let (_t, base) = tempdir_real();
        let source = base.join("source");
        init_repo(&source);
        let repo = base.join("super");
        init_repo(&repo);
        add_submodule(&repo, &source, "sub");
        git(&repo, &["commit", "-q", "-m", "add submodule"]);
        write(&repo, "sub/extra.txt", "untracked inside the submodule\n");
        write(&repo, "top.txt", "t\n");

        let run = run_main(&repo, &base);

        assert_eq!(run.state.error, None);
        assert_eq!(run.state.untracked_count, 1, "only top.txt");
        // Submodule status is `Ok(None)` by design, so git's `.M S..U` is not counted.
        assert_eq!(run.state.dirty_count, 0);
        assert_eq!(run.state.untracked_count, porcelain(&repo, None).untracked);
    }

    /// A repo with `core.autocrlf=true` and one committed 100755 file, `run.sh`.
    fn exec_repo(base: &Path) -> PathBuf {
        let repo = base.join("r");
        init_repo(&repo);
        git(&repo, &["config", "core.autocrlf", "true"]);
        write(&repo, "run.sh", "#!/bin/sh\n");
        std::fs::set_permissions(repo.join("run.sh"), Permissions::from_mode(0o755))
            .expect("chmod");
        git(&repo, &["add", "run.sh"]);
        git(&repo, &["commit", "-q", "-m", "exec"]);
        assert!(git(&repo, &["ls-files", "-s", "run.sh"]).starts_with("100755"));
        repo
    }

    #[test]
    fn executable_tracked_file_replaced_by_a_symlink_is_unsafe() {
        let (_t, base) = tempdir_real();
        let repo = exec_repo(&base);
        let huge = base.join("huge");
        sparse_file(&huge, FOUR_GIB);
        std::fs::remove_file(repo.join("run.sh")).expect("rm");
        std::os::unix::fs::symlink(&huge, repo.join("run.sh")).expect("symlink");

        let run = run_main(&repo, &base);

        assert_eq!(
            run.state.error.as_deref(),
            Some("git error: unsafe tracked file")
        );
        assert_eq!(run.state.dirty_count, 0);
    }

    #[test]
    fn executable_tracked_file_replaced_by_a_fifo_is_unsafe_and_never_opened() {
        let (_t, base) = tempdir_real();
        let repo = exec_repo(&base);
        std::fs::remove_file(repo.join("run.sh")).expect("rm");
        mkfifo(&repo.join("run.sh"));

        let run = run_main(&repo, &base);

        assert_eq!(
            run.state.error.as_deref(),
            Some("git error: unsafe tracked file")
        );
    }

    #[test]
    fn the_unsafe_file_check_fires_only_for_present_non_regular_100755_entries() {
        let (_t, base) = tempdir_real();
        let repo = exec_repo(&base);
        let untouched = run_main(&repo, &base);
        std::fs::remove_file(repo.join("run.sh")).expect("rm");
        let deleted = run_main(&repo, &base);
        // A 100644 file replaced by a FIFO is never opened by gix: a type change, no error.
        write(&repo, "plain.txt", "p\n");
        git(&repo, &["add", "plain.txt"]);
        git(&repo, &["commit", "-q", "-m", "plain"]);
        std::fs::remove_file(repo.join("plain.txt")).expect("rm");
        mkfifo(&repo.join("plain.txt"));
        let plain_fifo = run_main(&repo, &base);

        assert_eq!(
            (untouched.state.error, untouched.state.dirty_count),
            (None, 0)
        );
        assert_eq!((deleted.state.error, deleted.state.dirty_count), (None, 1));
        assert_eq!(plain_fifo.state.error, None);
        assert_eq!(
            plain_fifo.state.dirty_count, 2,
            "run.sh deleted, plain.txt type-changed"
        );
    }

    #[test]
    fn big_sparse_tracked_file_under_autocrlf_and_ident_is_dirty_without_a_read() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        git(&repo, &["config", "core.autocrlf", "true"]);
        write(&repo, ".gitattributes", "* ident\n");
        git(&repo, &["add", ".gitattributes"]);
        git(&repo, &["commit", "-q", "-m", "ident"]);
        // Committed at its full size, so the index stat size equals the worktree size and gix
        // would hash it (through gix-filter's whole-file read) once the mtime differs.
        sparse_file(&repo.join("big.bin"), 300 * MIB);
        git(&repo, &["add", "big.bin"]);
        git(&repo, &["commit", "-q", "-m", "big"]);
        leave_only_this_file_for_content_comparison(&repo, "big.bin");

        let run = run_main(&repo, &base);

        assert_eq!(run.state.error, None);
        assert_eq!(run.state.dirty_count, 1);
        assert_eq!(
            run.compare_deferrals, 0,
            "the file must not reach gix's comparison"
        );
    }

    #[test]
    fn compare_blobs_cap_is_exactly_64_mib() {
        let cases = [
            // (file size, dirty_count, times gix was asked to compare)
            (64 * MIB, 0, 1),
            (64 * MIB + 1, 1, 0),
        ];
        for (len, dirty, deferrals) in cases {
            let (_t, base) = tempdir_real();
            let repo = base.join("r");
            init_repo(&repo);
            sparse_file(&repo.join("blob.bin"), len);
            git(&repo, &["add", "blob.bin"]);
            git(&repo, &["commit", "-q", "-m", "blob"]);
            leave_only_this_file_for_content_comparison(&repo, "blob.bin");

            let run = run_main(&repo, &base);

            assert_eq!(run.state.error, None, "len {len}");
            assert_eq!(run.state.dirty_count, dirty, "len {len}");
            assert_eq!(run.compare_deferrals, deferrals, "len {len}");
        }
    }

    // ---- revision 12: pattern blobs named by the index --------------------------------------------

    const PATTERN_BLOB_CAP: usize = 256 * 1024;
    const PATTERNS_TOO_LARGE: &str = "git error: pattern files too large";

    /// Stores a comment-only blob of exactly `len` bytes and stages it at `path` (100644)
    /// without a worktree file.
    fn stage_pattern_blob(repo: &Path, path: &str, len: usize) {
        let mut content = "#".repeat(len - 1);
        content.push('\n');
        let id = git_in(repo, &["hash-object", "-w", "--stdin"], &content);
        let entry = format!("100644,{id},{path}");
        git(repo, &["update-index", "--add", "--cacheinfo", &entry]);
    }

    fn pattern_error(blobs: &[(&str, usize, bool)]) -> Option<String> {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        for (path, len, skip_worktree) in blobs {
            stage_pattern_blob(&repo, path, *len);
            if *skip_worktree {
                git(&repo, &["update-index", "--skip-worktree", path]);
            }
        }
        run_main(&repo, &base).state.error
    }

    #[test]
    fn a_subdirectory_gitattributes_blob_is_capped_at_256_kib() {
        assert_eq!(
            pattern_error(&[("sub/.gitattributes", PATTERN_BLOB_CAP, false)]),
            None
        );
        assert_eq!(
            pattern_error(&[("sub/.gitattributes", PATTERN_BLOB_CAP + 1, false)]).as_deref(),
            Some(PATTERNS_TOO_LARGE),
            "a 300 KiB-class blob"
        );
        assert_eq!(
            pattern_error(&[("sub/.gitattributes", 300 * 1024, false)]).as_deref(),
            Some(PATTERNS_TOO_LARGE)
        );
    }

    #[test]
    fn skip_worktree_gitignore_blobs_are_capped_at_1_mib_in_total() {
        let blobs = |n: usize, skip: bool| -> Vec<(String, usize, bool)> {
            (0..n)
                .map(|i| (format!("d{i}/.gitignore"), 200 * 1024, skip))
                .collect()
        };
        let error_for = |n: usize, skip: bool| {
            let owned = blobs(n, skip);
            let borrowed: Vec<(&str, usize, bool)> =
                owned.iter().map(|(p, l, s)| (p.as_str(), *l, *s)).collect();
            pattern_error(&borrowed)
        };

        assert_eq!(error_for(8, true).as_deref(), Some(PATTERNS_TOO_LARGE));
        assert_eq!(error_for(5, true), None, "5 x 200 KiB is under 1 MiB");
        assert_eq!(
            error_for(6, true).as_deref(),
            Some(PATTERNS_TOO_LARGE),
            "6 x 200 KiB is over"
        );
        // A `.gitignore` blob without SKIP_WORKTREE is never read from the index by gix.
        assert_eq!(error_for(8, false), None);
    }

    #[test]
    fn a_gitattributes_blob_counts_only_at_mode_100644() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        let huge = "#".repeat(PATTERN_BLOB_CAP + 1);
        let id = git_in(&repo, &["hash-object", "-w", "--stdin"], &huge);
        git(
            &repo,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("100755,{id},sub/.gitattributes"),
            ],
        );

        assert_eq!(run_main(&repo, &base).state.error, None);
    }

    // ---- revision 12: rename similarity is bounded ------------------------------------------------

    #[test]
    fn staged_rename_of_a_huge_blob_with_an_edit_is_a_deletion_plus_an_addition() {
        let (_t, base) = tempdir_real();
        let repo = base.join("r");
        init_repo(&repo);
        // 60 MiB of one-byte lines: the worst case for a line diff.
        std::fs::write(repo.join("old.txt"), vec![b'\n'; 60 * MIB as usize]).expect("write");
        git(&repo, &["add", "old.txt"]);
        git(&repo, &["commit", "-q", "-m", "old"]);
        git(&repo, &["mv", "old.txt", "new.txt"]);
        let mut edited = std::fs::read(repo.join("new.txt")).expect("read");
        edited[0] = b'x';
        std::fs::write(repo.join("new.txt"), edited).expect("write");
        git(&repo, &["add", "new.txt"]);

        let started = Instant::now();
        let run = run_main(&repo, &base);

        assert_eq!(run.state.error, None);
        assert_eq!(
            run.state.dirty_count, 2,
            "above core.bigFileThreshold nothing pairs"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "took {:?}",
            started.elapsed()
        );
    }
}
