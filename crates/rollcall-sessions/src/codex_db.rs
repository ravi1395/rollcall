//! Codex thread database (§5.2 primary source): finding, reading and re-reading `state_<N>.sqlite`.
//!
//! Read-only on agent data. Rollout parsing (`last_prompt`, `message_count`, the third child rule,
//! branch fallback) belongs to `codex_rollout`, which calls these functions.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rollcall_core::text::{filter_prompt_block, grapheme_len};
use rollcall_core::{
    Backing, Result, Session, SkippedCounts, SourceKind, TextField, TextRef, Timestamp, TitleKind,
    Uuid,
};
use rusqlite::types::FromSql;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row};

/// `threads` columns the database must have to be usable (§5.2).
pub const REQUIRED_COLUMNS: [&str; 10] = [
    "id",
    "rollout_path",
    "created_at",
    "updated_at",
    "source",
    "cwd",
    "title",
    "git_branch",
    "archived",
    "first_user_message",
];

/// §5.2: a title longer than this many graphemes is a prompt, not a thread name.
const MAX_THREAD_TITLE_GRAPHEMES: usize = 200;

/// One `threads` row that is not (yet) known to be a child, with paths already vetted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbThread {
    pub id: Uuid,
    /// Canonical path under the canonical Codex root; `None` when empty, missing or rejected.
    pub rollout_path: Option<PathBuf>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub source: Option<String>,
    pub cwd: PathBuf,
    pub title: String,
    pub git_branch: Option<String>,
    pub archived: bool,
    pub first_user_message: Option<String>,
}

/// Everything one pass over the `threads` table produced.
#[derive(Debug, Clone)]
pub struct DbRead {
    pub db_path: PathBuf,
    pub db_version: u32,
    /// Non-child threads. Rule 3 (rollout `parent_thread_id`) is applied through `exclude_where`.
    pub threads: Vec<DbThread>,
    /// `malformed` (unusable row), `non_uuid`, `bad_rollout_path`; other counters stay 0.
    pub skipped: SkippedCounts,
    pub excluded_children: u32,
    /// Every row id seen (children included), for `vanished`.
    seen_ids: HashSet<Uuid>,
}

/// `busy_timeout` for every open (§5.2).
const BUSY_TIMEOUT: Duration = Duration::from_millis(2000);

/// Every `state_<N>.sqlite` directly under `codex_home`, highest `N` first.
pub fn list_db_files(codex_home: &Path) -> Vec<(PathBuf, u32)> {
    let Ok(entries) = fs::read_dir(codex_home) else {
        return Vec::new();
    };
    let mut found: Vec<(PathBuf, u32)> = entries
        .filter_map(|entry| {
            let name = entry.ok()?.file_name();
            let version = db_version(name.to_str()?)?;
            Some((codex_home.join(name), version))
        })
        .collect();
    found.sort_by_key(|(_, version)| Reverse(*version));
    found
}

/// `state_<digits>.sqlite` → the number.
fn db_version(file_name: &str) -> Option<u32> {
    let digits = file_name.strip_prefix("state_")?.strip_suffix(".sqlite")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The highest-numbered `state_<N>.sqlite` whose `threads` table has every required column.
/// Unopenable or column-lacking files are skipped.
pub fn find_db(codex_home: &Path) -> Option<(PathBuf, u32)> {
    list_db_files(codex_home)
        .into_iter()
        .find(|(path, _)| has_required_columns(path).unwrap_or(false))
}

fn has_required_columns(db_path: &Path) -> Result<bool> {
    let conn = open_read_only(db_path)?;
    let mut stmt = conn.prepare("SELECT name FROM pragma_table_info('threads')")?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<HashSet<String>>>()?;
    Ok(REQUIRED_COLUMNS.iter().all(|c| columns.contains(*c)))
}

/// `mode=ro` via URI, never `immutable=1`: on a live WAL database `immutable` ignores the
/// `-wal` file and can return stale or corrupt results (§5.2).
fn open_read_only(db_path: &Path) -> Result<Connection> {
    let uri = format!("file:{}?mode=ro", uri_escape(db_path));
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_URI
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(uri, flags)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(conn)
}

/// Percent-encodes everything but unreserved characters and `/`, so `?`, `#` and `%` in a path
/// cannot be read as URI syntax.
fn uri_escape(path: &Path) -> String {
    let mut out = String::new();
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Reads all threads, applying child rules 1 (spawn edge) and 2 (`source` JSON has `subagent`)
/// and vetting `rollout_path` against the canonical `codex_home`.
pub fn read_threads(codex_home: &Path, db_path: &Path, db_version: u32) -> Result<DbRead> {
    let home = fs::canonicalize(codex_home)?;
    let conn = open_read_only(db_path)?;
    let mut read = DbRead {
        db_path: db_path.to_owned(),
        db_version,
        threads: Vec::new(),
        skipped: SkippedCounts::default(),
        excluded_children: 0,
        seen_ids: HashSet::new(),
    };
    // One statement = one short read transaction; the edge test rides in the same snapshot.
    let spawned = if has_spawn_edges(&conn)? {
        "id IN (SELECT child_thread_id FROM thread_spawn_edges)"
    } else {
        "0"
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT id, rollout_path, created_at, updated_at, source, cwd, title, git_branch, \
         archived, first_user_message, {spawned} FROM threads"
    ))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        read.absorb(&home, row);
    }
    Ok(read)
}

