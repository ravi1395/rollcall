//! §4.1–§4.2 classification: what a `.git` entry is, where it points, which repos
//! enclose or group each other. Nothing here opens a repo with gix.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fs::{self, FileType, Metadata, OpenOptions};
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use rollcall_core::{DiscoveredRepo, LinkedWorktree, RepoKind};

/// §4.1: a `.git` file larger than this is not a gitdir pointer.
pub const GIT_FILE_CAP: u64 = 4 * 1024;
/// §4.4 cap for the worktree-root `.gitmodules`.
pub const GITMODULES_CAP: u64 = 1024 * 1024;
/// A repo with more linked worktrees than this is listed with the first ones only.
pub const MAX_LINKED_WORKTREES: usize = 10_000;
/// §4.2 directory names skipped anywhere (config `skip_dirs` adds to these).
pub const SKIP_DIR_NAMES: [&str; 9] = [
    "node_modules",
    "target",
    ".venv",
    "venv",
    "dist",
    "build",
    ".cache",
    ".Trash",
    "__pycache__",
];

/// macOS `SF_DATALESS` (`sys/stat.h`): a cloud placeholder that must not be opened.
const SF_DATALESS: u32 = 0x4000_0000;

const BAD_GITDIR: &str = "bad gitdir: ";

pub fn is_dataless(flags: u32) -> bool {
    flags & SF_DATALESS != 0
}

#[cfg(target_os = "macos")]
pub fn metadata_is_dataless(md: &Metadata) -> bool {
    is_dataless(std::os::macos::fs::MetadataExt::st_flags(md))
}

#[cfg(not(target_os = "macos"))]
pub fn metadata_is_dataless(_md: &Metadata) -> bool {
    false
}

/// §4.2 path skips for `home`, by realpath.
pub fn path_skips(home: &Path) -> Vec<PathBuf> {
    let relative = if cfg!(target_os = "macos") {
        "Library"
    } else {
        ".local/share/Trash"
    };
    let home = fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let skip = home.join(relative);
    vec![fs::canonicalize(&skip).unwrap_or(skip)]
}

/// Reads a regular file of at most `cap` bytes, opened `O_NOFOLLOW | O_NONBLOCK` and
/// `fstat`-checked before any read, so a FIFO or symlink can neither block nor redirect us.
pub fn read_small_file(path: &Path, cap: u64) -> io::Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let too_big = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("larger than {cap} bytes"),
        )
    };
    let md = file.metadata()?;
    if !md.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    if md.len() > cap {
        return Err(too_big());
    }
    let mut buf = Vec::new();
    // One byte past the cap catches a file that grew after the fstat.
    file.take(cap + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > cap {
        return Err(too_big());
    }
    Ok(buf)
}

/// The path in a `gitdir: <path>` line (first line, surrounding whitespace trimmed).
pub fn parse_gitdir(content: &[u8]) -> Option<&[u8]> {
    let first_line = content.split(|&b| b == b'\n').next()?;
    let rest = first_line.strip_prefix(b"gitdir:")?.trim_ascii();
    (!rest.is_empty()).then_some(rest)
}

/// Where a `.git` file's `gitdir:` points, by the layout of the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitFile {
    /// `<main_git_dir>/worktrees/<name>`.
    Linked {
        main_git_dir: PathBuf,
        admin_dir: PathBuf,
    },
    /// `<parent_git_dir>/modules/<name>`.
    Submodule {
        parent_git_dir: PathBuf,
        git_dir: PathBuf,
    },
    /// Anything else (e.g. `git init --separate-git-dir`).
    Other { git_dir: PathBuf },
}

/// Reads and resolves a `.git` file. `Err` is a full `bad gitdir: ` message.
pub fn resolve_git_file(file: &Path) -> Result<GitFile, String> {
    let bad = |why: String| format!("{BAD_GITDIR}{why}");
    let content =
        read_small_file(file, GIT_FILE_CAP).map_err(|e| bad(format!("{}: {e}", file.display())))?;
    let raw = parse_gitdir(&content)
        .ok_or_else(|| bad(format!("{}: no `gitdir:` line", file.display())))?;
    let pointer = Path::new(OsStr::from_bytes(raw));
    let joined = match file.parent() {
        Some(dir) if pointer.is_relative() => dir.join(pointer),
        _ => pointer.to_path_buf(),
    };
    let target =
        fs::canonicalize(&joined).map_err(|e| bad(format!("{}: {e}", joined.display())))?;
    if !target.is_dir() {
        return Err(bad(format!("{} is not a directory", target.display())));
    }
    Ok(classify_target(target))
}

