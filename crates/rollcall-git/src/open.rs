//! §4.4 pre-open checks and the user's `core.excludesFile`.
//!
//! gix's own size caps run after `File::open`, which blocks on a FIFO and follows symlinks, so
//! every file gix may read is `lstat`-checked here first (regular file, within its cap).

use std::collections::HashSet;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{BufReader, ErrorKind, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use gix::bstr::ByteSlice;

use crate::classify::{MAX_LINKED_WORKTREES, read_small_file};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const SMALL_FILE_CAP: u64 = MIB;
/// Pattern files gix parses into ~70x their size (P15a U-7), so they get a tighter cap.
const PATTERN_FILE_CAP: u64 = 256 * KIB;
const REF_CAP: u64 = 4 * KIB;
const PACKED_REFS_CAP: u64 = 64 * MIB;
const INDEX_CAP: u64 = 256 * MIB;
/// `refs/` entries (and all-caps pseudo-ref entries) examined per call before giving up.
const MAX_REF_ENTRIES: usize = 10_000;
const MAX_ALTERNATE_HOPS: usize = 8;

/// Index extension caps and the nesting bound (§4.4 index validation, P15a U-4 / U-8).
const TREE_EXT_CAP: u64 = 16 * MIB;
const UNTR_EXT_CAP: u64 = 4 * MIB;
const REUC_EXT_CAP: u64 = MIB;
const MAX_INDEX_NESTING: usize = 1024;
const MAX_INDEX_NAME: usize = 4096;
/// The lock builds gix with `sha1` only.
const HASH_LEN: usize = 20;
const INDEX_HEADER_LEN: u64 = 12;
/// Entry bytes before the name: ten `u32` stat fields, the object id, the `u16` flags.
const ENTRY_FIXED_LEN: usize = 10 * 4 + HASH_LEN + 2;
const ENTRY_EXTENDED_FLAG: u16 = 0x4000;
const ENTRY_NAME_LEN_MASK: u16 = 0x0FFF;
/// `UNTR`: two 36-byte stat blocks, `dir_flags`, and two object ids, before `exclude_per_dir`.
const UNTR_FIXED_LEN: u64 = 36 + 36 + 4 + 2 * HASH_LEN as u64;
/// Digits of a `TREE` count: enough for any `i32`, few enough to never overflow an `i64`.
const MAX_COUNT_DIGITS: usize = 11;
const INDEX_BUF_LEN: usize = 64 * 1024;

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
    ("info/exclude", PATTERN_FILE_CAP),
    ("info/attributes", PATTERN_FILE_CAP),
    ("objects/info/alternates", SMALL_FILE_CAP),
    (
        "objects/info/commit-graphs/commit-graph-chain",
        SMALL_FILE_CAP,
    ),
];