fn has_spawn_edges(conn: &Connection) -> Result<bool> {
    let found = conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('thread_spawn_edges') WHERE name = 'child_thread_id'",
            [],
            |_| Ok(()),
        )
        .optional()?;
    Ok(found.is_some())
}

/// A cell that may be NULL; a value of the wrong type reads as absent.
fn cell<T: FromSql>(row: &Row, index: usize) -> Option<T> {
    row.get::<_, Option<T>>(index).ok().flatten()
}

/// §5.2 rule 2: `source` parses as a JSON object containing key `subagent`.
fn source_is_subagent(source: Option<&str>) -> bool {
    source
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .is_some_and(|v| v.get("subagent").is_some())
}

enum RolloutPath {
    Absent,
    Ok(PathBuf),
    Bad,
}

/// §5.2: the rollout path must canonicalise under the Codex root. A file that does not exist is
/// simply absent (spec: missing rollout → `null` / `0`), not a bad path.
fn vet_rollout_path(home: &Path, raw: &str) -> RolloutPath {
    if raw.is_empty() {
        return RolloutPath::Absent;
    }
    let path = Path::new(raw);
    if !path.is_absolute() {
        return RolloutPath::Bad;
    }
    match fs::canonicalize(path) {
        Ok(canonical) if canonical.starts_with(home) => RolloutPath::Ok(canonical),
        Ok(_) => RolloutPath::Bad,
        Err(e) if e.kind() == io::ErrorKind::NotFound => RolloutPath::Absent,
        Err(_) => RolloutPath::Bad,
    }
}

impl DbRead {
    fn absorb(&mut self, home: &Path, row: &Row) {
        let Some(raw_id) = cell::<String>(row, 0) else {
            self.skipped.malformed += 1;
            return;
        };
        let Some(id) = Uuid::parse(&raw_id) else {
            self.skipped.non_uuid += 1;
            return;
        };
        self.seen_ids.insert(id.clone());
        let source = cell::<String>(row, 4);
        if cell::<bool>(row, 10).unwrap_or(false) || source_is_subagent(source.as_deref()) {
            self.excluded_children += 1;
            return;
        }
        let times = cell::<i64>(row, 2)
            .zip(cell::<i64>(row, 3))
            .and_then(|(created, updated)| {
                Some((
                    Timestamp::from_second(created).ok()?,
                    Timestamp::from_second(updated).ok()?,
                ))
            });
        let cwd = cell::<String>(row, 5).filter(|c| !c.is_empty());
        let (Some((created_at, updated_at)), Some(cwd)) = (times, cwd) else {
            self.skipped.malformed += 1;
            return;
        };
        let raw_rollout = cell::<String>(row, 1).unwrap_or_default();
        let rollout_path = match vet_rollout_path(home, &raw_rollout) {
            RolloutPath::Absent => None,
            RolloutPath::Ok(path) => Some(path),
            RolloutPath::Bad => {
                self.skipped.bad_rollout_path += 1;
                None
            }
        };
        self.threads.push(DbThread {
            id,
            rollout_path,
            created_at,
            updated_at,
            source,
            cwd: PathBuf::from(cwd),
            title: cell(row, 6).unwrap_or_default(),
            git_branch: cell::<String>(row, 7).filter(|b| !b.trim().is_empty()),
            archived: cell::<bool>(row, 8).unwrap_or(false),
            first_user_message: cell(row, 9),
        });
    }