fn classify_target(target: PathBuf) -> GitFile {
    let names: Vec<&OsStr> = target
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    if names.len() >= 2
        && names[names.len() - 2] == "worktrees"
        // Every real admin dir has a `gitdir` file; `~/worktrees/x` as a bare gitdir does not.
        && fs::symlink_metadata(target.join("gitdir")).is_ok_and(|md| md.is_file())
        && let Some(main_git_dir) = target.parent().and_then(Path::parent)
    {
        return GitFile::Linked {
            main_git_dir: main_git_dir.to_path_buf(),
            admin_dir: target,
        };
    }
    // `.git/modules/<name>`, and `<that>/modules/<name>` for a submodule of a submodule.
    let in_dot_git_modules = names
        .windows(2)
        .any(|w| w[0] == ".git" && w[1] == "modules");
    if in_dot_git_modules
        && let Some(at) = names.iter().rposition(|n| *n == "modules")
        && at + 1 < names.len()
    {
        let keep = target.components().count() - (names.len() - at);
        let parent_git_dir: PathBuf = target.components().take(keep).collect();
        return GitFile::Submodule {
            parent_git_dir,
            git_dir: target,
        };
    }
    GitFile::Other { git_dir: target }
}

/// What `<workdir>/.git` turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DotGit {
    Dir {
        git_dir: PathBuf,
    },
    File(GitFile),
    /// Unusable; the message is a full `bad gitdir: ` error string.
    Bad(String),
    /// A symlink that policy leaves alone; the message is a `doctor` note.
    Ignored(String),
}

/// Classifies `<workdir>/.git` with `lstat` only. A FIFO or socket is never opened.
/// A symlink counts only with `follow_symlinks` and a realpath under `root`.
pub fn classify_dot_git(workdir: &Path, follow_symlinks: bool, root: &Path) -> DotGit {
    let dot_git = workdir.join(".git");
    let md = match fs::symlink_metadata(&dot_git) {
        Ok(md) => md,
        Err(e) => return DotGit::Bad(format!("{BAD_GITDIR}{}: {e}", dot_git.display())),
    };
    let (path, md) = if md.file_type().is_symlink() {
        let ignored =
            |why: &str| DotGit::Ignored(format!("{}: .git is a symlink, {why}", workdir.display()));
        if !follow_symlinks {
            return ignored("not followed");
        }
        let target = fs::canonicalize(&dot_git)
            .ok()
            .filter(|target| target.starts_with(root));
        let Some((target, md)) =
            target.and_then(|t| fs::symlink_metadata(&t).ok().map(|md| (t, md)))
        else {
            return ignored("target is not under the root");
        };
        (target, md)
    } else {
        (dot_git, md)
    };
    if md.is_dir() {
        return DotGit::Dir { git_dir: path };
    }
    if !md.is_file() {
        return DotGit::Bad(format!(
            "{BAD_GITDIR}{} is not a regular file",
            path.display()
        ));
    }
    match resolve_git_file(&path) {
        Ok(file) => DotGit::File(file),
        Err(message) => DotGit::Bad(message),
    }
}

/// Names seen while listing one directory, enough to tell a bare repo (§4.1).
#[derive(Debug, Default)]
pub struct DirShape {
    head_file: bool,
    objects_dir: bool,
    refs_dir: bool,
}

impl DirShape {
    pub fn observe(&mut self, name: &str, file_type: FileType) {
        match name {
            "HEAD" => self.head_file |= file_type.is_file(),
            "objects" => self.objects_dir |= file_type.is_dir(),
            "refs" => self.refs_dir |= file_type.is_dir(),
            _ => {}
        }
    }

    pub fn is_bare(&self) -> bool {
        self.head_file && self.objects_dir && self.refs_dir
    }
}

/// One thing the walk (or the enclosing-repo search) found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    DotGit {
        workdir: PathBuf,
        result: DotGit,
        enclosing: bool,
    },
    Bare(PathBuf),
}