/// Worktree-root files gix reads during status.
const WORKDIR_FILES: [(&str, u64); 3] = [
    (".gitmodules", SMALL_FILE_CAP),
    (".gitignore", PATTERN_FILE_CAP),
    (".gitattributes", PATTERN_FILE_CAP),
];

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
/// the rest ≤ 1 MiB. Also `sharedindex.*` (split-index bases): regular files within the index cap.
fn check_pseudo_refs(dir: &Path, seen: &mut usize) -> Result<(), String> {
    let Some(entries) = read_dir_opt(dir)? else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry.map_err(|e| untrusted(dir, &e.to_string()))?;
        let name = entry.file_name();
        let bytes = name.as_bytes();
        let shared_index = bytes.starts_with(b"sharedindex.");
        if !shared_index
            && (bytes.is_empty() || !bytes.iter().all(|b| b.is_ascii_uppercase() || *b == b'_'))
        {
            continue;
        }
        count_ref_entry(seen)?;
        let path = entry.path();
        let md = entry
            .metadata()
            .map_err(|e| untrusted(&path, &e.to_string()))?;
        // gix reads the one a `link` extension names; every candidate gets the `index` rule.
        if shared_index {
            if let Some(why) = metadata_problem(&md, INDEX_CAP) {
                return Err(untrusted(&path, &why));
            }
            continue;
        }
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

/// Forward-only reader over one index file that never reads past `end` (the trailer is
/// excluded) and holds only a fixed buffer, never the file.
struct IndexStream {
    file: BufReader<File>,
    pos: u64,
    end: u64,
}

const PAST_END: &str = "runs past the end of its data";

impl IndexStream {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), String> {
        if buf.len() as u64 > self.end - self.pos {
            return Err(PAST_END.to_string());
        }
        self.file.read_exact(buf).map_err(|e| e.to_string())?;
        self.pos += buf.len() as u64;
        Ok(())
    }

    fn byte(&mut self) -> Result<u8, String> {
        let mut one = [0u8];
        self.fill(&mut one)?;
        Ok(one[0])
    }

    fn skip(&mut self, len: u64) -> Result<(), String> {
        if len > self.end - self.pos {
            return Err(PAST_END.to_string());
        }
        let len_i64 = i64::try_from(len).map_err(|e| e.to_string())?;
        self.file
            .seek_relative(len_i64)
            .map_err(|e| e.to_string())?;
        self.pos += len;
        Ok(())
    }

    /// Consumes through the next NUL and returns the bytes before it; `Err` once more than
    /// `max` precede it.
    fn skip_cstr(&mut self, max: usize) -> Result<usize, String> {
        let mut len = 0;
        while self.byte()? != 0 {
            len += 1;
            if len > max {
                return Err(format!("string longer than {max} bytes"));
            }
        }
        Ok(len)
    }

    /// git's offset varint (`decode_varint`), overflow refused.
    fn varint(&mut self) -> Result<u64, String> {
        let mut byte = self.byte()?;
        let mut value = u64::from(byte & 0x7f);
        while byte & 0x80 != 0 {
            byte = self.byte()?;
            let shifted = value.checked_add(1).and_then(|v| v.checked_mul(128));
            value = shifted.ok_or("varint overflow")? + u64::from(byte & 0x7f);
        }
        Ok(value)
    }

    /// An ASCII decimal (optionally negative) ended by `stop`, as in a `TREE` node header.
    fn decimal(&mut self, stop: u8) -> Result<i64, String> {
        let (mut value, mut digits, mut negative) = (0i64, 0, false);
        loop {
            let byte = self.byte()?;
            if byte == stop {
                break;
            } else if byte == b'-' && digits == 0 && !negative {
                negative = true;
            } else if byte.is_ascii_digit() && digits < MAX_COUNT_DIGITS {
                value = value * 10 + i64::from(byte - b'0');
                digits += 1;
            } else {
                return Err("malformed number".to_string());
            }
        }
        if digits == 0 {
            return Err("malformed number".to_string());
        }
        Ok(if negative { -value } else { value })
    }
}

/// Walks every entry (versions 2 to 4) without keeping any; true when one has an empty name.
fn skip_index_entries(stream: &mut IndexStream, version: u32, count: u32) -> Result<bool, String> {
    let mut previous_len = 0usize;
    let mut empty_name = false;
    for _ in 0..count {
        let mut fixed = [0u8; ENTRY_FIXED_LEN];
        stream.fill(&mut fixed)?;
        let flags = u16::from_be_bytes([fixed[ENTRY_FIXED_LEN - 2], fixed[ENTRY_FIXED_LEN - 1]]);
        let mut entry_len = ENTRY_FIXED_LEN as u64;
        // gix reads the extended flags whatever the version, so the walk must too.
        if flags & ENTRY_EXTENDED_FLAG != 0 {
            stream.skip(2)?;
            entry_len += 2;
        }
        let name_len = if version == 4 {
            let strip = usize::try_from(stream.varint()?).map_err(|e| e.to_string())?;
            let kept = previous_len
                .checked_sub(strip)
                .ok_or("prefix strip longer than the previous name")?;
            kept + stream.skip_cstr(MAX_INDEX_NAME)?
        } else {
            let declared = flags & ENTRY_NAME_LEN_MASK;
            // gix does not skip the padding after a name whose length field is saturated but
            // git writes it; the two would then disagree on where the extensions start.
            if declared == ENTRY_NAME_LEN_MASK {
                return Err("name length field saturated".to_string());
            }
            stream.skip(u64::from(declared))?;
            // One to eight NULs pad the entry to a multiple of eight bytes.
            stream.skip(8 - (entry_len + u64::from(declared)) % 8)?;
            usize::from(declared)
        };
        if name_len > MAX_INDEX_NAME {
            return Err(format!("name longer than {MAX_INDEX_NAME} bytes"));
        }
        previous_len = name_len;
        empty_name |= name_len == 0;
    }
    Ok(empty_name)
}

/// Marks a finished node: its parent loses one pending child, and any ancestor left with none
/// is finished in turn.
fn finish_node(pending: &mut Vec<u64>) {
    while let Some(top) = pending.last_mut() {
        *top -= 1;
        if *top > 0 {
            break;
        }
        pending.pop();
    }
}

