//! Claude Code source: one session per `projects/*/*.jsonl` file (§5.1).
//!
//! Record types this parser knows: `user`, `assistant`, `custom-title`, `ai-title` and
//! `last-prompt`. Every other `type` (or none) is skipped and counted as `unknown_type`; such
//! records contribute nothing, not even a timestamp. `records` counts the JSON objects read by
//! this scan, so an incremental scan reports only what it appended.

#[cfg(test)]
use std::cell::Cell;
use std::collections::HashMap;
use std::fs::{self, File};
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use rollcall_core::text::filter_prompt_block;
use rollcall_core::{
    Backing, FileState, Session, SkippedCounts, SourceDiagnostics, SourceKind, SourceMode,
    SourceScan, SourceState, TextField, TextRef, Timestamp, TitleKind, UnmatchedCounts, Uuid,
};
use serde_json::Value;

use crate::PARSER_VERSION;
use crate::codex_rollout::keep_latest_per_id;
use crate::jsonl::{
    Line, LineReader, ReadFrom, head_fp, open_log, plan_read, resolve_log_ref, tail_fp,
};

const LOG_EXTENSION: &str = "jsonl";
const SUBAGENTS_DIR: &str = "subagents";
/// `gitBranch` value Claude records for a detached HEAD.
const DETACHED_BRANCH: &str = "HEAD";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    User,
    Assistant,
    CustomTitle,
    AiTitle,
    LastPrompt,
}

fn kind_of(v: &Value) -> Option<Kind> {
    match v.get("type")?.as_str()? {
        "user" => Some(Kind::User),
        "assistant" => Some(Kind::Assistant),
        "custom-title" => Some(Kind::CustomTitle),
        "ai-title" => Some(Kind::AiTitle),
        "last-prompt" => Some(Kind::LastPrompt),
        _ => None,
    }
}

/// A string field that is present and not blank.
fn text_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)?.as_str().filter(|s| !s.trim().is_empty())
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key) == Some(&Value::Bool(true))
}

/// The human prompt a `user` record carries (§5.1), after the §5.0 per-block filter.
fn human_prompt(v: &Value) -> Option<String> {
    if kind_of(v)? != Kind::User || flag(v, "isMeta") || flag(v, "isSidechain") {
        return None;
    }
    let blocks: Vec<&str> = match v.pointer("/message/content")? {
        Value::String(s) => vec![s],
        Value::Array(items) => items
            .iter()
            .map(
                |b| match (b.get("type")?.as_str()?, b.get("text")?.as_str()?) {
                    ("text", text) => Some(text),
                    _ => None,
                },
            )
            .collect::<Option<_>>()?,
        _ => return None,
    };
    let kept: Vec<String> = blocks.into_iter().filter_map(filter_prompt_block).collect();
    (!kept.is_empty()).then(|| kept.join(" "))
}

/// The raw text `field` names on this record, re-applying scan-time extraction (§6.2).
fn extract(v: &Value, field: TextField) -> Option<String> {
    let key = match (kind_of(v)?, field) {
        (Kind::User, _) => return human_prompt(v),
        (Kind::CustomTitle, TextField::Title) => "customTitle",
        (Kind::AiTitle, TextField::Title) => "aiTitle",
        (Kind::LastPrompt, TextField::LastPrompt) => "lastPrompt",
        _ => return None,
    };
    text_field(v, key).map(str::to_owned)
}

enum Skip {
    Malformed,
    Unknown,
}

struct Record {
    kind: Kind,
    value: Value,
    timestamp: Option<Timestamp>,
}

fn parse_record(bytes: &[u8]) -> Result<Record, Skip> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| Skip::Malformed)?;
    if !value.is_object() {
        return Err(Skip::Malformed);
    }
    let kind = kind_of(&value).ok_or(Skip::Unknown)?;
    let timestamp = match value.get("timestamp") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.parse().map_err(|_| Skip::Malformed)?),
        Some(_) => return Err(Skip::Malformed),
    };
    Ok(Record {
        kind,
        value,
        timestamp,
    })
}

enum Dropped {
    NonUuid,
    /// No cwd or no timestamp anywhere: nothing to match the session by.
    Incomplete,
}

/// A session under construction: the prior session (incremental) plus this scan's records.
#[derive(Default)]
struct Acc {
    id: Option<String>,
    cwd: Option<PathBuf>,
    branch: Option<String>,
    started_at: Option<Timestamp>,
    ended_at: Option<Timestamp>,
    title: Option<(TextRef, TitleKind)>,
    /// Latest `last-prompt` record seen by this scan.
    prompt_record: Option<TextRef>,
    /// Latest human prompt seen by this scan.
    human_prompt: Option<TextRef>,
    prior_last_prompt: Option<TextRef>,
    message_count: u32,
}

impl Acc {
    fn seeded(prior: &Session) -> Acc {
        Acc {
            id: Some(prior.id.as_str().to_owned()),
            cwd: Some(prior.cwd.clone()),
            branch: prior.branch.clone(),
            started_at: Some(prior.started_at),
            ended_at: Some(prior.ended_at),
            title: prior.title.clone(),
            prior_last_prompt: prior.last_prompt.clone(),
            message_count: prior.message_count,
            ..Acc::default()
        }
    }

    fn apply(&mut self, rec: &Record, at: impl Fn(TextField) -> TextRef) {
        let v = &rec.value;
        if self.id.is_none() {
            self.id = text_field(v, "sessionId").map(str::to_owned);
        }
        if let Some(cwd) = text_field(v, "cwd") {
            self.cwd = Some(PathBuf::from(cwd));
        }
        if let Some(branch) = text_field(v, "gitBranch") {
            self.branch = (branch != DETACHED_BRANCH).then(|| branch.to_owned());
        }
        if let Some(t) = rec.timestamp {
            self.started_at = Some(self.started_at.map_or(t, |s| s.min(t)));
            self.ended_at = Some(self.ended_at.map_or(t, |e| e.max(t)));
        }
        match rec.kind {
            Kind::User if human_prompt(v).is_some() => {
                self.message_count += 1;
                self.human_prompt = Some(at(TextField::LastPrompt));
                if self.title.is_none() {
                    self.title = Some((at(TextField::Title), TitleKind::Prompt));
                }
            }
            Kind::CustomTitle if text_field(v, "customTitle").is_some() => {
                self.title = Some((at(TextField::Title), TitleKind::Custom));
            }
            Kind::AiTitle
                if text_field(v, "aiTitle").is_some()
                    && !matches!(self.title, Some((_, TitleKind::Custom))) =>
            {
                self.title = Some((at(TextField::Title), TitleKind::Ai));
            }
            Kind::LastPrompt if text_field(v, "lastPrompt").is_some() => {
                self.prompt_record = Some(at(TextField::LastPrompt));
            }
            _ => {}
        }
    }