/// §4.1 enclosing repo: nearest `.git` above `root`, by `lstat`, never examining `home` or `/`.
pub fn enclosing_repo(root: &Path, home: &Path) -> Option<Found> {
    for dir in root.ancestors().skip(1) {
        if dir == home || dir.parent().is_none() {
            return None;
        }
        if fs::symlink_metadata(dir.join(".git")).is_ok() {
            return Some(Found::DotGit {
                workdir: dir.to_path_buf(),
                result: classify_dot_git(dir, false, root),
                enclosing: true,
            });
        }
    }
    None
}

/// `<git_dir>/worktrees/*`, each with the directory its `gitdir` file names.
pub fn linked_worktrees(git_dir: &Path, notes: &mut Vec<String>) -> Vec<LinkedWorktree> {
    let worktrees = git_dir.join("worktrees");
    // lstat: a symlinked `worktrees` is never followed.
    if !fs::symlink_metadata(&worktrees).is_ok_and(|md| md.is_dir()) {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(&worktrees) else {
        return Vec::new();
    };
    let mut admin_dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    admin_dirs.sort();
    if admin_dirs.len() > MAX_LINKED_WORKTREES {
        notes.push(format!(
            "{}: more than {MAX_LINKED_WORKTREES} linked worktrees, listing the first {MAX_LINKED_WORKTREES}",
            git_dir.display()
        ));
        admin_dirs.truncate(MAX_LINKED_WORKTREES);
    }
    admin_dirs.into_iter().map(linked_worktree).collect()
}

fn linked_worktree(admin_dir: PathBuf) -> LinkedWorktree {
    match worktree_dir(&admin_dir) {
        Ok(path) => {
            let missing = fs::symlink_metadata(&path).is_err();
            LinkedWorktree {
                path,
                admin_dir,
                missing,
                error: None,
            }
        }
        Err(why) => LinkedWorktree {
            path: admin_dir.clone(),
            admin_dir,
            missing: false,
            error: Some(format!("{BAD_GITDIR}{why}")),
        },
    }
}

/// The worktree directory named by `<admin_dir>/gitdir` (which holds the path of its `.git` file).
/// The whole file is the path minus git's one trailing newline: a path may itself hold newlines.
fn worktree_dir(admin_dir: &Path) -> Result<PathBuf, String> {
    let bytes =
        read_small_file(&admin_dir.join("gitdir"), GIT_FILE_CAP).map_err(|e| e.to_string())?;
    let line = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
    if line.is_empty() {
        return Err("empty gitdir file".to_string());
    }
    Path::new(OsStr::from_bytes(line))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "gitdir has no parent directory".to_string())
}

/// A repo row under construction.
struct Candidate {
    repo: DiscoveredRepo,
    /// Modern submodule: the git dir of the superproject.
    submodule_parent_git_dir: Option<PathBuf>,
    /// A plain `.git` directory: a submodule if the enclosing repo's `.gitmodules` lists it.
    may_be_old_submodule: bool,
    enclosing: bool,
    is_submodule: bool,
}

impl Candidate {
    fn new(git_dir: PathBuf, workdir: Option<PathBuf>, kind: RepoKind) -> Self {
        Candidate {
            repo: DiscoveredRepo {
                git_dir,
                workdir,
                kind,
                contains_root: false,
                submodule_of: None,
                linked: Vec::new(),
                error: None,
            },
            submodule_parent_git_dir: None,
            may_be_old_submodule: false,
            enclosing: false,
            is_submodule: false,
        }
    }

    fn sort_key(&self) -> (&Path, &Path) {
        let repo = &self.repo;
        (
            repo.workdir.as_deref().unwrap_or(&repo.git_dir),
            &repo.git_dir,
        )
    }
}

