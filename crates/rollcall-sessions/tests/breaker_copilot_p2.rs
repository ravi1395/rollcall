//! Breaker tests for P2 (copilot.rs parser and incremental reads), spec §5.0, §5.3, §6.2, §6.3.

use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use rollcall_core::{Session, SourceScan, SourceState, TextRef, Timestamp, TitleKind};
use rollcall_sessions::copilot::{resolve, scan};
use serde_json::{Value, json};
use tempfile::TempDir;

const S1: &str = "11111111-1111-4111-8111-111111111111";
const S2: &str = "22222222-2222-4222-8222-222222222222";
const MIB: usize = 1 << 20;

fn t(sec: u32) -> String {
    format!("2026-10-07T18:{:02}:{:02}.000Z", sec / 60, sec % 60)
}

fn ev(kind: &str, sec: u32, data: Value) -> Value {
    json!({"type":kind,"data":data,"id":S2,"timestamp":t(sec),"parentId":null})
}

fn start(sec: u32, cwd: &str, branch: &str) -> Value {
    ev(
        "session.start",
        sec,
        json!({"sessionId":S1,"context":{"cwd":cwd,"gitRoot":cwd,"branch":branch}}),
    )
}

fn user(sec: u32, text: &str) -> Value {
    ev(
        "user.message",
        sec,
        json!({"content":text,"delivery":"idle"}),
    )
}

fn jsonl(records: &[Value]) -> Vec<u8> {
    let mut out = Vec::new();
    for r in records {
        out.extend_from_slice(r.to_string().as_bytes());
        out.push(b'\n');
    }
    out
}

struct Home(TempDir);

