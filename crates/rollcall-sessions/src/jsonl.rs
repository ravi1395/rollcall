//! Log file I/O shared by the Claude and Codex rollout sources (§5.0, §6.2, §6.3).

use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::Path;

use rollcall_core::{FileState, TextRef};

/// Lines longer than this are skipped and counted, never buffered (§5.0).
pub const MAX_LINE_BYTES: usize = 1 << 20;
const CHUNK_BYTES: usize = 8 * 1024;
const HEAD_FP_MAX_BYTES: usize = 4 * 1024;
const TAIL_FP_BYTES: u64 = 64;

/// Where `scan` starts reading a file it has seen before (§6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadFrom {
    Start,
    Offset(u64),
}

/// A complete line: `offset` of its first byte, `bytes` without the trailing `\n`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub offset: u64,
    pub bytes: Vec<u8>,
}

/// Reads complete lines from `start` up to `end` (the size observed at open, §5.0) with `pread`.
pub struct LineReader<'a> {
    file: &'a File,
    end: u64,
    read_pos: u64,
    chunk: Vec<u8>,
    chunk_pos: usize,
    chunk_len: usize,
    /// The current line so far; empty while skipping an oversize line.
    buf: Vec<u8>,
    /// Offset of the first byte of the line being assembled: the resume point.
    line_start: u64,
    /// Bytes of the current line seen so far, buffered or skipped.
    line_len: usize,
    skipping: bool,
    /// Complete lines over `MAX_LINE_BYTES` skipped so far.
    pub oversize: u32,
    #[cfg(test)]
    pub high_water: usize,
}

/// Opens a session log without following symlinks or blocking on FIFOs; requires a regular file.
pub fn open_log(path: &Path) -> io::Result<(File, Metadata)> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok((file, meta))
}

impl<'a> LineReader<'a> {
    pub fn new(file: &'a File, start: u64, end: u64) -> Self {
        LineReader {
            file,
            end,
            read_pos: start,
            chunk: vec![0; CHUNK_BYTES],
            chunk_pos: 0,
            chunk_len: 0,
            buf: Vec::new(),
            line_start: start,
            line_len: 0,
            skipping: false,
            oversize: 0,
            #[cfg(test)]
            high_water: 0,
        }
    }

    /// The next complete line, or `None` at the end; a partial trailing line is left unread.
    pub fn next_line(&mut self) -> io::Result<Option<Line>> {
        loop {
            if self.chunk_pos == self.chunk_len && !self.refill()? {
                return Ok(None);
            }
            let data = &self.chunk[self.chunk_pos..self.chunk_len];
            let newline = data.iter().position(|&b| b == b'\n');
            let piece = &data[..newline.unwrap_or(data.len())];
            self.line_len += piece.len();
            if !self.skipping && self.line_len > MAX_LINE_BYTES {
                self.skipping = true;
                self.buf.clear();
            }
            if !self.skipping {
                self.buf.extend_from_slice(piece);
                #[cfg(test)]
                {
                    self.high_water = self.high_water.max(self.buf.len());
                }
            }
            self.chunk_pos += piece.len() + usize::from(newline.is_some());
            if newline.is_none() {
                continue;
            }
            let offset = self.line_start;
            self.line_start += self.line_len as u64 + 1;
            self.line_len = 0;
            if std::mem::take(&mut self.skipping) {
                self.oversize += 1;
                continue;
            }
            return Ok(Some(Line {
                offset,
                bytes: std::mem::take(&mut self.buf),
            }));
        }
    }

    /// Offset just past the last complete line: where the next scan resumes.
    pub fn offset(&self) -> u64 {
        self.line_start
    }

    /// Loads the next chunk; `false` at `end`, or if the file shrank underneath us.
    fn refill(&mut self) -> io::Result<bool> {
        let want = self
            .end
            .saturating_sub(self.read_pos)
            .min(CHUNK_BYTES as u64) as usize;
        if want == 0 {
            return Ok(false);
        }
        let n = self.file.read_at(&mut self.chunk[..want], self.read_pos)?;
        self.read_pos += n as u64;
        self.chunk_pos = 0;
        self.chunk_len = n;
        Ok(n > 0)
    }
}

/// First 16 bytes of the SHA-1 of `bytes`; a detected collision is an error so callers re-read.
fn fingerprint(bytes: &[u8]) -> io::Result<[u8; 16]> {
    let mut hasher = gix_hash::hasher(gix_hash::Kind::Sha1);
    hasher.update(bytes);
    let id = hasher
        .try_finalize()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let mut fp = [0u8; 16];
    fp.copy_from_slice(&id.as_bytes()[..16]);
    Ok(fp)
}