/// Turns raw findings into rows: dedupes, groups linked worktrees and submodules under
/// their repo, adds `external_main` rows, marks nested repos. Sorted by path.
pub fn assemble(found: Vec<Found>, submodules: bool) -> (Vec<DiscoveredRepo>, Vec<String>) {
    let mut notes = Vec::new();
    let mut candidates: Vec<Candidate> = Vec::new();
    // Main git dir of a linked worktree → whether that worktree encloses the root.
    let mut mains_of_linked: BTreeMap<PathBuf, bool> = BTreeMap::new();

    for item in found {
        match item {
            Found::Bare(dir) => candidates.push(Candidate::new(dir, None, RepoKind::Bare)),
            Found::DotGit {
                workdir,
                result,
                enclosing,
            } => {
                let mut candidate = match result {
                    DotGit::Ignored(note) => {
                        notes.push(note);
                        continue;
                    }
                    DotGit::Bad(message) => {
                        let git_dir = workdir.join(".git");
                        let mut c = Candidate::new(git_dir, Some(workdir), RepoKind::Normal);
                        c.repo.error = Some(message);
                        c
                    }
                    DotGit::Dir { git_dir } => {
                        let mut c = Candidate::new(git_dir, Some(workdir), RepoKind::Normal);
                        c.may_be_old_submodule = true;
                        c
                    }
                    DotGit::File(GitFile::Linked { main_git_dir, .. }) => {
                        *mains_of_linked.entry(main_git_dir).or_default() |= enclosing;
                        continue;
                    }
                    DotGit::File(GitFile::Submodule {
                        parent_git_dir,
                        git_dir,
                    }) => {
                        let mut c = Candidate::new(git_dir, Some(workdir), RepoKind::Normal);
                        c.submodule_parent_git_dir = Some(parent_git_dir);
                        c.is_submodule = true;
                        c
                    }
                    DotGit::File(GitFile::Other { git_dir }) => {
                        notes.push(format!(
                            "{}: unusual layout, .git points at {}",
                            workdir.display(),
                            git_dir.display()
                        ));
                        Candidate::new(git_dir, Some(workdir), RepoKind::Normal)
                    }
                };
                candidate.repo.contains_root = enclosing;
                candidate.enclosing = enclosing;
                candidates.push(candidate);
            }
        }
    }

    // A linked worktree's main repo that the walk did not find is `external_main`.
    for (main_git_dir, encloses_root) in mains_of_linked {
        match candidates
            .iter_mut()
            .find(|c| c.repo.git_dir == main_git_dir)
        {
            Some(existing) => existing.repo.contains_root |= encloses_root,
            None => {
                let workdir = if main_git_dir.file_name() == Some(OsStr::new(".git")) {
                    main_git_dir.parent().map(Path::to_path_buf)
                } else {
                    None
                };
                let mut c = Candidate::new(main_git_dir, workdir, RepoKind::ExternalMain);
                c.repo.contains_root = encloses_root;
                c.enclosing = encloses_root;
                candidates.push(c);
            }
        }
    }

    candidates.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    candidates.dedup_by(|dup, keep| keep.sort_key() == dup.sort_key());

    group_and_nest(&mut candidates, &mut notes);

    let keep: Vec<bool> = candidates
        .iter()
        .map(|c| submodules || !c.is_submodule)
        .collect();
    let mut new_index = vec![None; candidates.len()];
    let mut next = 0;
    for (index, &kept) in keep.iter().enumerate() {
        if kept {
            new_index[index] = Some(next);
            next += 1;
        }
    }
    let mut repos = Vec::with_capacity(next);
    for (index, candidate) in candidates.into_iter().enumerate() {
        if !keep[index] {
            continue;
        }
        let mut repo = candidate.repo;
        repo.submodule_of = repo.submodule_of.and_then(|parent| new_index[parent]);
        if repo.error.is_none() {
            repo.linked = linked_worktrees(&repo.git_dir, &mut notes);
        }
        repos.push(repo);
    }
    (repos, notes)
}

/// Sets `submodule_of` and marks nested repos, over candidates sorted by path.
fn group_and_nest(candidates: &mut [Candidate], notes: &mut Vec<String>) {
    let by_git_dir: HashMap<PathBuf, usize> = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| (c.repo.git_dir.clone(), i))
        .collect();
    // Only walked working trees can parent a nested repo or an old-style submodule; the
    // enclosing repo and `external_main` rows are not walked into.
    let by_workdir: HashMap<PathBuf, usize> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.enclosing && c.repo.kind != RepoKind::ExternalMain)
        .filter_map(|(i, c)| Some((c.repo.workdir.clone()?, i)))
        .collect();
    let mut gitmodules: HashMap<usize, Vec<PathBuf>> = HashMap::new();

    for i in 0..candidates.len() {
        let nearest_parent = candidates[i].repo.workdir.as_deref().and_then(|w| {
            w.ancestors()
                .skip(1)
                .find_map(|a| by_workdir.get(a).copied())
        });
        let modern_parent = candidates[i]
            .submodule_parent_git_dir
            .as_ref()
            .and_then(|g| by_git_dir.get(g).copied());
        let old_style_parent = nearest_parent.filter(|&parent| {
            candidates[i].may_be_old_submodule
                && lists_submodule(&mut gitmodules, candidates, parent, i)
        });

        let candidate = &mut candidates[i];
        if candidate.is_submodule && modern_parent.is_none() {
            let at = candidate
                .repo
                .workdir
                .as_deref()
                .unwrap_or(&candidate.repo.git_dir);
            notes.push(format!(
                "{}: submodule of a repo outside the scan, listed on its own",
                at.display()
            ));
        }
        candidate.repo.submodule_of = modern_parent.or(old_style_parent);
        // Without a parent in the scan the row stays listed on its own, submodules or not.
        candidate.is_submodule = candidate.repo.submodule_of.is_some();
        if !candidate.is_submodule
            && candidate.repo.kind == RepoKind::Normal
            && candidate.repo.error.is_none()
            && nearest_parent.is_some()
        {
            candidate.repo.kind = RepoKind::Nested;
        }
    }
}

