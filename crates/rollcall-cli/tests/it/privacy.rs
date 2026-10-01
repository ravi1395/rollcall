//! §12.1 #10 (P35), the non-MCP parts (MCP defaults are P34): scrubbing, truncation, `redact_prompts`, cache
//! modes under umask 022, and no fixture prompt in any cache file. The privacy and sanitise corpora live on
//! `F_clean` worlds of their own so a test can change the config or the logs.
use super::build_sessions::{CodexMode, build_corpora};
use super::support::{
    Corpus, RUN_DEADLINE, Run, Which, World, fresh, run_rollcall, spawn_watchdog,
};
use rollcall_cli::output::{TextPolicy, render_session};
use rollcall_core::SourceState;
use rollcall_sessions::{Claude, SessionSource};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const MAX_GRAPHEMES: usize = 200;
const EMAIL_SESSION: &str = "c1a00004-0000-4000-8000-00000000000e";
const EMAIL_PROJECT_DIR: &str = "privacy-14";
const MIN_SESSION_ROWS: usize = 16;
/// Raw prompt words at least this long are searched for in the cache (secrets, distinctive names).
const LONG_WORD: usize = 16;

fn world() -> World {
    let w = fresh(Which::FClean);
    build_corpora(&w, &[Corpus::Privacy, Corpus::Sanitise], CodexMode::Db);
    w
}

fn root_arg(w: &World) -> String {
    w.root.to_str().expect("UTF-8 root").to_owned()
}

fn status_json(w: &World) -> (Run, Value) {
    let run = run_rollcall(w, &["status", "--json", &root_arg(w)]);
    let doc: Value = serde_json::from_str(&run.stdout)
        .unwrap_or_else(|e| panic!("status --json: {e}\n{}", run.stderr));
    (run, doc)
}

fn write_config(w: &World, toml: &str) {
    let dir = w.sb.xdg_config.join("rollcall");
    std::fs::create_dir_all(&dir).expect("create config dir");
    std::fs::write(dir.join("config.toml"), toml).expect("write config");
}

/// Every worktree's `last_session` in a `StatusDoc`.
fn last_sessions(doc: &Value) -> Vec<&Value> {
    doc["repos"]
        .as_array()
        .expect("repos")
        .iter()
        .flat_map(|r| r["worktrees"].as_array().into_iter().flatten())
        .map(|wt| &wt["last_session"])
        .filter(|s| !s.is_null())
        .collect()
}

/// Extended grapheme clusters, approximated for this corpus: combining marks, ZWJ and variation selectors
/// join the previous cluster, and regional indicators pair up into flags.
// ponytail: no unicode-segmentation dev-dependency (R1); exact for the fixture's text, upgrade path is that crate.
fn graphemes(s: &str) -> usize {
    let mut clusters = 0;
    let mut pending_indicator = false;
    for c in s.chars() {
        let joins = matches!(c, '\u{0300}'..='\u{036F}' | '\u{200D}' | '\u{FE0F}');
        let indicator = matches!(c, '\u{1F1E6}'..='\u{1F1FF}');
        if joins {
            continue;
        }
        if indicator && pending_indicator {
            pending_indicator = false;
            continue;
        }
        pending_indicator = indicator;
        clusters += 1;
    }
    clusters
}

#[test]
fn graphemes_counts_flags_and_combining_marks_as_one_cluster() {
    assert_eq!(graphemes("ab"), 2);
    assert_eq!(graphemes("e\u{0301}"), 1);
    assert_eq!(graphemes("\u{1F1EF}\u{1F1F5}\u{1F1EF}\u{1F1F5}"), 2);
    assert_eq!(graphemes("a\u{200D}b"), 2);
}

/// The `[[claude]]` rows of the privacy and sanitise corpora.
fn text_rows() -> Vec<&'static super::support::Claude> {
    super::support::manifest()
        .claude
        .iter()
        .filter(|r| matches!(r.corpus, Corpus::Privacy | Corpus::Sanitise))
        .collect()
}

#[test]
fn every_scrub_pattern_and_truncation_case_matches_the_oracle() {
    let w = world();
    let claude = Claude::new(w.sb.claude_projects());
    let scan = claude.scan(&SourceState::default());
    let rows = text_rows();
    assert_eq!(
        scan.sessions.len(),
        rows.len(),
        "every log becomes a session"
    );
    let policy = TextPolicy {
        prompts: true,
        redact: false,
    };
    for row in rows {
        let session = scan
            .sessions
            .iter()
            .find(|s| s.id.as_str() == row.id)
            .unwrap_or_else(|| panic!("session {} not scanned", row.id));
        let out = render_session(session, &claude, policy);
        for text in [&out.title, &out.last_prompt].into_iter().flatten() {
            assert!(graphemes(text) <= MAX_GRAPHEMES, "{}: {text:?}", row.id);
        }
        let expect = row.expect.as_ref().expect("text rows carry an expectation");
        if row.id == EMAIL_SESSION {
            // `render_session` has no config, so emails stay; the `status` test below covers `redact_emails`.
            assert!(out.last_prompt.as_deref().is_some_and(|p| p.contains('@')));
            continue;
        }
        let want = |v: &Option<String>| v.as_deref().map(|s| w.tokens.subst(s));
        assert_eq!(out.title, want(&expect.title), "{} title", row.id);
        assert_eq!(
            out.last_prompt,
            want(&expect.last_prompt),
            "{} last_prompt",
            row.id
        );
        assert_eq!(
            serde_json::to_value(out.title_kind).expect("kind serialises"),
            json!(expect.title_kind),
            "{} title_kind",
            row.id
        );
    }
}

