//! GitHub Copilot CLI source: one session per `session-state/<uuid>/events.jsonl` (spec §5.3).
//!
//! The `Copilot` source is still the inert placeholder (P3 registers a real one); `scan` and
//! `resolve` below are the dark parser. Counts follow the Claude reader: `records` is the JSON
//! objects read by this scan (unknown types included, malformed and oversize lines not), so an
//! incremental scan reports only what it appended.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rollcall_core::text::filter_prompt_block;
use rollcall_core::{
    Backing, FileState, Session, SkippedCounts, SourceDiagnostics, SourceKind, SourceMode,
    SourceScan, SourceState, TextField, TextRef, Timestamp, TitleKind, UnmatchedCounts, Uuid,
};
use serde_json::Value;

use crate::claude::{is_file_at, paths, store_index};
use crate::codex_rollout::keep_latest_per_id;
use crate::jsonl::{
    Line, LineReader, ReadFrom, head_fp, open_log, plan_read, resolve_log_ref, tail_fp,
};
use crate::{PARSER_VERSION, SessionSource};

const STORE_DIR: &str = "session-state";
const LOG_NAME: &str = "events.jsonl";
/// `branch` value Copilot records for a detached HEAD.
const DETACHED_BRANCH: &str = "HEAD";
/// First dotted components of the event types this parser knows (§5.3), besides `abort`.
const KNOWN_FAMILIES: [&str; 10] = [
    "session",
    "user",
    "assistant",
    "tool",
    "subagent",
    "hook",
    "skill",
    "permission",
    "system",
    "model",
];

pub struct Copilot;

impl SessionSource for Copilot {
    fn kind(&self) -> SourceKind {
        SourceKind::Copilot
    }

    fn scan(&self, _prior: &SourceState) -> SourceScan {
        SourceScan {
            files: Vec::new(),
            sessions: Vec::new(),
            diag: SourceDiagnostics {
                source: SourceKind::Copilot,
                mode: SourceMode::Absent,
                db_version: None,
                files: 0,
                records: 0,
                skipped: SkippedCounts::default(),
                sessions: 0,
                excluded_children: 0,
                vanished: 0,
                matched: 0,
                unmatched: UnmatchedCounts::default(),
            },
            notes: Vec::new(),
        }
    }

    fn resolve(&self, _r: &TextRef) -> Option<String> {
        None
    }
}

fn type_of(v: &Value) -> &str {
    v.get("type").and_then(Value::as_str).unwrap_or_default()
}

fn is_known(kind: &str) -> bool {
    kind == "abort" || KNOWN_FAMILIES.contains(&kind.split('.').next().unwrap_or_default())
}

/// Sub-agent events carry an envelope `agentId` (assumption, §5.3).
fn has_agent(v: &Value) -> bool {
    v.get("agentId").is_some_and(|a| !a.is_null())
}

/// The `(cwd, branch)` pair a line records, if it is a settled context object (§5.3).
fn context_of(v: &Value) -> Option<(PathBuf, Option<String>)> {
    if has_agent(v) {
        return None;
    }
    let data = v.get("data")?;
    let ctx = match type_of(v) {
        "session.start" | "session.resume" => data.get("context")?,
        "session.context_changed" if data.get("pendingGitContext") != Some(&Value::Bool(true)) => {
            data
        }
        _ => return None,
    };
    let text = |key| {
        ctx.get(key)?
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
    };
    let branch = text("branch").filter(|b| b != DETACHED_BRANCH);
    Some((PathBuf::from(text("cwd")?), branch))
}

/// The human prompt a `user.message` line carries (§5.3), after the §5.0 filter.
fn human_prompt(v: &Value) -> Option<String> {
    if type_of(v) != "user.message" || has_agent(v) {
        return None;
    }
    let data = v.get("data")?;
    if data.get("isAutopilotContinuation") == Some(&Value::Bool(true)) {
        return None;
    }
    filter_prompt_block(data.get("content")?.as_str()?)
}

enum Skip {
    Malformed,
    Unknown,
}

struct Record {
    value: Value,
    timestamp: Option<Timestamp>,
}

/// Order of checks: JSON object, known type with an object `data`, then the timestamp.
fn parse_record(bytes: &[u8]) -> Result<Record, Skip> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| Skip::Malformed)?;
    if !value.is_object() {
        return Err(Skip::Malformed);
    }
    if !is_known(type_of(&value)) || !value.get("data").is_some_and(Value::is_object) {
        return Err(Skip::Unknown);
    }
    let timestamp = match value.get("timestamp") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.parse().map_err(|_| Skip::Malformed)?),
        Some(_) => return Err(Skip::Malformed),
    };
    Ok(Record { value, timestamp })
}