    /// Hook for rule 3: removes (and counts as children) the threads for which `is_child` holds.
    pub fn exclude_where(&mut self, mut is_child: impl FnMut(&DbThread) -> bool) {
        let before = self.threads.len();
        self.threads.retain(|t| !is_child(t));
        self.excluded_children += u32::try_from(before - self.threads.len()).unwrap_or(u32::MAX);
    }

    /// Prior `CodexThread`-backed sessions whose row no longer exists.
    pub fn vanished(&self, prior: &[Session]) -> u32 {
        let gone = prior.iter().filter(|s| match &s.backing {
            Backing::CodexThread { thread_id, .. } => !self.seen_ids.contains(thread_id),
            Backing::LogFile { .. } => false,
        });
        u32::try_from(gone.count()).unwrap_or(u32::MAX)
    }

    /// One session per remaining thread; `last_prompt` `None`, `message_count` 0 (rollout data).
    pub fn sessions(&self) -> Vec<Session> {
        self.threads.iter().map(|t| self.session(t)).collect()
    }

    fn session(&self, t: &DbThread) -> Session {
        let title_ref = TextRef::CodexThread {
            thread_id: t.id.clone(),
            field: TextField::Title,
        };
        Session {
            source: SourceKind::Codex,
            id: t.id.clone(),
            cwd: t.cwd.clone(),
            branch: t.git_branch.clone(),
            started_at: t.created_at,
            ended_at: t.updated_at,
            title: t.title().map(|(_, kind)| (title_ref, kind)),
            last_prompt: None,
            message_count: 0,
            backing: Backing::CodexThread {
                db_path: self.db_path.clone(),
                thread_id: t.id.clone(),
            },
        }
    }
}

impl DbThread {
    /// The title text and kind per §5.2, or `None` when there is neither title nor prompt.
    pub fn title(&self) -> Option<(String, TitleKind)> {
        title_text(&self.title, self.first_user_message.as_deref())
    }
}

/// §5.2 title kind for a non-empty title.
pub fn title_kind(title: &str, first_user_message: Option<&str>) -> TitleKind {
    let title = title.trim();
    if grapheme_len(title) > MAX_THREAD_TITLE_GRAPHEMES {
        return TitleKind::Prompt;
    }
    let stem = title
        .strip_suffix('…')
        .or_else(|| title.strip_suffix("..."))
        .unwrap_or(title)
        .trim_end();
    // An empty stem would be a substring of everything, which says nothing about the title.
    let echoes_message = !stem.is_empty() && first_user_message.is_some_and(|m| m.contains(stem));
    if echoes_message {
        TitleKind::Prompt
    } else {
        TitleKind::Thread
    }
}

/// Title if non-empty (kind by `title_kind`), else the §5.0-filtered `first_user_message` (`Prompt`).
pub fn title_text(title: &str, first_user_message: Option<&str>) -> Option<(String, TitleKind)> {
    if !title.trim().is_empty() {
        return Some((title.to_owned(), title_kind(title, first_user_message)));
    }
    let prompt = filter_prompt_block(first_user_message?)?;
    Some((prompt, TitleKind::Prompt))
}

/// Re-reads the row by id and recomputes the title text and its kind.
pub fn resolve_thread_title(db_path: &Path, thread_id: &Uuid) -> Option<(String, TitleKind)> {
    let conn = open_read_only(db_path).ok()?;
    let (title, first_user_message) = conn
        .query_row(
            "SELECT title, first_user_message FROM threads WHERE id = ?1",
            [thread_id.as_str()],
            |row| Ok((cell::<String>(row, 0), cell::<String>(row, 1))),
        )
        .ok()?;
    title_text(&title.unwrap_or_default(), first_user_message.as_deref())
}