/// Bounds the nesting of a `TREE` or `UNTR` node forest with an explicit stack: `visit_node`
/// consumes one node and returns its child count.
fn check_nesting(
    stream: &mut IndexStream,
    mut visit_node: impl FnMut(&mut IndexStream) -> Result<u64, String>,
) -> Result<(), String> {
    let mut pending = Vec::new();
    loop {
        if pending.len() >= MAX_INDEX_NESTING {
            return Err(format!("nested deeper than {MAX_INDEX_NESTING} levels"));
        }
        match visit_node(stream)? {
            0 => finish_node(&mut pending),
            children => pending.push(children),
        }
        if pending.is_empty() {
            return Ok(());
        }
    }
}

/// `TREE`: per node `path NUL entries SP subtrees LF` plus an object id unless entries < 0.
fn check_tree_nesting(stream: &mut IndexStream) -> Result<(), String> {
    check_nesting(stream, |s| {
        s.skip_cstr(usize::MAX)?;
        let entries = s.decimal(b' ')?;
        let children = u64::try_from(s.decimal(b'\n')?).map_err(|_| "negative subtree count")?;
        if entries >= 0 {
            s.skip(HASH_LEN as u64)?;
        }
        Ok(children)
    })
}

/// `UNTR`: a header, then directory blocks in preorder (`files dirs name NUL` and the file
/// names); the bitmaps after the blocks are not needed for nesting.
fn check_untr_nesting(stream: &mut IndexStream) -> Result<(), String> {
    let identifier_len = stream.varint()?;
    stream.skip(identifier_len)?;
    stream.skip(UNTR_FIXED_LEN)?;
    stream.skip_cstr(usize::MAX)?;
    if stream.varint()? == 0 {
        return Ok(());
    }
    check_nesting(stream, |s| {
        let files = s.varint()?;
        let dirs = s.varint()?;
        s.skip_cstr(usize::MAX)?;
        for _ in 0..files {
            s.skip_cstr(usize::MAX)?;
        }
        Ok(dirs)
    })
}

fn read_link_id(stream: &mut IndexStream) -> Result<[u8; HASH_LEN], String> {
    let mut id = [0u8; HASH_LEN];
    stream.fill(&mut id)?;
    Ok(id)
}

/// Checks every extension header and returns the object id of the `link` extension, if any.
fn check_index_extensions(stream: &mut IndexStream) -> Result<Option<[u8; HASH_LEN]>, String> {
    let mut seen: Vec<[u8; 4]> = Vec::new();
    let mut link = None;
    while stream.pos < stream.end {
        let mut header = [0u8; 8];
        stream.fill(&mut header)?;
        let signature = [header[0], header[1], header[2], header[3]];
        let size = u64::from(u32::from_be_bytes([
            header[4], header[5], header[6], header[7],
        ]));
        let cap = match &signature {
            b"TREE" => Some(TREE_EXT_CAP),
            b"UNTR" => Some(UNTR_EXT_CAP),
            b"REUC" => Some(REUC_EXT_CAP),
            _ => None,
        };
        let name = String::from_utf8_lossy(&signature).into_owned();
        if cap.is_some_and(|cap| size > cap) {
            return Err(format!("{name} extension larger than its cap"));
        }
        if size > stream.end - stream.pos {
            return Err(format!("{name} extension {PAST_END}"));
        }
        // gix decodes every copy; one of each keeps the caps meaningful.
        if (cap.is_some() || &signature == b"link") && seen.contains(&signature) {
            return Err(format!("duplicate {name} extension"));
        }
        seen.push(signature);
        let extension_end = stream.pos + size;
        let outer_end = std::mem::replace(&mut stream.end, extension_end);
        let checked = match &signature {
            b"TREE" => check_tree_nesting(stream),
            b"UNTR" => check_untr_nesting(stream),
            b"link" => read_link_id(stream).map(|id| link = Some(id)),
            _ => Ok(()),
        };
        stream.end = outer_end;
        checked.map_err(|why| format!("{name} extension: {why}"))?;
        stream.skip(extension_end - stream.pos)?;
    }
    Ok(link)
}