    /// A `last-prompt` record beats any human prompt; among equals the latest wins. The prior
    /// session stores one ref without saying which kind, so `prior_is_record` is asked only when
    /// it decides the outcome.
    fn last_prompt(&mut self, prior_is_record: impl Fn(&TextRef) -> bool) -> Option<TextRef> {
        let prior = self.prior_last_prompt.take();
        match (self.prompt_record.take(), self.human_prompt.take(), prior) {
            (Some(record), _, _) => Some(record),
            (None, Some(_), Some(prior)) if prior_is_record(&prior) => Some(prior),
            (None, human, prior) => human.or(prior),
        }
    }

    fn finish(
        mut self,
        file_stem: &str,
        backing: Backing,
        prior_is_record: impl Fn(&TextRef) -> bool,
    ) -> Result<Session, Dropped> {
        let id = Uuid::parse(self.id.as_deref().unwrap_or(file_stem)).ok_or(Dropped::NonUuid)?;
        let last_prompt = self.last_prompt(prior_is_record);
        let (Some(cwd), Some(started_at), Some(ended_at)) =
            (self.cwd, self.started_at, self.ended_at)
        else {
            return Err(Dropped::Incomplete);
        };
        Ok(Session {
            source: SourceKind::Claude,
            id,
            cwd,
            branch: self.branch,
            started_at,
            ended_at,
            title: self.title,
            last_prompt,
            message_count: self.message_count,
            backing,
        })
    }
}

fn is_last_prompt_record(file: &File, r: &TextRef) -> bool {
    let TextRef::LogLine { offset, len, .. } = r else {
        return false;
    };
    let mut line = vec![0; *len as usize];
    file.read_exact_at(&mut line, *offset).is_ok()
        && serde_json::from_slice::<Value>(&line)
            .is_ok_and(|v| matches!(kind_of(&v), Some(Kind::LastPrompt)))
}

/// Child entries in a stable order; an unreadable or missing directory has none.
fn children(dir: &Path) -> Vec<(PathBuf, fs::FileType)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|e| Some((e.path(), e.file_type().ok()?)))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn is_log(path: &Path, ty: &fs::FileType) -> bool {
    !ty.is_dir() && path.extension().is_some_and(|e| e == LOG_EXTENSION)
}

/// Real (non-symlink) subdirectories of `dir`.
fn subdirs(dir: &Path) -> impl Iterator<Item = PathBuf> + use<> {
    children(dir)
        .into_iter()
        .filter(|(_, ty)| ty.is_dir())
        .map(|(path, _)| path)
}

/// `projects/*/*.jsonl`.
fn session_files(projects_dir: &Path) -> Vec<PathBuf> {
    subdirs(projects_dir)
        .flat_map(|project| children(&project))
        .filter(|(path, ty)| is_log(path, ty))
        .map(|(path, _)| path)
        .collect()
}

/// `projects/*/<sessionId>/subagents/*.jsonl`, which are not sessions (§5.0).
fn count_subagent_files(projects_dir: &Path) -> u32 {
    let count = subdirs(projects_dir)
        .flat_map(|project| subdirs(&project))
        .flat_map(|session_dir| children(&session_dir.join(SUBAGENTS_DIR)))
        .filter(|(path, ty)| is_log(path, ty))
        .count();
    count as u32
}

struct ScanCtx<'a> {
    prior_files: HashMap<(u64, u64), &'a FileState>,
    prior_sessions: HashMap<(u64, u64), &'a Session>,
    /// Every log opened this scan and where it lives: the vanished check and the resolve index.
    seen: HashMap<(u64, u64), PathBuf>,
    incomplete: u32,
    out: SourceScan,
}

impl ScanCtx<'_> {
    fn read_file(&mut self, path: &Path) {
        self.out.diag.files += 1;
        let Ok((file, meta)) = open_log(path) else {
            return;
        };
        let (dev, inode) = (meta.dev(), meta.ino());
        self.seen.insert((dev, inode), path.to_path_buf());
        let prior_file = self.prior_files.get(&(dev, inode)).copied();
        let prior_session = self.prior_sessions.get(&(dev, inode)).copied();
        // A prior file state without its session (dropped last time) cannot be merged into.
        let (start, mut acc) = match (plan_read(prior_file, &file, &meta), prior_session) {
            (ReadFrom::Offset(offset), Some(session)) => (offset, Acc::seeded(session)),
            _ => (0, Acc::default()),
        };
        let size_at_scan = meta.len();
        let at = |line: &Line, field| TextRef::LogLine {
            dev,
            inode,
            size_at_scan,
            offset: line.offset,
            len: line.bytes.len() as u32,
            field,
        };

        let mut reader = LineReader::new(&file, start, size_at_scan);
        loop {
            let line = match reader.next_line() {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(e) => {
                    let note = format!("claude: read error in {}: {e}", path.display());
                    self.out.notes.push(note);
                    break;
                }
            };
            match parse_record(&line.bytes) {
                Err(Skip::Malformed) => self.out.diag.skipped.malformed += 1,
                Err(Skip::Unknown) => {
                    self.out.diag.records += 1;
                    self.out.diag.skipped.unknown_type += 1;
                }
                Ok(rec) => {
                    self.out.diag.records += 1;
                    acc.apply(&rec, |field| at(&line, field));
                }
            }
        }
        self.out.diag.skipped.oversize += reader.oversize;

        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let backing = Backing::LogFile {
            dev,
            inode,
            path: path.to_path_buf(),
        };
        match acc.finish(stem, backing, |r| is_last_prompt_record(&file, r)) {
            Ok(session) => {
                let offset = reader.offset();
                self.out.files.push(FileState {
                    dev,
                    inode,
                    path: path.to_path_buf(),
                    size: size_at_scan,
                    mtime: Timestamp::new(meta.mtime(), meta.mtime_nsec() as i32)
                        .unwrap_or(Timestamp::UNIX_EPOCH),
                    offset,
                    // Zeroed on failure: never matches a real fingerprint, so the next scan
                    // re-reads from 0.
                    head_fp: head_fp(&file).unwrap_or_default(),
                    tail_fp: tail_fp(&file, offset).unwrap_or_default(),
                    parser_version: PARSER_VERSION,
                });
                self.out.sessions.push(session);
            }
            Err(Dropped::NonUuid) => self.out.diag.skipped.non_uuid += 1,
            Err(Dropped::Incomplete) => self.incomplete += 1,
        }
    }
}