/// A session under construction: the prior session (incremental) plus this scan's records.
#[derive(Default)]
struct Acc {
    context: Option<(PathBuf, Option<String>)>,
    started_at: Option<Timestamp>,
    ended_at: Option<Timestamp>,
    title: Option<(TextRef, TitleKind)>,
    last_prompt: Option<TextRef>,
    message_count: u32,
}

impl Acc {
    fn seeded(prior: &Session) -> Acc {
        Acc {
            context: Some((prior.cwd.clone(), prior.branch.clone())),
            started_at: Some(prior.started_at),
            ended_at: Some(prior.ended_at),
            title: prior.title.clone(),
            last_prompt: prior.last_prompt.clone(),
            message_count: prior.message_count,
        }
    }

    fn apply(&mut self, rec: &Record, at: impl Fn(TextField) -> TextRef) {
        if let Some(ctx) = context_of(&rec.value) {
            self.context = Some(ctx);
        }
        if let Some(t) = rec.timestamp {
            self.started_at = Some(self.started_at.map_or(t, |s| s.min(t)));
            self.ended_at = Some(self.ended_at.map_or(t, |e| e.max(t)));
        }
        if human_prompt(&rec.value).is_some() {
            self.message_count += 1;
            self.last_prompt = Some(at(TextField::LastPrompt));
            if self.title.is_none() {
                self.title = Some((at(TextField::Title), TitleKind::Prompt));
            }
        }
    }

    /// `None` when no cwd or no timestamp was recorded: nothing to match the session by.
    fn finish(self, id: Uuid, backing: Backing) -> Option<Session> {
        let ((cwd, branch), started_at, ended_at) =
            (self.context?, self.started_at?, self.ended_at?);
        Some(Session {
            source: SourceKind::Copilot,
            id,
            cwd,
            branch,
            started_at,
            ended_at,
            title: self.title,
            last_prompt: self.last_prompt,
            message_count: self.message_count,
            backing,
        })
    }
}

/// `(id, log)` for each UUID-named real directory holding a non-directory `events.jsonl`, in a
/// stable order. Entries are judged by `symlink_metadata`, never followed; dot-named entries
/// are skipped silently, other non-UUID directories counted in `non_uuid`.
fn discover(store: &Path, non_uuid: &mut u32) -> Vec<(Uuid, PathBuf)> {
    let Ok(entries) = fs::read_dir(store) else {
        return Vec::new();
    };
    let mut dirs: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .filter(|(name, _)| !name.starts_with('.'))
        .collect();
    dirs.sort();
    let mut logs = Vec::new();
    for (name, dir) in dirs {
        let Some(id) = Uuid::parse(&name) else {
            *non_uuid += 1;
            continue;
        };
        let log = dir.join(LOG_NAME);
        if fs::symlink_metadata(&log).is_ok_and(|m| !m.is_dir()) {
            logs.push((id, log));
        }
    }
    logs
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
    fn read_file(&mut self, id: Uuid, path: &Path) {
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
                    let note = format!("copilot: read error in {}: {e}", path.display());
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
                    if type_of(&rec.value) == "subagent.started" {
                        self.out.diag.excluded_children += 1;
                    }
                    acc.apply(&rec, |field| at(&line, field));
                }
            }
        }
        self.out.diag.skipped.oversize += reader.oversize;

        let backing = Backing::LogFile {
            dev,
            inode,
            path: path.to_path_buf(),
        };
        let Some(session) = acc.finish(id, backing) else {
            self.incomplete += 1;
            return;
        };
        let offset = reader.offset();
        self.out.files.push(FileState {
            dev,
            inode,
            path: path.to_path_buf(),
            size: size_at_scan,
            mtime: Timestamp::new(meta.mtime(), meta.mtime_nsec() as i32)
                .unwrap_or(Timestamp::UNIX_EPOCH),
            offset,
            // Zeroed on failure: never matches a real fingerprint, so the next scan re-reads
            // from 0.
            head_fp: head_fp(&file).unwrap_or_default(),
            tail_fp: tail_fp(&file, offset).unwrap_or_default(),
            parser_version: PARSER_VERSION,
        });
        self.out.sessions.push(session);
    }
}

