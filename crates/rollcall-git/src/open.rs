//! §4.4 pre-open checks and the user's `core.excludesFile`.
//!
//! gix's own size caps run after `File::open`, which blocks on a FIFO and follows symlinks, so
//! every file gix may read is `lstat`-checked here first (regular file, within its cap).

use std::collections::HashSet;
use std::fs::{self, Metadata};
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use gix::bstr::ByteSlice;

use crate::classify::{MAX_LINKED_WORKTREES, read_small_file};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const SMALL_FILE_CAP: u64 = MIB;
const REF_CAP: u64 = 4 * KIB;
const PACKED_REFS_CAP: u64 = 64 * MIB;
const INDEX_CAP: u64 = 256 * MIB;
/// `refs/` entries (and all-caps pseudo-ref entries) examined per call before giving up.
const MAX_REF_ENTRIES: usize = 10_000;
const MAX_ALTERNATE_HOPS: usize = 8;

const UNTRUSTED_GIT_DIR: &str = "untrusted git dir";
const UNTRUSTED_WORKTREE_PATH: &str = "untrusted worktree path";

/// Files gix may read from a git dir, with their caps (§4.4).
const GIT_DIR_FILES: [(&str, u64); 12] = [
    ("config", SMALL_FILE_CAP),
    ("config.worktree", SMALL_FILE_CAP),
    ("HEAD", REF_CAP),
    ("packed-refs", PACKED_REFS_CAP),
    ("index", INDEX_CAP),
    ("commondir", SMALL_FILE_CAP),
    ("gitdir", SMALL_FILE_CAP),
    ("shallow", SMALL_FILE_CAP),
    ("info/exclude", SMALL_FILE_CAP),
    ("info/attributes", SMALL_FILE_CAP),
    ("objects/info/alternates", SMALL_FILE_CAP),
    (
        "objects/info/commit-graphs/commit-graph-chain",
        SMALL_FILE_CAP,
    ),
];

/// Worktree-root files gix reads during status.
const WORKDIR_FILES: [&str; 3] = [".gitmodules", ".gitignore", ".gitattributes"];

fn untrusted(path: &Path, why: &str) -> String {
    format!("{UNTRUSTED_GIT_DIR}: {}: {why}", path.display())
}

fn is_absent(kind: ErrorKind) -> bool {
    matches!(kind, ErrorKind::NotFound | ErrorKind::NotADirectory)
}

/// Why `md` is not an acceptable file: not a regular file (symlinks included), or over `cap`.
fn metadata_problem(md: &Metadata, cap: u64) -> Option<String> {
    if !md.is_file() {
        return Some("not a regular file".to_string());
    }
    (md.len() > cap).then(|| format!("larger than {cap} bytes"))
}

/// `lstat` check; a missing file (or a missing parent directory) is fine.
fn file_problem(path: &Path, cap: u64) -> Option<String> {
    match fs::symlink_metadata(path) {
        Ok(md) => metadata_problem(&md, cap),
        Err(e) if is_absent(e.kind()) => None,
        Err(e) => Some(e.to_string()),
    }
}

fn check_file(path: &Path, cap: u64) -> Result<(), String> {
    file_problem(path, cap).map_or(Ok(()), |why| Err(untrusted(path, &why)))
}

/// `read_dir` that treats a missing directory as empty.
fn read_dir_opt(dir: &Path) -> Result<Option<fs::ReadDir>, String> {
    match fs::read_dir(dir) {
        Ok(entries) => Ok(Some(entries)),
        Err(e) if is_absent(e.kind()) => Ok(None),
        Err(e) => Err(untrusted(dir, &e.to_string())),
    }
}

fn count_ref_entry(seen: &mut usize) -> Result<(), String> {
    *seen += 1;
    if *seen > MAX_REF_ENTRIES {
        return Err(format!(
            "{UNTRUSTED_GIT_DIR}: more than {MAX_REF_ENTRIES} ref entries"
        ));
    }
    Ok(())
}

/// Every entry under `refs`: directories must be real directories, files regular and ≤ 4 KiB.
fn check_refs_tree(refs: &Path, seen: &mut usize) -> Result<(), String> {
    match fs::symlink_metadata(refs) {
        Ok(md) if md.is_dir() => {}
        Ok(_) => return Err(untrusted(refs, "not a directory")),
        Err(e) if is_absent(e.kind()) => return Ok(()),
        Err(e) => return Err(untrusted(refs, &e.to_string())),
    }
    let mut stack = vec![refs.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| untrusted(&dir, &e.to_string()))?;
        for entry in entries {
            let entry = entry.map_err(|e| untrusted(&dir, &e.to_string()))?;
            count_ref_entry(seen)?;
            let path = entry.path();
            // `DirEntry::metadata` does not traverse symlinks.
            let md = entry
                .metadata()
                .map_err(|e| untrusted(&path, &e.to_string()))?;
            if md.is_dir() {
                stack.push(path);
            } else if let Some(why) = metadata_problem(&md, REF_CAP) {
                return Err(untrusted(&path, &why));
            }
        }
    }
    Ok(())
}