/// `Title` → `resolve_thread_title`; `LastPrompt` → `None` (Codex last prompts are `LogLine`
/// refs into rollouts, never thread refs).
pub fn resolve_thread(db_path: &Path, thread_id: &Uuid, field: TextField) -> Option<String> {
    if field != TextField::Title {
        return None;
    }
    resolve_thread_title(db_path, thread_id).map(|(text, _)| text)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::SystemTime;

    use rusqlite::params;
    use tempfile::TempDir;

    use super::*;

    const UUID_A: &str = "11111111-1111-4111-8111-111111111111";
    const UUID_B: &str = "22222222-2222-4222-8222-222222222222";
    const UUID_C: &str = "33333333-3333-4333-8333-333333333333";

    const THREADS_SQL: &str = "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT, \
        created_at INTEGER, updated_at INTEGER, source TEXT, cwd TEXT, title TEXT, \
        git_branch TEXT, archived INTEGER, first_user_message TEXT)";
    const EDGES_SQL: &str = "CREATE TABLE thread_spawn_edges (parent_thread_id TEXT, \
        child_thread_id TEXT)";

    /// A fake `$CODEX_HOME`; canonical so path comparisons survive `/var` → `/private/var`.
    struct Home {
        _dir: TempDir,
        path: PathBuf,
    }

    fn home() -> Home {
        let dir = tempfile::tempdir().unwrap();
        let path = fs::canonicalize(dir.path()).unwrap();
        Home { _dir: dir, path }
    }

    fn db_path(home: &Path, n: u32) -> PathBuf {
        home.join(format!("state_{n}.sqlite"))
    }

    fn create_db(home: &Path, n: u32) -> Connection {
        let conn = Connection::open(db_path(home, n)).unwrap();
        conn.execute_batch(THREADS_SQL).unwrap();
        conn
    }

    /// Inserts a thread with the given id, title and first message; other columns get defaults.
    fn insert(conn: &Connection, id: &str, title: &str, first: Option<&str>) {
        insert_full(conn, id, "", "{\"cli\":true}", "/w/app", title, first);
    }

    fn insert_full(
        conn: &Connection,
        id: &str,
        rollout: &str,
        source: &str,
        cwd: &str,
        title: &str,
        first: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO threads VALUES (?1, ?2, 1000, 2000, ?3, ?4, ?5, 'main', 0, ?6)",
            params![id, rollout, source, cwd, title, first],
        )
        .unwrap();
    }

    fn read(home: &Home, n: u32) -> DbRead {
        read_threads(&home.path, &db_path(&home.path, n), n).unwrap()
    }

    fn ids(read: &DbRead) -> Vec<&str> {
        read.threads.iter().map(|t| t.id.as_str()).collect()
    }

    #[test]
    fn reader_sees_uncheckpointed_wal_write_while_writer_is_open() {
        let h = home();
        let writer = create_db(&h.path, 5);
        let mode: String = writer
            .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        writer.execute_batch("PRAGMA wal_autocheckpoint=0").unwrap();
        insert(&writer, UUID_A, "in the wal", None);
        // `immutable=1` would ignore the -wal file and see no row (or no table at all).
        let (found, version) = find_db(&h.path).expect("db found through the wal");
        assert_eq!(version, 5);
        let read = read_threads(&h.path, &found, version).unwrap();
        assert_eq!(ids(&read), [UUID_A]);
        drop(writer);
    }

    #[test]
    fn find_db_picks_the_highest_number() {
        let h = home();
        for n in [4, 5] {
            create_db(&h.path, n);
        }
        assert_eq!(find_db(&h.path), Some((db_path(&h.path, 5), 5)));
    }

    #[test]
    fn find_db_orders_numerically_not_lexically() {
        let h = home();
        for n in [9, 10] {
            create_db(&h.path, n);
        }
        assert_eq!(find_db(&h.path), Some((db_path(&h.path, 10), 10)));
    }

    #[test]
    fn find_db_skips_a_database_lacking_a_required_column() {
        let h = home();
        create_db(&h.path, 4);
        let bad = Connection::open(db_path(&h.path, 6)).unwrap();
        bad.execute_batch(&THREADS_SQL.replace("archived INTEGER, ", ""))
            .unwrap();
        assert_eq!(find_db(&h.path), Some((db_path(&h.path, 4), 4)));
    }

    #[test]
    fn find_db_skips_a_file_that_is_not_a_database() {
        let h = home();
        create_db(&h.path, 4);
        fs::write(db_path(&h.path, 7), b"not sqlite at all").unwrap();
        assert_eq!(find_db(&h.path), Some((db_path(&h.path, 4), 4)));
    }

    #[test]
    fn find_db_ignores_other_names_and_reports_none_when_absent() {
        let h = home();
        assert_eq!(find_db(&h.path), None);
        let conn = create_db(&h.path, 3);
        drop(conn);
        for name in [
            "state_x.sqlite",
            "state_.sqlite",
            "state_3.sqlite-wal",
            "logs_9.sqlite",
        ] {
            fs::copy(db_path(&h.path, 3), h.path.join(name)).unwrap();
        }
        let files = list_db_files(&h.path);
        assert_eq!(files, [(db_path(&h.path, 3), 3)]);
        assert_eq!(find_db(&h.path), Some((db_path(&h.path, 3), 3)));
    }

    #[test]
    fn find_db_returns_none_when_no_database_is_usable() {
        let h = home();
        let bad = Connection::open(db_path(&h.path, 2)).unwrap();
        bad.execute_batch("CREATE TABLE threads (id TEXT)").unwrap();
        assert_eq!(find_db(&h.path), None);
        assert_eq!(list_db_files(&h.path).len(), 1);
    }

    #[test]
    fn rule_one_spawn_edge_child_is_excluded() {
        let h = home();
        let conn = create_db(&h.path, 5);
        conn.execute_batch(EDGES_SQL).unwrap();
        insert(&conn, UUID_A, "parent", None);
        insert(&conn, UUID_B, "child", None);
        conn.execute(
            "INSERT INTO thread_spawn_edges VALUES (?1, ?2)",
            params![UUID_A, UUID_B],
        )
        .unwrap();
        let read = read(&h, 5);
        assert_eq!(ids(&read), [UUID_A]);
        assert_eq!(read.excluded_children, 1);
    }

    #[test]
    fn missing_spawn_edges_table_is_not_an_error() {
        let h = home();
        let conn = create_db(&h.path, 5);
        insert(&conn, UUID_A, "solo", None);
        let read = read(&h, 5);
        assert_eq!(ids(&read), [UUID_A]);
        assert_eq!(read.excluded_children, 0);
    }

    #[test]
    fn rule_two_source_json_with_subagent_key_is_excluded() {
        let h = home();
        let conn = create_db(&h.path, 5);
        insert_full(&conn, UUID_A, "", "cli", "/w", "plain string source", None);
        insert_full(
            &conn,
            UUID_B,
            "",
            "{\"subagent\":{\"thread_spawn\":{}}}",
            "/w",
            "sub",
            None,
        );
        insert_full(
            &conn,
            UUID_C,
            "",
            "{\"other\":1}",
            "/w",
            "json without the key",
            None,
        );
        let read = read(&h, 5);
        let mut kept = ids(&read);
        kept.sort();
        assert_eq!(kept, [UUID_A, UUID_C]);
        assert_eq!(read.excluded_children, 1);
    }

    #[test]
    fn rule_three_hook_excludes_by_rollout_and_counts() {
        let h = home();
        let conn = create_db(&h.path, 5);
        let rollout = h.path.join("sessions/rollout-a.jsonl");
        fs::create_dir_all(rollout.parent().unwrap()).unwrap();
        fs::write(&rollout, b"{}\n").unwrap();
        insert_full(
            &conn,
            UUID_A,
            rollout.to_str().unwrap(),
            "cli",
            "/w",
            "child by rollout",
            None,
        );
        insert_full(&conn, UUID_B, "", "cli", "/w", "no rollout", None);
        let mut read = read(&h, 5);
        read.exclude_where(|t| t.rollout_path.as_deref() == Some(rollout.as_path()));
        assert_eq!(ids(&read), [UUID_B]);
        assert_eq!(read.excluded_children, 1);
        assert_eq!(read.sessions().len(), 1);
    }

    #[test]
    fn title_kind_prompt_cases() {
        let msg = Some("fix the flaky test in ci");
        assert_eq!(
            title_kind("fix the flaky test in ci", msg),
            TitleKind::Prompt,
            "equal"
        );
        assert_eq!(
            title_kind("fix the flaky…", msg),
            TitleKind::Prompt,
            "ellipsis char"
        );
        assert_eq!(
            title_kind("fix the flaky...", msg),
            TitleKind::Prompt,
            "three dots"
        );
        assert_eq!(
            title_kind("  flaky test  ", msg),
            TitleKind::Prompt,
            "trimmed substring"
        );
        let long = "x".repeat(201);
        assert_eq!(
            title_kind(&long, None),
            TitleKind::Prompt,
            "over 200 graphemes"
        );
    }

    #[test]
    fn title_kind_unrelated_is_thread() {
        let msg = Some("fix the flaky test in ci");
        assert_eq!(title_kind("Release planning", msg), TitleKind::Thread);
        assert_eq!(title_kind("Release planning", None), TitleKind::Thread);
        assert_eq!(
            title_kind("…", msg),
            TitleKind::Thread,
            "nothing left after stripping"
        );
    }

    #[test]
    fn title_kind_length_counts_graphemes_not_chars() {
        let cluster = "e\u{301}"; // two chars, one grapheme
        assert_eq!(title_kind(&cluster.repeat(200), None), TitleKind::Thread);
        assert_eq!(title_kind(&cluster.repeat(201), None), TitleKind::Prompt);
    }

    #[test]
    fn resolve_thread_title_recomputes_the_kind_from_the_row() {
        let h = home();
        let conn = create_db(&h.path, 1);
        insert(
            &conn,
            UUID_A,
            "Release planning",
            Some("fix the flaky test"),
        );
        let id = Uuid::parse(UUID_A).unwrap();
        let db = db_path(&h.path, 1);
        assert_eq!(
            resolve_thread_title(&db, &id),
            Some(("Release planning".to_owned(), TitleKind::Thread))
        );
        conn.execute("UPDATE threads SET title = ''", []).unwrap();
        assert_eq!(
            resolve_thread_title(&db, &id),
            Some(("fix the flaky test".to_owned(), TitleKind::Prompt))
        );
    }

    #[test]
    fn empty_title_falls_back_to_the_filtered_first_message() {
        assert_eq!(
            title_text("  ", Some("  fix it <system-reminder>noise")),
            Some(("fix it".to_owned(), TitleKind::Prompt))
        );
        assert_eq!(
            title_text("", Some("<environment_context>")),
            None,
            "filtered away"
        );
        assert_eq!(title_text("", None), None);
        assert_eq!(
            title_text("Named", Some("something else")),
            Some(("Named".to_owned(), TitleKind::Thread))
        );
    }

    #[test]
    fn rollout_outside_codex_home_is_ignored_and_counted() {
        let h = home();
        let outside = tempfile::tempdir().unwrap();
        let stray = outside.path().join("rollout-x.jsonl");
        fs::write(&stray, b"{}\n").unwrap();
        let inside = h.path.join("sessions/rollout-y.jsonl");
        fs::create_dir_all(inside.parent().unwrap()).unwrap();
        fs::write(&inside, b"{}\n").unwrap();
        let conn = create_db(&h.path, 5);
        insert_full(
            &conn,
            UUID_A,
            stray.to_str().unwrap(),
            "cli",
            "/w",
            "outside",
            None,
        );
        insert_full(
            &conn,
            UUID_B,
            inside.to_str().unwrap(),
            "cli",
            "/w",
            "inside",
            None,
        );
        insert_full(
            &conn,
            UUID_C,
            "relative/rollout.jsonl",
            "cli",
            "/w",
            "relative",
            None,
        );
        let read = read(&h, 5);
        assert_eq!(read.skipped.bad_rollout_path, 2);
        let by_id = |id: &str| read.threads.iter().find(|t| t.id.as_str() == id).unwrap();
        assert_eq!(by_id(UUID_A).rollout_path, None);
        assert_eq!(by_id(UUID_C).rollout_path, None);
        assert_eq!(
            by_id(UUID_B).rollout_path.as_deref(),
            Some(fs::canonicalize(&inside).unwrap().as_path())
        );
        assert_eq!(
            read.threads.len(),
            3,
            "the thread itself is still a session"
        );
    }

    #[test]
    fn symlink_escaping_codex_home_is_bad_rollout_path() {
        let h = home();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("secret.jsonl");
        fs::write(&target, b"{}\n").unwrap();
        let link = h.path.join("rollout-link.jsonl");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let conn = create_db(&h.path, 5);
        insert_full(
            &conn,
            UUID_A,
            link.to_str().unwrap(),
            "cli",
            "/w",
            "t",
            None,
        );
        let read = read(&h, 5);
        assert_eq!(read.skipped.bad_rollout_path, 1);
        assert_eq!(read.threads[0].rollout_path, None);
    }

    #[test]
    fn missing_or_empty_rollout_is_not_counted() {
        let h = home();
        let conn = create_db(&h.path, 5);
        let gone = h.path.join("sessions/gone.jsonl");
        insert_full(
            &conn,
            UUID_A,
            gone.to_str().unwrap(),
            "cli",
            "/w",
            "gone",
            None,
        );
        insert_full(&conn, UUID_B, "", "cli", "/w", "empty", None);
        let read = read(&h, 5);
        assert_eq!(read.skipped.bad_rollout_path, 0);
        assert!(read.threads.iter().all(|t| t.rollout_path.is_none()));
    }

    #[test]
    fn non_uuid_and_unusable_rows_are_skipped_and_counted() {
        let h = home();
        let conn = create_db(&h.path, 5);
        insert(&conn, "not-a-uuid", "bad id", None);
        insert(&conn, UUID_A, "ok", None);
        insert_full(&conn, UUID_B, "", "cli", "", "no cwd", None);
        conn.execute(
            "INSERT INTO threads VALUES (?1, '', NULL, 2000, 'cli', '/w', 't', NULL, 0, NULL)",
            params![UUID_C],
        )
        .unwrap();
        let read = read(&h, 5);
        assert_eq!(ids(&read), [UUID_A]);
        assert_eq!(read.skipped.non_uuid, 1);
        assert_eq!(read.skipped.malformed, 2);
    }

    #[test]
    fn rows_become_sessions_with_thread_refs() {
        let h = home();
        let conn = create_db(&h.path, 5);
        insert_full(
            &conn,
            UUID_A,
            "",
            "cli",
            "/w/app",
            "Named thread",
            Some("hello"),
        );
        conn.execute(
            "INSERT INTO threads VALUES (?1, '', 1700000000, 1700000060, 'cli', '/w/b', '', '', 1, ?2)",
            params![UUID_B, "  ship it  "],
        )
        .unwrap();
        let read = read(&h, 5);
        assert_eq!(read.db_version, 5);
        let sessions = read.sessions();
        let a = sessions.iter().find(|s| s.id.as_str() == UUID_A).unwrap();
        let uuid_a = Uuid::parse(UUID_A).unwrap();
        assert_eq!(a.source, SourceKind::Codex);
        assert_eq!(a.cwd, PathBuf::from("/w/app"));
        assert_eq!(a.branch.as_deref(), Some("main"));
        assert_eq!(a.started_at, Timestamp::from_second(1000).unwrap());
        assert_eq!(a.ended_at, Timestamp::from_second(2000).unwrap());
        assert_eq!(
            a.title,
            Some((
                TextRef::CodexThread {
                    thread_id: uuid_a.clone(),
                    field: TextField::Title
                },
                TitleKind::Thread
            ))
        );
        assert_eq!(a.last_prompt, None);
        assert_eq!(a.message_count, 0);
        assert_eq!(
            a.backing,
            Backing::CodexThread {
                db_path: db_path(&h.path, 5),
                thread_id: uuid_a
            }
        );
        let b = sessions.iter().find(|s| s.id.as_str() == UUID_B).unwrap();
        assert_eq!(b.started_at, Timestamp::from_second(1_700_000_000).unwrap());
        assert_eq!(
            b.branch, None,
            "empty branch is left for the rollout to fill"
        );
        assert_eq!(b.title.as_ref().map(|t| t.1), Some(TitleKind::Prompt));
    }

    fn file_fingerprint(path: &Path) -> (Vec<u8>, SystemTime) {
        (
            fs::read(path).unwrap(),
            fs::metadata(path).unwrap().modified().unwrap(),
        )
    }

    #[test]
    fn scan_leaves_the_database_file_untouched() {
        for wal in [false, true] {
            let h = home();
            let conn = create_db(&h.path, 5);
            if wal {
                let _: String = conn
                    .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
                    .unwrap();
            }
            insert(&conn, UUID_A, "t", Some("first"));
            drop(conn); // closing checkpoints the wal into the main file
            let path = db_path(&h.path, 5);
            let before = file_fingerprint(&path);
            let (found, version) = find_db(&h.path).unwrap();
            let read = read_threads(&h.path, &found, version).unwrap();
            assert_eq!(read.threads.len(), 1);
            let id = Uuid::parse(UUID_A).unwrap();
            assert!(resolve_thread(&found, &id, TextField::Title).is_some());
            assert_eq!(before, file_fingerprint(&path), "wal={wal}");
        }
    }

    #[test]
    fn resolve_title_reflects_a_later_title_change() {
        let h = home();
        let conn = create_db(&h.path, 5);
        insert(&conn, UUID_A, "Old name", None);
        let id = Uuid::parse(UUID_A).unwrap();
        let path = db_path(&h.path, 5);
        assert_eq!(
            resolve_thread(&path, &id, TextField::Title).as_deref(),
            Some("Old name")
        );
        conn.execute(
            "UPDATE threads SET title='New name' WHERE id=?1",
            params![UUID_A],
        )
        .unwrap();
        assert_eq!(
            resolve_thread(&path, &id, TextField::Title).as_deref(),
            Some("New name")
        );
    }

    #[test]
    fn resolve_title_uses_first_message_when_title_is_empty() {
        let h = home();
        let conn = create_db(&h.path, 5);
        insert(&conn, UUID_A, "", Some("do the thing"));
        let id = Uuid::parse(UUID_A).unwrap();
        assert_eq!(
            resolve_thread(&db_path(&h.path, 5), &id, TextField::Title).as_deref(),
            Some("do the thing")
        );
    }

    #[test]
    fn resolve_is_none_for_last_prompt_deleted_rows_and_missing_db() {
        let h = home();
        let conn = create_db(&h.path, 5);
        insert(&conn, UUID_A, "t", Some("first"));
        let path = db_path(&h.path, 5);
        let id = Uuid::parse(UUID_A).unwrap();
        assert_eq!(resolve_thread(&path, &id, TextField::LastPrompt), None);
        conn.execute("DELETE FROM threads", []).unwrap();
        assert_eq!(resolve_thread(&path, &id, TextField::Title), None);
        let missing = h.path.join("state_9.sqlite");
        assert_eq!(resolve_thread(&missing, &id, TextField::Title), None);
        assert!(!missing.exists(), "a read-only open never creates the file");
    }

    #[test]
    fn deleted_row_is_dropped_and_counted_vanished() {
        let h = home();
        let conn = create_db(&h.path, 5);
        insert(&conn, UUID_A, "stays", None);
        insert(&conn, UUID_B, "goes", None);
        insert(&conn, UUID_C, "becomes a child", None);
        let prior = read(&h, 5).sessions();
        assert_eq!(prior.len(), 3);
        conn.execute("DELETE FROM threads WHERE id=?1", params![UUID_B])
            .unwrap();
        conn.execute(
            "UPDATE threads SET source='{\"subagent\":1}' WHERE id=?1",
            params![UUID_C],
        )
        .unwrap();
        let read = read(&h, 5);
        assert_eq!(
            read.vanished(&prior),
            1,
            "an excluded child is not vanished"
        );
        assert_eq!(read.excluded_children, 1);
        let now: Vec<_> = read
            .sessions()
            .into_iter()
            .map(|s| s.id.as_str().to_owned())
            .collect();
        assert_eq!(now, [UUID_A]);
    }

    #[test]
    fn path_with_uri_special_characters_opens() {
        let dir = tempfile::tempdir().unwrap();
        let odd = dir.path().join("a b?c%d#e");
        fs::create_dir(&odd).unwrap();
        let odd = fs::canonicalize(odd).unwrap();
        let conn = create_db(&odd, 5);
        insert(&conn, UUID_A, "t", None);
        let (found, version) = find_db(&odd).expect("found under an odd path");
        assert_eq!(
            read_threads(&odd, &found, version).unwrap().threads.len(),
            1
        );
    }

    #[test]
    fn unopenable_database_is_an_error_not_an_empty_scan() {
        let h = home();
        fs::write(db_path(&h.path, 5), b"garbage").unwrap();
        assert!(read_threads(&h.path, &db_path(&h.path, 5), 5).is_err());
    }
}
