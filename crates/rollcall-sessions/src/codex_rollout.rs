//! Codex source dispatch: the thread DB (`codex_db`) when present, else rollout files (§5.2).
//!
//! Rollouts supply `last_prompt`, `message_count`, the branch fallback and the `parent_thread_id`
//! child rule in database mode, and everything in rollouts-only mode. Every rollout is read
//! incrementally per file (§6.3).

#[cfg(test)]
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use rollcall_core::text::filter_prompt_block;
use rollcall_core::{
    Backing, FileState, Result, Session, SkippedCounts, SourceDiagnostics, SourceKind, SourceMode,
    SourceScan, SourceState, TextField, TextRef, Timestamp, TitleKind, Uuid,
};
use serde::Deserialize;
use serde_json::Value;

use crate::PARSER_VERSION;
use crate::codex_db;
use crate::jsonl::{self, Line, LineReader, ReadFrom};

const SESSIONS_DIR: &str = "sessions";
const ARCHIVED_DIR: &str = "archived_sessions";
const ROLLOUT_PREFIX: &str = "rollout-";
const JSONL_EXTENSION: &str = "jsonl";

const SESSION_META: &str = "session_meta";
const TURN_CONTEXT: &str = "turn_context";
const EVENT_MSG: &str = "event_msg";
const RESPONSE_ITEM: &str = "response_item";
const USER_MESSAGE: &str = "user_message";

type Inode = (u64, u64);

#[cfg(test)]
thread_local! {
    /// Tree walks this thread has done; the resolve-after-scan test asserts it does not move.
    static WALKS: Cell<u32> = const { Cell::new(0) };
}

/// Where a human prompt was found. A file's prompts come from one origin only (§5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// `event_msg` / `user_message`.
    Event,
    /// `response_item` message with role `user`; only used when a file has no events.
    Response,
}

/// An `event_msg` / `user_message` record, whether or not §5.0 leaves a prompt in it: its
/// presence alone makes a file an events file (§5.2).
fn is_user_message_event(record: &Value) -> bool {
    record.get("type").and_then(Value::as_str) == Some(EVENT_MSG)
        && record
            .get("payload")
            .and_then(|p| p.get("type"))
            .and_then(Value::as_str)
            == Some(USER_MESSAGE)
}

/// The human prompt on one rollout line, if any. The single definition of a prompt: `scan` counts
/// with it and `resolve` re-applies it (§6.2).
fn prompt_of(record: &Value) -> Option<(Origin, String)> {
    let payload = record.get("payload")?;
    let payload_kind = payload.get("type")?.as_str()?;
    match (record.get("type")?.as_str()?, payload_kind) {
        (EVENT_MSG, USER_MESSAGE) => {
            // §5.0 is common to every source: the message is one text block.
            let message = filter_prompt_block(payload.get("message")?.as_str()?)?;
            Some((Origin::Event, message))
        }
        (RESPONSE_ITEM, "message")
            if payload.get("role").and_then(Value::as_str) == Some("user") =>
        {
            let blocks = payload.get("content")?.as_array()?;
            let kept: Vec<String> = blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("input_text"))
                .filter_map(|b| filter_prompt_block(b.get("text")?.as_str()?))
                .collect();
            (!kept.is_empty()).then(|| (Origin::Response, kept.join(" ")))
        }
        _ => None,
    }
}

fn prompt_of_line(line: &[u8]) -> Option<(Origin, String)> {
    prompt_of(&serde_json::from_slice(line).ok()?)
}

// ---- one pass over a rollout ----

/// Just enough of a record to count it; the payload is skipped without being built.
#[derive(Deserialize)]
struct Head {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct Span {
    offset: u64,
    len: u32,
}

/// Human prompts of one origin found in a pass.
#[derive(Debug, Default)]
struct Prompts {
    count: u32,
    first: Option<Span>,
    last: Option<Span>,
}

impl Prompts {
    fn push(&mut self, span: Span) {
        self.count += 1;
        self.first.get_or_insert(span);
        self.last = Some(span);
    }
}

/// The first `session_meta` of a file.
#[derive(Debug)]
struct Meta {
    id: Option<String>,
    started_at: Option<Timestamp>,
    cwd: Option<PathBuf>,
    branch: Option<String>,
}

#[derive(Debug, Default)]
struct Pass {
    meta: Option<Meta>,
    turn_cwd: Option<PathBuf>,
    first_ts: Option<Timestamp>,
    last_ts: Option<Timestamp>,
    events: Prompts,
    responses: Prompts,
    /// Any `user_message` event was seen, even one that filtered to nothing.
    event_seen: bool,
    records: u64,
    skipped: SkippedCounts,
    /// `session_meta.payload.parent_thread_id` is set: reading stopped at that record.
    child: bool,
    /// Where the next scan resumes.
    offset: u64,
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn is_set(value: &Value) -> bool {
    !value.is_null() && value.as_str().is_none_or(|s| !s.is_empty())
}

fn contains(line: &[u8], needle: &str) -> bool {
    std::str::from_utf8(line).is_ok_and(|s| s.contains(needle))
}

impl Pass {
    /// `meta_open`: no `session_meta` has been taken yet (a fresh read; an incremental read
    /// already has the head of the file in the prior session).
    fn absorb(&mut self, line: &Line, meta_open: &mut bool) {
        if line.bytes.iter().all(u8::is_ascii_whitespace) {
            return;
        }
        let head = match serde_json::from_slice::<Head>(&line.bytes) {
            Ok(Head {
                timestamp,
                kind: Some(kind),
            }) => (timestamp, kind),
            _ => {
                self.skipped.malformed += 1;
                return;
            }
        };
        let (timestamp, kind) = head;
        self.records += 1;
        let at = timestamp.and_then(|t| t.parse::<Timestamp>().ok());
        if let Some(at) = at {
            self.first_ts = Some(self.first_ts.map_or(at, |t| t.min(at)));
            self.last_ts = Some(self.last_ts.map_or(at, |t| t.max(at)));
        }
        // Prompt lines are the only bulk records worth a full parse; a cheap substring test
        // skips the rest. A false positive costs one parse.
        // ponytail: a `\u`-escaped "user" / "user_message" would be missed; Codex never escapes
        // ASCII, so upgrade to a full parse of every record only if that ever changes.
        let needs_parse = match kind.as_str() {
            SESSION_META | TURN_CONTEXT => true,
            EVENT_MSG => contains(&line.bytes, USER_MESSAGE),
            RESPONSE_ITEM => contains(&line.bytes, "\"user\""),
            _ => {
                self.skipped.unknown_type += 1;
                return;
            }
        };
        if !needs_parse {
            return;
        }
        let Ok(record) = serde_json::from_slice::<Value>(&line.bytes) else {
            self.skipped.malformed += 1;
            return;
        };
        match kind.as_str() {
            SESSION_META => self.take_meta(&record, at, meta_open),
            TURN_CONTEXT => {
                if let Some(cwd) = record.get("payload").and_then(|p| string_field(p, "cwd")) {
                    self.turn_cwd = Some(PathBuf::from(cwd));
                }
            }
            _ => self.take_prompt(&record, line),
        }
    }

    /// A second `session_meta` is ignored: the session belongs to the first (§5.2).
    fn take_meta(&mut self, record: &Value, at: Option<Timestamp>, meta_open: &mut bool) {
        if !std::mem::take(meta_open) {
            return;
        }
        let Some(payload) = record.get("payload") else {
            return;
        };
        self.child = payload.get("parent_thread_id").is_some_and(is_set);
        self.meta = Some(Meta {
            id: string_field(payload, "id"),
            started_at: string_field(payload, "timestamp")
                .and_then(|t| t.parse().ok())
                .or(at),
            cwd: string_field(payload, "cwd").map(PathBuf::from),
            branch: payload.get("git").and_then(|g| string_field(g, "branch")),
        });
    }