/// Iterative, streamed validation of one index file: header, entries, extension table. `Ok`
/// carries the object id of its `link` extension (a split index), if any.
fn validate_index(file: File, len: u64) -> Result<Option<[u8; HASH_LEN]>, String> {
    let trailer = HASH_LEN as u64;
    if len < INDEX_HEADER_LEN + trailer {
        return Err("too small for a header and checksum".to_string());
    }
    let mut stream = IndexStream {
        file: BufReader::with_capacity(INDEX_BUF_LEN, file),
        pos: 0,
        end: len - trailer,
    };
    let mut header = [0u8; INDEX_HEADER_LEN as usize];
    stream.fill(&mut header)?;
    if &header[..4] != b"DIRC" {
        return Err("not an index file".to_string());
    }
    let version = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
    if !(2..=4).contains(&version) {
        return Err(format!("unsupported index version {version}"));
    }
    let count = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
    let empty_name = skip_index_entries(&mut stream, version, count)?;
    let link = check_index_extensions(&mut stream)?;
    // git's split index writes replaced entries with empty names; nothing else does.
    if empty_name && link.is_none() {
        return Err("empty entry name without a link extension".to_string());
    }
    Ok(link)
}

/// Opens an index-like file the way §4.4 requires (no symlink, no FIFO block, regular, within
/// the cap); `None` when it does not exist.
fn open_index(path: &Path) -> Result<Option<(File, u64)>, String> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if is_absent(e.kind()) => return Ok(None),
        Err(e) => return Err(untrusted(path, &e.to_string())),
    };
    let md = file
        .metadata()
        .map_err(|e| untrusted(path, &e.to_string()))?;
    match metadata_problem(&md, INDEX_CAP) {
        Some(why) => Err(untrusted(path, &why)),
        None => Ok(Some((file, md.len()))),
    }
}

