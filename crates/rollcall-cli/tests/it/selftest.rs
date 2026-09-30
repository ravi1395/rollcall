use super::support::*;
use std::process::Command;
use std::time::{Duration, Instant};

const WATCHDOG_DEADLINE: Duration = Duration::from_millis(500);
const RSS_ALLOC_BYTES: u64 = 64 * 1024 * 1024;
const PAGE_STEP: u64 = 4096;

#[test]
fn watchdog() {
    let mut cmd = Command::new("sleep");
    cmd.arg("30");
    let start = Instant::now();
    let res = spawn_watchdog(cmd, WATCHDOG_DEADLINE);
    let took = start.elapsed();
    assert!(
        res.is_err(),
        "a 30 s sleep must not finish under a 500 ms deadline"
    );
    assert!(
        took < WATCHDOG_DEADLINE + Duration::from_secs(1),
        "watchdog took {took:?}"
    );
    // A child that finishes in time must come back intact.
    let mut quick = Command::new("echo");
    quick.arg("hi");
    let out = spawn_watchdog(quick, Duration::from_secs(30)).expect("echo finishes");
    assert_eq!(out.stdout, b"hi\n");
}

#[test]
fn manifest_parses() {
    let text = std::fs::read_to_string(manifest_path()).expect("read MANIFEST.toml");
    let m = parse_manifest(&text).expect("MANIFEST.toml parses with no unknown keys");
    assert!(!m.repo.is_empty() && !m.claude.is_empty());
    assert!(m.expect_counts.f_repos > 0);
    let bad = format!("{text}\n[meta_extra]\nx = 1\n");
    assert!(
        parse_manifest(&bad).is_err(),
        "an unknown table must be rejected"
    );
}

#[test]
fn peak_rss() {
    let Some(python) = require_tool("python3") else {
        return;
    };
    if require_tool("/usr/bin/time").is_none() {
        return;
    }
    let script = format!(
        "b = bytearray({RSS_ALLOC_BYTES})\nfor i in range(0, {RSS_ALLOC_BYTES}, {PAGE_STEP}): b[i] = 1"
    );
    let mut cmd = Command::new(python);
    cmd.args(["-c", &script]);
    let (out, rss) = super::support::peak_rss(cmd, Duration::from_secs(60));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(rss >= RSS_ALLOC_BYTES, "peak rss {rss} < {RSS_ALLOC_BYTES}");
    assert!(
        rss < 4 * RSS_ALLOC_BYTES,
        "peak rss {rss} is implausibly large (unit mix-up?)"
    );
}

#[test]
fn tokens_and_expected() {
    use std::os::unix::fs::FileTypeExt;
    let tmp = tempfile::tempdir().unwrap();
    let mut tokens = Tokens::new(tmp.path().join("fifos"));
    tokens.add("$H", "/h".into());
    tokens.add("$HOME", "/home".into());
    tokens.add("$F", "/f".into());
    assert_eq!(
        tokens.subst("$HOME/a $H/b $F/c $Zed"),
        "/home/a /h/b /f/c $Zed"
    );
    let fifo = tokens.subst("$FIFO:probe-1/x");
    assert_eq!(
        fifo,
        format!("{}/x", tmp.path().join("fifos/probe-1").display())
    );
    let made = std::fs::metadata(tmp.path().join("fifos/probe-1")).unwrap();
    assert!(made.file_type().is_fifo(), "a FIFO is created");
    let v = expected_from(
        r#"{"_note":"x","p":"$F/r","l":[{"_c":1,"q":"$H"}]}"#,
        &tokens,
    );
    assert_eq!(v, serde_json::json!({"p":"/f/r","l":[{"q":"/h"}]}));
}

#[test]
fn error_prefix() {
    use serde_json::json;
    assert_error_prefix("ok", &json!("open failed: boom"), &json!("open failed: "));
    assert_error_prefix("ok", &json!(null), &json!(null));
    for (actual, prefix) in [
        (json!("timed out"), json!("open failed: ")),
        (json!(null), json!("open failed: ")),
        (json!("open failed: x"), json!(null)),
    ] {
        let r = std::panic::catch_unwind(|| assert_error_prefix("bad", &actual, &prefix));
        assert!(r.is_err(), "{actual} vs {prefix} must fail");
    }
}

#[test]
fn validate_schemas() {
    let read = |name: &str| -> serde_json::Value {
        let path = repo_root().join("schema/examples").join(name);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    };
    // status.valid.json reaches `rollcall:status.json#/$defs/...` refs, so this fails if the registry is wrong.
    assert_eq!(validate("status", &read("status.valid.json")), Ok(()));
    assert!(validate("status", &read("status.invalid-extra-key.json")).is_err());
}