/// Entries of `dir` named only `A`–`Z` / `_` (`HEAD`, `ORIG_HEAD`, …): gix reads these through
/// symbolic refs (`HEAD` holding `ref: EVIL`), so each must be a regular file: `HEAD` ≤ 4 KiB,
/// the rest ≤ 1 MiB.
fn check_pseudo_refs(dir: &Path, seen: &mut usize) -> Result<(), String> {
    let Some(entries) = read_dir_opt(dir)? else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry.map_err(|e| untrusted(dir, &e.to_string()))?;
        let name = entry.file_name();
        let bytes = name.as_bytes();
        if bytes.is_empty() || !bytes.iter().all(|b| b.is_ascii_uppercase() || *b == b'_') {
            continue;
        }
        count_ref_entry(seen)?;
        let path = entry.path();
        let md = entry
            .metadata()
            .map_err(|e| untrusted(&path, &e.to_string()))?;
        // Only `HEAD` is a real ref; `FETCH_HEAD`, `MERGE_MSG`, … legitimately outgrow 4 KiB.
        let cap = if bytes == b"HEAD" {
            REF_CAP
        } else {
            SMALL_FILE_CAP
        };
        if let Some(why) = metadata_problem(&md, cap) {
            return Err(untrusted(&path, &why));
        }
    }
    Ok(())
}

/// The per-admin-dir subset (`config.worktree`, pseudo-refs, `refs/**`) for each
/// `<dir>/worktrees/<name>/`. The full list runs when an admin dir is itself the git dir.
fn check_admin_dirs(dir: &Path, seen: &mut usize) -> Result<(), String> {
    let worktrees = dir.join("worktrees");
    // lstat: a symlinked `worktrees` is never enumerated (see `classify::linked_worktrees`).
    if !fs::symlink_metadata(&worktrees).is_ok_and(|md| md.is_dir()) {
        return Ok(());
    }
    let Some(entries) = read_dir_opt(&worktrees)? else {
        return Ok(());
    };
    let mut admin_dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    admin_dirs.sort();
    admin_dirs.truncate(MAX_LINKED_WORKTREES);
    for admin in admin_dirs {
        check_file(&admin.join("config.worktree"), SMALL_FILE_CAP)?;
        check_pseudo_refs(&admin, seen)?;
        check_refs_tree(&admin.join("refs"), seen)?;
    }
    Ok(())
}

/// The full §4.4 file list plus refs and admin dirs, for one git dir or common dir.
fn check_git_dir(dir: &Path, seen: &mut usize) -> Result<(), String> {
    for (relative, cap) in GIT_DIR_FILES {
        check_file(&dir.join(relative), cap)?;
    }
    check_pseudo_refs(dir, seen)?;
    check_refs_tree(&dir.join("refs"), seen)?;
    check_admin_dirs(dir, seen)
}

/// Realpaths of `(git_dir, common_dir)`, with the common dir taken from rollcall's own read of
/// `commondir` and compared to `expected_common` (trust rule (b), before gix sees anything).
fn resolve_common(
    git_dir: &Path,
    expected_common: Option<&Path>,
) -> Result<(PathBuf, PathBuf), String> {
    let real_git = fs::canonicalize(git_dir).map_err(|e| untrusted(git_dir, &e.to_string()))?;
    let commondir_file = git_dir.join("commondir");
    check_file(&commondir_file, SMALL_FILE_CAP)?;
    let mismatch = |why: String| format!("{UNTRUSTED_WORKTREE_PATH}: {why}");
    let common = match read_small_file(&commondir_file, SMALL_FILE_CAP) {
        Ok(bytes) => {
            // Same trimming as gix (`bstr` `trim_end`), so both resolve the same path.
            let trimmed = bytes.trim_end();
            if trimmed.is_empty() {
                return Err(mismatch(format!("empty {}", commondir_file.display())));
            }
            let target = git_dir.join(Path::new(std::ffi::OsStr::from_bytes(trimmed)));
            fs::canonicalize(&target)
                .map_err(|e| mismatch(format!("commondir {}: {e}", target.display())))?
        }
        Err(e) if e.kind() == ErrorKind::NotFound => real_git.clone(),
        Err(e) => return Err(untrusted(&commondir_file, &e.to_string())),
    };
    let expected = match expected_common {
        Some(path) => fs::canonicalize(path)
            .map_err(|e| mismatch(format!("expected common dir {}: {e}", path.display())))?,
        None => real_git.clone(),
    };
    if common != expected {
        return Err(mismatch(format!(
            "common dir {} is not {}",
            common.display(),
            expected.display()
        )));
    }
    Ok((real_git, common))
}

/// Alternate object dirs listed in an `alternates` file, parsed like gix (lines, `#` comments).
/// Quoted entries (C-style escapes, embedded newlines) are refused rather than re-implemented.
fn alternate_entries(bytes: &[u8], file: &Path) -> Result<Vec<PathBuf>, String> {
    let mut entries = Vec::new();
    for line in bytes.split(|&b| b == b'\n') {
        if line.is_empty() || line.starts_with(b"#") {
            continue;
        }
        if line.starts_with(b"\"") {
            return Err(untrusted(file, "quoted alternates entry"));
        }
        entries.push(PathBuf::from(std::ffi::OsStr::from_bytes(line)));
    }
    Ok(entries)
}

