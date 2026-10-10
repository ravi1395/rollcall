//! Breaker tests for commit 32ab23a (strict RFC 3339 envelope timestamps, non-empty string
//! `agentId`), spec §5.3 "Envelope `timestamp`" / "Human prompt record" / "Children",
//! ACCEPTANCE P2-19(e), P2-21. Every test drives the public `scan` / `resolve` entry points.

use std::fs;
use std::path::PathBuf;

use rollcall_core::{SourceScan, SourceState, TextRef, Timestamp};
use rollcall_sessions::copilot::{resolve, scan};
use serde_json::{Value, json};
use tempfile::TempDir;

const S1: &str = "11111111-1111-4111-8111-111111111111";
const S2: &str = "22222222-2222-4222-8222-222222222222";

fn t(sec: u32) -> String {
    format!("2026-10-07T18:{:02}:{:02}.000Z", sec / 60, sec % 60)
}

fn ts(sec: u32) -> Timestamp {
    t(sec).parse().unwrap()
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

struct Home(TempDir);

impl Home {
    fn new() -> Home {
        Home(TempDir::new().unwrap())
    }
    fn root(&self) -> PathBuf {
        self.0.path().join("copilot")
    }
    fn put(&self, name: &str, records: &[Value]) {
        let dir = self.root().join("session-state").join(name);
        fs::create_dir_all(&dir).unwrap();
        let mut out = Vec::new();
        for r in records {
            out.extend_from_slice(r.to_string().as_bytes());
            out.push(b'\n');
        }
        fs::write(dir.join("events.jsonl"), out).unwrap();
    }
    fn fresh(&self) -> SourceScan {
        scan(&self.root(), &SourceState::default())
    }
    fn text(&self, r: &TextRef) -> Option<String> {
        resolve(&self.root(), r)
    }
}

/// P2-19 base log with U2's `timestamp` replaced: `(malformed, records, message_count, ended_at)`.
fn stamp_case(stamp: &str) -> (u32, u64, u32, Option<Timestamp>) {
    let mut u = user(3, "u2");
    u["timestamp"] = json!(stamp);
    let h = Home::new();
    h.put(S1, &[start(1, "/A", "main"), user(2, "u1"), u]);
    let s = h.fresh();
    assert_eq!(s.sessions.len(), 1, "{stamp}: {:?}", s.sessions);
    (
        s.diag.skipped.malformed,
        s.diag.records,
        s.sessions[0].message_count,
        Some(s.sessions[0].ended_at),
    )
}

fn malformed() -> (u32, u64, u32, Option<Timestamp>) {
    (1, 2, 1, Some(ts(2)))
}

fn used(at: &str) -> (u32, u64, u32, Option<Timestamp>) {
    (0, 3, 2, Some(at.parse().unwrap()))
}

// ------------------------------------------------------------------ timestamps (§5.3, P2-19 e)

/// §5.3: "an optional `.` and one or more digits" — RFC 3339 `time-secfrac = "." 1*DIGIT` has
/// no upper bound. A 10-digit fraction is a valid shape and a valid instant.
#[test]
fn breaker_fraction_longer_than_nine_digits_is_a_timestamp() {
    let ten = "2026-10-07T18:04:00.1234567890Z";
    let (malformed, records, count, ended) = stamp_case(ten);
    assert_eq!((malformed, records, count), (0, 3, 2), "{ten}");
    // The spec fixes no rounding rule past jiff's nanosecond: only bound the instant.
    let lo: Timestamp = "2026-10-07T18:04:00.123456789Z".parse().unwrap();
    let hi: Timestamp = "2026-10-07T18:04:00.123456790Z".parse().unwrap();
    let ended = ended.unwrap();
    assert!(ended >= lo && ended <= hi, "{ten}: ended_at {ended}");
}

#[test]
fn breaker_fraction_of_thirty_digits_is_a_timestamp() {
    let long = format!("2026-10-07T18:04:00.{}Z", "9".repeat(30));
    let (malformed, records, count, ended) = stamp_case(&long);
    assert_eq!((malformed, records, count), (0, 3, 2), "{long}");
    let lo: Timestamp = "2026-10-07T18:04:00.999999999Z".parse().unwrap();
    let hi: Timestamp = "2026-10-07T18:04:01Z".parse().unwrap();
    let ended = ended.unwrap();
    assert!(ended >= lo && ended <= hi, "{long}: ended_at {ended}");
}

#[test]
fn breaker_nine_digit_fraction_is_a_timestamp() {
    let nine = "2026-10-07T18:04:00.123456789Z";
    assert_eq!(stamp_case(nine), used(nine));
}

/// RFC 3339 §5.6: `time-numoffset = ("+" / "-") time-hour ":" time-minute`, `time-hour` 00-23,
/// `time-minute` 00-59. jiff's `Offset` range is wider (±25:59:59).
#[test]
fn breaker_offset_hour_24_is_malformed() {
    let stamps = [
        "2026-10-07T18:04:00+24:00",
        "2026-10-07T18:04:00-24:00",
        "2026-10-07T18:04:00+25:59",
    ];
    let accepted: Vec<&str> = stamps
        .into_iter()
        .filter(|s| stamp_case(s) != malformed())
        .collect();
    assert!(accepted.is_empty(), "accepted as timestamps: {accepted:?}");
}

#[test]
fn breaker_offset_minute_60_is_malformed() {
    for bad in ["2026-10-07T18:04:00+05:60", "2026-10-07T18:04:00+05:99"] {
        assert_eq!(stamp_case(bad), malformed(), "{bad}");
    }
}

#[test]
fn breaker_offset_beyond_jiff_range_is_malformed() {
    for bad in ["2026-10-07T18:04:00+26:00", "2026-10-07T18:04:00+99:00"] {
        assert_eq!(stamp_case(bad), malformed(), "{bad}");
    }
}

/// `-00:00` is RFC 3339 (unknown local offset) and `+23:59` is the largest legal offset.
#[test]
fn breaker_extreme_legal_offsets_are_timestamps() {
    assert_eq!(
        stamp_case("2026-10-07T18:04:00-00:00"),
        used("2026-10-07T18:04:00Z")
    );
    assert_eq!(
        stamp_case("2026-10-08T18:03:00+23:59"),
        used("2026-10-07T18:04:00Z")
    );
    assert_eq!(
        stamp_case("2026-10-06T18:05:00-23:59"),
        used("2026-10-07T18:04:00Z")
    );
}

/// Every proper prefix of a valid stamp is malformed (truncation at every boundary).
#[test]
fn breaker_every_prefix_of_a_valid_stamp_is_malformed() {
    for full in ["2026-10-07T18:04:00.123Z", "2026-10-07T18:04:00+05:30"] {
        for n in 0..full.len() {
            let p = &full[..n];
            assert_eq!(stamp_case(p), malformed(), "prefix {p:?}");
        }
    }
}

/// Bytes after the offset, single or multibyte.
#[test]
fn breaker_trailing_bytes_after_the_offset_are_malformed() {
    for bad in [
        "2026-10-07T18:04:00ZZ",
        "2026-10-07T18:04:00Z0",
        "2026-10-07T18:04:00+05:300",
        "2026-10-07T18:04:00+05:30Z",
        "2026-10-07T18:04:00+05:30\u{0}",
        "2026-10-07T18:04:00Z\u{e9}",
        "2026-10-07T18:04:00Z\u{1F600}",
        "2026-10-07T18:04:00.123Z\n",
        "\u{feff}2026-10-07T18:04:00Z",
    ] {
        assert_eq!(stamp_case(bad), malformed(), "{bad:?}");
    }
}

/// Non-ASCII look-alikes for digits, `T`, `Z`, `-`, `:`.
#[test]
fn breaker_non_ascii_lookalikes_are_malformed() {
    for bad in [
        "\u{FF12}026-10-07T18:04:00Z",      // fullwidth 2
        "\u{0662}026-10-07T18:04:00Z",      // arabic-indic 2
        "2026-10-07\u{FF34}18:04:00Z",      // fullwidth T
        "2026-10-07T18:04:00\u{FF3A}",      // fullwidth Z
        "2026\u{2010}10-07T18:04:00Z",      // unicode hyphen
        "2026-10-07T18\u{FF1A}04:00Z",      // fullwidth colon
        "2026-10-07T18:04:00\u{2212}05:00", // minus sign
        "2026-10-07T18:04:00.\u{FF11}Z",    // fullwidth digit in fraction
        "2026-10-07T18:04:00+\u{FF10}5:00",
    ] {
        assert_eq!(stamp_case(bad), malformed(), "{bad:?}");
    }
}

/// Valid shape, invalid instant.
#[test]
fn breaker_valid_shape_invalid_instant_is_malformed() {
    for bad in [
        "2026-02-30T18:04:00Z",
        "2026-02-29T18:04:00Z",
        "2026-13-07T18:04:00Z",
        "2026-00-07T18:04:00Z",
        "2026-10-00T18:04:00Z",
        "2026-10-32T18:04:00Z",
        "2026-04-31T18:04:00Z",
        "2026-10-07T24:00:00Z",
        "2026-10-07T18:60:00Z",
        "2026-10-07T18:04:61Z",
    ] {
        assert_eq!(stamp_case(bad), malformed(), "{bad}");
    }
}

/// Shape variants the grammar excludes.
#[test]
fn breaker_shape_variants_are_malformed() {
    for bad in [
        "2026-10-07T18:04:00.Z",
        "2026-10-07T18:04:00.123.4Z",
        "2026-10-07T18:04:00,123Z",
        "2026-10-07T18:04:00.123",
        "2026-10-07T18:04:00.123 Z",
        "2026-10-07T18:04:00 +05:30",
        "2026-10-07T18:04:00+5:30",
        "2026-10-07T18:04:00+05",
        "2026-10-07T18:04:00+0530",
        "2026-10-07T18:04:00+05:3",
        "2026-10-07T18:04:00-5:30",
        "2026-10-07T18:04:00UTC",
        "2026-10-07T18:04:00Z[UTC]",
        "+2026-10-07T18:04:00Z",
        "02026-10-07T18:04:00Z",
        "026-10-07T18:04:00Z",
        "2026-1-07T18:04:00Z",
        "2026-10-7T18:04:00Z",
        "2026-10-07T8:04:00Z",
        "2026-10-07T18:4:00Z",
        "2026-10-07T18:04:0Z",
        "2026/10/07T18:04:00Z",
        "2026-10-07_18:04:00Z",
        " 2026-10-07T18:04:00Z",
        "",
        "Z",
        "T",
    ] {
        assert_eq!(stamp_case(bad), malformed(), "{bad:?}");
    }
}

/// Lowercase `t`/`z` and the year-0000 / 9999 edges are in-grammar and valid instants.
#[test]
fn breaker_grammar_edges_are_timestamps() {
    assert_eq!(
        stamp_case("2026-10-07t18:04:00z"),
        used("2026-10-07T18:04:00Z")
    );
    assert_eq!(
        stamp_case("2026-10-07t18:04:00.5Z"),
        used("2026-10-07T18:04:00.5Z")
    );
    assert_eq!(
        stamp_case("2026-10-07T23:34:00.25+05:30"),
        used("2026-10-07T18:04:00.25Z")
    );
    assert_eq!(
        stamp_case("2026-10-07T13:04:00.250000000-05:00"),
        used("2026-10-07T18:04:00.25Z")
    );
    // ended_at is max: 9999-12-30 is inside jiff's Timestamp range and later than U1.
    assert_eq!(
        stamp_case("9999-12-30T00:00:00Z"),
        used("9999-12-30T00:00:00Z")
    );
    // Year 0000 is RFC 3339 and jiff-representable; ended_at stays U1's (max), malformed 0.
    assert_eq!(stamp_case("0000-01-01T00:00:00Z"), (0, 3, 2, Some(ts(2))));
}

/// A `session.start` with a malformed timestamp is itself malformed: no context object, so
/// the log yields no session (rule applies to every line, not only `user.message`).
#[test]
fn breaker_malformed_timestamp_on_the_context_line_drops_the_session() {
    let mut s = start(1, "/A", "main");
    s["timestamp"] = json!("2026-10-07T18:04Z");
    let h = Home::new();
    h.put(S1, &[s, user(2, "u1")]);
    let scan = h.fresh();
    assert_eq!(scan.diag.skipped.malformed, 1);
    assert_eq!(scan.diag.records, 1);
    assert!(scan.sessions.is_empty(), "{:?}", scan.sessions);
    assert_eq!(scan.diag.sessions, 0);
}

// ------------------------------------------------------------------ agentId (§5.3, P2-21)

/// `message_count` with U2's envelope `agentId` set to `id`.
fn count_with_agent(id: Value) -> u32 {
    let mut u = user(3, "u2");
    u["agentId"] = id;
    let h = Home::new();
    h.put(S1, &[start(1, "/A", "main"), user(2, "u1"), u]);
    let s = h.fresh();
    assert_eq!(s.sessions.len(), 1);
    s.sessions[0].message_count
}

/// Spec: only a non-empty *string* marks a sub-agent. Every non-string JSON value is human.
#[test]
fn breaker_non_string_agent_ids_are_human() {
    for human in [
        json!(null),
        json!(""),
        json!(0),
        json!(-1),
        json!(1.5),
        json!(true),
        json!(false),
        json!({}),
        json!({"id":"a1"}),
        json!([]),
        json!(["a1"]),
    ] {
        assert_eq!(count_with_agent(human.clone()), 2, "{human}");
    }
}

/// Whitespace-only and multibyte strings are non-empty strings: sub-agent by the letter of
/// the spec. AMBIGUOUS if the author meant trimmed.
#[test]
fn breaker_whitespace_only_agent_id_is_a_sub_agent() {
    for agent in [
        json!(" "),
        json!("\t"),
        json!("\n"),
        json!("\u{a0}"),
        json!("\u{0}"),
    ] {
        assert_eq!(count_with_agent(agent.clone()), 1, "{agent:?}");
    }
    assert_eq!(count_with_agent(json!("\u{1F600}")), 1);
    assert_eq!(count_with_agent(json!("a1")), 1);
}

/// Consistency: settled `session.context_changed` lines with a non-marker `agentId` move cwd;
/// with a string marker they do not.
#[test]
fn breaker_agent_id_rule_applies_to_context_objects() {
    let case = |id: Value| {
        let mut c = ev(
            "session.context_changed",
            2,
            json!({"cwd":"/B","gitRoot":"/B","branch":"dev"}),
        );
        c["agentId"] = id;
        let h = Home::new();
        h.put(S1, &[start(1, "/A", "main"), c]);
        let s = h.fresh();
        assert_eq!(s.sessions.len(), 1);
        (s.sessions[0].cwd.clone(), s.sessions[0].branch.clone())
    };
    let moved = (PathBuf::from("/B"), Some("dev".to_owned()));
    let stayed = (PathBuf::from("/A"), Some("main".to_owned()));
    for human in [json!(null), json!(""), json!(7), json!({}), json!([])] {
        assert_eq!(case(human.clone()), moved, "{human}");
    }
    for agent in [json!("x"), json!(" ")] {
        assert_eq!(case(agent.clone()), stayed, "{agent}");
    }
}

/// Consistency: `session.start` / `session.resume` context with a string `agentId` is not a
/// context object either.
#[test]
fn breaker_agent_id_rule_applies_to_start_and_resume_context() {
    let h = Home::new();
    let mut s = start(1, "/A", "main");
    s["agentId"] = json!("x");
    let mut r = ev(
        "session.resume",
        2,
        json!({"context":{"cwd":"/R","gitRoot":"/R","branch":"r"}}),
    );
    r["agentId"] = json!("");
    h.put(S1, &[s, r]);
    let scan = h.fresh();
    assert_eq!(scan.sessions.len(), 1, "{:?}", scan.notes);
    assert_eq!(scan.sessions[0].cwd, PathBuf::from("/R"));
    assert_eq!(scan.sessions[0].branch.as_deref(), Some("r"));
}

/// §5.3 Children: "each `subagent.started` line read counts once in `excluded_children`" —
/// unconditional on its `agentId` (or the lack of one).
#[test]
fn breaker_subagent_started_counts_whatever_its_agent_id() {
    let h = Home::new();
    let data = json!({"toolCallId":"t","agentName":"n","agentDisplayName":"d","model":"m"});
    let mut lines = vec![start(1, "/A", "main")];
    for (i, id) in [json!("a1"), json!(""), json!(null), json!(7)]
        .into_iter()
        .enumerate()
    {
        let mut l = ev("subagent.started", 2 + i as u32, data.clone());
        l["agentId"] = id;
        lines.push(l);
    }
    let mut no_key = ev("subagent.started", 9, data.clone());
    no_key.as_object_mut().unwrap().remove("agentId");
    lines.push(no_key);
    h.put(S1, &lines);
    let scan = h.fresh();
    assert_eq!(scan.diag.excluded_children, 5);
    assert_eq!(scan.diag.records, 6);
    assert_eq!(scan.sessions.len(), 1);
    assert_eq!(scan.sessions[0].message_count, 0);
}

/// Sub-agent lines still supply timestamps (they are not skipped) and resolve re-applies the
/// same rule: a prompt whose envelope `agentId` is `""` resolves to its text.
#[test]
fn breaker_agent_lines_supply_timestamps_and_resolve_matches_scan() {
    let h = Home::new();
    let mut late = user(9, "sub");
    late["agentId"] = json!("a1");
    let mut empty = user(3, "PROMPT-MARKER empty-id");
    empty["agentId"] = json!("");
    h.put(S1, &[start(1, "/A", "main"), user(2, "u1"), empty, late]);
    let scan = h.fresh();
    let s = &scan.sessions[0];
    assert_eq!(s.ended_at, ts(9));
    assert_eq!(s.message_count, 2);
    let last = s.last_prompt.as_ref().unwrap();
    assert_eq!(h.text(last).as_deref(), Some("PROMPT-MARKER empty-id"));
}

/// A `user.message` with a string `agentId` is not a prompt, so a title ref is never taken
/// from it; and `resolve` of a ref pointing at such a line must be `None` (same rule).
#[test]
fn breaker_resolve_never_yields_a_sub_agent_prompt() {
    let h = Home::new();
    let mut sub = user(2, "SUB-TEXT");
    sub["agentId"] = json!("a1");
    h.put(S1, &[start(1, "/A", "main"), sub.clone(), user(3, "human")]);
    let scan = h.fresh();
    let s = &scan.sessions[0];
    let (title, _) = s.title.as_ref().unwrap();
    assert_eq!(h.text(title).as_deref(), Some("human"));
    // Forge a ref at the sub-agent line: same file, the byte range of line 2.
    let TextRef::LogLine {
        dev,
        inode,
        size_at_scan,
        field,
        ..
    } = title.clone()
    else {
        panic!("not a log line")
    };
    let first = start(1, "/A", "main").to_string().len() as u64 + 1;
    let sub_len = sub.to_string().len();
    let forge = |offset: u64, len: usize| TextRef::LogLine {
        dev,
        inode,
        size_at_scan,
        offset,
        len: len as u32,
        field,
    };
    // Positive control with the same arithmetic: line 3 forged by hand resolves to its text.
    let third = forge(
        first + sub_len as u64 + 1,
        user(3, "human").to_string().len(),
    );
    assert_eq!(h.text(&third).as_deref(), Some("human"));
    assert_eq!(h.text(&forge(first, sub_len)), None);
}