pub fn scan(copilot_home: &Path, prior: &SourceState) -> SourceScan {
    let store = copilot_home.join(STORE_DIR);
    let mut non_uuid = 0;
    let logs = discover(&store, &mut non_uuid);
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
                source: SourceKind::Copilot,
                mode: SourceMode::Absent,
                db_version: None,
                files: 0,
                records: 0,
                skipped: SkippedCounts {
                    non_uuid,
                    ..SkippedCounts::default()
                },
                sessions: 0,
                excluded_children: 0,
                vanished: 0,
                matched: 0,
                unmatched: UnmatchedCounts::default(),
            },
            notes: Vec::new(),
        },
    };
    for (id, path) in logs.iter().cloned() {
        ctx.read_file(id, &path);
    }

    let vanished = ctx
        .prior_sessions
        .keys()
        .filter(|key| !ctx.seen.contains_key(*key))
        .count();
    store_index(&store, ctx.seen);
    let mut out = ctx.out;
    let (sessions, duplicates) = keep_latest_per_id(out.sessions);
    out.sessions = sessions;
    out.diag.vanished = vanished as u32;
    out.diag.sessions = out.sessions.len() as u32;
    if !logs.is_empty() {
        out.diag.mode = SourceMode::Jsonl;
    }
    if duplicates > 0 {
        out.notes.push(format!(
            "copilot: {duplicates} session file(s) duplicate a session id already seen and were skipped"
        ));
    }
    if ctx.incomplete > 0 {
        out.notes.push(format!(
            "copilot: {} session file(s) skipped: no cwd or timestamp recorded",
            ctx.incomplete
        ));
    }
    out
}

pub fn resolve(copilot_home: &Path, r: &TextRef) -> Option<String> {
    let TextRef::LogLine { dev, inode, .. } = r else {
        return None;
    };
    let path = locate(&copilot_home.join(STORE_DIR), *dev, *inode)?;
    let line = resolve_log_ref(r, &path)?;
    human_prompt(&serde_json::from_slice(&line).ok()?)
}

/// The path of the log with this inode: from the index `scan` filled, else one walk of the
/// store that re-indexes it. Only a hint: `resolve_log_ref` re-checks (dev, inode) on every read.
fn locate(store: &Path, dev: u64, inode: u64) -> Option<PathBuf> {
    let hit = paths().get(&(store.to_path_buf(), dev, inode)).cloned();
    if let Some(path) = hit.filter(|p| is_file_at(p, dev, inode)) {
        return Some(path);
    }
    let found: HashMap<_, _> = discover(store, &mut 0)
        .into_iter()
        .filter_map(|(_, p)| {
            let meta = fs::symlink_metadata(&p).ok()?;
            Some(((meta.dev(), meta.ino()), p))
        })
        .collect();
    let path = found.get(&(dev, inode)).cloned();
    store_index(store, found);
    path
}

#[cfg(test)]
mod tests {
    use rollcall_core::{
        Backing, FileState, Session, SkippedCounts, SourceKind, SourceMode, SourceState, TextField,
        TextRef, Timestamp, UnmatchedCounts, Uuid,
    };

    use super::*;

    const ID: &str = "0a1b2c3d-0000-4000-8000-0123456789ab";

    fn busy_prior() -> SourceState {
        let ts: Timestamp = "2026-09-29T10:00:00Z".parse().unwrap();
        SourceState {
            files: vec![FileState {
                dev: 1,
                inode: 2,
                path: "/copilot/session-state/a/events.jsonl".into(),
                size: 10,
                mtime: ts,
                offset: 10,
                head_fp: [1; 16],
                tail_fp: [2; 16],
                parser_version: 1,
            }],
            sessions: vec![Session {
                source: SourceKind::Copilot,
                id: Uuid::parse(ID).unwrap(),
                cwd: "/A".into(),
                branch: None,
                started_at: ts,
                ended_at: ts,
                title: None,
                last_prompt: None,
                message_count: 1,
                backing: Backing::LogFile {
                    dev: 1,
                    inode: 2,
                    path: "/copilot/session-state/a/events.jsonl".into(),
                },
            }],
        }
    }

    #[test]
    fn the_inert_source_is_absent_and_resolves_nothing() {
        let src = Copilot;
        assert_eq!(src.kind(), SourceKind::Copilot);
        for prior in [SourceState::default(), busy_prior()] {
            let scan = src.scan(&prior);
            assert!(scan.files.is_empty(), "{:?}", scan.files);
            assert!(scan.sessions.is_empty(), "{:?}", scan.sessions);
            assert!(scan.notes.is_empty(), "{:?}", scan.notes);
            let d = &scan.diag;
            assert_eq!(
                (d.source, d.mode, d.db_version),
                (SourceKind::Copilot, SourceMode::Absent, None)
            );
            assert_eq!(
                (d.files, d.records, d.sessions, d.excluded_children),
                (0, 0, 0, 0)
            );
            assert_eq!((d.vanished, d.matched), (0, 0));
            assert_eq!(d.skipped, SkippedCounts::default());
            assert_eq!(d.unmatched, UnmatchedCounts::default());
        }
        let log = TextRef::LogLine {
            dev: 1,
            inode: 2,
            size_at_scan: 10,
            offset: 0,
            len: 5,
            field: TextField::Title,
        };
        let thread = TextRef::CodexThread {
            thread_id: Uuid::parse(ID).unwrap(),
            field: TextField::LastPrompt,
        };
        assert_eq!(src.resolve(&log), None);
        assert_eq!(src.resolve(&thread), None);
    }

    // ---- P2: parser and incremental reads over a scratch COPILOT_HOME ----

    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::mpsc;
    use std::time::Duration;