impl Home {
    fn new() -> Home {
        Home(TempDir::new().unwrap())
    }
    fn root(&self) -> PathBuf {
        self.0.path().join("copilot")
    }
    fn store(&self) -> PathBuf {
        self.root().join("session-state")
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

/// Every `Session` field, refs compared by resolved text (as P2-12 prescribes).
fn view(h: &Home, s: &Session) -> View {
    (
        s.id.as_str().to_owned(),
        s.cwd.clone(),
        s.branch.clone(),
        s.started_at,
        s.ended_at,
        s.title.as_ref().and_then(|(r, k)| Some((h.text(r)?, *k))),
        s.last_prompt.as_ref().and_then(|r| h.text(r)),
        s.message_count,
    )
}

/// §6.3 "Partial trailing line → stop before it" + §5.3 "An incremental read yields the same
/// `Session` as a full re-read": split the log at every byte, not only at line boundaries.
#[test]
fn breaker_copilot_incremental_equals_full_at_every_byte_split() {
    let lines = [
        start(1, "/A", "main"),
        ev(
            "session.context_changed",
            2,
            json!({"cwd":"/B","pendingGitContext":true}),
        ),
        user(3, "one"),
        ev(
            "session.context_changed",
            4,
            json!({"cwd":"/B","gitRoot":"/B","branch":"dev"}),
        ),
        {
            let mut a = user(5, "sub");
            a["agentId"] = json!("a1");
            a
        },
        user(6, "two"),
    ];
    let mut bytes = jsonl(&lines);
    let mid = jsonl(&lines[..3]).len();
    bytes.splice(mid..mid, b"{\n".iter().copied());
    bytes.extend_from_slice(b"{\"type\":\"made.up\",\"data\":{}}\n");

    let full = Home::new();
    full.put_bytes(S1, &bytes);
    let want_scan = full.fresh();
    let want = view(&full, only(&want_scan));
    assert_eq!(want.7, 2);

    for k in 1..bytes.len() {
        let h = Home::new();
        let path = h.put_bytes(S1, &bytes[..k]);
        let first = h.fresh();
        append(&path, &bytes[k..]);
        let second = h.scan(&state(&first));
        assert_eq!(view(&h, only(&second)), want, "split at byte {k}");
    }
}

/// §5.0 "lines > 1 MiB are skipped": a line of exactly 1 MiB is not oversize; it is a prompt
/// record and resolvable (§6.2 "read len (≤ 1 MiB) bytes").
#[test]
fn breaker_copilot_a_line_of_exactly_one_mib_is_a_prompt() {
    let probe = user(2, "").to_string().len();
    let text = "y".repeat(MIB - probe);
    let line = user(2, &text).to_string();
    assert_eq!(line.len(), MIB);
    let h = Home::new();
    let mut bytes = jsonl(&[start(1, "/A", "main")]);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    h.put_bytes(S1, &bytes);
    let scan = h.fresh();
    assert_eq!(scan.diag.skipped.oversize, 0);
    assert_eq!(scan.diag.records, 2);
    let s = only(&scan);
    assert_eq!(s.message_count, 1);
    assert_eq!(s.last_prompt.as_ref().and_then(|r| h.text(r)), Some(text));
}

/// §6.3 partial trailing line, for an oversize line torn across two scans: the first scan stops
/// before it, the second counts it once.
#[test]
fn breaker_copilot_a_torn_oversize_line_is_counted_once_after_its_newline() {
    for written in [MIB / 2, MIB + 10] {
        let h = Home::new();
        let mut bytes = jsonl(&[start(1, "/A", "main")]);
        let complete = bytes.len() as u64;
        bytes.extend(std::iter::repeat_n(b'x', written));
        let path = h.put_bytes(S1, &bytes);
        let first = h.fresh();
        assert_eq!(first.diag.skipped.oversize, 0, "written {written}");
        assert_eq!(first.files[0].offset, complete, "written {written}");

        let mut rest = vec![b'x'; MIB + 1 - written.min(MIB + 1)];
        rest.push(b'\n');
        rest.extend(jsonl(&[user(2, "after")]));
        append(&path, &rest);
        let second = h.scan(&state(&first));
        assert_eq!(second.diag.skipped.oversize, 1, "written {written}");
        assert_eq!(only(&second).message_count, 1, "written {written}");
    }
}

/// §6.3 / §5.3 incremental equivalence when the prefix scan produced no session (no timestamp,
/// or no context object yet).
#[test]
fn breaker_copilot_incremental_from_an_incomplete_prefix_equals_full() {
    let mut untimed = start(1, "/A", "main");
    untimed["timestamp"] = Value::Null;
    let cases = [
        (vec![untimed], vec![user(2, "p")]),
        (
            vec![ev("session.model_change", 1, json!({}))],
            vec![start(2, "/A", "main"), user(3, "p")],
        ),
    ];
    for (prefix, rest) in cases {
        let all: Vec<Value> = prefix.iter().chain(&rest).cloned().collect();
        let full = Home::new();
        full.put_bytes(S1, &jsonl(&all));
        let want = view(&full, only(&full.fresh()));

        let h = Home::new();
        let path = h.put_bytes(S1, &jsonl(&prefix));
        let first = h.fresh();
        assert!(first.sessions.is_empty());
        append(&path, &jsonl(&rest));
        let second = h.scan(&state(&first));
        assert_eq!(view(&h, only(&second)), want);
    }
}

/// §6.2: resolution keys on (dev, inode); the path is only a hint. A renamed session directory
/// (same inode) still resolves.
#[test]
fn breaker_copilot_resolve_follows_a_renamed_session_dir() {
    let h = Home::new();
    h.put_bytes(S1, &jsonl(&[start(1, "/A", "main"), user(2, "hello")]));
    let first = h.fresh();
    let r = only(&first).last_prompt.clone().unwrap();
    fs::rename(h.store().join(S1), h.store().join(S2)).unwrap();
    assert_eq!(h.text(&r).as_deref(), Some("hello"));
}

/// §6.2 "require the same (dev, inode)": a byte-identical replacement file (new inode), a
/// symlink to the original, or another home all resolve to `None`.
#[test]
fn breaker_copilot_resolve_refuses_a_different_file() {
    let bytes = jsonl(&[start(1, "/A", "main"), user(2, "hello")]);

    let h = Home::new();
    let path = h.put_bytes(S1, &bytes);
    let r = only(&h.fresh()).last_prompt.clone().unwrap();
    let other = Home::new();
    other.put_bytes(S1, &bytes);
    other.fresh();
    assert_eq!(other.text(&r), None, "ref from another home");

    let tmp = path.with_extension("new");
    fs::write(&tmp, &bytes).unwrap();
    fs::rename(&tmp, &path).unwrap();
    assert_eq!(h.text(&r), None, "replaced file, new inode");

    let h = Home::new();
    let path = h.put_bytes(S1, &bytes);
    let r = only(&h.fresh()).last_prompt.clone().unwrap();
    let kept = h.0.path().join("kept.jsonl");
    fs::rename(&path, &kept).unwrap();
    symlink(&kept, &path).unwrap();
    assert_eq!(h.text(&r), None, "symlink to the original inode");
}

// ---- AMBIGUOUS probes: the spec sentence each relies on is silent on the exact case ----

/// §5.3 "a string that is not RFC 3339 → malformed". `2026-10-07T18:04Z` (no seconds) is not
/// RFC 3339; the reader uses the shared timestamp parser, which may accept it.
#[test]
fn breaker_copilot_ambiguous_non_rfc3339_timestamp_is_malformed() {
    let mut accepted = Vec::new();
    for bad in [
        "2026-10-07T18:04Z",
        "20261007T180400Z",
        "2026-10-07T18:04:00Z[UTC]",
    ] {
        let mut u = user(3, "u2");
        u["timestamp"] = json!(bad);
        let h = Home::new();
        h.put_bytes(S1, &jsonl(&[start(1, "/A", "main"), user(2, "u1"), u]));
        if h.fresh().diag.skipped.malformed != 1 {
            accepted.push(bad);
        }
    }
    assert!(accepted.is_empty(), "accepted as RFC 3339: {accepted:?}");
}

/// §5.3 "no envelope agentId": is `"agentId": ""` an agent line? The reader says yes.
#[test]
fn breaker_copilot_ambiguous_empty_agent_id_is_not_an_agent() {
    let mut u = user(2, "hi");
    u["agentId"] = json!("");
    let h = Home::new();
    h.put_bytes(S1, &jsonl(&[start(1, "/A", "main"), u]));
    assert_eq!(only(&h.fresh()).message_count, 1);
}

/// §6.3 "tail_fp differs → re-read from 0": a rewind that truncates and then appends past the old
/// offset must rebuild, giving what a full scan of the new content gives.
#[test]
fn breaker_copilot_rewind_then_regrow_past_old_offset_rebuilds() {
    let old = [
        start(1, "/A", "main"),
        user(2, "a"),
        user(3, "b"),
        user(4, "c"),
    ];
    let new_tail = [
        user(5, "x-longer-text"),
        user(6, "y-longer-text"),
        user(7, "z"),
    ];
    let h = Home::new();
    let path = h.put_bytes(S1, &jsonl(&old));
    let first = h.fresh();
    let keep = jsonl(&old[..2]);
    let mut regrown = keep.clone();
    regrown.extend(jsonl(&new_tail));
    assert!(regrown.len() > jsonl(&old).len());
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(keep.len() as u64)
        .unwrap();
    append(&path, &jsonl(&new_tail));
    let second = h.scan(&state(&first));

    let full = Home::new();
    full.put_bytes(S1, &regrown);
    let want = view(&full, only(&full.fresh()));
    assert_eq!(view(&h, only(&second)), want);
    assert_eq!(want.7, 4);
}

/// §5.0 "read only up to the size observed at open" + §6.3 partial lines, under a real
/// concurrent writer appending a few bytes at a time: chained incremental scans end equal to a
/// full scan.
#[test]
fn breaker_copilot_incremental_scans_under_a_concurrent_writer_equal_full() {
    let mut lines = vec![start(1, "/A", "main")];
    for i in 0..40 {
        lines.push(user(2 + i, &format!("prompt {i}")));
        if i == 20 {
            lines.push(ev(
                "session.context_changed",
                100,
                json!({"cwd":"/B","gitRoot":"/B","branch":"dev"}),
            ));
        }
    }
    let bytes = jsonl(&lines);
    let h = Home::new();
    let head = jsonl(&lines[..1]);
    let path = h.put_bytes(S1, &head);
    let rest = bytes[head.len()..].to_vec();
    let writer_path = path.clone();
    let writer = std::thread::spawn(move || {
        for chunk in rest.chunks(7) {
            append(&writer_path, chunk);
        }
    });
    let mut prior = state(&h.fresh());
    while !writer.is_finished() {
        prior = state(&h.scan(&prior));
    }
    writer.join().unwrap();
    let last = h.scan(&prior);

    let full = Home::new();
    full.put_bytes(S1, &bytes);
    let want = view(&full, only(&full.fresh()));
    assert_eq!(view(&h, only(&last)), want);
    assert_eq!(want.7, 40);
}
