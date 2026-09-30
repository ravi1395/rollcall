//! Gates P32 (spec 12.1 #13, non-TUI; 3.1 non-TTY): a run with no agent data, and `rollcall PATH` piped.
use crate::discovery::{Private, json_of, own_f};
use crate::support::{RUN_DEADLINE, Which};
use serde_json::Value;

#[test]
fn no_agent_dirs_leave_every_session_null() {
    let p = Private::new(Which::FClean);
    let (code, status) = p.status(&[]);
    assert_eq!(code, 0, "F_clean has no error rows");

    let worktrees: Vec<&Value> = status["repos"]
        .as_array()
        .expect("repos")
        .iter()
        .flat_map(|r| r["worktrees"].as_array().expect("worktrees"))
        .collect();
    assert!(
        !worktrees.is_empty(),
        "no worktrees, so nothing was checked"
    );
    for w in worktrees {
        assert!(
            w["last_session"].is_null(),
            "{}: {}",
            w["path"],
            w["last_session"]
        );
        assert_eq!(w["older_sessions"], 0, "{}", w["path"]);
    }

    let root = p.w.root.to_str().expect("utf-8 root");
    let (_, doctor) = json_of(p.run(&["doctor", "--json", root], &[], RUN_DEADLINE));
    let sources = doctor["sources"].as_array().expect("sources");
    let modes: Vec<(&str, &str)> = sources
        .iter()
        .map(|s| {
            (
                s["source"].as_str().unwrap_or("?"),
                s["mode"].as_str().unwrap_or("?"),
            )
        })
        .collect();
    assert_eq!(modes, [("claude", "absent"), ("codex", "absent")]);
}

#[test]
fn piped_rollcall_equals_status() {
    let p = own_f();
    let root = p.w.root.to_str().expect("utf-8 root");
    // The watchdog runner captures stdout through a pipe, so stdout is not a TTY.
    let piped = p.run(&[root], &[], RUN_DEADLINE);
    let status = p.run(&["status", root], &[], RUN_DEADLINE);
    assert!(
        status.stdout.contains("dirty-staged"),
        "status output lacks the fixture repos: {:.300}",
        status.stdout
    );
    assert_eq!(piped.stdout, status.stdout);
    assert_eq!(piped.code, status.code);
}