    use rollcall_core::TitleKind;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    const S1: &str = "11111111-1111-4111-8111-111111111111";
    const S2: &str = "22222222-2222-4222-8222-222222222222";
    const S3: &str = "33333333-3333-4333-8333-333333333333";
    const S4: &str = "44444444-4444-4444-8444-444444444444";
    const S5: &str = "55555555-5555-4555-8555-555555555555";
    const PARENT: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    const MARKER: &str = "PROMPT-MARKER";
    const WATCHDOG: Duration = Duration::from_secs(10);

    fn t(sec: u32) -> String {
        format!("2026-10-07T18:{:02}:{:02}.000Z", sec / 60, sec % 60)
    }

    fn ts(sec: u32) -> Timestamp {
        t(sec).parse().unwrap()
    }

    fn ev(kind: &str, sec: u32, data: Value) -> Value {
        json!({"type":kind,"data":data,"id":S2,"timestamp":t(sec),"parentId":null})
    }

    fn context(cwd: &str, branch: &str) -> Value {
        json!({"cwd":cwd,"gitRoot":cwd,"branch":branch})
    }

    fn start(sec: u32, cwd: &str, branch: &str) -> Value {
        ev(
            "session.start",
            sec,
            json!({"sessionId":S1,"version":1,"producer":"copilot-agent",
                   "copilotVersion":"1.0.93","context":context(cwd, branch)}),
        )
    }

    fn resume(sec: u32, ctx: Value) -> Value {
        ev("session.resume", sec, json!({"eventCount":3,"context":ctx}))
    }

    fn pending(sec: u32, cwd: &str) -> Value {
        let data = json!({"cwd":cwd,"pendingGitContext":true});
        ev("session.context_changed", sec, data)
    }

    fn settled(sec: u32, cwd: &str, branch: &str) -> Value {
        ev("session.context_changed", sec, context(cwd, branch))
    }

    fn shutdown(sec: u32) -> Value {
        ev("session.shutdown", sec, json!({"shutdownType":"routine"}))
    }

    fn user(sec: u32, text: &str) -> Value {
        ev(
            "user.message",
            sec,
            json!({"content":text,"delivery":"idle","parentAgentTaskId":PARENT}),
        )
    }

    fn other(kind: &str, sec: u32) -> Value {
        ev(kind, sec, json!({}))
    }

    fn agent(mut v: Value) -> Value {
        v["agentId"] = json!("agent-1");
        v
    }

    fn jsonl(records: &[Value]) -> Vec<u8> {
        let mut out = Vec::new();
        for r in records {
            out.extend_from_slice(r.to_string().as_bytes());
            out.push(b'\n');
        }
        out
    }

    struct Home {
        tmp: TempDir,
    }

    impl Home {
        fn new() -> Home {
            Home {
                tmp: TempDir::new().unwrap(),
            }
        }

        fn root(&self) -> PathBuf {
            self.tmp.path().join("copilot")
        }

        fn store(&self) -> PathBuf {
            self.root().join("session-state")
        }

        /// `session-state/<name>/events.jsonl`.
        fn put(&self, name: &str, records: &[Value]) -> PathBuf {
            self.put_bytes(name, &jsonl(records))
        }

        fn put_bytes(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let dir = self.store().join(name);
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join("events.jsonl");
            fs::write(&path, bytes).unwrap();
            path
        }

        fn scan(&self, prior: &SourceState) -> SourceScan {
            scan(&self.root(), prior)
        }

        fn fresh(&self) -> SourceScan {
            self.scan(&SourceState::default())
        }

        fn text(&self, r: &TextRef) -> Option<String> {
            resolve(&self.root(), r)
        }
    }