/// Reads up to `len` bytes at `offset`; shorter only at EOF.
fn read_up_to(file: &File, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0; len];
    let mut filled = 0;
    while filled < len {
        let n = file.read_at(&mut buf[filled..], offset + filled as u64)?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    buf.truncate(filled);
    Ok(buf)
}

/// Fingerprint of the first line (without its newline), at most 4 KiB.
pub fn head_fp(file: &File) -> io::Result<[u8; 16]> {
    let head = read_up_to(file, 0, HEAD_FP_MAX_BYTES)?;
    let line = head.split(|&b| b == b'\n').next().unwrap_or_default();
    fingerprint(line)
}

/// Fingerprint of the 64 bytes ending at `offset` (fewer when `offset < 64`).
pub fn tail_fp(file: &File, offset: u64) -> io::Result<[u8; 16]> {
    let len = offset.min(TAIL_FP_BYTES);
    let tail = read_up_to(file, offset - len, len as usize)?;
    if tail.len() as u64 != len {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    fingerprint(&tail)
}

/// Decides where to resume reading a file (§6.3). Any doubt, including an unreadable or
/// colliding fingerprint, means re-read from the start.
pub fn plan_read(prior: Option<&FileState>, file: &File, meta: &Metadata) -> ReadFrom {
    let Some(prior) = prior else {
        return ReadFrom::Start;
    };
    let unchanged = prior.parser_version == crate::PARSER_VERSION
        && prior.dev == meta.dev()
        && prior.inode == meta.ino()
        && meta.len() >= prior.offset
        && head_fp(file).is_ok_and(|fp| fp == prior.head_fp)
        && tail_fp(file, prior.offset).is_ok_and(|fp| fp == prior.tail_fp);
    if unchanged {
        ReadFrom::Offset(prior.offset)
    } else {
        ReadFrom::Start
    }
}

/// The raw line a `TextRef::LogLine` points at, only if the file is still the one scanned (§6.2).
pub fn resolve_log_ref(r: &TextRef, path: &Path) -> Option<Vec<u8>> {
    let TextRef::LogLine {
        dev,
        inode,
        size_at_scan,
        offset,
        len,
        ..
    } = *r
    else {
        return None;
    };
    let len = len as usize;
    let (file, meta) = open_log(path).ok()?;
    let in_bounds = offset
        .checked_add(len as u64)
        .is_some_and(|end| end <= meta.len());
    if meta.dev() != dev
        || meta.ino() != inode
        || meta.len() < size_at_scan
        || len > MAX_LINE_BYTES
        || !in_bounds
    {
        return None;
    }
    let mut buf = vec![0; len];
    file.read_exact_at(&mut buf, offset).ok()?;
    Some(buf)
}

/// Whether the file a state describes is still at its path: the vanished check (§5.0).
pub fn file_exists_same(state: &FileState) -> bool {
    open_log(&state.path)
        .is_ok_and(|(_, meta)| meta.dev() == state.dev && meta.ino() == state.inode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rollcall_core::{TextField, Timestamp, Uuid};
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, symlink};
    use std::process::Command;
    use std::sync::mpsc;
    use std::time::Duration;

    const WATCHDOG: Duration = Duration::from_secs(5);
    const HIGH_WATER_SLACK: usize = 8 * 1024;
    const ZERO_UUID: &str = "00000000-0000-0000-0000-000000000000";

    fn write(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
    }

    fn append(path: &Path, bytes: &[u8]) {
        let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(bytes).unwrap();
    }

    /// Every complete line of the file plus the reader's final state.
    fn read_all(path: &Path) -> (Vec<Line>, u64, u32, usize) {
        let (file, meta) = open_log(path).unwrap();
        let mut r = LineReader::new(&file, 0, meta.len());
        let mut lines = Vec::new();
        while let Some(l) = r.next_line().unwrap() {
            lines.push(l);
        }
        (lines, r.offset(), r.oversize, r.high_water)
    }

    /// FileState as a completed scan of the whole file would record it.
    fn state_at(path: &Path, offset: u64) -> FileState {
        let (file, meta) = open_log(path).unwrap();
        FileState {
            dev: meta.dev(),
            inode: meta.ino(),
            path: path.to_path_buf(),
            size: meta.len(),
            mtime: Timestamp::UNIX_EPOCH,
            offset,
            head_fp: head_fp(&file).unwrap(),
            tail_fp: tail_fp(&file, offset).unwrap(),
            parser_version: crate::PARSER_VERSION,
        }
    }

    fn plan(prior: &FileState, path: &Path) -> ReadFrom {
        let (file, meta) = open_log(path).unwrap();
        plan_read(Some(prior), &file, &meta)
    }

    fn log_ref(path: &Path, size_at_scan: u64, offset: u64, len: u32) -> TextRef {
        let meta = fs::metadata(path).unwrap();
        TextRef::LogLine {
            dev: meta.dev(),
            inode: meta.ino(),
            size_at_scan,
            offset,
            len,
            field: TextField::Title,
        }
    }

    fn on_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || tx.send(f()).ok());
        rx.recv_timeout(WATCHDOG)
            .expect("blocked past the watchdog")
    }

    /// First line short, the rest long enough that the 64-byte tail window never touches it.
    fn body() -> Vec<u8> {
        let mut b = b"first-line\n".to_vec();
        for i in 0..20 {
            b.extend_from_slice(format!("record number {i:04} padding padding\n").as_bytes());
        }
        b
    }

    const HELLO_FILE: &[u8] = b"aaa\nhello world\nzzz\n";

    #[test]
    fn three_lines_and_a_partial() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        write(&p, b"one\ntwo\nthree\npart");
        let (lines, offset, _, _) = read_all(&p);
        let got: Vec<_> = lines.iter().map(|l| (l.offset, l.bytes.clone())).collect();
        assert_eq!(
            got,
            vec![
                (0, b"one".to_vec()),
                (4, b"two".to_vec()),
                (8, b"three".to_vec())
            ]
        );
        assert_eq!(offset, 14, "offset stays before the partial line");
    }

    #[test]
    fn partial_line_is_picked_up_once_completed() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        write(&p, b"one\npart");
        assert_eq!(read_all(&p).1, 4);
        append(&p, b"ial\n");
        let (file, meta) = open_log(&p).unwrap();
        let mut r = LineReader::new(&file, 4, meta.len());
        let l = r.next_line().unwrap().unwrap();
        assert_eq!((l.offset, l.bytes), (4, b"partial".to_vec()));
        assert_eq!(r.offset(), 12);
    }

    #[test]
    fn empty_file_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        write(&p, b"");
        let (lines, offset, _, _) = read_all(&p);
        assert!(lines.is_empty());
        assert_eq!(offset, 0);
    }

    #[test]
    fn empty_lines_are_lines() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        write(&p, b"\n\nx\n");
        let (lines, _, _, _) = read_all(&p);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].bytes.is_empty());
    }

    #[test]
    fn never_reads_past_size_at_open() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        write(&p, b"one\ntwo\n");
        let (file, meta) = open_log(&p).unwrap();
        append(&p, b"three\nfour\n");
        let mut r = LineReader::new(&file, 0, meta.len());
        let mut n = 0;
        while r.next_line().unwrap().is_some() {
            n += 1;
        }
        assert_eq!(n, 2);
        assert_eq!(r.offset(), 8);
    }

    #[test]
    fn lines_spanning_many_chunks_are_reassembled() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        let big = vec![b'x'; 100_000];
        let mut data = big.clone();
        data.push(b'\n');
        data.extend_from_slice(b"tail\n");
        write(&p, &data);
        let (lines, _, oversize, _) = read_all(&p);
        assert_eq!(lines[0].bytes, big);
        assert_eq!(lines[1].offset, 100_001);
        assert_eq!(oversize, 0);
    }

    #[test]
    fn oversize_line_between_small_lines_is_skipped_and_counted() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        let mut data = b"before\n".to_vec();
        data.extend(std::iter::repeat_n(b'z', 2 << 20));
        data.extend_from_slice(b"\nafter\n");
        write(&p, &data);
        let (lines, _, oversize, high_water) = read_all(&p);
        let got: Vec<_> = lines.iter().map(|l| l.bytes.clone()).collect();
        assert_eq!(got, vec![b"before".to_vec(), b"after".to_vec()]);
        assert_eq!(oversize, 1);
        assert_eq!(lines[1].offset, 7 + (2 << 20) as u64 + 1);
        assert!(
            high_water <= MAX_LINE_BYTES + HIGH_WATER_SLACK,
            "buffered {high_water} bytes"
        );
    }

    #[test]
    fn line_of_exactly_the_cap_is_kept_one_over_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        let mut data = vec![b'a'; MAX_LINE_BYTES];
        data.push(b'\n');
        data.extend(std::iter::repeat_n(b'b', MAX_LINE_BYTES + 1));
        data.push(b'\n');
        write(&p, &data);
        let (lines, _, oversize, _) = read_all(&p);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].bytes.len(), MAX_LINE_BYTES);
        assert_eq!(oversize, 1);
    }

    #[test]
    fn oversize_partial_tail_is_not_counted_and_offset_stays_before_it() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        let mut data = b"ok\n".to_vec();
        data.extend(std::iter::repeat_n(b'q', 2 << 20));
        write(&p, &data);
        let (lines, offset, oversize, _) = read_all(&p);
        assert_eq!(lines.len(), 1);
        assert_eq!(oversize, 0);
        assert_eq!(offset, 3);
    }

    #[test]
    fn plan_without_prior_starts_at_zero() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        write(&p, &body());
        let (file, meta) = open_log(&p).unwrap();
        assert_eq!(plan_read(None, &file, &meta), ReadFrom::Start);
    }

    #[test]
    fn plan_is_incremental_when_only_appended() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        let b = body();
        write(&p, &b);
        let prior = state_at(&p, b.len() as u64);
        append(&p, b"new line\n");
        assert_eq!(plan(&prior, &p), ReadFrom::Offset(b.len() as u64));
    }

    #[test]
    fn plan_rereads_after_truncate() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        let b = body();
        write(&p, &b);
        let prior = state_at(&p, b.len() as u64);
        write(&p, b"first-line\n");
        assert_eq!(plan(&prior, &p), ReadFrom::Start);
    }

    #[test]
    fn plan_rereads_after_truncate_then_regrow_via_tail_fp() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        let b = body();
        write(&p, &b);
        let prior = state_at(&p, b.len() as u64);
        // Same inode, same first line, larger than before, different bytes at the old offset.
        let mut regrown = b"first-line\n".to_vec();
        while regrown.len() <= b.len() + 100 {
            regrown.extend_from_slice(b"completely different content here!!\n");
        }
        write(&p, &regrown);
        let meta = fs::metadata(&p).unwrap();
        assert_eq!(meta.ino(), prior.inode);
        assert!(
            meta.len() >= prior.offset,
            "size check alone must not catch this"
        );
        assert_eq!(plan(&prior, &p), ReadFrom::Start);
    }

    #[test]
    fn plan_rereads_when_head_changes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        let b = body();
        write(&p, &b);
        let prior = state_at(&p, b.len() as u64);
        let mut changed = b.clone();
        changed[0] = b'F';
        write(&p, &changed);
        assert_eq!(plan(&prior, &p), ReadFrom::Start);
    }

    #[test]
    fn plan_rereads_on_older_parser_version() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        let b = body();
        write(&p, &b);
        let mut prior = state_at(&p, b.len() as u64);
        prior.parser_version = crate::PARSER_VERSION - 1;
        assert_eq!(plan(&prior, &p), ReadFrom::Start);
    }

    #[test]
    fn plan_rereads_for_a_different_inode() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        let b = body();
        write(&p, &b);
        let mut prior = state_at(&p, b.len() as u64);
        prior.inode += 1;
        assert_eq!(plan(&prior, &p), ReadFrom::Start);
    }

    #[test]
    fn plan_is_incremental_after_rename_to_a_new_path() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("live.jsonl");
        let new = dir.path().join("archived.jsonl");
        let b = body();
        write(&old, &b);
        let prior = state_at(&old, b.len() as u64);
        fs::rename(&old, &new).unwrap();
        append(&new, b"more\n");
        assert_eq!(plan(&prior, &new), ReadFrom::Offset(b.len() as u64));
    }

    #[test]
    fn fingerprints_cover_the_documented_windows() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        // Identical first 4 KiB of a long first line, different afterwards.
        let mut da = vec![b'h'; 5000];
        let mut db = da.clone();
        da.extend_from_slice(b"AAA\n");
        db.extend_from_slice(b"BBB\n");
        write(&a, &da);
        write(&b, &db);
        let (fa, fb) = (File::open(&a).unwrap(), File::open(&b).unwrap());
        assert_eq!(head_fp(&fa).unwrap(), head_fp(&fb).unwrap());
        // Tail is the 64 bytes ending at the offset: shifting the offset changes it.
        let end = da.len() as u64;
        assert_ne!(tail_fp(&fa, end).unwrap(), tail_fp(&fa, end - 1).unwrap());
        assert_ne!(tail_fp(&fa, end).unwrap(), tail_fp(&fb, end).unwrap());
        // Offsets below 64 hash what exists; offset 0 is defined.
        assert_ne!(tail_fp(&fa, 10).unwrap(), tail_fp(&fa, 0).unwrap());
        // An offset past EOF cannot be fingerprinted.
        assert!(tail_fp(&fa, end + 100).is_err());
    }

    #[test]
    fn open_log_rejects_a_fifo_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("pipe.jsonl");
        assert!(Command::new("mkfifo").arg(&p).status().unwrap().success());
        let r = on_thread(move || open_log(&p).map(|_| ()));
        assert!(r.is_err());
    }

    #[test]
    fn open_log_rejects_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.jsonl");
        let link = dir.path().join("link.jsonl");
        write(&real, b"x\n");
        symlink(&real, &link).unwrap();
        let r = on_thread(move || open_log(&link).map(|_| ()));
        assert!(r.is_err());
        assert!(open_log(&real).is_ok());
    }

    #[test]
    fn open_log_rejects_a_directory_and_a_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        assert!(open_log(dir.path()).is_err());
        assert!(open_log(&dir.path().join("nope")).is_err());
    }

    #[test]
    fn resolve_returns_the_line_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        write(&p, HELLO_FILE);
        let r = log_ref(&p, HELLO_FILE.len() as u64, 4, 11);
        assert_eq!(resolve_log_ref(&r, &p), Some(b"hello world".to_vec()));
        // Stable after unrelated appends.
        append(&p, b"more\n");
        assert_eq!(resolve_log_ref(&r, &p), Some(b"hello world".to_vec()));
    }

    #[test]
    fn resolve_is_none_for_a_new_inode() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        write(&p, HELLO_FILE);
        let r = log_ref(&p, HELLO_FILE.len() as u64, 4, 11);
        let tmp = dir.path().join("b.jsonl");
        write(&tmp, HELLO_FILE);
        fs::rename(&tmp, &p).unwrap();
        assert_eq!(resolve_log_ref(&r, &p), None);
    }

    #[test]
    fn resolve_is_none_when_the_file_shrank_below_size_at_scan() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        write(&p, HELLO_FILE);
        let size = HELLO_FILE.len() as u64;
        let r = log_ref(&p, size, 4, 11);
        let f = fs::OpenOptions::new().write(true).open(&p).unwrap();
        f.set_len(size - 1).unwrap();
        assert_eq!(resolve_log_ref(&r, &p), None);
    }

    #[test]
    fn resolve_is_none_when_a_symlink_is_swapped_in() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        let other = dir.path().join("other.jsonl");
        write(&p, HELLO_FILE);
        write(&other, HELLO_FILE);
        let size = HELLO_FILE.len() as u64;
        let r = log_ref(&p, size, 4, 11);
        fs::remove_file(&p).unwrap();
        symlink(&other, &p).unwrap();
        assert_eq!(resolve_log_ref(&r, &p), None);
        // Even the target's own inode is refused when reached through the symlink.
        assert_eq!(resolve_log_ref(&log_ref(&other, size, 4, 11), &p), None);
    }

    #[test]
    fn resolve_is_none_for_out_of_range_oversize_and_foreign_refs() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        write(&p, HELLO_FILE);
        let size = HELLO_FILE.len() as u64;
        assert_eq!(resolve_log_ref(&log_ref(&p, size, 4, 500), &p), None);
        assert_eq!(resolve_log_ref(&log_ref(&p, size, u64::MAX, 11), &p), None);
        let too_long = MAX_LINE_BYTES as u32 + 1;
        assert_eq!(resolve_log_ref(&log_ref(&p, size, 0, too_long), &p), None);
        assert_eq!(
            resolve_log_ref(&log_ref(&p, size, 4, 11), &dir.path().join("gone")),
            None
        );
        let thread = TextRef::CodexThread {
            thread_id: Uuid::parse(ZERO_UUID).unwrap(),
            field: TextField::Title,
        };
        assert_eq!(resolve_log_ref(&thread, &p), None);
    }

    #[test]
    fn vanished_and_replaced_files_are_not_the_same_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        let b = body();
        write(&p, &b);
        let st = state_at(&p, b.len() as u64);
        assert!(file_exists_same(&st));
        fs::remove_file(&p).unwrap();
        assert!(!file_exists_same(&st), "vanished");
        write(&p, &b);
        assert!(!file_exists_same(&st), "new inode at the same path");
    }
}