/// Follows `objects/info/alternates` the way gix will (relative entries resolve against the
/// root objects dir), checking each hop's `info/alternates`; the 9th hop is refused.
fn check_alternates_chain(common: &Path) -> Result<(), String> {
    let objects = common.join("objects");
    let mut visited = HashSet::new();
    let mut level = vec![objects.clone()];
    for depth in 0..=MAX_ALTERNATE_HOPS {
        let mut next = Vec::new();
        for dir in level {
            let Ok(real) = fs::canonicalize(&dir) else {
                continue;
            };
            if !visited.insert(real) {
                continue;
            }
            let file = dir.join("info/alternates");
            check_file(&file, SMALL_FILE_CAP)?;
            let bytes = match read_small_file(&file, SMALL_FILE_CAP) {
                Ok(bytes) => bytes,
                Err(e) if is_absent(e.kind()) => continue,
                Err(e) => return Err(untrusted(&file, &e.to_string())),
            };
            next.extend(
                alternate_entries(&bytes, &file)?
                    .into_iter()
                    .map(|entry| objects.join(entry)),
            );
        }
        if next.is_empty() {
            return Ok(());
        }
        if depth == MAX_ALTERNATE_HOPS {
            return Err(format!(
                "{UNTRUSTED_GIT_DIR}: alternates chain longer than {MAX_ALTERNATE_HOPS} hops"
            ));
        }
        level = next;
    }
    Ok(())
}

/// Runs every §4.4 pre-open check for the repo gix is about to open and returns the resolved
/// common dir. `git_dir` is the directory gix will open (a linked worktree's admin dir),
/// `workdir` the worktree root (`None` for bare), `expected_common` the enumerating repo's
/// `.git` for a linked worktree (`None` for a main or bare repo).
pub fn pre_open_checks(
    git_dir: &Path,
    workdir: Option<&Path>,
    expected_common: Option<&Path>,
) -> Result<PathBuf, String> {
    let (real_git, common) = resolve_common(git_dir, expected_common)?;
    let mut refs_seen = 0;
    check_git_dir(git_dir, &mut refs_seen)?;
    if common != real_git {
        check_git_dir(&common, &mut refs_seen)?;
    }
    check_alternates_chain(&common)?;
    if let Some(workdir) = workdir {
        for name in WORKDIR_FILES {
            check_file(&workdir.join(name), SMALL_FILE_CAP)?;
        }
    }
    Ok(common)
}

/// The unquoted, unescaped value of a git-config value (text after `=`), or `None` when the
/// quoting or an escape is malformed.
fn parse_config_value(text: &str) -> Option<String> {
    let mut out = String::new();
    // Length of `out` without trailing unquoted whitespace.
    let mut keep = 0;
    let mut in_quote = false;
    let mut chars = text.trim_start().chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push(match chars.next()? {
                    'n' => '\n',
                    't' => '\t',
                    'b' => '\x08',
                    escaped @ ('"' | '\\') => escaped,
                    _ => return None,
                });
                keep = out.len();
            }
            '"' => in_quote = !in_quote,
            ';' | '#' if !in_quote => break,
            c if c.is_whitespace() && !in_quote => out.push(c),
            c => {
                out.push(c);
                keep = out.len();
            }
        }
    }
    if in_quote {
        return None;
    }
    out.truncate(keep);
    Some(out)
}

/// `core.excludesFile` (last assignment wins) from git-config text. Section and key names are
/// case-insensitive; `[core "x"]` and other sections do not count; includes are not looked at.
fn parse_excludes_file(text: &str) -> Option<String> {
    let mut in_core = false;
    let mut found = None;
    let mut lines = text.lines();
    while let Some(first) = lines.next() {
        let mut line = first.to_string();
        // A trailing odd number of backslashes continues the line.
        while (line.len() - line.trim_end_matches('\\').len()) % 2 == 1 {
            let Some(next) = lines.next() else { break };
            line.pop();
            line.push_str(next);
        }
        let mut rest = line.trim_start();
        while let Some(header) = rest.strip_prefix('[') {
            let Some(end) = header.find(']') else { break };
            in_core = header[..end].trim().eq_ignore_ascii_case("core");
            rest = header[end + 1..].trim_start();
        }
        if !in_core || !rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
            continue;
        }
        let key_len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .unwrap_or(rest.len());
        let (key, after) = rest.split_at(key_len);
        let Some(value) = after.trim_start().strip_prefix('=') else {
            continue;
        };
        if key.eq_ignore_ascii_case("excludesFile") {
            found = parse_config_value(value);
        }
    }
    found
}

/// `~` / `~/…` expanded against `home`; anything else must already be absolute.
fn expand_config_path(value: &str, home: &Path) -> Option<PathBuf> {
    if value == "~" {
        return Some(home.to_path_buf());
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return Some(home.join(rest));
    }
    let path = PathBuf::from(value);
    path.is_absolute().then_some(path)
}

/// `core.excludesFile` as set by `config`, checked and read like any hostile-tolerant file.
fn excludes_setting(config: &Path, notes: &mut Vec<String>) -> Option<String> {
    let bytes = match read_small_file(config, SMALL_FILE_CAP) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return None,
        Err(e) => {
            notes.push(format!("{}: ignored: {e}", config.display()));
            return None;
        }
    };
    match String::from_utf8(bytes) {
        Ok(text) => parse_excludes_file(&text),
        Err(_) => {
            notes.push(format!("{}: ignored: not valid UTF-8", config.display()));
            None
        }
    }
}