pub fn scan(projects_dir: &Path, prior: &SourceState) -> SourceScan {
    let logs = session_files(projects_dir);
    let mut ctx = ScanCtx {
        prior_files: prior.files.iter().map(|f| ((f.dev, f.inode), f)).collect(),
        prior_sessions: prior
            .sessions
            .iter()
            .filter_map(|s| match &s.backing {
                Backing::LogFile { dev, inode, .. } => Some(((*dev, *inode), s)),
                Backing::CodexThread { .. } => None,
            })
            .collect(),
        seen: HashMap::new(),
        incomplete: 0,
        out: SourceScan {
            files: Vec::new(),
            sessions: Vec::new(),
            diag: SourceDiagnostics {
                source: SourceKind::Claude,
                mode: SourceMode::Absent,
                db_version: None,
                files: 0,
                records: 0,
                skipped: SkippedCounts::default(),
                sessions: 0,
                excluded_children: count_subagent_files(projects_dir),
                vanished: 0,
                matched: 0,
                unmatched: UnmatchedCounts::default(),
            },
            notes: Vec::new(),
        },
    };
    for path in &logs {
        ctx.read_file(path);
    }

    let vanished = ctx
        .prior_sessions
        .keys()
        .filter(|key| !ctx.seen.contains_key(*key))
        .count();
    store_index(projects_dir, ctx.seen);
    let mut out = ctx.out;
    // Sessions are keyed by (source, id): several files claiming one id leave the latest.
    let (sessions, duplicates) = keep_latest_per_id(out.sessions);
    out.sessions = sessions;
    out.diag.vanished = vanished as u32;
    out.diag.sessions = out.sessions.len() as u32;
    if !logs.is_empty() {
        out.diag.mode = SourceMode::Jsonl;
    }
    if duplicates > 0 {
        out.notes.push(format!(
            "claude: {duplicates} session file(s) duplicate a session id already seen and were skipped"
        ));
    }
    if ctx.incomplete > 0 {
        out.notes.push(format!(
            "claude: {} session file(s) skipped: no cwd or timestamp recorded",
            ctx.incomplete
        ));
    }
    out
}

pub fn resolve(projects_dir: &Path, r: &TextRef) -> Option<String> {
    let TextRef::LogLine {
        dev, inode, field, ..
    } = r
    else {
        return None;
    };
    let path = locate(projects_dir, *dev, *inode)?;
    let line = resolve_log_ref(r, &path)?;
    extract(&serde_json::from_slice(&line).ok()?, *field)
}

type PathIndex = HashMap<(PathBuf, u64, u64), PathBuf>;

/// (projects dir, dev, inode) -> path. A ref carries no path, so `scan` records where every log
/// it opened lives. Only a hint: `resolve_log_ref` re-checks (dev, inode) on every read.
static PATHS: OnceLock<Mutex<PathIndex>> = OnceLock::new();

#[cfg(test)]
thread_local! {
    /// Index rebuilds (full walks) made by `resolve` on this thread.
    static REBUILDS: Cell<u32> = const { Cell::new(0) };
}

fn paths() -> MutexGuard<'static, PathIndex> {
    // A poisoned cache is still a valid hint map.
    PATHS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Replaces everything indexed for `projects_dir`, dropping entries for files that are gone.
fn store_index(projects_dir: &Path, found: HashMap<(u64, u64), PathBuf>) {
    let mut index = paths();
    index.retain(|(dir, _, _), _| dir != projects_dir);
    index.extend(
        found
            .into_iter()
            .map(|((dev, inode), path)| ((projects_dir.to_path_buf(), dev, inode), path)),
    );
}

fn is_file_at(path: &Path, dev: u64, inode: u64) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.dev() == dev && m.ino() == inode)
}

