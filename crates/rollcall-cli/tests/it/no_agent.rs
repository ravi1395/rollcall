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

/// §8.2 through the real binary: one unindented line per repo, one indented line per worktree,
/// no trailing whitespace, and worktree badges (col2) / sessions (col3) start on shared columns.
#[test]
fn breaker_plain_status_binary_lines_and_columns() {
    use ratatui::text::Span;
    use rollcall_core::text::sanitise;
    let p = own_f();
    let root = p.w.root.to_str().expect("utf-8 root");
    let plain = p.run(&["status", root], &[], RUN_DEADLINE).stdout;
    let (_, doc) = json_of(p.run(&["status", "--json", root], &[], RUN_DEADLINE));
    let lines: Vec<&str> = plain.split_terminator('\n').collect();
    let repos = doc["repos"].as_array().expect("repos");
    let total: usize = repos
        .iter()
        .map(|r| 1 + r["worktrees"].as_array().expect("worktrees").len())
        .sum();
    assert_eq!(lines.len(), total, "{plain}");
    assert!(total > 3, "fixture too small to check alignment: {plain}");

    let width = |s: &str| Span::raw(s).width();
    let (mut col2, mut col3) = (Vec::new(), Vec::new());
    let mut i = 0;
    for repo in repos {
        let line = lines[i];
        i += 1;
        assert_eq!(line, line.trim_end(), "trailing space: {line:?}");
        let name = sanitise(repo["name"].as_str().expect("name"));
        assert!(
            !line.starts_with(' ') && line.starts_with(name.as_str()),
            "{line:?}"
        );
        // Repo col3 is "quiet Nd", an age or "—": the last cell, after the last two-space gap.
        let s3 = match line.find("  quiet ") {
            Some(q) => q + 2,
            None => line.rfind("  ").expect("repo col3") + 2,
        };
        let c2 = line[name.len()..s3].trim();
        if width(&name) <= 40 && width(c2) <= 40 {
            col3.push((width(&line[..s3]), line));
        }
        for wt in repo["worktrees"].as_array().expect("worktrees") {
            let line = lines[i];
            i += 1;
            assert_eq!(line, line.trim_end(), "trailing space: {line:?}");
            let label = match (
                wt["branch"].as_str(),
                wt["detached"].as_bool(),
                wt["head"].as_str(),
            ) {
                (Some(b), ..) => b.to_owned(),
                (None, Some(true), Some(h)) => format!("(detached {})", &h[..7]),
                _ => wt["path"].as_str().expect("path").to_owned(),
            };
            let c1 = format!("  {}", sanitise(&label));
            assert!(
                line.starts_with(c1.as_str()) && !line[2..].starts_with(' '),
                "{line:?}"
            );
            let rest = &line[c1.len()..];
            let at = c1.len() + rest.len() - rest.trim_start().len();
            assert!(at - c1.len() >= 2, "{line:?}");
            if width(&c1) <= 40 {
                col2.push((width(&line[..at]), line));
            }
            if let Some(src) = wt["last_session"]["source"].as_str() {
                let s3 = line[at..].find(&format!("  {src} ")).expect("session cell") + at + 2;
                let c2 = line[at..s3].trim_end();
                if width(&c1) <= 40 && width(c2) <= 40 {
                    col3.push((width(&line[..s3]), line));
                }
            }
        }
    }
    assert!(col2.len() > 1 && col3.len() > 1, "{plain}");
    for col in [col2, col3] {
        if let Some((first, _)) = col.first() {
            assert!(col.iter().all(|(w, _)| w == first), "misaligned: {col:#?}");
        }
    }
}
