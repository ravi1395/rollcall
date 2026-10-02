//! Gates P32 (spec 12.1 #13, non-TUI; 3.1 non-TTY): a run with no agent data, and `rollcall PATH` piped.
use crate::discovery::{Private, json_of, own_f};
use crate::support::{RUN_DEADLINE, Which, git_path, spawn_watchdog};
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

/// §8.2 through the real binary, in a world of its own: quiet repos whose names are over 40 display
/// columns leave every other line byte-identical, and their `quiet Nd` cell starts at the badge
/// column's stop or two past the end of the name, whichever is later.
#[test]
fn plain_status_binary_over_cap_repo_names_shift_only_their_own_line() {
    use ratatui::text::Span;
    let width = |s: &str| Span::raw(s).width();
    let p = Private::new(Which::F);
    let root = p.w.root.to_str().expect("utf-8 root");
    let before = p.run(&["status", root], &[], RUN_DEADLINE).stdout;
    // The last column's stop, read off a row whose cells are all within the cap.
    let normal = before
        .lines()
        .find(|l| l.starts_with("normal "))
        .expect("normal");
    let s3 = width(&normal[..normal.find("quiet").expect("quiet cell")]);

    // 41 ASCII columns (rejoins the stop), 60 (past the stop), 21 x 2-column chars = 42 columns
    // (21 chars, 63 bytes: only a display-width measure puts it over the cap).
    let names = ["a".repeat(41), "b".repeat(60), "漢".repeat(21)];
    assert!(
        s3 > 41 + 2 && s3 < 60 + 2 && s3 > 42 + 2,
        "stop {s3}: {before}"
    );
    for name in &names {
        let dir = p.w.root.join(name);
        std::fs::create_dir(&dir).expect("repo dir");
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        ] {
            let mut cmd = p.w.sb.command(git_path());
            cmd.current_dir(&dir)
                .env("GIT_AUTHOR_DATE", "2026-09-01T00:00:00Z")
                .env("GIT_COMMITTER_DATE", "2026-09-01T00:00:00Z")
                .args(&args);
            let out = spawn_watchdog(cmd, RUN_DEADLINE).expect("git finishes");
            assert!(out.status.success(), "git {args:?} failed in {dir:?}");
        }
    }
    let after = p.run(&["status", root], &[], RUN_DEADLINE).stdout;

    let lines: Vec<&str> = after.lines().collect();
    let (mut others, mut seen, mut i) = (Vec::new(), 0, 0);
    while i < lines.len() {
        let line = lines[i];
        assert_eq!(line, line.trim_end(), "trailing space: {line:?}");
        match names.iter().find(|n| line.starts_with(n.as_str())) {
            Some(name) => {
                seen += 1;
                let cell = &line[name.len()..];
                let at = width(name) + cell.len() - cell.trim_start().len();
                assert!(cell.trim_start().starts_with("quiet "), "{line:?}");
                assert_eq!(at, s3.max(width(name) + 2), "{line:?}");
                assert!(lines[i + 1].starts_with("  main"), "{after}");
                i += 2;
            }
            None => {
                others.push(line);
                i += 1;
            }
        }
    }
    assert_eq!(seen, names.len(), "{after}");
    let before: Vec<&str> = before.lines().collect();
    assert_eq!(others, before, "an over-cap name moved other rows");
}

/// §8.2 AC6 and §9 U+2028/U+2029 through the real binary: a repo whose directory name sanitises
/// to empty is shown by its sanitised path on an unindented line; a separator in a repo or branch
/// name is a space in plain output and exact in `--json`.
#[test]
fn breaker2_binary_empty_name_fallback_and_separators() {
    use rollcall_core::text::sanitise;
    let p = Private::new(Which::FClean);
    let root = p.w.root.to_str().expect("utf-8 root");
    for (name, branch) in [("\u{200b}", "main"), ("z\u{2028}w", "b\u{2029}c")] {
        let dir = p.w.root.join(name);
        std::fs::create_dir(&dir).expect("repo dir");
        for args in [
            vec!["init", "-q", "-b", branch],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        ] {
            let mut cmd = p.w.sb.command(git_path());
            cmd.current_dir(&dir)
                .env("GIT_AUTHOR_DATE", "2026-09-01T00:00:00Z")
                .env("GIT_COMMITTER_DATE", "2026-09-01T00:00:00Z")
                .args(&args);
            let out = spawn_watchdog(cmd, RUN_DEADLINE).expect("git finishes");
            assert!(out.status.success(), "git {args:?} failed in {dir:?}");
        }
    }
    let plain = p.run(&["status", root], &[], RUN_DEADLINE).stdout;
    let (_, doc) = json_of(p.run(&["status", "--json", root], &[], RUN_DEADLINE));
    assert!(
        !plain.contains(['\u{2028}', '\u{2029}', '\u{200b}']),
        "{plain:?}"
    );
    let repos = doc["repos"].as_array().expect("repos");
    let lines: Vec<&str> = plain.lines().collect();
    let total: usize = repos
        .iter()
        .map(|r| 1 + r["worktrees"].as_array().expect("worktrees").len())
        .sum();
    assert_eq!(lines.len(), total, "{plain}");
    let unindented = lines.iter().filter(|l| !l.starts_with(' ')).count();
    assert_eq!(unindented, repos.len(), "{plain}");

    let find = |name: &str| {
        repos
            .iter()
            .find(|r| r["name"] == name)
            .unwrap_or_else(|| panic!("no repo named {name:?} in --json"))
    };
    let blank = find("\u{200b}");
    let shown = sanitise(blank["path"].as_str().expect("path"));
    assert!(shown.starts_with('/'), "{shown:?}");
    let at = lines
        .iter()
        .position(|l| l.starts_with(&format!("{shown}  ")))
        .unwrap_or_else(|| panic!("no line for {shown:?}: {plain}"));
    assert!(lines[at + 1].starts_with("  main"), "{plain}");

    let sep = find("z\u{2028}w");
    assert_eq!(sep["worktrees"][0]["branch"], "b\u{2029}c", "{sep}");
    assert!(sep["path"].as_str().expect("path").ends_with("z\u{2028}w"));
    let at = lines
        .iter()
        .position(|l| l.starts_with("z w  "))
        .unwrap_or_else(|| panic!("no line for \"z w\": {plain}"));
    assert!(lines[at + 1].starts_with("  b c  "), "{plain}");
}