/// The user's `core.excludesFile`: from `~/.gitconfig`, else `$XDG_CONFIG_HOME/git/config`
/// (default `~/.config/git/config`). Same file checks as the pre-open list, 1 MiB cap, no
/// includes. Anything unusable is `None` plus a note. A configured file that does not exist is
/// returned as is (gix treats a missing excludes file as empty).
pub fn user_excludes_file_with_notes(
    home: &Path,
    xdg_config_home: Option<&Path>,
    notes: &mut Vec<String>,
) -> Option<PathBuf> {
    let xdg = xdg_config_home
        .filter(|dir| !dir.as_os_str().is_empty())
        .map_or_else(|| home.join(".config"), Path::to_path_buf);
    let configs = [home.join(".gitconfig"), xdg.join("git/config")];
    let raw = configs
        .iter()
        .find_map(|config| excludes_setting(config, notes))?;
    let Some(path) = expand_config_path(&raw, home) else {
        notes.push(format!(
            "core.excludesFile ignored: unsupported value {raw:?}"
        ));
        return None;
    };
    if let Some(why) = file_problem(&path, SMALL_FILE_CAP) {
        notes.push(format!(
            "core.excludesFile {} ignored: {why}",
            path.display()
        ));
        return None;
    }
    Some(path)
}