/// The path of the log with this inode: from the index, else one walk of `projects_dir` that
/// re-indexes it. A hit that no longer names that inode (file moved or replaced) falls back to
/// the same single rebuild.
fn locate(projects_dir: &Path, dev: u64, inode: u64) -> Option<PathBuf> {
    let hit = paths()
        .get(&(projects_dir.to_path_buf(), dev, inode))
        .cloned();
    if let Some(path) = hit.filter(|p| is_file_at(p, dev, inode)) {
        return Some(path);
    }
    #[cfg(test)]
    REBUILDS.with(|n| n.set(n.get() + 1));
    let found: HashMap<_, _> = session_files(projects_dir)
        .into_iter()
        .filter_map(|p| {
            let meta = fs::symlink_metadata(&p).ok()?;
            Some(((meta.dev(), meta.ino()), p))
        })
        .collect();
    let path = found.get(&(dev, inode)).cloned();
    store_index(projects_dir, found);
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use rollcall_core::{Backing, Session, SourceMode, TextField, TitleKind};
    use serde_json::{Value, json};
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::MetadataExt;
    use tempfile::TempDir;

    const S1: &str = "11111111-1111-4111-8111-111111111111";
    const S2: &str = "22222222-2222-4222-8222-222222222222";
    const CWD: &str = "/home/u/p/app";
    const PROJECT: &str = "-home-u-p-app";

    fn t(sec: u32) -> String {
        format!("2026-09-28T18:{:02}:{:02}.000Z", sec / 60, sec % 60)
    }

    fn ts(sec: u32) -> Timestamp {
        t(sec).parse().unwrap()
    }

    fn user(sid: &str, sec: u32, content: Value) -> Value {
        json!({"type":"user","sessionId":sid,"timestamp":t(sec),"cwd":CWD,"gitBranch":"main",
               "isSidechain":false,"message":{"role":"user","content":content}})
    }

    fn assistant(sid: &str, sec: u32) -> Value {
        json!({"type":"assistant","sessionId":sid,"timestamp":t(sec),"cwd":CWD,"gitBranch":"main",
               "message":{"role":"assistant","content":[{"type":"text","text":"ok"}]}})
    }

    fn custom_title(sid: &str, title: &str) -> Value {
        json!({"type":"custom-title","sessionId":sid,"customTitle":title})
    }

    fn ai_title(sid: &str, title: &str) -> Value {
        json!({"type":"ai-title","sessionId":sid,"aiTitle":title})
    }

    fn last_prompt(sid: &str, text: &str) -> Value {
        json!({"type":"last-prompt","sessionId":sid,"leafUuid":S2,"lastPrompt":text})
    }

    fn jsonl(records: &[Value]) -> Vec<u8> {
        let mut out = Vec::new();
        for r in records {
            out.extend_from_slice(r.to_string().as_bytes());
            out.push(b'\n');
        }
        out
    }

    struct Tree {
        _tmp: TempDir,
        projects: PathBuf,
    }

    impl Tree {
        fn new() -> Tree {
            let tmp = TempDir::new().unwrap();
            let projects = tmp.path().join("projects");
            fs::create_dir_all(&projects).unwrap();
            Tree {
                _tmp: tmp,
                projects,
            }
        }

        fn put(&self, name: &str, records: &[Value]) -> PathBuf {
            self.put_bytes(name, &jsonl(records))
        }

        fn put_bytes(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let dir = self.projects.join(PROJECT);
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join(name);
            fs::write(&path, bytes).unwrap();
            path
        }

        fn scan(&self, prior: &SourceState) -> SourceScan {
            scan(&self.projects, prior)
        }

        fn scan_fresh(&self) -> SourceScan {
            self.scan(&SourceState::default())
        }

        fn text(&self, r: &TextRef) -> Option<String> {
            resolve(&self.projects, r)
        }
    }

    fn append(path: &Path, records: &[Value]) {
        let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(&jsonl(records)).unwrap();
    }

    fn state(s: &SourceScan) -> SourceState {
        SourceState {
            files: s.files.clone(),
            sessions: s.sessions.clone(),
        }
    }

    fn only(s: &SourceScan) -> &Session {
        assert_eq!(s.sessions.len(), 1, "sessions: {:?}", s.sessions);
        &s.sessions[0]
    }

    fn title_of(tree: &Tree, s: &Session) -> Option<(String, TitleKind)> {
        let (r, kind) = s.title.as_ref()?;
        Some((tree.text(r)?, *kind))
    }

    fn prompt_of(tree: &Tree, s: &Session) -> Option<String> {
        tree.text(s.last_prompt.as_ref()?)
    }

    /// Everything about a session except the byte positions of its refs, which legitimately
    /// differ between an incremental merge and a scan from zero.
    type View = (
        String,
        PathBuf,
        Option<String>,
        Timestamp,
        Timestamp,
        Option<(String, TitleKind)>,
        Option<String>,
        u32,
    );

    fn view(tree: &Tree, s: &Session) -> View {
        (
            s.id.as_str().to_owned(),
            s.cwd.clone(),
            s.branch.clone(),
            s.started_at,
            s.ended_at,
            title_of(tree, s),
            prompt_of(tree, s),
            s.message_count,
        )
    }

    #[test]
    fn two_files_with_one_session_id_yield_the_later_session() {
        let tree = Tree::new();
        tree.put("a.jsonl", &[user(S1, 1, json!("early file"))]);
        tree.put("b.jsonl", &[user(S1, 9, json!("late file"))]);
        let first = tree.scan_fresh();
        assert_eq!(only(&first).ended_at, ts(9));

        let second = tree.scan(&state(&first));
        assert_eq!(only(&second).ended_at, ts(9));
        assert_eq!(second.diag.sessions, 1);
    }

    #[test]
    fn human_prompt_becomes_prompt_title_last_prompt_and_count() {
        let tree = Tree::new();
        let path = tree.put(
            &format!("{S1}.jsonl"),
            &[
                user(S1, 1, json!("fix the flaky test")),
                assistant(S1, 2),
                user(S1, 3, json!("now run it 20 times")),
            ],
        );
        let scan = tree.scan_fresh();
        let s = only(&scan);
        let meta = fs::metadata(&path).unwrap();

        assert_eq!(s.id.as_str(), S1);
        assert_eq!(s.cwd, PathBuf::from(CWD));
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!((s.started_at, s.ended_at), (ts(1), ts(3)));
        assert_eq!(s.message_count, 2);
        assert_eq!(
            title_of(&tree, s),
            Some(("fix the flaky test".into(), TitleKind::Prompt))
        );
        assert_eq!(prompt_of(&tree, s).as_deref(), Some("now run it 20 times"));
        assert_eq!(
            s.backing,
            Backing::LogFile {
                dev: meta.dev(),
                inode: meta.ino(),
                path
            }
        );
        assert_eq!(scan.diag.mode, SourceMode::Jsonl);
        assert_eq!((scan.diag.files, scan.diag.sessions), (1, 1));
        assert_eq!(scan.diag.records, 3);
        assert_eq!(scan.files.len(), 1);
    }

    #[test]
    fn tool_result_as_last_user_record_is_not_a_prompt() {
        let tree = Tree::new();
        tree.put(
            &format!("{S1}.jsonl"),
            &[
                user(S1, 1, json!("fix the flaky test")),
                user(
                    S1,
                    20,
                    json!([{"type":"tool_result","tool_use_id":"t1","content":"..."}]),
                ),
            ],
        );
        let scan = tree.scan_fresh();
        let s = only(&scan);

        assert_eq!(prompt_of(&tree, s).as_deref(), Some("fix the flaky test"));
        assert_eq!(s.message_count, 1);
        assert_eq!(
            s.ended_at,
            ts(20),
            "the record still extends the time range"
        );
    }

    #[test]
    fn is_meta_records_are_not_prompts() {
        let tree = Tree::new();
        let mut meta = user(S1, 5, json!([{"type":"text","text":"real looking text"}]));
        meta["isMeta"] = json!(true);
        tree.put(&format!("{S1}.jsonl"), &[meta]);
        let scan = tree.scan_fresh();
        let s = only(&scan);

        assert_eq!(s.title, None);
        assert_eq!(s.last_prompt, None);
        assert_eq!(s.message_count, 0);
    }

    #[test]
    fn sidechain_user_records_are_not_prompts() {
        let tree = Tree::new();
        let mut side = user(S1, 5, json!("delegated work"));
        side["isSidechain"] = json!(true);
        tree.put(&format!("{S1}.jsonl"), &[side, user(S1, 6, json!("mine"))]);
        let scan = tree.scan_fresh();
        let s = only(&scan);

        assert_eq!(s.message_count, 1);
        assert_eq!(prompt_of(&tree, s).as_deref(), Some("mine"));
    }

    #[test]
    fn command_name_block_is_not_a_prompt() {
        let tree = Tree::new();
        tree.put(
            &format!("{S1}.jsonl"),
            &[
                user(
                    S1,
                    1,
                    json!([{"type":"text","text":"<command-name>/clear</command-name>"}]),
                ),
                user(S1, 2, json!("# AGENTS.md instructions for /home/u")),
            ],
        );
        let scan = tree.scan_fresh();
        let s = only(&scan);

        assert_eq!(s.message_count, 0);
        assert_eq!(s.title, None);
        assert_eq!(s.last_prompt, None);
    }

    #[test]
    fn injected_marker_after_user_text_is_cut() {
        let tree = Tree::new();
        tree.put(
            &format!("{S1}.jsonl"),
            &[
                user(
                    S1,
                    1,
                    json!("fix it\n<system-reminder>be careful</system-reminder>"),
                ),
                user(
                    S1,
                    2,
                    json!("then ship it <task-notification>done</task-notification>"),
                ),
            ],
        );
        let scan = tree.scan_fresh();
        let s = only(&scan);

        assert_eq!(
            title_of(&tree, s),
            Some(("fix it".into(), TitleKind::Prompt))
        );
        assert_eq!(prompt_of(&tree, s).as_deref(), Some("then ship it"));
        assert_eq!(s.message_count, 2);
    }

    #[test]
    fn text_blocks_are_filtered_one_by_one_and_joined_with_a_space() {
        let tree = Tree::new();
        tree.put(
            &format!("{S1}.jsonl"),
            &[user(
                S1,
                1,
                json!([
                    {"type":"text","text":"<ide_opened_file>a.rs</ide_opened_file>"},
                    {"type":"text","text":"fix the test"},
                    {"type":"text","text":"and the lint"}
                ]),
            )],
        );
        let scan = tree.scan_fresh();

        assert_eq!(
            prompt_of(&tree, only(&scan)).as_deref(),
            Some("fix the test and the lint")
        );
    }

    #[test]
    fn custom_title_beats_ai_title_and_prompt() {
        let tree = Tree::new();
        tree.put(
            &format!("{S1}.jsonl"),
            &[
                user(S1, 1, json!("fix the flaky test")),
                custom_title(S1, "First name"),
                custom_title(S1, "Flaky test hunt"),
                ai_title(S1, "Fix flaky e2e test"),
            ],
        );
        let scan = tree.scan_fresh();

        assert_eq!(
            title_of(&tree, only(&scan)),
            Some(("Flaky test hunt".into(), TitleKind::Custom))
        );
    }

    #[test]
    fn latest_ai_title_beats_prompt() {
        let tree = Tree::new();
        tree.put(
            &format!("{S1}.jsonl"),
            &[
                ai_title(S1, "Older"),
                user(S1, 1, json!("fix the flaky test")),
                ai_title(S1, "Fix flaky e2e test"),
            ],
        );
        let scan = tree.scan_fresh();

        assert_eq!(
            title_of(&tree, only(&scan)),
            Some(("Fix flaky e2e test".into(), TitleKind::Ai))
        );
    }

    #[test]
    fn first_prompt_is_the_title_when_nothing_better_exists() {
        let tree = Tree::new();
        tree.put(
            &format!("{S1}.jsonl"),
            &[user(S1, 1, json!("first")), user(S1, 2, json!("second"))],
        );
        let scan = tree.scan_fresh();

        assert_eq!(
            title_of(&tree, only(&scan)),
            Some(("first".into(), TitleKind::Prompt))
        );
    }

    #[test]
    fn last_prompt_record_wins_over_a_later_human_prompt_and_stays_truncated() {
        let tree = Tree::new();
        tree.put(
            &format!("{S1}.jsonl"),
            &[
                user(S1, 1, json!("fix the flaky test")),
                last_prompt(S1, "now run it 20 tim…"),
                user(S1, 3, json!("a newer human prompt")),
            ],
        );
        let scan = tree.scan_fresh();
        let s = only(&scan);

        assert_eq!(prompt_of(&tree, s).as_deref(), Some("now run it 20 tim…"));
        assert_eq!(s.message_count, 2);
    }

    #[test]
    fn empty_last_prompt_record_is_ignored() {
        let tree = Tree::new();
        tree.put(
            &format!("{S1}.jsonl"),
            &[user(S1, 1, json!("real prompt")), last_prompt(S1, "  ")],
        );
        let scan = tree.scan_fresh();

        assert_eq!(
            prompt_of(&tree, only(&scan)).as_deref(),
            Some("real prompt")
        );
    }

    #[test]
    fn last_prompt_with_zero_human_prompts_is_valid() {
        let tree = Tree::new();
        tree.put(
            &format!("{S1}.jsonl"),
            &[assistant(S1, 1), last_prompt(S1, "carried over")],
        );
        let scan = tree.scan_fresh();
        let s = only(&scan);

        assert_eq!(s.message_count, 0);
        assert_eq!(prompt_of(&tree, s).as_deref(), Some("carried over"));
        assert_eq!(s.title, None);
    }

    #[test]
    fn cwd_is_the_latest_recorded() {
        let tree = Tree::new();
        let mut moved = assistant(S1, 2);
        moved["cwd"] = json!("/home/u/p/app/sub");
        let mut moved_again = assistant(S1, 3);
        moved_again["cwd"] = json!("/home/u/p/other");
        tree.put(
            &format!("{S1}.jsonl"),
            &[
                user(S1, 1, json!("hi")),
                moved,
                moved_again,
                custom_title(S1, "x"),
            ],
        );
        let scan = tree.scan_fresh();

        assert_eq!(only(&scan).cwd, PathBuf::from("/home/u/p/other"));
    }

    #[test]
    fn head_branch_is_detached_and_the_latest_branch_wins() {
        let tree = Tree::new();
        let mut detached = assistant(S1, 2);
        detached["gitBranch"] = json!("HEAD");
        tree.put(
            &format!("{S1}.jsonl"),
            &[user(S1, 1, json!("hi")), detached],
        );
        assert_eq!(only(&tree.scan_fresh()).branch, None);

        let tree = Tree::new();
        let mut feature = assistant(S1, 2);
        feature["gitBranch"] = json!("feature/x");
        tree.put(&format!("{S1}.jsonl"), &[user(S1, 1, json!("hi")), feature]);
        assert_eq!(
            only(&tree.scan_fresh()).branch.as_deref(),
            Some("feature/x")
        );

        let tree = Tree::new();
        let mut bare = user(S1, 1, json!("hi"));
        bare.as_object_mut().unwrap().remove("gitBranch");
        tree.put(&format!("{S1}.jsonl"), &[bare]);
        assert_eq!(only(&tree.scan_fresh()).branch, None);
    }

    #[test]
    fn started_and_ended_are_min_and_max_timestamp() {
        let tree = Tree::new();
        tree.put(
            &format!("{S1}.jsonl"),
            &[
                user(S1, 30, json!("a")),
                assistant(S1, 10),
                assistant(S1, 50),
            ],
        );
        let scan = tree.scan_fresh();
        let s = only(&scan);

        assert_eq!((s.started_at, s.ended_at), (ts(10), ts(50)));
    }

    #[test]
    fn id_falls_back_to_the_filename_stem() {
        let tree = Tree::new();
        let mut rec = user(S2, 1, json!("hi"));
        rec.as_object_mut().unwrap().remove("sessionId");
        tree.put(&format!("{S2}.jsonl"), &[rec]);
        let scan = tree.scan_fresh();

        assert_eq!(only(&scan).id.as_str(), S2);
    }

    #[test]
    fn subagent_files_are_excluded_and_counted() {
        let tree = Tree::new();
        tree.put(&format!("{S1}.jsonl"), &[user(S1, 1, json!("hi"))]);
        let sub = tree.projects.join(PROJECT).join(S1).join("subagents");
        fs::create_dir_all(&sub).unwrap();
        fs::write(
            sub.join("agent-a.jsonl"),
            jsonl(&[user(S2, 1, json!("child"))]),
        )
        .unwrap();
        fs::write(
            sub.join("agent-b.jsonl"),
            jsonl(&[user(S2, 2, json!("child"))]),
        )
        .unwrap();
        let scan = tree.scan_fresh();

        assert_eq!(only(&scan).id.as_str(), S1);
        assert_eq!(scan.diag.excluded_children, 2);
        assert_eq!(scan.diag.files, 1);
    }

    #[test]
    fn non_uuid_ids_are_skipped_and_counted() {
        let tree = Tree::new();
        tree.put("not-a-uuid.jsonl", &[user("not-a-uuid", 1, json!("hi"))]);
        let mut no_id = user(S1, 1, json!("hi"));
        no_id.as_object_mut().unwrap().remove("sessionId");
        tree.put("agent-abc.jsonl", &[no_id]);
        tree.put(&format!("{S1}.jsonl"), &[user(S1, 1, json!("ok"))]);
        let scan = tree.scan_fresh();

        assert_eq!(only(&scan).id.as_str(), S1);
        assert_eq!(scan.diag.skipped.non_uuid, 2);
        assert_eq!(scan.diag.files, 3);
        assert_eq!(
            scan.files.len(),
            1,
            "only the session that was built keeps a file state"
        );
    }

    #[test]
    fn oversize_line_is_skipped_and_the_lines_around_it_are_read() {
        let tree = Tree::new();
        let mut bytes = jsonl(&[user(S1, 1, json!("before"))]);
        bytes.extend_from_slice(&vec![b'x'; 2 << 20]);
        bytes.push(b'\n');
        bytes.extend_from_slice(&jsonl(&[user(S1, 2, json!("after"))]));
        tree.put_bytes(&format!("{S1}.jsonl"), &bytes);
        let scan = tree.scan_fresh();
        let s = only(&scan);

        assert_eq!(scan.diag.skipped.oversize, 1);
        assert_eq!(scan.diag.records, 2);
        assert_eq!(s.message_count, 2);
        assert_eq!(prompt_of(&tree, s).as_deref(), Some("after"));
    }

    #[test]
    fn malformed_and_unknown_records_are_skipped_and_counted() {
        let tree = Tree::new();
        let mut bytes = b"{not json\n[1,2]\n".to_vec();
        bytes.extend_from_slice(&jsonl(&[
            json!({"type":"file-history-snapshot","sessionId":S1}),
            json!({"sessionId":S1}),
            user(S1, 1, json!("hi")),
        ]));
        tree.put_bytes(&format!("{S1}.jsonl"), &bytes);
        let scan = tree.scan_fresh();

        assert_eq!(scan.diag.skipped.malformed, 2);
        assert_eq!(scan.diag.skipped.unknown_type, 2);
        assert_eq!(scan.diag.records, 3);
        assert_eq!(only(&scan).message_count, 1);
    }

    #[test]
    fn unparseable_timestamp_makes_the_record_malformed() {
        let tree = Tree::new();
        let mut bad = user(S1, 2, json!("bad clock"));
        bad["timestamp"] = json!("yesterday");
        tree.put(&format!("{S1}.jsonl"), &[user(S1, 1, json!("good")), bad]);
        let scan = tree.scan_fresh();
        let s = only(&scan);

        assert_eq!(scan.diag.skipped.malformed, 1);
        assert_eq!(s.message_count, 1);
        assert_eq!(s.ended_at, ts(1));
    }

    #[test]
    fn a_file_with_no_cwd_or_timestamps_builds_no_session() {
        let tree = Tree::new();
        tree.put(&format!("{S1}.jsonl"), &[custom_title(S1, "orphan")]);
        let scan = tree.scan_fresh();

        assert!(scan.sessions.is_empty());
        assert!(scan.files.is_empty());
        assert_eq!(scan.notes.len(), 1, "notes: {:?}", scan.notes);
    }

    #[test]
    fn missing_projects_dir_and_empty_tree_are_absent() {
        let tmp = TempDir::new().unwrap();
        let missing = scan(&tmp.path().join("nope"), &SourceState::default());
        assert_eq!(missing.diag.mode, SourceMode::Absent);
        assert!(missing.sessions.is_empty());

        let tree = Tree::new();
        fs::create_dir_all(tree.projects.join(PROJECT)).unwrap();
        fs::write(tree.projects.join(PROJECT).join("notes.txt"), "x").unwrap();
        assert_eq!(tree.scan_fresh().diag.mode, SourceMode::Absent);
    }

    #[test]
    fn only_the_given_projects_dir_is_read() {
        let a = Tree::new();
        let b = Tree::new();
        a.put(&format!("{S1}.jsonl"), &[user(S1, 1, json!("in a"))]);
        b.put(&format!("{S2}.jsonl"), &[user(S2, 1, json!("in b"))]);
        let scan_a = a.scan_fresh();
        let scan_b = b.scan_fresh();

        assert_eq!(only(&scan_a).id.as_str(), S1);
        let b_ref = only(&scan_b).last_prompt.clone().unwrap();
        assert_eq!(b.text(&b_ref).as_deref(), Some("in b"));
        assert_eq!(
            a.text(&b_ref),
            None,
            "a ref into b's tree must not resolve in a"
        );
    }

    #[test]
    fn symlinked_logs_are_not_read() {
        let tree = Tree::new();
        let real = tree.put("real.jsonl.bak", &[user(S1, 1, json!("hi"))]);
        std::os::unix::fs::symlink(
            &real,
            tree.projects.join(PROJECT).join(format!("{S1}.jsonl")),
        )
        .unwrap();
        let scan = tree.scan_fresh();

        assert!(scan.sessions.is_empty());
    }

    #[test]
    fn incremental_scan_reads_only_appended_records_and_merges() {
        let tree = Tree::new();
        let path = tree.put(
            &format!("{S1}.jsonl"),
            &[user(S1, 20, json!("first")), assistant(S1, 21)],
        );
        let first = tree.scan_fresh();
        assert_eq!(first.diag.records, 2);

        let mut later = user(S1, 40, json!("second"));
        later["cwd"] = json!("/home/u/p/elsewhere");
        later["gitBranch"] = json!("HEAD");
        append(&path, &[assistant(S1, 5), later]);
        let second = tree.scan(&state(&first));
        let s = only(&second);

        assert_eq!(
            second.diag.records, 2,
            "only the appended records were read"
        );
        assert_eq!((s.started_at, s.ended_at), (ts(5), ts(40)));
        assert_eq!(s.cwd, PathBuf::from("/home/u/p/elsewhere"));
        assert_eq!(s.branch, None);
        assert_eq!(s.message_count, 2);
        assert_eq!(prompt_of(&tree, s).as_deref(), Some("second"));
        assert_eq!(
            title_of(&tree, s),
            Some(("first".into(), TitleKind::Prompt))
        );
        let full = tree.scan_fresh();
        assert_eq!(view(&tree, s), view(&tree, only(&full)));
    }

    #[test]
    fn incremental_keeps_the_first_prompt_ref_and_custom_title_upgrades_it() {
        let tree = Tree::new();
        let path = tree.put(
            &format!("{S1}.jsonl"),
            &[user(S1, 1, json!("fix the flaky test"))],
        );
        let first = tree.scan_fresh();
        let prompt_title = only(&first).title.clone().unwrap();
        assert_eq!(prompt_title.1, TitleKind::Prompt);

        append(&path, &[user(S1, 2, json!("a second prompt"))]);
        let second = tree.scan(&state(&first));
        assert_eq!(
            only(&second).title.as_ref(),
            Some(&prompt_title),
            "a later prompt must not replace the first-prompt ref"
        );

        append(&path, &[custom_title(S1, "Flaky test hunt")]);
        let third = tree.scan(&state(&second));
        assert_eq!(
            title_of(&tree, only(&third)),
            Some(("Flaky test hunt".into(), TitleKind::Custom))
        );

        append(
            &path,
            &[
                ai_title(S1, "Fix flaky e2e test"),
                user(S1, 3, json!("third")),
            ],
        );
        let fourth = tree.scan(&state(&third));
        assert_eq!(
            title_of(&tree, only(&fourth)),
            Some(("Flaky test hunt".into(), TitleKind::Custom)),
            "a later ai title does not displace a custom one"
        );
        assert_eq!(
            view(&tree, only(&fourth)),
            view(&tree, only(&tree.scan_fresh()))
        );
    }

    #[test]
    fn incremental_ai_title_upgrades_a_prompt_title_and_a_newer_ai_title_replaces_it() {
        let tree = Tree::new();
        let path = tree.put(&format!("{S1}.jsonl"), &[user(S1, 1, json!("first"))]);
        let first = tree.scan_fresh();

        append(&path, &[ai_title(S1, "One")]);
        let second = tree.scan(&state(&first));
        assert_eq!(
            title_of(&tree, only(&second)),
            Some(("One".into(), TitleKind::Ai))
        );

        append(&path, &[ai_title(S1, "Two")]);
        let third = tree.scan(&state(&second));
        assert_eq!(
            title_of(&tree, only(&third)),
            Some(("Two".into(), TitleKind::Ai))
        );
    }

    #[test]
    fn incremental_human_prompt_does_not_displace_a_prior_last_prompt_record() {
        let tree = Tree::new();
        let path = tree.put(
            &format!("{S1}.jsonl"),
            &[
                user(S1, 1, json!("first")),
                last_prompt(S1, "from the record"),
            ],
        );
        let first = tree.scan_fresh();

        append(&path, &[user(S1, 3, json!("newer human prompt"))]);
        let second = tree.scan(&state(&first));
        assert_eq!(
            prompt_of(&tree, only(&second)).as_deref(),
            Some("from the record")
        );

        append(&path, &[last_prompt(S1, "a newer record")]);
        let third = tree.scan(&state(&second));
        assert_eq!(
            prompt_of(&tree, only(&third)).as_deref(),
            Some("a newer record")
        );
        assert_eq!(
            view(&tree, only(&third)),
            view(&tree, only(&tree.scan_fresh()))
        );
    }

    #[test]
    fn incremental_human_prompt_replaces_a_prior_human_last_prompt() {
        let tree = Tree::new();
        let path = tree.put(&format!("{S1}.jsonl"), &[user(S1, 1, json!("first"))]);
        let first = tree.scan_fresh();

        append(&path, &[user(S1, 2, json!("second"))]);
        let second = tree.scan(&state(&first));

        assert_eq!(prompt_of(&tree, only(&second)).as_deref(), Some("second"));
    }

    #[test]
    fn truncated_file_is_rebuilt_from_zero() {
        let tree = Tree::new();
        let path = tree.put(
            &format!("{S1}.jsonl"),
            &[user(S1, 1, json!("old one")), user(S1, 2, json!("old two"))],
        );
        let first = tree.scan_fresh();
        fs::write(&path, jsonl(&[user(S1, 9, json!("new"))])).unwrap();
        let second = tree.scan(&state(&first));
        let s = only(&second);

        assert_eq!(s.message_count, 1);
        assert_eq!((s.started_at, s.ended_at), (ts(9), ts(9)));
        assert_eq!(prompt_of(&tree, s).as_deref(), Some("new"));
    }

    #[test]
    fn older_parser_version_forces_a_full_reread() {
        let tree = Tree::new();
        tree.put(&format!("{S1}.jsonl"), &[user(S1, 1, json!("hi"))]);
        let first = tree.scan_fresh();
        let mut prior = state(&first);
        prior.files[0].parser_version = 0;
        let second = tree.scan(&prior);

        assert_eq!(second.diag.records, 1);
        assert_eq!(only(&second).message_count, 1, "no double counting");
    }

    #[test]
    fn moved_file_keeps_its_session_and_continues_incrementally() {
        let tree = Tree::new();
        let path = tree.put(&format!("{S1}.jsonl"), &[user(S1, 1, json!("hi"))]);
        let first = tree.scan_fresh();
        let other = tree.projects.join("-elsewhere");
        fs::create_dir_all(&other).unwrap();
        let moved = other.join(format!("{S1}.jsonl"));
        fs::rename(&path, &moved).unwrap();
        append(&moved, &[assistant(S1, 2)]);
        let second = tree.scan(&state(&first));
        let s = only(&second);

        assert_eq!(second.diag.records, 1);
        assert_eq!(second.diag.vanished, 0);
        assert_eq!(s.message_count, 1);
        assert!(matches!(&s.backing, Backing::LogFile { path, .. } if *path == moved));
        assert_eq!(second.files[0].path, moved);
        assert_eq!(
            prompt_of(&tree, s).as_deref(),
            Some("hi"),
            "the old ref resolves at the new path"
        );
    }

    #[test]
    fn vanished_file_drops_its_session() {
        let tree = Tree::new();
        let path = tree.put(&format!("{S1}.jsonl"), &[user(S1, 1, json!("hi"))]);
        tree.put(&format!("{S2}.jsonl"), &[user(S2, 1, json!("stays"))]);
        let first = tree.scan_fresh();
        fs::remove_file(&path).unwrap();
        let second = tree.scan(&state(&first));

        assert_eq!(only(&second).id.as_str(), S2);
        assert_eq!(second.diag.vanished, 1);
        assert_eq!(second.files.len(), 1);
    }

    #[test]
    fn unchanged_file_is_not_reread() {
        let tree = Tree::new();
        tree.put(&format!("{S1}.jsonl"), &[user(S1, 1, json!("hi"))]);
        let first = tree.scan_fresh();
        let second = tree.scan(&state(&first));

        assert_eq!(second.diag.records, 0);
        assert_eq!(view(&tree, only(&second)), view(&tree, only(&first)));
    }

    #[test]
    fn resolve_is_stable_after_unrelated_appends() {
        let tree = Tree::new();
        let path = tree.put(
            &format!("{S1}.jsonl"),
            &[
                user(S1, 1, json!("fix the flaky test")),
                custom_title(S1, "Hunt"),
            ],
        );
        let scan = tree.scan_fresh();
        let s = only(&scan);
        let before = (title_of(&tree, s), prompt_of(&tree, s));

        append(
            &path,
            &[
                assistant(S1, 2),
                user(S1, 3, json!("later")),
                ai_title(S1, "x"),
            ],
        );

        assert_eq!(before, (title_of(&tree, s), prompt_of(&tree, s)));
        assert_eq!(before.0, Some(("Hunt".into(), TitleKind::Custom)));
    }

    #[test]
    fn resolve_is_none_after_the_log_is_replaced() {
        let tree = Tree::new();
        let path = tree.put(
            &format!("{S1}.jsonl"),
            &[user(S1, 1, json!("secret prompt"))],
        );
        let scan = tree.scan_fresh();
        let r = only(&scan).last_prompt.clone().unwrap();
        assert_eq!(tree.text(&r).as_deref(), Some("secret prompt"));

        let swap = tree.projects.join(PROJECT).join("swap.tmp");
        fs::write(&swap, fs::read(&path).unwrap()).unwrap();
        fs::rename(&swap, &path).unwrap();
        assert_eq!(tree.text(&r), None, "same bytes, new inode");
    }

    fn rebuilds() -> u32 {
        REBUILDS.with(Cell::get)
    }

    /// Simulates a process that never scanned this tree.
    fn forget_index(tree: &Tree) {
        paths().retain(|(dir, _, _), _| *dir != tree.projects);
    }

    #[test]
    fn resolve_after_scan_does_not_walk() {
        let tree = Tree::new();
        tree.put(&format!("{S1}.jsonl"), &[user(S1, 1, json!("hi"))]);
        let scan = tree.scan_fresh();
        let s = only(&scan);
        let before = rebuilds();

        assert_eq!(title_of(&tree, s).map(|t| t.0).as_deref(), Some("hi"));
        assert_eq!(prompt_of(&tree, s).as_deref(), Some("hi"));
        assert_eq!(rebuilds(), before);
    }

    #[test]
    fn resolve_without_a_scan_walks_once_then_hits_the_index() {
        let tree = Tree::new();
        tree.put(&format!("{S1}.jsonl"), &[user(S1, 1, json!("hi"))]);
        tree.put(&format!("{S2}.jsonl"), &[user(S2, 1, json!("other"))]);
        let scan = tree.scan_fresh();
        forget_index(&tree);
        let before = rebuilds();

        assert_eq!(prompt_of(&tree, &scan.sessions[0]).as_deref(), Some("hi"));
        assert_eq!(rebuilds(), before + 1);
        assert_eq!(
            prompt_of(&tree, &scan.sessions[1]).as_deref(),
            Some("other")
        );
        assert_eq!(prompt_of(&tree, &scan.sessions[0]).as_deref(), Some("hi"));
        assert_eq!(rebuilds(), before + 1, "the rebuild indexed every log");
    }

    #[test]
    fn a_file_moved_after_scan_resolves_via_one_rebuild() {
        let tree = Tree::new();
        let path = tree.put(&format!("{S1}.jsonl"), &[user(S1, 1, json!("hi"))]);
        let scan = tree.scan_fresh();
        let elsewhere = tree.projects.join("-elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::rename(&path, elsewhere.join(format!("{S1}.jsonl"))).unwrap();
        let before = rebuilds();

        assert_eq!(prompt_of(&tree, only(&scan)).as_deref(), Some("hi"));
        assert_eq!(rebuilds(), before + 1);
        assert_eq!(prompt_of(&tree, only(&scan)).as_deref(), Some("hi"));
        assert_eq!(rebuilds(), before + 1);
    }

    #[test]
    fn a_stale_index_entry_never_resolves_a_replaced_log() {
        let tree = Tree::new();
        let path = tree.put(&format!("{S1}.jsonl"), &[user(S1, 1, json!("secret"))]);
        let scan = tree.scan_fresh();
        let swap = tree.projects.join(PROJECT).join("swap.tmp");
        fs::write(&swap, fs::read(&path).unwrap()).unwrap();
        fs::rename(&swap, &path).unwrap();

        assert_eq!(prompt_of(&tree, only(&scan)), None);
    }

    #[test]
    fn indexes_of_different_projects_dirs_do_not_mix() {
        let a = Tree::new();
        let b = Tree::new();
        a.put(&format!("{S1}.jsonl"), &[user(S1, 1, json!("in a"))]);
        b.put(&format!("{S1}.jsonl"), &[user(S1, 1, json!("in b"))]);
        let (scan_a, scan_b) = (a.scan_fresh(), b.scan_fresh());

        assert_eq!(prompt_of(&a, only(&scan_a)).as_deref(), Some("in a"));
        assert_eq!(prompt_of(&b, only(&scan_b)).as_deref(), Some("in b"));
    }

    #[test]
    fn resolve_field_must_fit_the_line_it_points_at() {
        let tree = Tree::new();
        tree.put(
            &format!("{S1}.jsonl"),
            &[
                user(S1, 1, json!("p")),
                custom_title(S1, "T"),
                last_prompt(S1, "L"),
            ],
        );
        let scan = tree.scan_fresh();
        let s = only(&scan);
        let (title_ref, _) = s.title.clone().unwrap();
        let TextRef::LogLine {
            dev,
            inode,
            size_at_scan,
            offset,
            len,
            ..
        } = title_ref
        else {
            panic!("expected a log ref");
        };
        let with_field = |field| TextRef::LogLine {
            dev,
            inode,
            size_at_scan,
            offset,
            len,
            field,
        };

        assert_eq!(
            tree.text(&with_field(TextField::Title)).as_deref(),
            Some("T")
        );
        assert_eq!(tree.text(&with_field(TextField::LastPrompt)), None);
    }
}