    fn take_prompt(&mut self, record: &Value, line: &Line) {
        self.event_seen |= is_user_message_event(record);
        let Some((origin, _)) = prompt_of(record) else {
            return;
        };
        let span = Span {
            offset: line.offset,
            // Lines are capped at 1 MiB by the reader.
            len: u32::try_from(line.bytes.len()).unwrap_or(u32::MAX),
        };
        match origin {
            Origin::Event => self.events.push(span),
            Origin::Response => self.responses.push(span),
        }
    }
}

/// Reads `start..end` of a rollout. With `expect_meta` it stops at a child's `session_meta`,
/// since nothing after it matters.
fn read_pass(file: &File, start: u64, end: u64, expect_meta: bool) -> io::Result<Pass> {
    let mut reader = LineReader::new(file, start, end);
    let mut pass = Pass::default();
    let mut meta_open = expect_meta;
    while let Some(line) = reader.next_line()? {
        pass.absorb(&line, &mut meta_open);
        if pass.child {
            break;
        }
    }
    pass.skipped.oversize = reader.oversize;
    pass.offset = reader.offset();
    Ok(pass)
}

// ---- merging a pass into the prior session (§6.3) ----

/// What the prior scan knew about a rollout: the session it built from it.
#[derive(Debug, Clone)]
struct Prior {
    id: String,
    started_at: Timestamp,
    ended_at: Timestamp,
    cwd: PathBuf,
    branch: Option<String>,
    count: u32,
    /// The first-prompt ref carried as the title (rollouts mode only).
    first_prompt: Option<TextRef>,
    last_prompt: Option<TextRef>,
}

impl Prior {
    fn from_session(s: &Session) -> Prior {
        let first_prompt = match &s.title {
            Some((r @ TextRef::LogLine { .. }, TitleKind::Prompt)) => Some(r.clone()),
            _ => None,
        };
        Prior {
            id: s.id.as_str().to_owned(),
            started_at: s.started_at,
            ended_at: s.ended_at,
            cwd: s.cwd.clone(),
            branch: s.branch.clone(),
            count: s.message_count,
            first_prompt,
            last_prompt: s.last_prompt.clone(),
        }
    }
}

/// A rollout after merging: what both modes build sessions from.
#[derive(Debug)]
struct Facts {
    state: FileState,
    id: Option<String>,
    started_at: Option<Timestamp>,
    ended_at: Option<Timestamp>,
    cwd: Option<PathBuf>,
    branch: Option<String>,
    count: u32,
    first_prompt: Option<TextRef>,
    last_prompt: Option<TextRef>,
}

enum Outcome {
    Child,
    Session(Box<Facts>),
}

struct Read {
    records: u64,
    skipped: SkippedCounts,
    outcome: Outcome,
}

struct Reread;

/// The origin of the prior session's prompts, which decides whether appended prompts of the
/// other origin count. `Err` when it cannot be told: the file must be read from the start.
fn prior_origin(
    pass: &Pass,
    prior: &Prior,
    path: &Path,
) -> std::result::Result<Option<Origin>, Reread> {
    if pass.responses.count == 0 && !pass.event_seen {
        return Ok(None);
    }
    if prior.count == 0 {
        // Nothing says whether the head of the file had a (filtered-out) user_message event; if
        // response prompts arrive without one, only a full read can tell.
        return if pass.responses.count > 0 && !pass.event_seen {
            Err(Reread)
        } else {
            Ok(None)
        };
    }
    let known = prior
        .last_prompt
        .as_ref()
        .and_then(|r| jsonl::resolve_log_ref(r, path))
        .and_then(|line| prompt_of_line(&line))
        .map(|(origin, _)| origin);
    match known {
        Some(Origin::Response) if pass.event_seen => Err(Reread),
        None => Err(Reread),
        origin => Ok(origin),
    }
}

fn mtime(meta: &Metadata) -> Timestamp {
    let nanos = i32::try_from(meta.mtime_nsec()).unwrap_or(0);
    // Informational only; an unrepresentable mtime reads as the epoch.
    Timestamp::new(meta.mtime(), nanos).unwrap_or(Timestamp::UNIX_EPOCH)
}

/// Reads a rollout, resuming after `prior_file` when it is still the same file and a `prior`
/// session exists to merge into. `parent_thread_id` and `session_meta` live at the head of a
/// file, so a file with no prior session (first sighting, or a child excluded last time) is
/// always read from the start.
fn read_rollout(
    path: &Path,
    file: &File,
    meta: &Metadata,
    prior_file: Option<&FileState>,
    prior: Option<&Prior>,
) -> io::Result<Read> {
    let end = meta.len();
    let resume = match (prior, jsonl::plan_read(prior_file, file, meta)) {
        (Some(_), ReadFrom::Offset(offset)) => Some(offset),
        _ => None,
    };
    let mut prior = prior.filter(|_| resume.is_some());
    let mut pass = read_pass(file, resume.unwrap_or(0), end, resume.is_none())?;
    let mut origin = None;
    if let Some(p) = prior {
        match prior_origin(&pass, p, path) {
            Ok(found) => origin = found,
            Err(Reread) => {
                pass = read_pass(file, 0, end, true)?;
                prior = None;
            }
        }
    }
    let (records, skipped) = (pass.records, std::mem::take(&mut pass.skipped));
    if pass.child {
        return Ok(Read {
            records,
            skipped,
            outcome: Outcome::Child,
        });
    }

    let (dev, inode) = (meta.dev(), meta.ino());
    let line_ref = |span: Span, field| TextRef::LogLine {
        dev,
        inode,
        size_at_scan: end,
        offset: span.offset,
        len: span.len,
        field,
    };
    let use_events = pass.event_seen || origin == Some(Origin::Event);
    let chosen = if use_events {
        &pass.events
    } else {
        &pass.responses
    };
    let head = pass.meta.take();
    let facts = Facts {
        state: FileState {
            dev,
            inode,
            path: path.to_owned(),
            size: end,
            mtime: mtime(meta),
            offset: pass.offset,
            head_fp: jsonl::head_fp(file)?,
            tail_fp: jsonl::tail_fp(file, pass.offset)?,
            parser_version: PARSER_VERSION,
        },
        id: match prior {
            Some(p) => Some(p.id.clone()),
            None => head.as_ref().and_then(|m| m.id.clone()),
        },
        started_at: [
            prior.map(|p| p.started_at),
            head.as_ref().and_then(|m| m.started_at).or(pass.first_ts),
        ]
        .into_iter()
        .flatten()
        .min(),
        ended_at: [prior.map(|p| p.ended_at), pass.last_ts]
            .into_iter()
            .flatten()
            .max(),
        cwd: pass
            .turn_cwd
            .take()
            .or_else(|| prior.map(|p| p.cwd.clone()))
            .or_else(|| head.as_ref().and_then(|m| m.cwd.clone())),
        branch: match prior {
            Some(p) => p.branch.clone(),
            None => head.and_then(|m| m.branch),
        },
        count: prior.map_or(0, |p| p.count) + chosen.count,
        first_prompt: prior
            .and_then(|p| p.first_prompt.clone())
            .or_else(|| chosen.first.map(|s| line_ref(s, TextField::Title))),
        last_prompt: chosen
            .last
            .map(|s| line_ref(s, TextField::LastPrompt))
            .or_else(|| prior.and_then(|p| p.last_prompt.clone())),
    };
    Ok(Read {
        records,
        skipped,
        outcome: Outcome::Session(Box::new(facts)),
    })
}

// ---- scan bookkeeping ----

/// Totals across every rollout of one scan.
#[derive(Default)]
struct Tally {
    files: u32,
    records: u64,
    skipped: SkippedCounts,
    excluded_children: u32,
    unreadable: u32,
}

fn add_skipped(into: &mut SkippedCounts, from: &SkippedCounts) {
    into.malformed += from.malformed;
    into.unknown_type += from.unknown_type;
    into.oversize += from.oversize;
    into.non_uuid += from.non_uuid;
    into.bad_rollout_path += from.bad_rollout_path;
}

impl Tally {
    fn open(&mut self, path: &Path) -> Option<(File, Metadata)> {
        match jsonl::open_log(path) {
            Ok(opened) => {
                self.files += 1;
                Some(opened)
            }
            Err(_) => {
                self.unreadable += 1;
                None
            }
        }
    }