pub fn user_excludes_file(home: &Path, xdg_config_home: Option<&Path>) -> Option<PathBuf> {
    user_excludes_file_with_notes(home, xdg_config_home, &mut Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testgit::{add_worktree, init_bare, init_repo, mkfifo, tempdir_real, within};
    use std::fs::File;
    use std::os::unix::fs::symlink;

    const NOT_TRUSTED: &str = "untrusted git dir";

    fn bare() -> (tempfile::TempDir, PathBuf) {
        let (guard, root) = tempdir_real();
        let dir = root.join("repo.git");
        init_bare(&dir);
        (guard, dir)
    }

    fn check(dir: &Path) -> Result<PathBuf, String> {
        pre_open_checks(dir, None, None)
    }

    fn expect_untrusted(result: Result<PathBuf, String>) {
        let err = result.expect_err("expected an untrusted verdict");
        assert!(err.starts_with(NOT_TRUSTED), "unexpected error: {err}");
    }

    fn make_parent(path: &Path) {
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    }

    fn remove(path: &Path) {
        let _ = fs::remove_file(path);
    }

    fn make_sparse(path: &Path, len: u64) {
        make_parent(path);
        remove(path);
        File::create(path)
            .expect("create")
            .set_len(len)
            .expect("set_len");
    }

    fn make_symlink(path: &Path) {
        make_parent(path);
        remove(path);
        let target = path.with_extension("target-file");
        fs::write(&target, "x").expect("write target");
        symlink(&target, path).expect("symlink");
    }

    fn make_fifo(path: &Path) {
        make_parent(path);
        remove(path);
        mkfifo(path);
    }

    /// All four verdicts for one file of the pre-open list, on fresh repos.
    fn exercise_file(relative: &str, cap: u64) {
        let (_g, dir) = bare();
        remove(&dir.join(relative));
        assert!(check(&dir).is_ok(), "{relative}: absent must be fine");

        make_symlink(&dir.join(relative));
        expect_untrusted(check(&dir));

        make_sparse(&dir.join(relative), cap + 1);
        expect_untrusted(check(&dir));

        make_fifo(&dir.join(relative));
        let owned = dir.clone();
        expect_untrusted(within(move || check(&owned)));
    }

    macro_rules! file_tests {
        ($($name:ident: $relative:literal, $cap:expr;)*) => {
            $(#[test]
            fn $name() {
                exercise_file($relative, $cap);
            })*
        };
    }

    file_tests! {
        config_file: "config", MIB;
        config_worktree_file: "config.worktree", MIB;
        head_file: "HEAD", 4 * KIB;
        packed_refs_file: "packed-refs", 64 * MIB;
        index_file: "index", 256 * MIB;
        commondir_file: "commondir", MIB;
        gitdir_file: "gitdir", MIB;
        shallow_file: "shallow", MIB;
        info_exclude_file: "info/exclude", MIB;
        info_attributes_file: "info/attributes", MIB;
        alternates_file: "objects/info/alternates", MIB;
        commit_graph_chain_file: "objects/info/commit-graphs/commit-graph-chain", MIB;
    }

    #[test]
    fn caps_are_inclusive() {
        let (_g, dir) = bare();
        make_sparse(&dir.join("HEAD"), 4 * KIB);
        make_sparse(&dir.join("config"), MIB);
        assert!(check(&dir).is_ok());
        make_sparse(&dir.join("HEAD"), 4 * KIB + 1);
        expect_untrusted(check(&dir));
    }

    #[test]
    fn fresh_bare_repo_is_trusted_and_returns_itself_as_common() {
        let (_g, dir) = bare();
        assert_eq!(check(&dir), Ok(dir.clone()));
    }

    #[test]
    fn worktree_root_files_are_checked_only_when_a_workdir_is_given() {
        for name in WORKDIR_FILES {
            let (_g, root) = tempdir_real();
            let repo = root.join("repo");
            init_repo(&repo);
            let git_dir = repo.join(".git");
            let file = repo.join(name);
            remove(&file);
            assert!(pre_open_checks(&git_dir, Some(&repo), None).is_ok());

            make_symlink(&file);
            expect_untrusted(pre_open_checks(&git_dir, Some(&repo), None));
            assert!(pre_open_checks(&git_dir, None, None).is_ok(), "{name}");

            make_sparse(&file, MIB + 1);
            expect_untrusted(pre_open_checks(&git_dir, Some(&repo), None));

            make_fifo(&file);
            let (git_dir, repo) = (git_dir.clone(), repo.clone());
            expect_untrusted(within(move || pre_open_checks(&git_dir, Some(&repo), None)));
        }
    }

    #[test]
    fn loose_refs_must_be_small_regular_files() {
        let (_g, dir) = bare();
        let refs = dir.join("refs/heads");
        fs::write(refs.join("ok"), "0".repeat(4 * 1024)).unwrap();
        assert!(check(&dir).is_ok());

        make_sparse(&refs.join("big"), 4 * KIB + 1);
        expect_untrusted(check(&dir));
        remove(&refs.join("big"));

        make_symlink(&refs.join("link"));
        expect_untrusted(check(&dir));
        remove(&refs.join("link"));

        make_fifo(&refs.join("pipe"));
        let owned = dir.clone();
        expect_untrusted(within(move || check(&owned)));
    }

    #[test]
    fn refs_subdirectories_must_not_be_symlinks() {
        let (_g, dir) = bare();
        let elsewhere = dir.parent().unwrap().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        symlink(&elsewhere, dir.join("refs/heads/linked")).unwrap();
        expect_untrusted(check(&dir));
    }

    #[test]
    fn refs_directory_itself_must_not_be_a_symlink() {
        let (_g, dir) = bare();
        let elsewhere = dir.parent().unwrap().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        fs::remove_dir_all(dir.join("refs")).unwrap();
        symlink(&elsewhere, dir.join("refs")).unwrap();
        expect_untrusted(check(&dir));
    }

    fn write_refs(dir: &Path, count: usize) {
        let heads = dir.join("refs/heads");
        for i in 0..count {
            fs::write(heads.join(format!("r{i}")), "0").unwrap();
        }
    }

    #[test]
    fn ten_thousand_ref_entries_pass_and_one_more_fails() {
        let (_g, dir) = bare();
        // A fresh bare repo already counts `HEAD`, `refs/heads` and `refs/tags`.
        write_refs(&dir, MAX_REF_ENTRIES - 3);
        assert!(check(&dir).is_ok());
        write_refs(&dir, MAX_REF_ENTRIES - 2);
        expect_untrusted(check(&dir));
    }

    #[test]
    fn ten_thousand_and_one_loose_refs_are_untrusted() {
        let (_g, dir) = bare();
        write_refs(&dir, MAX_REF_ENTRIES + 1);
        expect_untrusted(check(&dir));
    }

    /// `n` chained alternate dirs; returns the repo git dir whose alternates lists the first.
    fn alternates_chain(n: usize) -> (tempfile::TempDir, PathBuf) {
        let (guard, dir) = bare();
        let base = dir.parent().unwrap().to_path_buf();
        let hop = |i: usize| base.join(format!("alt{i}"));
        for i in 1..=n {
            fs::create_dir_all(hop(i).join("info")).unwrap();
            if i < n {
                fs::write(
                    hop(i).join("info/alternates"),
                    format!("{}\n", hop(i + 1).display()),
                )
                .unwrap();
            }
        }
        fs::write(
            dir.join("objects/info/alternates"),
            format!("{}\n", hop(1).display()),
        )
        .unwrap();
        (guard, dir)
    }

    #[test]
    fn alternates_chain_of_eight_hops_is_trusted() {
        let (_g, dir) = alternates_chain(MAX_ALTERNATE_HOPS);
        assert!(check(&dir).is_ok());
    }

    #[test]
    fn alternates_chain_of_nine_hops_is_untrusted() {
        let (_g, dir) = alternates_chain(MAX_ALTERNATE_HOPS + 1);
        expect_untrusted(check(&dir));
    }

    #[test]
    fn alternates_hop_files_are_checked() {
        let (_g, dir) = alternates_chain(3);
        let hop_file = dir.parent().unwrap().join("alt2/info/alternates");
        make_symlink(&hop_file);
        expect_untrusted(check(&dir));
        make_sparse(&hop_file, MIB + 1);
        expect_untrusted(check(&dir));
        make_fifo(&hop_file);
        let owned = dir.clone();
        expect_untrusted(within(move || check(&owned)));
    }

    #[test]
    fn alternates_entries_skip_comments_and_resolve_relative_to_objects() {
        let (_g, dir) = bare();
        let base = dir.parent().unwrap();
        fs::create_dir_all(base.join("alt/info")).unwrap();
        fs::write(base.join("alt/info/alternates"), "").unwrap();
        fs::write(
            dir.join("objects/info/alternates"),
            "# comment\n\n../../alt\n",
        )
        .unwrap();
        assert!(check(&dir).is_ok());
        // A poisoned hop reached through the relative entry proves it was followed.
        make_symlink(&base.join("alt/info/alternates"));
        expect_untrusted(check(&dir));
    }

    #[test]
    fn quoted_alternates_entries_are_refused() {
        let (_g, dir) = bare();
        fs::write(dir.join("objects/info/alternates"), "\"/tmp/x\"\n").unwrap();
        expect_untrusted(check(&dir));
    }

    #[test]
    fn alternates_cycle_terminates() {
        let (_g, dir) = bare();
        fs::write(
            dir.join("objects/info/alternates"),
            format!("{}\n", dir.join("objects").display()),
        )
        .unwrap();
        let owned = dir.clone();
        assert!(within(move || check(&owned)).is_ok());
    }

    #[test]
    fn head_naming_a_symlink_pseudo_ref_is_untrusted() {
        let (_g, dir) = bare();
        fs::write(dir.join("HEAD"), "ref: EVIL\n").unwrap();
        assert!(check(&dir).is_ok(), "an absent EVIL is fine");
        make_symlink(&dir.join("EVIL"));
        expect_untrusted(check(&dir));
    }

    #[test]
    fn pseudo_ref_names_are_all_caps_and_underscore_only() {
        let (_g, dir) = bare();
        make_symlink(&dir.join("Evil"));
        make_symlink(&dir.join("lowercase"));
        make_symlink(&dir.join("EVIL2"));
        assert!(check(&dir).is_ok());
        fs::write(dir.join("ORIG_HEAD"), "0".repeat(40)).unwrap();
        assert!(check(&dir).is_ok());
        make_sparse(&dir.join("FETCH_HEAD"), MIB + 1);
        expect_untrusted(check(&dir));
    }

    #[test]
    fn non_head_pseudo_refs_may_be_up_to_one_mib() {
        let (_g, dir) = bare();
        make_sparse(&dir.join("FETCH_HEAD"), 50 * KIB);
        assert!(check(&dir).is_ok());
        make_sparse(&dir.join("FETCH_HEAD"), MIB);
        assert!(check(&dir).is_ok());
        make_sparse(&dir.join("FETCH_HEAD"), MIB + 1);
        expect_untrusted(check(&dir));
        remove(&dir.join("FETCH_HEAD"));

        make_sparse(&dir.join("COMMIT_EDITMSG"), 50 * KIB);
        assert!(check(&dir).is_ok());

        // Still untrusted at any size: a symlink, a FIFO, a directory.
        make_symlink(&dir.join("MERGE_MSG"));
        expect_untrusted(check(&dir));
        remove(&dir.join("MERGE_MSG"));
        fs::create_dir(dir.join("MERGE_MODE")).unwrap();
        expect_untrusted(check(&dir));
        fs::remove_dir(dir.join("MERGE_MODE")).unwrap();
        make_fifo(&dir.join("ORIG_HEAD"));
        let owned = dir.clone();
        expect_untrusted(within(move || check(&owned)));
    }

    #[test]
    fn head_keeps_the_four_kib_cap() {
        let (_g, dir) = bare();
        make_sparse(&dir.join("HEAD"), 4 * KIB + 1);
        expect_untrusted(check(&dir));
        let (_g, linked_main, _worktree, admin) = linked();
        make_sparse(&admin.join("HEAD"), 4 * KIB + 1);
        expect_untrusted(pre_open_checks(
            &linked_main.join(".git"),
            Some(&linked_main),
            None,
        ));
    }

    #[test]
    fn large_non_head_pseudo_ref_in_an_admin_dir_passes() {
        let (_g, main, _worktree, admin) = linked();
        make_sparse(&admin.join("FETCH_HEAD"), 50 * KIB);
        assert!(pre_open_checks(&main.join(".git"), Some(&main), None).is_ok());
    }

    fn linked() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let (guard, root) = tempdir_real();
        let main = root.join("main");
        init_repo(&main);
        let worktree = root.join("wt");
        add_worktree(&main, &worktree, "wt-branch");
        let admin = main.join(".git/worktrees/wt");
        (guard, main, worktree, admin)
    }

    #[test]
    fn normal_linked_worktree_resolves_the_main_git_dir() {
        let (_g, main, worktree, admin) = linked();
        let main_git = main.join(".git");
        assert_eq!(
            pre_open_checks(&admin, Some(&worktree), Some(&main_git)),
            Ok(main_git.clone())
        );
        assert_eq!(pre_open_checks(&main_git, Some(&main), None), Ok(main_git));
    }

    #[test]
    fn all_caps_fifo_in_an_admin_dir_is_untrusted_from_both_sides() {
        let (_g, main, worktree, admin) = linked();
        let main_git = main.join(".git");
        make_fifo(&admin.join("EVIL_FIFO"));
        let (g, m) = (main_git.clone(), main.clone());
        expect_untrusted(within(move || pre_open_checks(&g, Some(&m), None)));
        let (a, w, g) = (admin.clone(), worktree.clone(), main_git.clone());
        expect_untrusted(within(move || pre_open_checks(&a, Some(&w), Some(&g))));
    }

    #[test]
    fn admin_dir_config_worktree_and_refs_are_checked_from_the_main_repo() {
        let (_g, main, _worktree, admin) = linked();
        let main_git = main.join(".git");
        let from_main = || pre_open_checks(&main_git, Some(&main), None);
        assert!(from_main().is_ok());

        make_symlink(&admin.join("config.worktree"));
        expect_untrusted(from_main());
        remove(&admin.join("config.worktree"));

        make_sparse(&admin.join("refs/bisect/bad"), 4 * KIB + 1);
        expect_untrusted(from_main());
    }

    #[test]
    fn admin_dir_refs_share_the_entry_budget() {
        let (_g, main, _worktree, admin) = linked();
        let bisect = admin.join("refs/bisect");
        fs::create_dir_all(&bisect).unwrap();
        for i in 0..MAX_REF_ENTRIES {
            fs::write(bisect.join(format!("r{i}")), "0").unwrap();
        }
        expect_untrusted(pre_open_checks(&main.join(".git"), Some(&main), None));
    }

    #[test]
    fn linked_worktree_checks_run_on_the_resolved_common_dir() {
        let (_g, main, worktree, admin) = linked();
        let main_git = main.join(".git");
        let from_admin = || pre_open_checks(&admin, Some(&worktree), Some(&main_git));
        assert!(from_admin().is_ok());

        make_symlink(&main_git.join("config"));
        expect_untrusted(from_admin());
        remove(&main_git.join("config"));

        make_symlink(&main_git.join("EVIL"));
        expect_untrusted(from_admin());
    }

    #[test]
    fn main_repo_with_a_commondir_naming_another_repo_is_an_untrusted_worktree_path() {
        let (_g, root) = tempdir_real();
        let (main, other) = (root.join("main"), root.join("other"));
        init_repo(&main);
        init_repo(&other);
        let main_git = main.join(".git");
        fs::write(
            main_git.join("commondir"),
            format!("{}\n", other.join(".git").display()),
        )
        .unwrap();
        let err = pre_open_checks(&main_git, Some(&main), None).unwrap_err();
        assert!(err.starts_with("untrusted worktree path"), "{err}");
    }

    #[test]
    fn commondir_naming_the_git_dir_itself_is_fine_for_a_main_repo() {
        let (_g, dir) = bare();
        fs::write(dir.join("commondir"), ".\n").unwrap();
        assert_eq!(check(&dir), Ok(dir.clone()));
    }

    #[test]
    fn linked_worktree_commondir_naming_another_repo_is_an_untrusted_worktree_path() {
        let (_g, main, worktree, admin) = linked();
        let other = main.parent().unwrap().join("other");
        init_repo(&other);
        fs::write(
            admin.join("commondir"),
            format!("{}\n", other.join(".git").display()),
        )
        .unwrap();
        let err = pre_open_checks(&admin, Some(&worktree), Some(&main.join(".git"))).unwrap_err();
        assert!(err.starts_with("untrusted worktree path"), "{err}");
    }

    #[test]
    fn linked_worktree_without_a_commondir_or_with_a_dangling_one_is_untrusted() {
        let (_g, main, worktree, admin) = linked();
        let main_git = main.join(".git");
        let run = || pre_open_checks(&admin, Some(&worktree), Some(&main_git)).unwrap_err();
        fs::write(admin.join("commondir"), "../../nowhere\n").unwrap();
        assert!(run().starts_with("untrusted worktree path"));
        remove(&admin.join("commondir"));
        assert!(run().starts_with("untrusted worktree path"));
        fs::write(admin.join("commondir"), "  \n").unwrap();
        assert!(run().starts_with("untrusted worktree path"));
    }

    #[test]
    fn commondir_is_trimmed_like_gix_before_it_is_compared() {
        let (_g, main, worktree, admin) = linked();
        let main_git = main.join(".git");
        // U+3000 is Unicode whitespace: gix trims it, so a `..\u{3000}` alias must not be
        // resolved differently by rollcall.
        let alias = admin.join("../..\u{3000}");
        symlink(&main_git, &alias).unwrap();
        fs::write(admin.join("commondir"), "../..\u{3000}\n").unwrap();
        assert!(pre_open_checks(&admin, Some(&worktree), Some(&main_git)).is_ok());
    }

    #[test]
    fn commondir_symlink_is_an_untrusted_git_dir_not_a_path_verdict() {
        let (_g, main, worktree, admin) = linked();
        make_symlink(&admin.join("commondir"));
        expect_untrusted(pre_open_checks(
            &admin,
            Some(&worktree),
            Some(&main.join(".git")),
        ));
    }

    fn home_with_gitconfig(content: &str) -> (tempfile::TempDir, PathBuf) {
        let (guard, home) = tempdir_real();
        fs::write(home.join(".gitconfig"), content).unwrap();
        (guard, home)
    }

    fn excludes(home: &Path, xdg: Option<&Path>) -> (Option<PathBuf>, Vec<String>) {
        let mut notes = Vec::new();
        (user_excludes_file_with_notes(home, xdg, &mut notes), notes)
    }

    #[test]
    fn excludes_file_is_read_from_dot_gitconfig_with_tilde_expansion() {
        let (_g, home) = home_with_gitconfig("[core]\n\texcludesFile = ~/.gitignore_global\n");
        fs::write(home.join(".gitignore_global"), "*.log\n").unwrap();
        let (found, notes) = excludes(&home, None);
        assert_eq!(found, Some(home.join(".gitignore_global")));
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(
            user_excludes_file(&home, None),
            Some(home.join(".gitignore_global"))
        );
    }

    #[test]
    fn excludes_file_accepts_absolute_quoted_values_and_git_config_syntax() {
        let (_g, home) = tempdir_real();
        let target = home.join("my ignores");
        fs::write(&target, "x").unwrap();
        let config = format!(
            "# lead comment\n[USER]\n  excludesfile = /wrong\n[Core] ; trailing\n  EXCLUDESFILE = \"{}\" # note\n[core \"sub\"]\n  excludesFile = /wrong\n",
            target.display()
        );
        fs::write(home.join(".gitconfig"), config).unwrap();
        assert_eq!(excludes(&home, None).0, Some(target));
    }

    #[test]
    fn excludes_file_on_the_section_line_and_last_assignment_win() {
        let (_g, home) = home_with_gitconfig(&format!(
            "[core] excludesFile = /first\n[core]\n excludesFile = {}\n",
            "/second"
        ));
        assert_eq!(excludes(&home, None).0, Some(PathBuf::from("/second")));
    }

    #[test]
    fn excludes_file_supports_line_continuation() {
        let (_g, home) = home_with_gitconfig("[core]\n excludesFile = /a\\\nb\n");
        assert_eq!(excludes(&home, None).0, Some(PathBuf::from("/ab")));
    }

    #[test]
    fn excludes_file_falls_back_to_xdg_config_home() {
        let (_g, home) = home_with_gitconfig("[user]\n name = x\n");
        let (_h, xdg) = tempdir_real();
        fs::create_dir(xdg.join("git")).unwrap();
        fs::write(xdg.join("git/config"), "[core]\nexcludesFile = /from-xdg\n").unwrap();
        assert_eq!(
            excludes(&home, Some(&xdg)).0,
            Some(PathBuf::from("/from-xdg"))
        );
    }

    #[test]
    fn excludes_file_defaults_xdg_to_dot_config_and_prefers_dot_gitconfig() {
        let (_g, home) = tempdir_real();
        fs::create_dir_all(home.join(".config/git")).unwrap();
        fs::write(
            home.join(".config/git/config"),
            "[core]\nexcludesFile = /from-default-xdg\n",
        )
        .unwrap();
        assert_eq!(
            excludes(&home, None).0,
            Some(PathBuf::from("/from-default-xdg"))
        );
        assert_eq!(
            excludes(&home, Some(Path::new(""))).0,
            Some(PathBuf::from("/from-default-xdg"))
        );
        fs::write(
            home.join(".gitconfig"),
            "[core]\nexcludesFile = /home-wins\n",
        )
        .unwrap();
        assert_eq!(excludes(&home, None).0, Some(PathBuf::from("/home-wins")));
    }

    #[test]
    fn include_path_is_not_followed() {
        let (_g, home) = home_with_gitconfig("[include]\n\tpath = extra\n");
        fs::write(home.join("extra"), "[core]\nexcludesFile = /included\n").unwrap();
        let (found, notes) = excludes(&home, None);
        assert_eq!(found, None);
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn oversized_gitconfig_is_none_with_a_note() {
        let (_g, home) = tempdir_real();
        make_sparse(&home.join(".gitconfig"), MIB + 1);
        let (found, notes) = excludes(&home, None);
        assert_eq!(found, None);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains(".gitconfig"), "{notes:?}");
    }

    #[test]
    fn symlinked_or_fifo_gitconfig_is_none_with_a_note() {
        let (_g, home) = tempdir_real();
        make_symlink(&home.join(".gitconfig"));
        let (found, notes) = excludes(&home, None);
        assert_eq!((found, notes.len()), (None, 1));

        make_fifo(&home.join(".gitconfig"));
        let owned = home.clone();
        let (found, notes) = within(move || excludes(&owned, None));
        assert_eq!((found, notes.len()), (None, 1));
    }

    #[test]
    fn unusable_excludes_file_is_none_with_a_note() {
        let (_g, home) = home_with_gitconfig("[core]\nexcludesFile = ~/ignores\n");
        let ignores = home.join("ignores");

        make_symlink(&ignores);
        let (found, notes) = excludes(&home, None);
        assert_eq!((found, notes.len()), (None, 1));

        make_sparse(&ignores, MIB + 1);
        let (found, notes) = excludes(&home, None);
        assert_eq!((found, notes.len()), (None, 1));

        make_fifo(&ignores);
        let owned = home.clone();
        let (found, notes) = within(move || excludes(&owned, None));
        assert_eq!((found, notes.len()), (None, 1));

        make_sparse(&ignores, MIB);
        assert_eq!(excludes(&home, None).0, Some(ignores));
    }

    #[test]
    fn relative_excludes_value_is_none_with_a_note() {
        let (_g, home) = home_with_gitconfig("[core]\nexcludesFile = relative/path\n");
        let (found, notes) = excludes(&home, None);
        assert_eq!((found, notes.len()), (None, 1));
    }

    #[test]
    fn malformed_excludes_values_are_ignored() {
        for value in ["\"unterminated", "bad\\qescape", "~other/x"] {
            let (_g, home) = home_with_gitconfig(&format!("[core]\nexcludesFile = {value}\n"));
            assert_eq!(excludes(&home, None).0, None, "{value}");
        }
    }

    #[test]
    fn missing_configs_give_no_excludes_and_no_notes() {
        let (_g, home) = tempdir_real();
        assert_eq!(excludes(&home, None), (None, Vec::new()));
    }
}