    fn append(path: &Path, bytes: &[u8]) {
        let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(bytes).unwrap();
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

    fn title_of(h: &Home, s: &Session) -> Option<(String, TitleKind)> {
        let (r, kind) = s.title.as_ref()?;
        Some((h.text(r)?, *kind))
    }

    fn prompt_of(h: &Home, s: &Session) -> Option<String> {
        h.text(s.last_prompt.as_ref()?)
    }

    /// Every field of a session except the byte positions of its refs, which legitimately
    /// differ between an incremental merge and a scan from zero.
    #[allow(clippy::type_complexity)]
    fn view(
        h: &Home,
        s: &Session,
    ) -> (
        String,
        PathBuf,
        Option<String>,
        Timestamp,
        Timestamp,
        Option<(String, TitleKind)>,
        Option<String>,
        u32,
    ) {
        (
            s.id.as_str().to_owned(),
            s.cwd.clone(),
            s.branch.clone(),
            s.started_at,
            s.ended_at,
            title_of(h, s),
            prompt_of(h, s),
            s.message_count,
        )
    }

    fn cwd_branch(s: &Session) -> (&str, Option<&str>) {
        (s.cwd.to_str().unwrap(), s.branch.as_deref())
    }

    fn on_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || tx.send(f()).ok());
        rx.recv_timeout(WATCHDOG)
            .expect("scan blocked past the watchdog")
    }

    fn mkfifo(path: &Path) {
        assert!(Command::new("mkfifo").arg(path).status().unwrap().success());
    }

    #[test]
    fn cwd_and_branch_follow_the_latest_settled_context_object() {
        let lines = [
            start(1, "/A", "main"),
            pending(2, "/B"),
            settled(3, "/B", "dev"),
            resume(4, json!({"cwd":"/C"})),
            ev(
                "session.context_changed",
                5,
                json!({"gitRoot":"/D","branch":"x"}),
            ),
        ];
        let want = [
            ("/A", Some("main")),
            ("/A", Some("main")),
            ("/B", Some("dev")),
            ("/C", None),
            ("/C", None),
        ];
        for (k, want) in want.iter().enumerate() {
            let h = Home::new();
            h.put(S1, &lines[..=k]);
            let scan = h.fresh();
            assert_eq!(cwd_branch(only(&scan)), *want, "after line {}", k + 1);
        }
    }

    #[test]
    fn a_resumed_log_takes_the_post_cd_context_and_ends_at_its_last_line() {
        let mut lines = vec![
            start(1, "/A", "main"),
            user(2, "first"),
            shutdown(3),
            resume(4, context("/A", "main")),
            pending(5, "/B"),
            settled(6, "/B", "dev"),
            shutdown(7),
        ];
        let h = Home::new();
        h.put(S1, &lines);
        let scan = h.fresh();
        let s = only(&scan);
        assert_eq!(cwd_branch(s), ("/B", Some("dev")));
        assert_eq!(
            (s.message_count, s.started_at, s.ended_at),
            (1, ts(1), ts(7))
        );
        assert_eq!(s.id.as_str(), S1);

        lines.remove(5);
        let h = Home::new();
        h.put(S1, &lines);
        let scan = h.fresh();
        assert_eq!(cwd_branch(only(&scan)), ("/A", Some("main")));
    }

    #[test]
    fn envelope_agent_id_lines_are_neither_prompts_nor_context() {
        let h = Home::new();
        h.put(
            S1,
            &[
                start(1, "/A", "main"),
                user(2, "one"),
                agent(user(3, "sub-agent text")),
                agent(settled(4, "/X", "z")),
                user(5, "two"),
            ],
        );
        let scan = h.fresh();
        let s = only(&scan);
        assert_eq!(s.message_count, 2);
        assert_eq!(prompt_of(&h, s).as_deref(), Some("two"));
        assert_eq!(title_of(&h, s), Some(("one".into(), TitleKind::Prompt)));
        assert_eq!(cwd_branch(s), ("/A", Some("main")));
    }

    #[test]
    fn autopilot_continuations_are_not_prompts() {
        let count = |flag: bool| {
            let mut u = user(2, "go on");
            u["data"]["isAutopilotContinuation"] = json!(flag);
            let h = Home::new();
            h.put(S1, &[start(1, "/A", "main"), u]);
            only(&h.fresh()).message_count
        };
        assert_eq!((count(true), count(false)), (0, 1));
    }

    #[test]
    fn parent_agent_task_id_does_not_make_a_prompt_a_sub_agent_one() {
        let h = Home::new();
        h.put(S1, &[start(1, "/A", "main"), user(2, "do it")]);
        let scan = h.fresh();
        let s = only(&scan);
        assert_eq!(s.message_count, 1);
        assert_eq!(prompt_of(&h, s).as_deref(), Some("do it"));

        let h = Home::new();
        h.put(S1, &[start(1, "/A", "main"), agent(user(2, "do it"))]);
        assert_eq!(only(&h.fresh()).message_count, 0);
    }

    #[test]
    fn prompts_starting_with_an_angle_bracket_are_filtered() {
        let h = Home::new();
        h.put(
            S1,
            &[
                start(1, "/A", "main"),
                user(2, "  <environment_context>x</environment_context>"),
            ],
        );
        let scan = h.fresh();
        let s = only(&scan);
        assert_eq!(
            (s.message_count, &s.title, &s.last_prompt),
            (0, &None, &None)
        );
    }

    #[test]
    fn subagent_started_lines_count_as_children_not_sessions() {
        let started = |sec| {
            agent(ev(
                "subagent.started",
                sec,
                json!({"toolCallId":"c","agentName":"n","agentDisplayName":"N","model":"m"}),
            ))
        };
        let h = Home::new();
        h.put(
            S1,
            &[
                start(1, "/A", "main"),
                started(2),
                started(3),
                user(4, "hi"),
            ],
        );
        let scan = h.fresh();
        assert_eq!(scan.diag.excluded_children, 2);
        assert_eq!((scan.diag.sessions, only(&scan).message_count), (1, 1));
    }

    #[test]
    fn dot_entries_are_silent_and_non_uuid_directories_are_counted() {
        let h = Home::new();
        let locks = h.store().join(".session-operation-locks");
        fs::create_dir_all(&locks).unwrap();
        fs::write(locks.join(format!("{S1}.lock")), b"").unwrap();
        h.put(&format!(".{S2}"), &[start(1, "/A", "main")]);
        let scan = h.fresh();
        assert_eq!(scan.sessions.len(), 0);
        assert_eq!(scan.diag.skipped, SkippedCounts::default());
        assert_eq!((scan.diag.files, scan.diag.mode), (0, SourceMode::Absent));

        h.put("not-a-uuid", &[start(1, "/A", "main")]);
        let scan = h.fresh();
        assert_eq!(scan.sessions.len(), 0);
        assert_eq!(scan.diag.skipped.non_uuid, 1);
        assert_eq!(scan.diag.files, 0);
    }

    #[test]
    fn legacy_and_odd_entries_produce_nothing_and_change_no_count() {
        let h = Home::new();
        let valid = jsonl(&[start(1, "/A", "main"), user(2, "x")]);
        // A symlink to a valid session directory that lives elsewhere.
        let elsewhere = h.tmp.path().join("elsewhere");
        fs::create_dir_all(elsewhere.join(S1)).unwrap();
        fs::write(elsewhere.join(S1).join("events.jsonl"), &valid).unwrap();
        fs::create_dir_all(h.store()).unwrap();
        symlink(elsewhere.join(S1), h.store().join(S1)).unwrap();
        // Flat legacy file, legacy store, and a directory without a log.
        fs::write(h.store().join(format!("{S2}.jsonl")), &valid).unwrap();
        let legacy = h.root().join("history-session-state").join(S3);
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("events.jsonl"), &valid).unwrap();
        fs::create_dir_all(h.store().join(S4)).unwrap();

        let scan = h.fresh();
        assert_eq!(scan.sessions.len(), 0);
        assert_eq!(scan.diag.skipped, SkippedCounts::default());
        assert_eq!((scan.diag.files, scan.diag.records), (0, 0));
        assert_eq!(scan.diag.mode, SourceMode::Absent);
        assert!(scan.notes.is_empty(), "{:?}", scan.notes);

        // A symlinked events.jsonl is counted in files, then refused.
        fs::create_dir_all(h.store().join(S5)).unwrap();
        symlink(
            elsewhere.join(S1).join("events.jsonl"),
            h.store().join(S5).join("events.jsonl"),
        )
        .unwrap();
        let scan = h.fresh();
        assert_eq!(scan.sessions.len(), 0);
        assert_eq!(scan.diag.skipped, SkippedCounts::default());
        assert_eq!((scan.diag.files, scan.diag.records), (1, 0));
        assert_eq!(scan.diag.mode, SourceMode::Jsonl);

        // So is a FIFO, and the scan does not block on it.
        let fifo_home = Home::new();
        fs::create_dir_all(fifo_home.store().join(S1)).unwrap();
        mkfifo(&fifo_home.store().join(S1).join("events.jsonl"));
        let scan = on_thread(move || fifo_home.fresh());
        assert_eq!(scan.sessions.len(), 0);
        assert_eq!(scan.diag.skipped, SkippedCounts::default());
        assert_eq!((scan.diag.files, scan.diag.records), (1, 0));
    }

    #[test]
    fn known_types_are_silent_and_unknown_malformed_oversize_lines_are_counted() {
        let known = [
            "model.call_start",
            "model.call_end",
            "session.usage_checkpoint",
            "session.info",
            "session.error",
            "session.model_change",
            "system.message",
            "assistant.message",
            "assistant.turn_start",
            "assistant.turn_end",
            "abort",
        ];
        let mut lines = vec![start(5, "/A", "main")];
        lines.extend(
            known
                .iter()
                .enumerate()
                .map(|(i, k)| other(k, 6 + i as u32)),
        );
        let h = Home::new();
        let path = h.put(S1, &lines);
        let scan = h.fresh();
        assert_eq!(scan.diag.skipped, SkippedCounts::default());
        assert_eq!(scan.diag.records, 12);

        let mut early = json!({"type":"made.up","data":{}});
        early["timestamp"] = json!(t(0));
        let mut late = json!({"type":"made.up","data":{}});
        late["timestamp"] = json!(t(50));
        append(&path, &jsonl(&[early, late]));
        let scan = h.fresh();
        assert_eq!(scan.diag.skipped.unknown_type, 2);
        let s = only(&scan);
        assert_eq!((s.started_at, s.ended_at), (ts(5), ts(16)));

        append(&path, b"{\n");
        let mut big = vec![b'x'; (1 << 20) + 1];
        big.push(b'\n');
        append(&path, &big);
        let scan = h.fresh();
        let k = &scan.diag.skipped;
        assert_eq!((k.unknown_type, k.malformed, k.oversize), (2, 1, 1));
    }

    #[test]
    fn a_session_with_no_prompt_is_listed_and_one_without_cwd_or_time_is_not() {
        let h = Home::new();
        h.put(
            S1,
            &[
                start(1, "/A", "main"),
                other("session.model_change", 2),
                shutdown(3),
            ],
        );
        let scan = h.fresh();
        let s = only(&scan);
        assert_eq!(
            (s.message_count, &s.title, &s.last_prompt),
            (0, &None, &None)
        );
        assert_eq!(cwd_branch(s), ("/A", Some("main")));
        assert_eq!((s.started_at, s.ended_at), (ts(1), ts(3)));

        let h = Home::new();
        h.put(
            S1,
            &[
                other("session.model_change", 1),
                other("session.model_change", 2),
            ],
        );
        h.put_bytes(S2, b"");
        let scan = h.fresh();
        assert_eq!(scan.sessions.len(), 0);
        assert_eq!(scan.diag.files, 2);
        assert_eq!(
            scan.notes,
            ["copilot: 2 session file(s) skipped: no cwd or timestamp recorded"]
        );
    }

    #[test]
    fn incremental_scan_equals_a_full_scan_at_every_split_point() {
        let lines = [
            start(1, "/A", "main"),
            user(2, "one"),
            user(3, "two"),
            shutdown(4),
            resume(5, context("/A", "main")),
            pending(6, "/B"),
            settled(7, "/B", "dev"),
            user(8, "three"),
            other("model.call_start", 9),
            other("assistant.turn_start", 10),
            other("assistant.message", 11),
            other("model.call_end", 12),
        ];
        let full = Home::new();
        full.put(S1, &lines);
        let want_scan = full.fresh();
        let want = view(&full, only(&want_scan));
        assert_eq!(want.7, 3);
        for k in 1..lines.len() {
            let h = Home::new();
            let path = h.put(S1, &lines[..k]);
            let first = h.fresh();
            append(&path, &jsonl(&lines[k..]));
            let second = h.scan(&state(&first));
            assert_eq!(view(&h, only(&second)), want, "split after line {k}");
        }
    }

    #[test]
    fn truncating_the_log_rebuilds_from_zero_and_drops_stale_refs() {
        let lines = [
            start(1, "/A", "main"),
            user(2, "a"),
            user(3, "b"),
            user(4, "c"),
        ];
        let h = Home::new();
        let path = h.put(S1, &lines);
        let first = h.fresh();
        let old = only(&first).last_prompt.clone().unwrap();
        assert_eq!(h.text(&old).as_deref(), Some("c"));
        assert_eq!(only(&first).message_count, 3);

        let keep = jsonl(&lines[..3]).len() as u64;
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(keep)
            .unwrap();
        let second = h.scan(&state(&first));
        let s = only(&second);
        assert_eq!((s.message_count, second.diag.records), (2, 3));
        assert_eq!(second.files[0].offset, keep);
        assert_eq!(h.text(&old), None);
        assert_eq!(prompt_of(&h, s).as_deref(), Some("b"));
    }

    #[test]
    fn a_trailing_line_without_a_newline_waits_for_its_newline() {
        let h = Home::new();
        let mut bytes = jsonl(&[start(1, "/A", "main"), user(2, "one")]);
        let complete = bytes.len() as u64;
        bytes.extend_from_slice(user(3, "two").to_string().as_bytes());
        let path = h.put_bytes(S1, &bytes);
        let first = h.fresh();
        assert_eq!(only(&first).message_count, 1);
        assert_eq!(first.files[0].offset, complete);

        append(&path, b"\n");
        let second = h.scan(&state(&first));
        let s = only(&second);
        assert_eq!(s.message_count, 2);
        assert_eq!(prompt_of(&h, s).as_deref(), Some("two"));
    }

    #[test]
    fn prompt_text_is_reachable_only_through_resolve() {
        let h = Home::new();
        let marked = |n: u32| format!("{MARKER} number {n}");
        let path = h.put(
            S1,
            &[
                start(1, "/A", "main"),
                user(2, &marked(1)),
                user(3, &marked(2)),
            ],
        );
        // A second session the reader drops, so a note exists too.
        h.put(S2, &[other("session.model_change", 1)]);
        let first = h.fresh();
        append(&path, &jsonl(&[user(4, &marked(3))]));
        let second = h.scan(&state(&first));
        for scan in [&first, &second] {
            let seen = format!(
                "{:?} {:?} {:?} {:?}",
                scan.notes, scan.diag, scan.files, scan.sessions
            );
            assert!(!seen.contains(MARKER), "{seen}");
        }
        assert!(!second.notes.is_empty());
        let s = &second.sessions[0];
        assert_eq!(prompt_of(&h, s), Some(marked(3)));
        assert_eq!(title_of(&h, s), Some((marked(1), TitleKind::Prompt)));
    }

    #[test]
    fn a_detached_head_branch_is_null() {
        let h = Home::new();
        let path = h.put(S1, &[start(1, "/A", "HEAD")]);
        let first = h.fresh();
        assert_eq!(cwd_branch(only(&first)), ("/A", None));
        append(&path, &jsonl(&[settled(2, "/A", "main")]));
        let second = h.scan(&state(&first));
        assert_eq!(cwd_branch(only(&second)), ("/A", Some("main")));
    }

    #[test]
    fn a_bad_envelope_timestamp_is_malformed_and_an_absent_one_is_not() {
        let base = |u2: Value| vec![start(1, "/A", "main"), user(2, "u1"), u2];
        let run = |lines: Vec<Value>| {
            let h = Home::new();
            h.put(S1, &lines);
            let scan = h.fresh();
            let s = only(&scan);
            (
                scan.diag.skipped.malformed,
                scan.diag.skipped.unknown_type,
                scan.diag.records,
                s.message_count,
                s.ended_at,
            )
        };
        for bad in [json!("yesterday"), json!(42)] {
            let mut u2 = user(3, "u2");
            u2["timestamp"] = bad;
            assert_eq!(run(base(u2)), (1, 0, 2, 1, ts(2)));
        }
        let mut u2 = user(3, "u2");
        u2.as_object_mut().unwrap().remove("timestamp");
        assert_eq!(run(base(u2)), (0, 0, 3, 2, ts(2)));

        let mut lines = base(user(3, "u2"));
        lines.push(json!({"type":"made.up","timestamp":"yesterday","data":{}}));
        assert_eq!(run(lines), (0, 1, 4, 2, ts(3)));
    }

    #[test]
    fn files_records_and_mode_are_counted_as_the_claude_reader_counts_them() {
        let h = Home::new();
        let valid = [start(1, "/A", "main"), user(2, "x")];
        let p1 = h.put(S1, &valid);
        h.put(S2, &valid);
        fs::create_dir_all(h.store().join(".session-operation-locks")).unwrap();
        h.put("not-a-uuid", &valid);
        symlink(h.store().join(S1), h.store().join(S3)).unwrap();
        let scan = h.fresh();
        assert_eq!((scan.diag.files, scan.diag.sessions), (2, 2));
        assert_eq!(scan.diag.mode, SourceMode::Jsonl);

        fs::create_dir_all(h.store().join(S4)).unwrap();
        symlink(&p1, h.store().join(S4).join("events.jsonl")).unwrap();
        fs::create_dir_all(h.store().join(S5)).unwrap();
        mkfifo(&h.store().join(S5).join("events.jsonl"));
        let root = h.root();
        let scan = on_thread(move || scan_keep(&root));
        assert_eq!((scan.diag.files, scan.diag.sessions), (4, 2));
        assert_eq!(scan.sessions.len(), 2);

        let fifo_only = Home::new();
        fs::create_dir_all(fifo_only.store().join(S1)).unwrap();
        mkfifo(&fifo_only.store().join(S1).join("events.jsonl"));
        let scan = on_thread(move || fifo_only.fresh());
        assert_eq!((scan.diag.mode, scan.diag.files), (SourceMode::Jsonl, 1));
        assert_eq!(scan.sessions.len(), 0);

        let empty = Home::new();
        fs::create_dir_all(empty.store()).unwrap();
        assert_eq!(empty.fresh().diag.mode, SourceMode::Absent);

        let h = Home::new();
        let mut bytes = jsonl(&[
            start(1, "/A", "main"),
            user(2, "x"),
            json!({"type":"made.up","data":{}}),
        ]);
        bytes.extend_from_slice(b"{\n");
        bytes.extend(std::iter::repeat_n(b'x', (1 << 20) + 1));
        bytes.push(b'\n');
        let path = h.put_bytes(S1, &bytes);
        let first = h.fresh();
        assert_eq!(first.diag.records, 3);
        append(
            &path,
            &jsonl(&[user(3, "y"), other("assistant.message", 4)]),
        );
        let second = h.scan(&state(&first));
        assert_eq!(second.diag.records, 2);
        assert_eq!(only(&second).message_count, 2);
    }

    fn scan_keep(root: &Path) -> SourceScan {
        scan(root, &SourceState::default())
    }

    #[test]
    fn a_removed_log_is_vanished_on_the_next_scan() {
        let h = Home::new();
        let path = h.put(S1, &[start(1, "/A", "main")]);
        let first = h.fresh();
        fs::remove_file(&path).unwrap();
        let second = h.scan(&state(&first));
        assert_eq!((second.sessions.len(), second.diag.vanished), (0, 1));
    }
}