/// Whether candidate `parent`'s `.gitmodules` lists candidate `child`'s working tree.
fn lists_submodule(
    cache: &mut HashMap<usize, Vec<PathBuf>>,
    candidates: &[Candidate],
    parent: usize,
    child: usize,
) -> bool {
    let (Some(parent_dir), Some(child_dir)) = (
        candidates[parent].repo.workdir.as_deref(),
        candidates[child].repo.workdir.as_deref(),
    ) else {
        return false;
    };
    let Ok(relative) = child_dir.strip_prefix(parent_dir) else {
        return false;
    };
    let listed = cache
        .entry(parent)
        .or_insert_with(|| read_gitmodules_paths(&parent_dir.join(".gitmodules")));
    listed
        .iter()
        .any(|path| path.components().eq(relative.components()))
}

/// The `path = …` values of a `.gitmodules`; an unreadable or oversized file lists nothing.
fn read_gitmodules_paths(file: &Path) -> Vec<PathBuf> {
    let Ok(bytes) = read_small_file(file, GITMODULES_CAP) else {
        return Vec::new();
    };
    bytes
        .split(|&b| b == b'\n')
        .filter_map(|line| {
            let eq = line.iter().position(|&b| b == b'=')?;
            if line[..eq].trim_ascii() != b"path" {
                return None;
            }
            let value = line[eq + 1..].trim_ascii();
            let value = value
                .strip_prefix(b"\"")
                .and_then(|v| v.strip_suffix(b"\""))
                .unwrap_or(value);
            Some(PathBuf::from(OsStr::from_bytes(value)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;
    use crate::testgit::{add_worktree, git, init_repo, mkfifo, tempdir_real, within};

    #[test]
    fn parse_gitdir_takes_the_first_line_trimmed() {
        assert_eq!(parse_gitdir(b"gitdir: ../x\n"), Some(&b"../x"[..]));
        assert_eq!(parse_gitdir(b"gitdir:/a b\r\nextra"), Some(&b"/a b"[..]));
    }

    #[test]
    fn parse_gitdir_rejects_other_content() {
        assert_eq!(parse_gitdir(b"ref: refs/heads/main\n"), None);
        assert_eq!(parse_gitdir(b"gitdir: \n"), None);
        assert_eq!(parse_gitdir(b""), None);
    }

    #[test]
    fn is_dataless_tests_only_the_sf_dataless_bit() {
        assert!(is_dataless(0x4000_0000));
        assert!(is_dataless(0x4000_0000 | 0x20));
        assert!(!is_dataless(0));
        assert!(!is_dataless(0x0000_4000));
    }

    #[test]
    fn read_small_file_reads_up_to_the_cap_and_no_further() {
        let (_t, dir) = tempdir_real();
        let file = dir.join("f");
        std::fs::write(&file, [b'a'; 8]).unwrap();
        assert_eq!(read_small_file(&file, 8).unwrap().len(), 8);
        assert!(read_small_file(&file, 7).is_err());
    }

    #[test]
    fn read_small_file_refuses_a_symlink() {
        let (_t, dir) = tempdir_real();
        std::fs::write(dir.join("real"), "x").unwrap();
        symlink(dir.join("real"), dir.join("link")).unwrap();
        assert!(read_small_file(&dir.join("link"), 16).is_err());
    }

    #[test]
    fn read_small_file_refuses_a_fifo_without_blocking() {
        let (_t, dir) = tempdir_real();
        let fifo = dir.join("pipe");
        mkfifo(&fifo);
        let result = within(move || read_small_file(&fifo, 16));
        assert!(result.is_err());
    }

    #[test]
    fn resolve_git_file_recognises_a_linked_worktree() {
        let (_t, dir) = tempdir_real();
        init_repo(&dir.join("main"));
        add_worktree(&dir.join("main"), &dir.join("wt"), "topic");
        let resolved = resolve_git_file(&dir.join("wt/.git")).unwrap();
        let GitFile::Linked {
            main_git_dir,
            admin_dir,
        } = resolved
        else {
            panic!("expected Linked, got {resolved:?}");
        };
        assert_eq!(main_git_dir, dir.join("main/.git"));
        assert_eq!(admin_dir, dir.join("main/.git/worktrees/wt"));
    }

    #[test]
    fn a_worktrees_directory_without_a_gitdir_file_is_just_another_layout() {
        let (_t, dir) = tempdir_real();
        std::fs::create_dir_all(dir.join("worktrees/x")).unwrap();
        std::fs::create_dir_all(dir.join("proj")).unwrap();
        let target = dir.join("worktrees/x");
        std::fs::write(
            dir.join("proj/.git"),
            format!("gitdir: {}\n", target.display()),
        )
        .unwrap();
        assert_eq!(
            resolve_git_file(&dir.join("proj/.git")).unwrap(),
            GitFile::Other { git_dir: target }
        );
    }

    #[test]
    fn resolve_git_file_recognises_a_submodule_layout() {
        let (_t, dir) = tempdir_real();
        std::fs::create_dir_all(dir.join("sup/.git/modules/a")).unwrap();
        std::fs::create_dir_all(dir.join("sup/a")).unwrap();
        std::fs::write(dir.join("sup/a/.git"), "gitdir: ../.git/modules/a\n").unwrap();
        assert_eq!(
            resolve_git_file(&dir.join("sup/a/.git")).unwrap(),
            GitFile::Submodule {
                parent_git_dir: dir.join("sup/.git"),
                git_dir: dir.join("sup/.git/modules/a"),
            }
        );
    }

    #[test]
    fn a_modules_directory_outside_dot_git_is_just_another_layout() {
        let (_t, dir) = tempdir_real();
        std::fs::create_dir_all(dir.join("modules/proj.git")).unwrap();
        std::fs::create_dir_all(dir.join("proj")).unwrap();
        let target = dir.join("modules/proj.git");
        std::fs::write(
            dir.join("proj/.git"),
            format!("gitdir: {}\n", target.display()),
        )
        .unwrap();
        assert_eq!(
            resolve_git_file(&dir.join("proj/.git")).unwrap(),
            GitFile::Other { git_dir: target }
        );
    }

    #[test]
    fn resolve_git_file_reports_a_missing_target_as_bad_gitdir() {
        let (_t, dir) = tempdir_real();
        std::fs::write(dir.join(".git"), "gitdir: nowhere\n").unwrap();
        let err = resolve_git_file(&dir.join(".git")).unwrap_err();
        assert!(err.starts_with("bad gitdir: "), "{err}");
    }

    #[test]
    fn resolve_git_file_rejects_an_oversized_file() {
        let (_t, dir) = tempdir_real();
        let mut body = b"gitdir: ".to_vec();
        body.resize(GIT_FILE_CAP as usize + 1, b'a');
        std::fs::write(dir.join(".git"), body).unwrap();
        let err = resolve_git_file(&dir.join(".git")).unwrap_err();
        assert!(err.starts_with("bad gitdir: "), "{err}");
    }

    #[test]
    fn classify_dot_git_never_opens_a_fifo() {
        let (_t, dir) = tempdir_real();
        mkfifo(&dir.join(".git"));
        let work = dir.clone();
        let result = within(move || classify_dot_git(&work, false, &work));
        assert!(matches!(result, DotGit::Bad(m) if m.starts_with("bad gitdir: ")));
    }

    #[test]
    fn classify_dot_git_leaves_a_symlink_alone_unless_following_within_root() {
        let (_t, dir) = tempdir_real();
        let real = dir.join("real/.git");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(dir.join("proj")).unwrap();
        symlink(&real, dir.join("proj/.git")).unwrap();
        let proj = dir.join("proj");
        assert!(matches!(
            classify_dot_git(&proj, false, &dir),
            DotGit::Ignored(_)
        ));
        assert_eq!(
            classify_dot_git(&proj, true, &dir),
            DotGit::Dir { git_dir: real }
        );
        assert!(matches!(
            classify_dot_git(&proj, true, &dir.join("elsewhere")),
            DotGit::Ignored(_)
        ));
    }

    #[test]
    fn dir_shape_is_bare_only_with_head_file_objects_and_refs() {
        let (_t, dir) = tempdir_real();
        std::fs::write(dir.join("HEAD"), "").unwrap();
        std::fs::create_dir(dir.join("objects")).unwrap();
        std::fs::create_dir(dir.join("refs")).unwrap();
        let shape_of = |names: &[&str]| {
            let mut shape = DirShape::default();
            for name in names {
                let ft = std::fs::symlink_metadata(dir.join(name))
                    .unwrap()
                    .file_type();
                shape.observe(name, ft);
            }
            shape.is_bare()
        };
        assert!(shape_of(&["HEAD", "objects", "refs"]));
        assert!(!shape_of(&["HEAD", "objects"]));
        assert!(!shape_of(&["objects", "refs"]));
    }

    #[test]
    fn dir_shape_ignores_a_file_named_objects() {
        let (_t, dir) = tempdir_real();
        std::fs::write(dir.join("HEAD"), "").unwrap();
        std::fs::write(dir.join("objects"), "").unwrap();
        std::fs::create_dir(dir.join("refs")).unwrap();
        let mut shape = DirShape::default();
        for name in ["HEAD", "objects", "refs"] {
            let ft = std::fs::symlink_metadata(dir.join(name))
                .unwrap()
                .file_type();
            shape.observe(name, ft);
        }
        assert!(!shape.is_bare());
    }

    #[test]
    fn path_skips_uses_the_home_realpath() {
        let (_t, dir) = tempdir_real();
        let skips = path_skips(&dir);
        assert_eq!(skips.len(), 1);
        assert!(skips[0].starts_with(&dir));
        #[cfg(target_os = "macos")]
        assert_eq!(skips[0], dir.join("Library"));
        #[cfg(not(target_os = "macos"))]
        assert_eq!(skips[0], dir.join(".local/share/Trash"));
    }

    #[test]
    fn linked_worktrees_lists_each_and_flags_a_missing_one() {
        let (_t, dir) = tempdir_real();
        let main = dir.join("main");
        init_repo(&main);
        add_worktree(&main, &dir.join("wt-a"), "a");
        add_worktree(&main, &dir.join("wt-b"), "b");
        std::fs::remove_dir_all(dir.join("wt-b")).unwrap();
        let mut notes = Vec::new();
        let linked = linked_worktrees(&main.join(".git"), &mut notes);
        assert_eq!(linked.len(), 2, "{linked:?}");
        let a = linked.iter().find(|w| w.path == dir.join("wt-a")).unwrap();
        let b = linked.iter().find(|w| w.path == dir.join("wt-b")).unwrap();
        assert!(!a.missing && a.error.is_none());
        assert!(b.missing);
        assert_eq!(a.admin_dir, main.join(".git/worktrees/wt-a"));
    }

    #[test]
    fn linked_worktrees_keeps_a_newline_in_the_worktree_path() {
        let (_t, dir) = tempdir_real();
        let main = dir.join("main");
        init_repo(&main);
        let wt = dir.join("wt\nnl");
        add_worktree(&main, &wt, "nl");
        let mut notes = Vec::new();
        let linked = linked_worktrees(&main.join(".git"), &mut notes);
        assert_eq!(linked.len(), 1, "{linked:?}");
        assert_eq!(linked[0].path, wt);
        assert!(
            !linked[0].missing && linked[0].error.is_none(),
            "{linked:?}"
        );
    }

    #[test]
    fn enclosing_repo_stops_before_home() {
        let (_t, dir) = tempdir_real();
        let home = dir.join("home");
        git(
            &{
                std::fs::create_dir_all(&home).unwrap();
                home.clone()
            },
            &["init", "-q"],
        );
        let root = home.join("proj");
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(enclosing_repo(&root, &home), None);
        // Without the home guard the same layout would find `home/.git`.
        assert!(enclosing_repo(&root, &dir.join("elsewhere")).is_some());
    }
}