    fn absorb(&mut self, read: &Read) {
        self.records += read.records;
        add_skipped(&mut self.skipped, &read.skipped);
    }

    fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if self.unreadable > 0 {
            notes.push(format!(
                "{} rollout file(s) could not be opened or read",
                self.unreadable
            ));
        }
        notes
    }
}

/// The prior scan indexed for lookups.
struct Priors<'a> {
    files: HashMap<Inode, &'a FileState>,
    /// Log-backed sessions by their file.
    by_inode: HashMap<Inode, &'a Session>,
    by_id: HashMap<&'a Uuid, &'a Session>,
}

impl<'a> Priors<'a> {
    fn new(prior: &'a SourceState) -> Self {
        Priors {
            files: prior.files.iter().map(|f| ((f.dev, f.inode), f)).collect(),
            by_inode: prior
                .sessions
                .iter()
                .filter_map(|s| match s.backing {
                    Backing::LogFile { dev, inode, .. } => Some(((dev, inode), s)),
                    Backing::CodexThread { .. } => None,
                })
                .collect(),
            by_id: prior.sessions.iter().map(|s| (&s.id, s)).collect(),
        }
    }
}

// ---- path cache and tree walk (resolve must not walk per call, §12.1 #17) ----

#[derive(Default)]
struct HomePaths {
    by_inode: HashMap<Inode, PathBuf>,
    /// A full tree walk has refilled the cache since the last scan.
    walked: bool,
}

static PATHS: OnceLock<Mutex<HashMap<PathBuf, HomePaths>>> = OnceLock::new();

fn with_paths<R>(f: impl FnOnce(&mut HashMap<PathBuf, HomePaths>) -> R) -> R {
    let mut guard = PATHS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    f(&mut guard)
}

/// A scan replaces its home's entries with the files it opened; a `TextRef` only ever points
/// into a file of the latest scan. The walk flag resets so a moved file still costs one walk.
fn remember_paths(home: &Path, entries: Vec<(Inode, PathBuf)>) {
    with_paths(|all| {
        all.insert(
            home.to_owned(),
            HomePaths {
                by_inode: entries.into_iter().collect(),
                walked: false,
            },
        );
    });
}

/// Every rollout under the Codex root, sorted, plus the count of unreadable directories.
/// Symlinks are never followed (`file_type` does not resolve them).
fn walk_rollouts(home: &Path) -> (Vec<PathBuf>, u32) {
    #[cfg(test)]
    WALKS.with(|w| w.set(w.get() + 1));
    let mut found = Vec::new();
    let mut unreadable = 0;
    let mut pending = vec![
        (home.join(SESSIONS_DIR), true),
        (home.join(ARCHIVED_DIR), false),
    ];
    while let Some((dir, recurse)) = pending.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                unreadable += 1;
                continue;
            }
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if kind.is_dir() && recurse {
                pending.push((entry.path(), true));
            } else if kind.is_file() && is_rollout_name(&name, recurse) {
                found.push(entry.path());
            }
        }
    }
    found.sort();
    (found, unreadable)
}

/// `sessions/**` holds `rollout-*.jsonl`; `archived_sessions/` holds any `*.jsonl`.
fn is_rollout_name(name: &str, in_sessions: bool) -> bool {
    let is_jsonl = Path::new(name)
        .extension()
        .is_some_and(|e| e == JSONL_EXTENSION);
    is_jsonl && (!in_sessions || name.starts_with(ROLLOUT_PREFIX))
}

/// One walk that refills the cache with every rollout's `(dev, inode)`.
fn rebuild_paths(home: &Path) {
    let (paths, _) = walk_rollouts(home);
    let entries: Vec<_> = paths
        .into_iter()
        .filter_map(|p| {
            let meta = fs::symlink_metadata(&p).ok()?;
            Some(((meta.dev(), meta.ino()), p))
        })
        .collect();
    with_paths(|all| {
        let home_paths = all.entry(home.to_owned()).or_default();
        home_paths.by_inode.extend(entries);
        home_paths.walked = true;
    });
}

fn cached_path(home: &Path, key: Inode) -> (Option<PathBuf>, bool) {
    with_paths(|all| {
        all.get(home)
            .map_or((None, false), |h| (h.by_inode.get(&key).cloned(), h.walked))
    })
}

// ---- dispatch ----