/// Validates `<dir>/index` (when present) and, for a split index, the `sharedindex.<id>` beside
/// it that its `link` extension names: gix reads exactly that file, which must exist and must
/// not itself link onward.
fn check_index(dir: &Path) -> Result<(), String> {
    let index = dir.join("index");
    let Some((file, len)) = open_index(&index)? else {
        return Ok(());
    };
    let link = validate_index(file, len).map_err(|why| untrusted(&index, &why))?;
    let Some(id) = link else {
        return Ok(());
    };
    let hex: String = id.iter().map(|byte| format!("{byte:02x}")).collect();
    let shared = dir.join(format!("sharedindex.{hex}"));
    let (file, len) = open_index(&shared)?
        .ok_or_else(|| untrusted(&shared, "named by the link extension but missing"))?;
    match validate_index(file, len).map_err(|why| untrusted(&shared, &why))? {
        None => Ok(()),
        Some(_) => Err(untrusted(&shared, "chained link extension")),
    }
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
    check_index(git_dir)?;
    if common != real_git {
        check_git_dir(&common, &mut refs_seen)?;
    }
    check_alternates_chain(&common)?;
    if let Some(workdir) = workdir {
        for (name, cap) in WORKDIR_FILES {
            check_file(&workdir.join(name), cap)?;
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
/// (default `~/.config/git/config`). Same file checks as the pre-open list, 1 MiB cap (256 KiB
/// for the excludes file it names), no includes. Anything unusable is `None` plus a note. A configured file that does not exist is
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
    if let Some(why) = file_problem(&path, PATTERN_FILE_CAP) {
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
    use crate::testgit::{
        add_worktree, commit_file, git, init_bare, init_repo, mkfifo, tempdir_real, within,
    };
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
        info_exclude_file: "info/exclude", PATTERN_FILE_CAP;
        info_attributes_file: "info/attributes", PATTERN_FILE_CAP;
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
        for (name, cap) in WORKDIR_FILES {
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

            make_sparse(&file, cap + 1);
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

        make_sparse(&ignores, PATTERN_FILE_CAP + 1);
        let (found, notes) = excludes(&home, None);
        assert_eq!((found, notes.len()), (None, 1));

        make_fifo(&ignores);
        let owned = home.clone();
        let (found, notes) = within(move || excludes(&owned, None));
        assert_eq!((found, notes.len()), (None, 1));

        make_sparse(&ignores, PATTERN_FILE_CAP);
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

    // ---- revision 12: pattern-file caps, sharedindex.*, index validation -------------------

    #[test]
    fn pattern_files_are_capped_at_256_kib_inclusive() {
        let cap = 256 * 1024;
        for relative in ["info/exclude", "info/attributes"] {
            let (_g, dir) = bare();
            make_sparse(&dir.join(relative), cap);
            assert!(check(&dir).is_ok(), "{relative} at the cap");
            make_sparse(&dir.join(relative), cap + 1);
            expect_untrusted(check(&dir));
        }
        for name in [".gitignore", ".gitattributes"] {
            let (_g, root) = tempdir_real();
            let repo = root.join("repo");
            init_repo(&repo);
            let git_dir = repo.join(".git");
            make_sparse(&repo.join(name), cap);
            assert!(
                pre_open_checks(&git_dir, Some(&repo), None).is_ok(),
                "{name}"
            );
            make_sparse(&repo.join(name), cap + 1);
            expect_untrusted(pre_open_checks(&git_dir, Some(&repo), None));
        }
    }

    #[test]
    fn gitmodules_keeps_its_one_mib_cap() {
        let (_g, root) = tempdir_real();
        let repo = root.join("repo");
        init_repo(&repo);
        let git_dir = repo.join(".git");
        make_sparse(&repo.join(".gitmodules"), MIB);
        assert!(pre_open_checks(&git_dir, Some(&repo), None).is_ok());
    }

    #[test]
    fn sharedindex_entries_follow_the_index_rule() {
        let (_g, dir) = bare();
        let shared = dir.join("sharedindex.x");
        make_symlink(&shared);
        expect_untrusted(check(&dir));

        make_sparse(&shared, INDEX_CAP + 1);
        expect_untrusted(check(&dir));

        make_fifo(&shared);
        let owned = dir.clone();
        expect_untrusted(within(move || check(&owned)));

        remove(&shared);
        assert!(check(&dir).is_ok());
    }

    #[test]
    fn sharedindex_entries_in_linked_admin_dirs_are_checked_from_the_main_repo() {
        let (_g, main, _worktree, admin) = linked();
        make_symlink(&admin.join("sharedindex.y"));
        expect_untrusted(pre_open_checks(&main.join(".git"), Some(&main), None));
    }

    const ZERO_OID: [u8; 20] = [0; 20];
    const SHARED_OID: [u8; 20] = [7; 20];
    const UNTR_FIXED_BYTES: usize = 36 + 36 + 4 + 20 + 20;

    /// git's offset varint (`encode_varint`).
    fn git_varint(mut n: u64) -> Vec<u8> {
        let mut out = vec![(n & 127) as u8];
        n >>= 7;
        while n > 0 {
            n -= 1;
            out.push(128 | (n & 127) as u8);
            n >>= 7;
        }
        out.reverse();
        out
    }

    /// One on-disk entry. v4 names are stored whole (strip the entire previous name).
    fn index_entry(version: u32, name: &[u8], prev: &[u8]) -> Vec<u8> {
        let mut e = vec![0u8; 62];
        e[24..28].copy_from_slice(&0o100644u32.to_be_bytes());
        let flags = name.len().min(0xFFF) as u16;
        e[60..62].copy_from_slice(&flags.to_be_bytes());
        if version == 4 {
            e.extend(git_varint(prev.len() as u64));
            e.extend(name);
            e.push(0);
        } else {
            e.extend(name);
            // One to eight NULs, the first terminating the name.
            let pad = 8 - e.len() % 8;
            e.extend(vec![0u8; pad]);
        }
        e
    }

    fn seal(mut bytes: Vec<u8>) -> Vec<u8> {
        let mut hasher = gix::hash::hasher(gix::hash::Kind::Sha1);
        hasher.update(&bytes);
        bytes.extend_from_slice(hasher.try_finalize().expect("sha1").as_slice());
        bytes
    }

    fn header(version: u32, entries: u32) -> Vec<u8> {
        let mut out = b"DIRC".to_vec();
        out.extend(version.to_be_bytes());
        out.extend(entries.to_be_bytes());
        out
    }

    fn extension(sig: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = sig.to_vec();
        out.extend((data.len() as u32).to_be_bytes());
        out.extend(data);
        out
    }

    fn build_index(version: u32, names: &[&[u8]], exts: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut out = header(version, names.len() as u32);
        let mut prev: &[u8] = b"";
        for name in names {
            out.extend(index_entry(version, name, prev));
            prev = name;
        }
        for (sig, data) in exts {
            out.extend(extension(sig, data));
        }
        seal(out)
    }

    /// `depth` nested `TREE` nodes (the root is level 1), one child each.
    fn tree_chain(depth: usize) -> Vec<u8> {
        let mut data = Vec::new();
        for level in 0..depth {
            let children = usize::from(level + 1 < depth);
            data.extend(if level == 0 { &b""[..] } else { &b"d"[..] });
            data.extend(format!("\0-1 {children}\n").as_bytes());
        }
        data
    }

    fn untr_header(blocks: usize) -> Vec<u8> {
        let mut data = git_varint(0); // empty identifier
        data.extend(vec![0u8; UNTR_FIXED_BYTES]);
        data.push(0); // empty exclude_per_dir
        data.extend(git_varint(blocks as u64));
        data
    }

    /// `depth` nested `UNTR` directory blocks, one sub-directory each.
    fn untr_chain(depth: usize) -> Vec<u8> {
        let mut data = untr_header(depth);
        for level in 0..depth {
            data.push(0); // untracked files
            data.push(u8::from(level + 1 < depth)); // sub-directories
            data.extend(b"d\0");
        }
        data
    }

    fn write_index(dir: &Path, bytes: &[u8]) {
        fs::write(dir.join("index"), bytes).expect("write index");
    }

    fn index_verdict(bytes: &[u8]) -> Result<PathBuf, String> {
        let (_g, dir) = bare();
        write_index(&dir, bytes);
        check(&dir)
    }

    fn link_ext(oid: &[u8; 20]) -> (&'static [u8; 4], Vec<u8>) {
        (b"link", oid.to_vec())
    }

    fn shared_name(oid: &[u8; 20]) -> String {
        let hex: String = oid.iter().map(|b| format!("{b:02x}")).collect();
        format!("sharedindex.{hex}")
    }

    fn plain_index(names: &[&[u8]]) -> Vec<u8> {
        build_index(2, names, &[])
    }

    #[test]
    fn a_plain_index_with_entries_is_trusted_in_every_version() {
        for version in [2, 3, 4] {
            let bytes = build_index(version, &[b"a", b"b/c", b"b/d"], &[]);
            assert!(index_verdict(&bytes).is_ok(), "v{version}");
        }
    }

    #[test]
    fn index_tree_depth_1025_is_untrusted_and_1024_is_not() {
        let deep = build_index(2, &[b"a"], &[(b"TREE", tree_chain(1025))]);
        expect_untrusted(index_verdict(&deep));
        for version in [2, 4] {
            let ok = build_index(version, &[b"a"], &[(b"TREE", tree_chain(1024))]);
            assert!(index_verdict(&ok).is_ok(), "v{version}");
        }
    }

    #[test]
    fn index_untr_depth_1025_is_untrusted_and_1024_is_not() {
        let deep = build_index(2, &[b"a"], &[(b"UNTR", untr_chain(1025))]);
        expect_untrusted(index_verdict(&deep));
        for version in [2, 4] {
            let ok = build_index(version, &[b"a"], &[(b"UNTR", untr_chain(1024))]);
            assert!(index_verdict(&ok).is_ok(), "v{version}");
        }
    }

    #[test]
    fn index_with_both_chains_at_1024_is_trusted() {
        let exts = [(b"TREE", tree_chain(1024)), (b"UNTR", untr_chain(1024))];
        assert!(index_verdict(&build_index(2, &[b"a"], &exts)).is_ok());
    }

    #[test]
    fn index_tree_depth_counts_a_chain_hanging_off_a_later_sibling() {
        // Root with two children; the second carries a chain that reaches level 1,025.
        let mut data = b"\0-1 2\nleaf\0-1 0\n".to_vec();
        let chain = 1024; // levels 2..=1025
        for level in 0..chain {
            let children = usize::from(level + 1 < chain);
            data.extend(format!("d\0-1 {children}\n").as_bytes());
        }
        expect_untrusted(index_verdict(&build_index(2, &[b"a"], &[(b"TREE", data)])));
    }

    #[test]
    fn index_reuc_over_one_mib_is_untrusted() {
        let bytes = build_index(2, &[b"a"], &[(b"REUC", vec![0; 2 * 1024 * 1024])]);
        expect_untrusted(index_verdict(&bytes));
        let at_cap = build_index(2, &[b"a"], &[(b"REUC", vec![0; 1024 * 1024])]);
        assert!(index_verdict(&at_cap).is_ok());
    }

    #[test]
    fn index_tree_cap_is_16_mib_inclusive() {
        let cap = 16 * 1024 * 1024;
        // One root whose path is filler: valid TREE syntax of exactly `cap` bytes.
        let mut exact = vec![b'x'; cap - 6];
        exact.extend(b"\0-1 0\n");
        assert!(index_verdict(&build_index(2, &[b"a"], &[(b"TREE", exact)])).is_ok());

        let mut over = vec![b'x'; cap - 5];
        over.extend(b"\0-1 0\n");
        expect_untrusted(index_verdict(&build_index(2, &[b"a"], &[(b"TREE", over)])));
    }

    #[test]
    fn index_untr_cap_is_4_mib_inclusive() {
        let cap = 4 * 1024 * 1024;
        let mut exact = untr_header(1);
        exact.extend([0u8, 0, 0]); // one empty root block
        exact.resize(cap, 0); // the parser ignores what follows the blocks
        assert!(index_verdict(&build_index(2, &[b"a"], &[(b"UNTR", exact.clone())])).is_ok());

        exact.push(0);
        expect_untrusted(index_verdict(&build_index(2, &[b"a"], &[(b"UNTR", exact)])));
    }

    #[test]
    fn index_entry_past_eof_is_untrusted() {
        let mut bytes = header(2, 1);
        bytes.extend(&index_entry(2, b"file", b"")[..40]);
        expect_untrusted(index_verdict(&seal(bytes)));
    }

    #[test]
    fn index_entry_count_beyond_the_entries_present_is_untrusted() {
        let mut bytes = plain_index(&[b"a"]);
        bytes[8..12].copy_from_slice(&2u32.to_be_bytes());
        expect_untrusted(index_verdict(&bytes));
    }

    #[test]
    fn index_extension_running_past_the_trailer_is_untrusted() {
        let mut body = header(2, 0);
        body.extend(b"TREE");
        body.extend(1000u32.to_be_bytes());
        body.extend(b"short");
        expect_untrusted(index_verdict(&seal(body)));
    }

    #[test]
    fn index_name_over_4096_bytes_is_untrusted_and_4096_is_not() {
        let long = vec![b'n'; 4097];
        expect_untrusted(index_verdict(&plain_index(&[&long])));
        expect_untrusted(index_verdict(&build_index(4, &[&long], &[])));
        assert!(index_verdict(&build_index(4, &[&vec![b'n'; 4096]], &[])).is_ok());
        assert!(index_verdict(&plain_index(&[&vec![b'n'; 4094]])).is_ok());
    }

    #[test]
    fn index_v2_saturated_name_length_field_is_untrusted() {
        // gix and git disagree on the padding after such a name, hence on where extensions start.
        expect_untrusted(index_verdict(&plain_index(&[&vec![b'n'; 4095]])));
        expect_untrusted(index_verdict(&plain_index(&[&vec![b'n'; 4096]])));
    }

    #[test]
    fn index_v4_strip_longer_than_the_previous_name_is_untrusted() {
        let mut bytes = header(4, 2);
        bytes.extend(index_entry(4, b"a", b""));
        bytes.extend(index_entry(4, b"b", b"aaaaaaaa")); // strips 8 from a 1-byte name
        expect_untrusted(index_verdict(&seal(bytes)));
    }

    #[test]
    fn index_with_a_bad_header_is_untrusted() {
        let mut bad_magic = header(2, 0);
        bad_magic[0] = b'X';
        expect_untrusted(index_verdict(&seal(bad_magic)));
        expect_untrusted(index_verdict(&seal(header(5, 0))));
        expect_untrusted(index_verdict(b"DIRC"));
    }

    #[test]
    fn index_duplicate_capped_extension_is_untrusted() {
        let exts = [(b"TREE", tree_chain(1)), (b"TREE", tree_chain(1))];
        expect_untrusted(index_verdict(&build_index(2, &[b"a"], &exts)));
    }

    #[test]
    fn index_empty_name_needs_a_link_extension() {
        expect_untrusted(index_verdict(&plain_index(&[b""])));
    }

    /// A git dir whose index links to the `SHARED_OID` shared index, written when given.
    fn split_repo(shared: Option<&[u8]>) -> (tempfile::TempDir, PathBuf) {
        let (guard, dir) = bare();
        write_index(&dir, &build_index(2, &[b""], &[link_ext(&SHARED_OID)]));
        if let Some(bytes) = shared {
            fs::write(dir.join(shared_name(&SHARED_OID)), bytes).expect("write shared");
        }
        (guard, dir)
    }

    #[test]
    fn a_valid_split_index_is_trusted_even_with_a_replaced_empty_name() {
        let (_g, dir) = split_repo(Some(&plain_index(&[b"a", b"b"])));
        assert!(check(&dir).is_ok());
    }

    #[test]
    fn a_link_to_a_missing_sharedindex_is_untrusted() {
        let (_g, dir) = split_repo(None);
        expect_untrusted(check(&dir));
    }

    #[test]
    fn a_link_to_a_symlinked_sharedindex_is_untrusted() {
        let (_g, dir) = split_repo(None);
        make_symlink(&dir.join(shared_name(&SHARED_OID)));
        expect_untrusted(check(&dir));
    }

    #[test]
    fn a_link_to_a_fifo_sharedindex_is_untrusted_without_blocking() {
        let (_g, dir) = split_repo(None);
        make_fifo(&dir.join(shared_name(&SHARED_OID)));
        expect_untrusted(within(move || check(&dir)));
    }

    #[test]
    fn a_chained_link_is_untrusted() {
        let chained = build_index(2, &[b"a"], &[link_ext(&ZERO_OID)]);
        let (_g, dir) = split_repo(Some(&chained));
        expect_untrusted(check(&dir));
    }

    #[test]
    fn a_bad_sharedindex_body_is_untrusted() {
        let bomb = build_index(2, &[b"a"], &[(b"TREE", tree_chain(1025))]);
        let (_g, dir) = split_repo(Some(&bomb));
        expect_untrusted(check(&dir));
    }

    #[test]
    fn a_link_shorter_than_an_object_id_is_untrusted() {
        let bytes = build_index(2, &[b"a"], &[(b"link", vec![1; 10])]);
        expect_untrusted(index_verdict(&bytes));
    }

    #[test]
    fn a_linked_worktrees_admin_dir_index_is_validated() {
        let (_g, main, worktree, admin) = linked();
        let bomb = build_index(2, &[b"a"], &[(b"TREE", tree_chain(1025))]);
        fs::write(admin.join("index"), bomb).unwrap();
        expect_untrusted(pre_open_checks(
            &admin,
            Some(&worktree),
            Some(&main.join(".git")),
        ));
    }

    fn contains_signature(bytes: &[u8], sig: &[u8; 4]) -> bool {
        bytes.windows(4).any(|w| w == sig)
    }

    #[test]
    fn a_real_git_index_with_tree_reuc_and_untr_is_trusted() {
        let (_g, root) = tempdir_real();
        let repo = root.join("repo");
        init_repo(&repo);
        git(&repo, &["config", "core.untrackedCache", "true"]);
        git(&repo, &["checkout", "-q", "-b", "side"]);
        commit_file(&repo, "README", "side\n", "side");
        git(&repo, &["checkout", "-q", "main"]);
        commit_file(&repo, "README", "main\n", "main");
        // `git()` asserts success and a conflicting merge exits 1, so run it inside an alias.
        git(&repo, &["-c", "alias.m=!git merge side; true", "m"]);
        // Resolving the conflict with `git add` records the REUC extension.
        fs::write(repo.join("README"), "resolved\n").unwrap();
        git(&repo, &["add", "README"]);
        git(&repo, &["write-tree"]);
        fs::create_dir_all(repo.join("untracked/deep")).unwrap();
        fs::write(repo.join("untracked/deep/file"), "x").unwrap();
        git(&repo, &["update-index", "--untracked-cache"]);
        git(&repo, &["status", "-s"]);
        git(&repo, &["status", "-s"]);

        let bytes = fs::read(repo.join(".git/index")).unwrap();
        for sig in [b"TREE", b"REUC", b"UNTR"] {
            assert!(
                contains_signature(&bytes, sig),
                "{} missing from the real index",
                String::from_utf8_lossy(sig)
            );
        }
        assert!(pre_open_checks(&repo.join(".git"), Some(&repo), None).is_ok());
    }

    #[test]
    fn a_real_git_split_index_repo_is_trusted() {
        for version in ["2", "4"] {
            let (_g, root) = tempdir_real();
            let repo = root.join("repo");
            init_repo(&repo);
            commit_file(&repo, "second", "2\n", "second");
            git(&repo, &["update-index", "--index-version", version]);
            git(&repo, &["update-index", "--split-index"]);
            // A tracked change after the split leaves a replaced (empty-name) entry behind.
            fs::write(repo.join("second"), "changed\n").unwrap();
            git(&repo, &["add", "second"]);

            let git_dir = repo.join(".git");
            let has_shared = fs::read_dir(&git_dir)
                .unwrap()
                .filter_map(Result::ok)
                .any(|e| e.file_name().to_string_lossy().starts_with("sharedindex."));
            assert!(has_shared, "v{version}: git wrote no sharedindex file");
            let index = fs::read(git_dir.join("index")).unwrap();
            assert!(contains_signature(&index, b"link"), "v{version}: no link");
            assert!(
                pre_open_checks(&git_dir, Some(&repo), None).is_ok(),
                "v{version}"
            );
        }
    }
}