#[test]
fn email_addresses_are_scrubbed_only_with_redact_emails() {
    let w = world();
    // Keep only the email session on `dirty-unstaged`, so it is that worktree's `last_session`.
    let projects = w.sb.claude_projects();
    for entry in std::fs::read_dir(&projects).expect("read projects") {
        let path = entry.expect("entry").path();
        let name = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        if name.starts_with("privacy-") && name != EMAIL_PROJECT_DIR {
            std::fs::remove_dir_all(&path).expect("remove other privacy log");
        }
    }
    let prompt = |doc: &Value| {
        let found: Vec<_> = last_sessions(doc)
            .into_iter()
            .filter(|s| s["id"] == EMAIL_SESSION)
            .map(|s| s["last_prompt"].as_str().unwrap_or_default().to_owned())
            .collect();
        assert_eq!(
            found.len(),
            1,
            "the email session is one last_session: {doc}"
        );
        found[0].clone()
    };
    let (_, doc) = status_json(&w);
    assert!(
        prompt(&doc).contains("ada.fixture@example.test"),
        "default keeps emails"
    );
    write_config(&w, "redact_emails = true\n");
    let (_, doc) = status_json(&w);
    assert_eq!(
        prompt(&doc),
        "mail [redacted] about the loris-teal-14 outage"
    );
}

/// A token of the shape `word-word-NN`, the fixture's per-prompt marker.
fn distinctive(text: &str) -> Option<&str> {
    text.split_whitespace().find(|w| {
        let parts: Vec<_> = w.split('-').collect();
        parts.len() == 3
            && parts[2].len() == 2
            && parts[2].bytes().all(|b| b.is_ascii_digit())
            && parts[..2]
                .iter()
                .all(|p| p.bytes().all(|b| b.is_ascii_lowercase()))
    })
}

#[test]
fn redact_prompts_hides_every_title_and_prompt() {
    let w = world();
    let (_, control) = status_json(&w);
    let shown = last_sessions(&control)
        .into_iter()
        .filter(|s| !s["last_prompt"].is_null())
        .count();
    assert!(
        shown >= 1,
        "control: without redact_prompts a prompt is shown"
    );

    write_config(&w, "redact_prompts = true\n");
    let (run, doc) = status_json(&w);
    let sessions = last_sessions(&doc);
    assert!(!sessions.is_empty(), "sessions still listed: {doc}");
    for s in sessions {
        assert!(
            s["title"].is_null() && s["title_kind"].is_null() && s["last_prompt"].is_null(),
            "{s}"
        );
    }
    for row in text_rows() {
        let token = row
            .expect
            .as_ref()
            .and_then(|e| e.last_prompt.as_deref())
            .and_then(distinctive);
        if let Some(token) = token {
            assert!(
                !run.stdout.contains(token),
                "{token} leaked into the output"
            );
        }
    }
}

// --------------------------------------------------------------- the cache

fn cache_dir(w: &World) -> PathBuf {
    rollcall_store::cache_dir(&w.sb.home, Some(&w.sb.xdg_cache))
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path)
        .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
        .permissions()
        .mode()
        & 0o777
}

#[test]
fn cache_modes_are_0700_and_0600_under_umask_022() {
    let w = world();
    let root = root_arg(&w);
    // The shell sets the umask and execs rollcall, so the process under test inherits it.
    let mut cmd = w.sb.command("sh");
    cmd.args([
        "-c",
        "umask 022; exec \"$0\" \"$@\"",
        env!("CARGO_BIN_EXE_rollcall"),
        "scan",
        &root,
    ]);
    spawn_watchdog(cmd, RUN_DEADLINE).expect("scan under umask 022");
    let dir = cache_dir(&w);
    assert_eq!(mode(&dir), 0o700, "{}", dir.display());
    // Only what rollcall itself left is judged; P26's `fs::tests::modes_are_explicit_under_umask_000` covers
    // the WAL and shared-memory files, which exist only while a connection is open.
    let db_files: Vec<_> = std::fs::read_dir(&dir)
        .expect("read cache dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("cache.sqlite"))
        })
        .collect();
    assert!(
        db_files.iter().any(|p| p.ends_with("cache.sqlite")),
        "{db_files:?}"
    );
    for file in &db_files {
        assert_eq!(mode(file), 0o600, "{}", file.display());
    }
}

#[test]
fn no_fixture_prompt_appears_in_any_cache_file() {
    let w = world();
    let (_, doc) = status_json(&w);
    assert!(!last_sessions(&doc).is_empty());
    let dir = cache_dir(&w);

    let conn = rusqlite::Connection::open_with_flags(
        dir.join("cache.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open the cache read-only");
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
        .expect("count sessions");
    assert!(rows as usize >= MIN_SESSION_ROWS, "{rows} session rows");

    // Every cache file, WAL and lock files included, as bytes.
    let mut haystack = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("read cache dir") {
        haystack.extend(std::fs::read(entry.expect("entry").path()).expect("read cache file"));
    }
    let contains = |needle: &str| {
        haystack
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    };
    // The search can see real content: paths are stored.
    assert!(
        contains("dirty-unstaged"),
        "control: the cache holds repo paths"
    );

    let mut searched = 0;
    for row in text_rows() {
        let raw: Value =
            serde_json::from_str(&row.lines[0].replace("$F", "/f")).expect("log line is JSON");
        let prompt = raw["message"]["content"]
            .as_str()
            .expect("user prompt text");
        let mut needles = vec![prompt];
        needles.extend(
            prompt
                .split_whitespace()
                .filter(|word| word.len() >= LONG_WORD),
        );
        needles.extend(distinctive(prompt));
        for needle in needles {
            assert!(!contains(needle), "{}: {needle:?} is in the cache", row.id);
        }
        searched += 1;
    }
    assert!(searched >= MIN_SESSION_ROWS, "{searched} prompts searched");
}