pub fn scan(codex_home: &Path, prior: &SourceState) -> SourceScan {
    let reason = match codex_db::find_db(codex_home) {
        Some((db_path, version)) => match scan_db(codex_home, prior, &db_path, version) {
            Ok(scanned) => return scanned,
            Err(e) => format!(
                "thread database {} could not be read ({e})",
                file_name(&db_path)
            ),
        },
        None => no_database_reason(codex_home),
    };
    scan_rollouts(codex_home, prior, &reason)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// Why no database is used: none at all, or present but unusable. `find_db` does not report
/// open errors, so the unusable case names the files rather than the error.
fn no_database_reason(codex_home: &Path) -> String {
    let present = codex_db::list_db_files(codex_home);
    if present.is_empty() {
        return "no thread database (state_<N>.sqlite) found".to_owned();
    }
    let names: Vec<String> = present.iter().map(|(p, _)| file_name(p)).collect();
    format!(
        "thread database present but unusable ({}): none opened with the required threads columns",
        names.join(", ")
    )
}

fn zero_diagnostics(mode: SourceMode, db_version: Option<u32>) -> SourceDiagnostics {
    SourceDiagnostics {
        source: SourceKind::Codex,
        mode,
        db_version,
        files: 0,
        records: 0,
        skipped: SkippedCounts::default(),
        sessions: 0,
        excluded_children: 0,
        vanished: 0,
        matched: 0,
        unmatched: Default::default(),
    }
}

// ---- database mode ----

/// §5.2 primary source. An `Err` means the database could not be read: the caller falls back.
fn scan_db(
    codex_home: &Path,
    prior: &SourceState,
    db_path: &Path,
    version: u32,
) -> Result<SourceScan> {
    let mut db = codex_db::read_threads(codex_home, db_path, version)?;
    let priors = Priors::new(prior);
    let mut tally = Tally::default();
    let mut facts: HashMap<Uuid, Facts> = HashMap::new();
    let mut children: HashSet<Uuid> = HashSet::new();
    let mut cache = Vec::new();
    for thread in &db.threads {
        let Some(path) = &thread.rollout_path else {
            continue;
        };
        let Some((file, meta)) = tally.open(path) else {
            continue;
        };
        let key = (meta.dev(), meta.ino());
        cache.push((key, path.clone()));
        let prior_view = priors.by_id.get(&thread.id).map(|s| Prior::from_session(s));
        let Ok(read) = read_rollout(
            path,
            &file,
            &meta,
            priors.files.get(&key).copied(),
            prior_view.as_ref(),
        ) else {
            tally.unreadable += 1;
            continue;
        };
        tally.absorb(&read);
        match read.outcome {
            Outcome::Child => {
                children.insert(thread.id.clone());
            }
            Outcome::Session(f) => {
                facts.insert(thread.id.clone(), *f);
            }
        }
    }
    db.exclude_where(|t| children.contains(&t.id));
    remember_paths(codex_home, cache);

    let mut sessions = db.sessions();
    for (session, thread) in sessions.iter_mut().zip(&db.threads) {
        let Some(f) = facts.get(&thread.id) else {
            continue;
        };
        session.last_prompt = f.last_prompt.clone();
        session.message_count = f.count;
        // ponytail: on an incremental read `f.branch` is the prior session's branch, so a
        // `git_branch` cleared after a scan keeps its old value until the next full read.
        if session.branch.is_none() {
            session.branch = f.branch.clone();
        }
    }
    let files: Vec<FileState> = db
        .threads
        .iter()
        .filter_map(|t| facts.get(&t.id))
        .map(|f| f.state.clone())
        .collect();
    let mut skipped = tally.skipped.clone();
    add_skipped(&mut skipped, &db.skipped);
    Ok(SourceScan {
        files,
        diag: SourceDiagnostics {
            files: tally.files,
            records: tally.records,
            skipped,
            sessions: u32::try_from(sessions.len()).unwrap_or(u32::MAX),
            excluded_children: db.excluded_children,
            vanished: db.vanished(&prior.sessions),
            ..zero_diagnostics(SourceMode::Db, Some(version))
        },
        sessions,
        notes: tally.notes(),
    })
}

// ---- rollouts-only mode ----

/// §5.2 fallback: sessions straight from `sessions/**/rollout-*.jsonl` and
/// `archived_sessions/*.jsonl`. `reason` says why the database was not used.
fn scan_rollouts(codex_home: &Path, prior: &SourceState, reason: &str) -> SourceScan {
    let (paths, unreadable_dirs) = walk_rollouts(codex_home);
    let priors = Priors::new(prior);
    let mut tally = Tally::default();
    let mut seen: HashSet<Inode> = HashSet::new();
    let mut cache = Vec::new();
    let mut files = Vec::new();
    let mut sessions = Vec::new();
    for path in &paths {
        let Some((file, meta)) = tally.open(path) else {
            continue;
        };
        let key = (meta.dev(), meta.ino());
        if !seen.insert(key) {
            continue;
        }
        cache.push((key, path.clone()));
        let prior_session = priors.by_inode.get(&key).map(|s| Prior::from_session(s));
        let Ok(read) = read_rollout(
            path,
            &file,
            &meta,
            priors.files.get(&key).copied(),
            prior_session.as_ref(),
        ) else {
            tally.unreadable += 1;
            continue;
        };
        tally.absorb(&read);
        match read.outcome {
            Outcome::Child => tally.excluded_children += 1,
            Outcome::Session(f) => {
                if let Some(session) = rollout_session(&f, &mut tally.skipped) {
                    files.push(f.state);
                    sessions.push(session);
                }
            }
        }
    }
    remember_paths(codex_home, cache);
    let vanished = prior
        .sessions
        .iter()
        .filter(|s| match s.backing {
            Backing::LogFile { dev, inode, .. } => !seen.contains(&(dev, inode)),
            Backing::CodexThread { .. } => false,
        })
        .count();
    let (sessions, duplicates) = keep_latest_per_id(sessions);

    // Nothing to read and no database file at all: the source is absent, not merely fallen back.
    let absent =
        paths.is_empty() && unreadable_dirs == 0 && codex_db::list_db_files(codex_home).is_empty();
    let (mode, first_note) = if absent {
        (
            SourceMode::Absent,
            "no Codex session data found: no thread database and no rollout files".to_owned(),
        )
    } else {
        (
            SourceMode::Rollouts,
            format!("{reason}; reading rollout files"),
        )
    };
    let mut notes = vec![first_note];
    notes.extend(tally.notes());
    if unreadable_dirs > 0 {
        notes.push(format!(
            "{unreadable_dirs} rollout director(ies) could not be read"
        ));
    }
    if duplicates > 0 {
        notes.push(format!(
            "{duplicates} rollout file(s) duplicate a session id already seen and were skipped"
        ));
    }
    SourceScan {
        files,
        diag: SourceDiagnostics {
            files: tally.files,
            records: tally.records,
            skipped: tally.skipped,
            sessions: u32::try_from(sessions.len()).unwrap_or(u32::MAX),
            excluded_children: tally.excluded_children,
            vanished: u32::try_from(vanished).unwrap_or(u32::MAX),
            ..zero_diagnostics(mode, None)
        },
        sessions,
        notes,
    }
}

/// A rollout's session, or `None` (counted) when it lacks what a session needs.
fn rollout_session(f: &Facts, skipped: &mut SkippedCounts) -> Option<Session> {
    let Some(raw_id) = &f.id else {
        skipped.malformed += 1;
        return None;
    };
    let Some(id) = Uuid::parse(raw_id) else {
        skipped.non_uuid += 1;
        return None;
    };
    let (Some(cwd), Some(started_at), Some(ended_at)) = (f.cwd.clone(), f.started_at, f.ended_at)
    else {
        skipped.malformed += 1;
        return None;
    };
    Some(Session {
        source: SourceKind::Codex,
        id,
        cwd,
        branch: f.branch.clone(),
        started_at,
        ended_at,
        title: f.first_prompt.clone().map(|r| (r, TitleKind::Prompt)),
        last_prompt: f.last_prompt.clone(),
        message_count: f.count,
        backing: Backing::LogFile {
            dev: f.state.dev,
            inode: f.state.inode,
            path: f.state.path.clone(),
        },
    })
}

/// Sessions are keyed by id: of several files claiming one id, the one that ended last wins
/// (the earliest path on a tie). Returns the survivors in order and how many were dropped.
pub(crate) fn keep_latest_per_id(sessions: Vec<Session>) -> (Vec<Session>, usize) {
    let mut best: HashMap<&Uuid, usize> = HashMap::new();
    for (i, s) in sessions.iter().enumerate() {
        let slot = best.entry(&s.id).or_insert(i);
        if sessions[*slot].ended_at < s.ended_at {
            *slot = i;
        }
    }
    let keep: HashSet<usize> = best.into_values().collect();
    let total = sessions.len();
    let kept: Vec<Session> = sessions
        .into_iter()
        .enumerate()
        .filter_map(|(i, s)| keep.contains(&i).then_some(s))
        .collect();
    let dropped = total - kept.len();
    (kept, dropped)
}

// ---- resolve ----

pub fn resolve(codex_home: &Path, r: &TextRef) -> Option<String> {
    match r {
        TextRef::LogLine { dev, inode, .. } => resolve_log_line(codex_home, (*dev, *inode), r),
        TextRef::CodexThread { thread_id, field } => {
            let (db_path, _) = codex_db::find_db(codex_home)?;
            codex_db::resolve_thread(&db_path, thread_id, *field)
        }
    }
}

/// Looks the file up in the cache a scan filled; on a miss or a stale hit, refills it with one
/// walk (at most once per scan generation) and tries again.
fn resolve_log_line(codex_home: &Path, key: Inode, r: &TextRef) -> Option<String> {
    let line_at = |path: &Path| jsonl::resolve_log_ref(r, path).and_then(|l| prompt_of_line(&l));
    let (hit, walked) = cached_path(codex_home, key);
    if let Some(prompt) = hit.as_deref().and_then(line_at) {
        return Some(prompt.1);
    }
    if walked {
        return None;
    }
    rebuild_paths(codex_home);
    let (path, _) = cached_path(codex_home, key);
    path.as_deref().and_then(line_at).map(|(_, text)| text)
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::os::unix::fs::symlink;

    use rollcall_core::{Backing, TextField, TitleKind};
    use rusqlite::{Connection, params};
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::*;

    const ID_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const ID_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    const ID_C: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    const ID_PARENT: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
    const START: &str = "2026-09-28T18:04:11.500Z";
    const T1: &str = "2026-09-28T18:04:12.000Z";
    const T2: &str = "2026-09-28T18:05:00.000Z";
    const T3: &str = "2026-09-28T18:06:00.000Z";
    const EARLIER: &str = "2026-09-28T18:01:00.000Z";
    const DAY_DIR: &str = "sessions/2026/09/28";

    const THREADS_SQL: &str = "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT, \
        created_at INTEGER, updated_at INTEGER, source TEXT, cwd TEXT, title TEXT, \
        git_branch TEXT, archived INTEGER, first_user_message TEXT)";

    /// A fake `$CODEX_HOME`; canonical so path comparisons survive `/var` -> `/private/var`.
    struct Home {
        _dir: TempDir,
        path: PathBuf,
    }

    fn home() -> Home {
        let dir = tempfile::tempdir().unwrap();
        let path = fs::canonicalize(dir.path()).unwrap();
        Home { _dir: dir, path }
    }

    impl Home {
        fn rollout(&self, dir: &str, name: &str) -> PathBuf {
            let dir = self.path.join(dir);
            fs::create_dir_all(&dir).unwrap();
            dir.join(name)
        }
    }

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn record(at: &str, kind: &str, payload: Value) -> String {
        json!({"timestamp": at, "type": kind, "payload": payload}).to_string()
    }

    fn meta(id: &str, cwd: &str, branch: Option<&str>) -> String {
        let git = branch.map_or(Value::Null, |b| json!({"branch": b}));
        record(
            START,
            "session_meta",
            json!({"id": id, "timestamp": START, "cwd": cwd, "git": git}),
        )
    }

    fn child_meta(id: &str) -> String {
        record(
            START,
            "session_meta",
            json!({"id": id, "timestamp": START, "cwd": "/w", "parent_thread_id": ID_PARENT}),
        )
    }

    fn turn(at: &str, cwd: &str) -> String {
        record(at, "turn_context", json!({"cwd": cwd, "model": "m"}))
    }

    fn user_event(at: &str, message: &str) -> String {
        record(
            at,
            "event_msg",
            json!({"type": "user_message", "message": message}),
        )
    }

    fn user_response(at: &str, blocks: &[&str]) -> String {
        let content: Vec<Value> = blocks
            .iter()
            .map(|t| json!({"type": "input_text", "text": t}))
            .collect();
        record(
            at,
            "response_item",
            json!({"type": "message", "role": "user", "content": content}),
        )
    }

    fn assistant(at: &str) -> String {
        record(
            at,
            "response_item",
            json!({"type": "message", "role": "assistant",
                   "content": [{"type": "output_text", "text": "user said hi"}]}),
        )
    }

    fn write(path: &Path, lines: &[String]) {
        let mut body = lines.join("\n");
        body.push('\n');
        fs::write(path, body).unwrap();
    }

    fn append(path: &Path, lines: &[String]) {
        let mut file = OpenOptions::new().append(true).open(path).unwrap();
        for l in lines {
            writeln!(file, "{l}").unwrap();
        }
    }

    fn scan_home(h: &Home) -> SourceScan {
        scan(&h.path, &SourceState::default())
    }

    fn rescan(h: &Home, prior: &SourceScan) -> SourceScan {
        let state = SourceState {
            files: prior.files.clone(),
            sessions: prior.sessions.clone(),
        };
        scan(&h.path, &state)
    }

    fn text(h: &Home, r: &Option<TextRef>) -> Option<String> {
        resolve(&h.path, r.as_ref().expect("a ref"))
    }

    fn title_ref(s: &rollcall_core::Session) -> Option<TextRef> {
        s.title.clone().map(|t| t.0)
    }

    fn walks() -> u32 {
        WALKS.with(Cell::get)
    }

    fn create_db(h: &Home) -> Connection {
        let conn = Connection::open(h.path.join("state_5.sqlite")).unwrap();
        conn.execute_batch(THREADS_SQL).unwrap();
        conn
    }

    fn insert_thread(conn: &Connection, id: &str, rollout: &Path, branch: &str, title: &str) {
        conn.execute(
            "INSERT INTO threads VALUES (?1, ?2, 1000, 2000, '{\"cli\":true}', '/w/db', ?3, ?4, 0, ?5)",
            params![id, rollout.to_str().unwrap(), title, branch, "first message"],
        )
        .unwrap();
    }

    // ---- prompts ----

    #[test]
    fn user_message_events_win_over_their_response_item_twins() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(
            &p,
            &[
                meta(ID_A, "/w/app", Some("main")),
                user_event(T1, "fix the flaky test"),
                user_response(
                    T1,
                    &[
                        "<environment_context>x</environment_context>",
                        "fix the flaky test",
                    ],
                ),
                user_event(T2, "now run it 20 times"),
                user_response(T2, &["now run it 20 times"]),
            ],
        );
        let s = scan_home(&h);
        let session = &s.sessions[0];
        assert_eq!(session.message_count, 2);
        assert_eq!(
            text(&h, &session.last_prompt).unwrap(),
            "now run it 20 times"
        );
        assert_eq!(text(&h, &title_ref(session)).unwrap(), "fix the flaky test");
        assert_eq!(session.title.as_ref().unwrap().1, TitleKind::Prompt);
    }

    #[test]
    fn response_items_count_only_in_files_without_user_message_events() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(
            &p,
            &[
                meta(ID_A, "/w/app", None),
                // Injection block first, user block second: only the user text survives.
                user_response(
                    T1,
                    &[
                        "<environment_context>cwd</environment_context>",
                        "fix the flaky test",
                    ],
                ),
                // Nothing but injected blocks: not a human prompt.
                user_response(T1, &["<environment_context>again</environment_context>"]),
                user_response(T2, &["second", "prompt"]),
                assistant(T2),
            ],
        );
        let s = scan_home(&h);
        let session = &s.sessions[0];
        assert_eq!(session.message_count, 2);
        assert_eq!(text(&h, &title_ref(session)).unwrap(), "fix the flaky test");
        assert_eq!(text(&h, &session.last_prompt).unwrap(), "second prompt");
    }

    #[test]
    fn empty_user_message_is_not_a_prompt() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(&p, &[meta(ID_A, "/w", None), user_event(T1, "   ")]);
        let session = &scan_home(&h).sessions[0];
        assert_eq!(session.message_count, 0);
        assert!(session.last_prompt.is_none() && session.title.is_none());
    }

    #[test]
    fn user_message_starting_with_a_tag_is_not_a_prompt_but_still_makes_an_events_file() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(
            &p,
            &[
                meta(ID_A, "/w", None),
                user_event(T1, "<environment_context>cwd</environment_context>"),
                // The file has a user_message event, so this response_item prompt does not count.
                user_response(T2, &["from a response item"]),
            ],
        );
        let s = scan_home(&h);
        let session = &s.sessions[0];
        assert_eq!(session.message_count, 0);
        assert!(session.last_prompt.is_none() && session.title.is_none());

        // Appending a response-only prompt to such a file must not start counting it either.
        append(&p, &[user_response(T3, &["another response item"])]);
        let again = rescan(&h, &s);
        assert_eq!(again.sessions[0].message_count, 0);
        assert!(again.sessions[0].last_prompt.is_none());
    }

    #[test]
    fn user_message_is_cut_at_a_system_reminder_when_scanned_and_resolved() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(
            &p,
            &[
                meta(ID_A, "/w", None),
                user_event(
                    T1,
                    "fix the test <system-reminder>be careful</system-reminder>",
                ),
                user_event(T2, "<system-reminder>only injected</system-reminder>"),
            ],
        );
        let session = &scan_home(&h).sessions[0];
        assert_eq!(session.message_count, 1);
        assert_eq!(text(&h, &session.last_prompt).unwrap(), "fix the test");
        assert_eq!(text(&h, &title_ref(session)).unwrap(), "fix the test");
    }

    // ---- rollouts-only fields ----

    #[test]
    fn rollout_fields_follow_the_spec() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(
            &p,
            &[
                meta(ID_A, "/w/from-meta", Some("feat/x")),
                turn(T1, "/w/first-turn"),
                assistant(T2),
                turn(T3, "/w/latest-turn"),
                assistant(EARLIER),
            ],
        );
        let s = scan_home(&h);
        let session = &s.sessions[0];
        assert_eq!(session.id.as_str(), ID_A);
        assert_eq!(session.cwd, PathBuf::from("/w/latest-turn"));
        assert_eq!(session.branch.as_deref(), Some("feat/x"));
        assert_eq!(session.started_at, ts(START));
        assert_eq!(session.ended_at, ts(T3));
        assert_eq!(session.message_count, 0);
        assert_eq!(s.diag.mode, SourceMode::Rollouts);
        assert_eq!(s.diag.db_version, None);
    }

    #[test]
    fn cwd_falls_back_to_session_meta_without_turn_context() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(&p, &[meta(ID_A, "/w/from-meta", None)]);
        let session = &scan_home(&h).sessions[0];
        assert_eq!(session.cwd, PathBuf::from("/w/from-meta"));
        assert_eq!(session.branch, None);
    }

    #[test]
    fn a_second_session_meta_is_ignored() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(
            &p,
            &[
                meta(ID_A, "/w/first", Some("main")),
                meta(ID_B, "/w/second", Some("other")),
            ],
        );
        let s = scan_home(&h);
        assert_eq!(s.sessions.len(), 1);
        let session = &s.sessions[0];
        assert_eq!(session.id.as_str(), ID_A);
        assert_eq!(session.cwd, PathBuf::from("/w/first"));
        assert_eq!(session.branch.as_deref(), Some("main"));
    }

    #[test]
    fn parent_thread_id_makes_a_child_that_is_excluded_and_counted() {
        let h = home();
        write(
            &h.rollout(DAY_DIR, "rollout-child.jsonl"),
            &[child_meta(ID_B), user_event(T1, "child prompt")],
        );
        write(
            &h.rollout(DAY_DIR, "rollout-a.jsonl"),
            &[meta(ID_A, "/w", None)],
        );
        let s = scan_home(&h);
        assert_eq!(s.sessions.len(), 1);
        assert_eq!(s.sessions[0].id.as_str(), ID_A);
        assert_eq!(s.diag.excluded_children, 1);
        assert_eq!(s.files.len(), 1, "a child keeps no file state");
    }

    #[test]
    fn non_uuid_id_and_missing_session_meta_are_skipped_and_counted() {
        let h = home();
        write(
            &h.rollout(DAY_DIR, "rollout-bad.jsonl"),
            &[meta("not-a-uuid", "/w", None)],
        );
        write(
            &h.rollout(DAY_DIR, "rollout-nometa.jsonl"),
            &[user_event(T1, "hi")],
        );
        let s = scan_home(&h);
        assert!(s.sessions.is_empty());
        assert_eq!(s.diag.skipped.non_uuid, 1);
        assert_eq!(s.diag.skipped.malformed, 1);
    }

    #[test]
    fn malformed_unknown_and_blank_lines_are_counted_correctly() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(
            &p,
            &[
                meta(ID_A, "/w", None),
                "not json".into(),
                String::new(),
                json!({"no_type": 1}).to_string(),
                record(T1, "compacted", json!({})),
                user_event(T1, "hi"),
            ],
        );
        let s = scan_home(&h);
        assert_eq!(s.diag.skipped.malformed, 2);
        assert_eq!(s.diag.skipped.unknown_type, 1);
        assert_eq!(s.diag.records, 3, "session_meta, compacted, user_message");
        assert_eq!(s.sessions[0].message_count, 1);
    }

    #[test]
    fn archived_sessions_are_found_and_symlinked_rollouts_are_not_followed() {
        let h = home();
        let archived = h.path.join("archived_sessions");
        fs::create_dir_all(&archived).unwrap();
        write(
            &archived.join("rollout-old.jsonl"),
            &[meta(ID_A, "/w", None)],
        );
        let real = h.rollout(DAY_DIR, "rollout-real.jsonl");
        write(&real, &[meta(ID_B, "/w", None)]);
        symlink(&real, h.path.join(DAY_DIR).join("rollout-link.jsonl")).unwrap();
        // Not a rollout name: ignored.
        write(
            &h.path.join(DAY_DIR).join("notes.jsonl"),
            &[meta(ID_C, "/w", None)],
        );
        let s = scan_home(&h);
        let mut ids: Vec<_> = s.sessions.iter().map(|s| s.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, [ID_A, ID_B]);
        assert_eq!(
            s.diag.files, 2,
            "the symlink and the non-rollout name are never opened"
        );
    }

    #[test]
    fn two_files_with_one_session_id_yield_one_session_and_a_note() {
        let h = home();
        write(
            &h.rollout(DAY_DIR, "rollout-1.jsonl"),
            &[meta(ID_A, "/w", None), assistant(T1)],
        );
        write(
            &h.rollout(DAY_DIR, "rollout-2.jsonl"),
            &[meta(ID_A, "/w", None), assistant(T3)],
        );
        let s = scan_home(&h);
        assert_eq!(s.sessions.len(), 1);
        assert_eq!(s.sessions[0].ended_at, ts(T3));
        assert!(
            s.notes.iter().any(|n| n.contains("duplicate")),
            "{:?}",
            s.notes
        );
    }

    // ---- dispatch ----

    #[test]
    fn database_absent_means_rollouts_mode_with_a_note() {
        let h = home();
        write(
            &h.rollout(DAY_DIR, "rollout-a.jsonl"),
            &[meta(ID_A, "/w", None)],
        );
        let s = scan_home(&h);
        assert_eq!(s.diag.mode, SourceMode::Rollouts);
        assert_eq!(s.diag.db_version, None);
        assert_eq!(s.sessions.len(), 1);
        assert!(
            s.notes.iter().any(|n| n.contains("no thread database")),
            "{:?}",
            s.notes
        );
    }

    #[test]
    fn database_present_means_db_mode_with_its_version() {
        let h = home();
        let rollout = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(
            &rollout,
            &[meta(ID_A, "/w/rollout", None), user_event(T1, "hi")],
        );
        let conn = create_db(&h);
        insert_thread(&conn, ID_A, &rollout, "main", "The title");
        let s = scan_home(&h);
        assert_eq!(s.diag.mode, SourceMode::Db);
        assert_eq!(s.diag.db_version, Some(5));
        assert!(s.notes.is_empty(), "{:?}", s.notes);
        assert_eq!(s.sessions.len(), 1);
        assert_eq!(s.sessions[0].cwd, PathBuf::from("/w/db"));
        assert!(matches!(s.sessions[0].backing, Backing::CodexThread { .. }));
    }

    #[test]
    fn unusable_database_falls_back_to_rollouts_and_says_so() {
        let h = home();
        let conn = Connection::open(h.path.join("state_5.sqlite")).unwrap();
        conn.execute_batch("CREATE TABLE threads (id TEXT)")
            .unwrap();
        write(
            &h.rollout(DAY_DIR, "rollout-a.jsonl"),
            &[meta(ID_A, "/w", None)],
        );
        let s = scan_home(&h);
        assert_eq!(s.diag.mode, SourceMode::Rollouts);
        assert_eq!(s.sessions.len(), 1);
        assert!(
            s.notes
                .iter()
                .any(|n| n.contains("unusable") && n.contains("state_5.sqlite")),
            "{:?}",
            s.notes
        );
    }

    #[test]
    fn unreadable_database_is_an_error_for_the_dispatcher_to_report() {
        let h = home();
        let missing = h.path.join("state_9.sqlite");
        assert!(scan_db(&h.path, &SourceState::default(), &missing, 9).is_err());
    }

    #[test]
    fn no_database_and_no_rollouts_is_absent() {
        let h = home();
        let s = scan_home(&h);
        assert_eq!(s.diag.mode, SourceMode::Absent);
        assert!(s.sessions.is_empty() && s.files.is_empty());
        assert!(!s.notes.is_empty());
        let gone = scan(&h.path.join("nope"), &SourceState::default());
        assert_eq!(gone.diag.mode, SourceMode::Absent);
    }

    // ---- database mode overlay ----

    #[test]
    fn db_sessions_take_prompts_and_count_from_the_rollout_and_branch_as_fallback() {
        let h = home();
        let with_branch = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(
            &with_branch,
            &[
                meta(ID_A, "/w", Some("rollout-branch")),
                user_event(T1, "one"),
                user_event(T2, "two"),
            ],
        );
        let db_wins = h.rollout(DAY_DIR, "rollout-b.jsonl");
        write(&db_wins, &[meta(ID_B, "/w", Some("rollout-branch"))]);
        let conn = create_db(&h);
        insert_thread(&conn, ID_A, &with_branch, "", "t");
        insert_thread(&conn, ID_B, &db_wins, "db-branch", "t");
        insert_thread(&conn, ID_C, &h.path.join("sessions/missing.jsonl"), "", "t");
        let s = scan_home(&h);
        let by_id = |id: &str| s.sessions.iter().find(|x| x.id.as_str() == id).unwrap();
        let a = by_id(ID_A);
        assert_eq!(a.branch.as_deref(), Some("rollout-branch"));
        assert_eq!(a.message_count, 2);
        assert_eq!(text(&h, &a.last_prompt).unwrap(), "two");
        assert_eq!(by_id(ID_B).branch.as_deref(), Some("db-branch"));
        let c = by_id(ID_C);
        assert_eq!(c.message_count, 0);
        assert!(c.last_prompt.is_none() && c.branch.is_none());
    }

    #[test]
    fn db_thread_whose_rollout_names_a_parent_is_excluded_and_counted() {
        let h = home();
        let child = h.rollout(DAY_DIR, "rollout-child.jsonl");
        write(&child, &[child_meta(ID_B)]);
        let normal = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(&normal, &[meta(ID_A, "/w", None)]);
        let conn = create_db(&h);
        insert_thread(&conn, ID_A, &normal, "main", "t");
        insert_thread(&conn, ID_B, &child, "main", "t");
        let s = scan_home(&h);
        assert_eq!(s.sessions.len(), 1);
        assert_eq!(s.sessions[0].id.as_str(), ID_A);
        assert_eq!(s.diag.excluded_children, 1);
        assert_eq!(s.diag.sessions, 1);
    }

    #[test]
    fn db_rollout_outside_the_root_is_counted_and_ignored() {
        let h = home();
        let outside = tempfile::tempdir().unwrap();
        let elsewhere = outside.path().join("rollout-x.jsonl");
        write(
            &elsewhere,
            &[meta(ID_A, "/w", None), user_event(T1, "secret")],
        );
        let conn = create_db(&h);
        insert_thread(&conn, ID_A, &elsewhere, "main", "t");
        let s = scan_home(&h);
        assert_eq!(s.diag.skipped.bad_rollout_path, 1);
        assert_eq!(s.sessions[0].message_count, 0);
    }

    #[test]
    fn deleted_db_row_is_dropped_and_counted_vanished() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(&p, &[meta(ID_A, "/w", None)]);
        let conn = create_db(&h);
        insert_thread(&conn, ID_A, &p, "main", "t");
        let first = scan_home(&h);
        conn.execute("DELETE FROM threads", []).unwrap();
        let second = rescan(&h, &first);
        assert!(second.sessions.is_empty());
        assert_eq!(second.diag.vanished, 1);
    }

    #[test]
    fn db_mode_reads_only_appended_lines_and_accumulates() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(&p, &[meta(ID_A, "/w", None), user_event(T1, "one")]);
        let conn = create_db(&h);
        insert_thread(&conn, ID_A, &p, "", "t");
        let first = scan_home(&h);
        assert_eq!(first.diag.records, 2);
        append(&p, &[user_event(T2, "two"), assistant(T2)]);
        let second = rescan(&h, &first);
        assert_eq!(second.diag.records, 2, "only the appended lines");
        let s = &second.sessions[0];
        assert_eq!(s.message_count, 2);
        assert_eq!(text(&h, &s.last_prompt).unwrap(), "two");
        assert_eq!(second.diag.mode, SourceMode::Db);
    }

    // ---- incremental reads (rollouts mode) ----

    #[test]
    fn append_merges_into_the_prior_session() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(
            &p,
            &[
                meta(ID_A, "/w/a", Some("main")),
                user_event(T1, "first"),
                turn(T1, "/w/a"),
            ],
        );
        let first = scan_home(&h);
        append(&p, &[user_event(T2, "second"), turn(T3, "/w/b")]);
        let second = rescan(&h, &first);
        assert_eq!(second.diag.records, 2);
        let s = &second.sessions[0];
        assert_eq!(s.message_count, 2);
        assert_eq!(s.cwd, PathBuf::from("/w/b"));
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.started_at, ts(START));
        assert_eq!(s.ended_at, ts(T3));
        assert_eq!(text(&h, &title_ref(s)).unwrap(), "first");
        assert_eq!(text(&h, &s.last_prompt).unwrap(), "second");
    }

    #[test]
    fn append_without_new_prompts_keeps_the_prior_last_prompt() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(&p, &[meta(ID_A, "/w", None), user_event(T1, "only")]);
        let first = scan_home(&h);
        append(&p, &[assistant(T3)]);
        let second = rescan(&h, &first);
        let s = &second.sessions[0];
        assert_eq!(s.message_count, 1);
        assert_eq!(text(&h, &s.last_prompt).unwrap(), "only");
        assert_eq!(s.ended_at, ts(T3));
    }

    #[test]
    fn rollout_moved_to_archived_sessions_is_read_incrementally() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(
            &p,
            &[
                meta(ID_A, "/w", None),
                user_event(T1, "one"),
                user_event(T2, "two"),
                user_event(T3, "three"),
            ],
        );
        let first = scan_home(&h);
        assert_eq!(first.diag.records, 4);
        let archived = h.path.join("archived_sessions");
        fs::create_dir_all(&archived).unwrap();
        let moved = archived.join("rollout-a.jsonl");
        fs::rename(&p, &moved).unwrap();

        let second = rescan(&h, &first);
        assert_eq!(second.diag.records, 0, "no re-read after the move");
        assert_eq!(second.diag.vanished, 0);
        assert_eq!(second.sessions.len(), 1);
        assert_eq!(second.sessions[0].message_count, 3);
        assert_eq!(second.files[0].path, moved);
        let Backing::LogFile { path, .. } = &second.sessions[0].backing else {
            panic!("log-backed");
        };
        assert_eq!(path, &moved);

        // Still incremental when it grows afterwards, and the refs still resolve.
        append(&moved, &[user_event(T3, "four")]);
        let third = rescan(&h, &second);
        assert_eq!(third.diag.records, 1);
        assert_eq!(third.sessions[0].message_count, 4);
        assert_eq!(text(&h, &third.sessions[0].last_prompt).unwrap(), "four");
    }

    #[test]
    fn truncated_or_replaced_file_is_re_read_from_the_start() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(
            &p,
            &[
                meta(ID_A, "/w", None),
                user_event(T1, "one"),
                user_event(T2, "two"),
            ],
        );
        let first = scan_home(&h);
        write(&p, &[meta(ID_A, "/w", None), user_event(T1, "x")]);
        let second = rescan(&h, &first);
        assert_eq!(second.diag.records, 2, "re-read from 0");
        assert_eq!(second.sessions[0].message_count, 1);
    }

    #[test]
    fn deleted_rollout_is_dropped_and_counted_vanished() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(&p, &[meta(ID_A, "/w", None)]);
        let first = scan_home(&h);
        fs::remove_file(&p).unwrap();
        let second = rescan(&h, &first);
        assert!(second.sessions.is_empty() && second.files.is_empty());
        assert_eq!(second.diag.vanished, 1);
    }

    #[test]
    fn a_user_message_appearing_after_response_item_prompts_re_reads_the_file() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(
            &p,
            &[
                meta(ID_A, "/w", None),
                user_response(T1, &["old style one"]),
                user_response(T2, &["old style two"]),
            ],
        );
        let first = scan_home(&h);
        assert_eq!(first.sessions[0].message_count, 2);
        append(
            &p,
            &[
                user_event(T3, "new style"),
                user_response(T3, &["new style"]),
            ],
        );
        let second = rescan(&h, &first);
        let s = &second.sessions[0];
        // Now the file has user_message events: they alone count.
        assert_eq!(s.message_count, 1);
        assert_eq!(text(&h, &title_ref(s)).unwrap(), "new style");
        assert_eq!(second.diag.records, 5, "whole file re-read");
    }

    #[test]
    fn response_items_appended_to_a_user_message_file_are_ignored() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(&p, &[meta(ID_A, "/w", None), user_event(T1, "one")]);
        let first = scan_home(&h);
        append(&p, &[user_response(T2, &["twin of nothing"])]);
        let second = rescan(&h, &first);
        assert_eq!(second.sessions[0].message_count, 1);
        assert_eq!(second.diag.records, 1);
    }

    // ---- resolve ----

    #[test]
    fn resolve_log_line_returns_the_prompt_and_none_once_the_file_is_replaced() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(&p, &[meta(ID_A, "/w", None), user_event(T1, "keep me")]);
        let s = scan_home(&h);
        let r = s.sessions[0].last_prompt.clone();
        assert_eq!(text(&h, &r).unwrap(), "keep me");
        // A new file (new inode) at the same path is not the scanned file.
        fs::remove_file(&p).unwrap();
        write(&p, &[meta(ID_A, "/w", None), user_event(T1, "impostor")]);
        assert_eq!(text(&h, &r), None);
    }

    #[test]
    fn resolve_does_not_walk_after_a_scan() {
        for with_db in [false, true] {
            let h = home();
            let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
            write(
                &p,
                &[
                    meta(ID_A, "/w", None),
                    user_event(T1, "one"),
                    user_event(T2, "two"),
                ],
            );
            if with_db {
                let conn = create_db(&h);
                insert_thread(&conn, ID_A, &p, "main", "t");
            }
            let s = scan_home(&h);
            let before = walks();
            assert_eq!(text(&h, &s.sessions[0].last_prompt).unwrap(), "two");
            if !with_db {
                assert_eq!(text(&h, &title_ref(&s.sessions[0])).unwrap(), "one");
            }
            assert_eq!(
                walks(),
                before,
                "resolve after scan must not walk (db: {with_db})"
            );
        }
    }

    #[test]
    fn resolve_after_the_file_moved_walks_once_then_hits_the_cache() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(
            &p,
            &[
                meta(ID_A, "/w", None),
                user_event(T1, "one"),
                user_event(T2, "two"),
            ],
        );
        let s = scan_home(&h);
        let session = &s.sessions[0];
        let archived = h.path.join("archived_sessions");
        fs::create_dir_all(&archived).unwrap();
        fs::rename(&p, archived.join("rollout-a.jsonl")).unwrap();
        let before = walks();
        assert_eq!(text(&h, &session.last_prompt).unwrap(), "two");
        assert_eq!(walks(), before + 1);
        assert_eq!(text(&h, &title_ref(session)).unwrap(), "one");
        assert_eq!(walks(), before + 1, "second resolve is a cache hit");
    }

    #[test]
    fn dead_refs_do_not_walk_again_after_the_first_walk() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(&p, &[meta(ID_A, "/w", None), user_event(T1, "one")]);
        let s = scan_home(&h);
        fs::remove_file(&p).unwrap();
        let r = s.sessions[0].last_prompt.clone();
        assert_eq!(text(&h, &r), None);
        let after_first = walks();
        assert_eq!(text(&h, &r), None);
        assert_eq!(text(&h, &r), None);
        assert_eq!(walks(), after_first, "at most one walk per scan generation");
    }

    #[test]
    fn resolve_of_a_ref_from_a_process_that_never_scanned_walks_once() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(&p, &[meta(ID_A, "/w", None), user_event(T1, "one")]);
        let s = scan_home(&h);
        let r = s.sessions[0].last_prompt.clone().unwrap();
        // Another spelling of the same home is a different cache key: a cold cache.
        let cold = h.path.join("sessions/..");
        let before = walks();
        assert_eq!(resolve(&cold, &r).unwrap(), "one");
        assert_eq!(walks(), before + 1);
        assert_eq!(resolve(&cold, &r).unwrap(), "one");
        assert_eq!(walks(), before + 1);
    }

    #[test]
    fn resolve_thread_title_reflects_the_database() {
        let h = home();
        let p = h.rollout(DAY_DIR, "rollout-a.jsonl");
        write(&p, &[meta(ID_A, "/w", None)]);
        let conn = create_db(&h);
        insert_thread(&conn, ID_A, &p, "main", "Original name");
        let s = scan_home(&h);
        let (thread_title, kind) = s.sessions[0].title.clone().unwrap();
        assert_eq!(kind, TitleKind::Thread);
        assert_eq!(resolve(&h.path, &thread_title).unwrap(), "Original name");
        conn.execute("UPDATE threads SET title = 'Renamed'", [])
            .unwrap();
        assert_eq!(resolve(&h.path, &thread_title).unwrap(), "Renamed");
        conn.execute("DELETE FROM threads", []).unwrap();
        assert_eq!(resolve(&h.path, &thread_title), None);
    }

    #[test]
    fn resolve_thread_without_a_database_is_none() {
        let h = home();
        let r = TextRef::CodexThread {
            thread_id: Uuid::parse(ID_A).unwrap(),
            field: TextField::Title,
        };
        assert_eq!(resolve(&h.path, &r), None);
    }
}
