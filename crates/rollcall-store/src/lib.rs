use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions, Permissions};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rollcall_core::{
    Index, RepoRow, Result, ScanDiagnostics, ScanResult, Session, SourceKind, SourceState,
    Timestamp, Uuid,
};
use rusqlite::types::FromSql;
use rusqlite::{Connection, OptionalExtension, Params, Transaction, TransactionBehavior, params};

pub mod fs;

#[cfg(test)]
thread_local! {
    static PANIC_AFTER_INSERTS: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// Bump on any schema change: a mismatch deletes and rebuilds the cache (§6.4).
const SCHEMA_VERSION: i64 = 1;
const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);
const DB_MODE: u32 = 0o600;

/// Rows hold `serde_json` of core types; log text never enters the DB, titles and prompts
/// are `TextRef` pointers inside those JSON blobs (§6.2). Path columns are the raw bytes.
const SCHEMA: &str = "
BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS source_files (
    source TEXT NOT NULL, seq INTEGER NOT NULL, state TEXT NOT NULL,
    PRIMARY KEY (source, seq));
CREATE TABLE IF NOT EXISTS sessions (
    source TEXT NOT NULL, seq INTEGER NOT NULL, id TEXT NOT NULL, session TEXT NOT NULL,
    PRIMARY KEY (source, seq));
CREATE TABLE IF NOT EXISTS roots (
    root BLOB PRIMARY KEY, scanned_at TEXT NOT NULL, submodules INTEGER NOT NULL,
    diagnostics TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS root_repos (
    root BLOB NOT NULL, seq INTEGER NOT NULL, repo_path BLOB NOT NULL, kind TEXT NOT NULL,
    contains_root INTEGER NOT NULL, repo TEXT NOT NULL,
    PRIMARY KEY (root, seq));
CREATE TABLE IF NOT EXISTS matches (
    root BLOB NOT NULL, worktree BLOB NOT NULL, seq INTEGER NOT NULL, session TEXT NOT NULL,
    PRIMARY KEY (root, worktree, seq));
PRAGMA user_version = 1;
COMMIT;
";

/// The SQLite-backed `Index` (§6).
#[derive(Debug)]
pub struct SqliteIndex {
    conn: Connection,
}

impl SqliteIndex {
    pub fn open(cache_dir: &Path) -> Result<SqliteIndex> {
        fs::ensure_cache_dir(cache_dir)?;
        let db = cache_dir.join(fs::DB_FILE);
        let mut conn = connect(&db)?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        // 0 is a database we just created; anything else but ours is another schema.
        if version != 0 && version != SCHEMA_VERSION {
            drop(conn);
            remove_db_files(&db)?;
            conn = connect(&db)?;
        }
        if version != SCHEMA_VERSION {
            conn.execute_batch(SCHEMA)?;
        }
        Ok(SqliteIndex { conn })
    }

    /// JSON blobs from the first column of `sql`, parsed with `parse`, in row order.
    fn rows<T>(
        &self,
        sql: &str,
        params: impl Params,
        parse: impl Fn(&str) -> serde_json::Result<T>,
    ) -> Result<Vec<T>> {
        let mut stmt = self.conn.prepare_cached(sql)?;
        let texts = stmt
            .query_map(params, |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let parsed = texts
            .iter()
            .map(|t| parse(t))
            .collect::<serde_json::Result<Vec<T>>>()?;
        Ok(parsed)
    }

    fn root_column<T: FromSql>(&self, column: &str, root: &Path) -> Result<Option<T>> {
        let sql = format!("SELECT {column} FROM roots WHERE root = ?1");
        Ok(self
            .conn
            .query_row(&sql, [path_key(root)], |r| r.get(0))
            .optional()?)
    }

    fn read_source_state(&self, s: SourceKind) -> Result<SourceState> {
        let name = source_name(s);
        Ok(SourceState {
            files: self.rows(
                "SELECT state FROM source_files WHERE source = ?1 ORDER BY seq",
                [name],
                |t| serde_json::from_str(t),
            )?,
            sessions: self.rows(
                "SELECT session FROM sessions WHERE source = ?1 ORDER BY seq",
                [name],
                |t| serde_json::from_str(t),
            )?,
        })
    }

    fn read_scanned_at(&self, root: &Path) -> Result<Option<Timestamp>> {
        let text: Option<String> = self.root_column("scanned_at", root)?;
        Ok(text.map(|t| t.parse::<Timestamp>()).transpose()?)
    }

    fn read_diagnostics(&self, root: &Path) -> Result<Option<ScanDiagnostics>> {
        let text: Option<String> = self.root_column("diagnostics", root)?;
        Ok(text
            .map(|t| serde_json::from_str::<ScanDiagnostics>(&t))
            .transpose()?)
    }
}

// The `Index` reads are infallible by contract. The cache is disposable, so a read that
// fails (busy past the timeout, an unreadable row) answers as "nothing cached" and the
// next scan repopulates it; `commit_scan` is the surface that reports errors.
impl Index for SqliteIndex {
    fn source_state(&self, s: SourceKind) -> SourceState {
        self.read_source_state(s).unwrap_or_default()
    }

    fn repos_for_root(&self, root: &Path) -> Vec<RepoRow> {
        self.rows(
            "SELECT repo FROM root_repos WHERE root = ?1 ORDER BY seq",
            [path_key(root)],
            |t| serde_json::from_str(t),
        )
        .unwrap_or_default()
    }

    fn sessions_for(&self, root: &Path, worktree: &Path) -> Vec<Session> {
        self.rows(
            "SELECT session FROM matches WHERE root = ?1 AND worktree = ?2 ORDER BY seq",
            (path_key(root), path_key(worktree)),
            |t| serde_json::from_str(t),
        )
        .unwrap_or_default()
    }

    fn scanned_at(&self, root: &Path) -> Option<Timestamp> {
        self.read_scanned_at(root).ok().flatten()
    }

    fn scanned_submodules(&self, root: &Path) -> Option<bool> {
        self.root_column("submodules", root).ok().flatten()
    }

    fn last_diagnostics(&self, root: &Path) -> Option<ScanDiagnostics> {
        self.read_diagnostics(root).ok().flatten()
    }

    fn commit_scan(&mut self, root: &Path, scan: ScanResult) -> Result<()> {
        // Resolve every match before touching the DB, as `MemIndex` does.
        let matches = resolve_matches(&scan)?;
        let root_key = path_key(root);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        for source in &scan.sources {
            let name = source_name(source.diag.source);
            tx.execute("DELETE FROM source_files WHERE source = ?1", [name])?;
            tx.execute("DELETE FROM sessions WHERE source = ?1", [name])?;
            for (seq, file) in (0_i64..).zip(source.files.iter()) {
                let state = serde_json::to_string(file)?;
                insert(
                    &tx,
                    "INSERT INTO source_files (source, seq, state) VALUES (?1, ?2, ?3)",
                    params![name, seq, state],
                )?;
            }
            for (seq, session) in (0_i64..).zip(source.sessions.iter()) {
                let json = serde_json::to_string(session)?;
                insert(
                    &tx,
                    "INSERT INTO sessions (source, seq, id, session) VALUES (?1, ?2, ?3, ?4)",
                    params![name, seq, session.id.as_str(), json],
                )?;
            }
        }

        tx.execute("DELETE FROM root_repos WHERE root = ?1", [&root_key])?;
        tx.execute("DELETE FROM matches WHERE root = ?1", [&root_key])?;
        let diagnostics = serde_json::to_string(&scan.diagnostics)?;
        insert(
            &tx,
            "INSERT OR REPLACE INTO roots (root, scanned_at, submodules, diagnostics)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                root_key,
                scan.scanned_at.to_string(),
                scan.submodules,
                diagnostics
            ],
        )?;
        for (seq, repo) in (0_i64..).zip(scan.repos.iter()) {
            insert(
                &tx,
                "INSERT INTO root_repos (root, seq, repo_path, kind, contains_root, repo)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    root_key,
                    seq,
                    path_key(&repo.path),
                    serde_json::to_string(&repo.kind)?,
                    repo.contains_root,
                    serde_json::to_string(repo)?,
                ],
            )?;
        }
        for (worktree, sessions) in &matches {
            for (seq, session) in (0_i64..).zip(sessions.iter()) {
                insert(
                    &tx,
                    "INSERT INTO matches (root, worktree, seq, session) VALUES (?1, ?2, ?3, ?4)",
                    params![root_key, worktree, seq, serde_json::to_string(session)?],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

/// Opens the database in WAL mode, creating it `0600` first whatever the umask.
fn connect(db: &Path) -> Result<Connection> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(DB_MODE)
        .open(db)?;
    std::fs::set_permissions(db, Permissions::from_mode(DB_MODE))?;
    let conn = Connection::open(db)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    // SQLite gives -wal and -shm the main file's mode.
    let mode: String = conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(format!("cannot enable WAL journaling (got {mode:?})").into());
    }
    Ok(conn)
}

fn remove_db_files(db: &Path) -> Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let mut name = db.as_os_str().to_owned();
        name.push(suffix);
        match std::fs::remove_file(&name) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    Ok(())
}

fn insert(tx: &Transaction<'_>, sql: &str, params: impl Params) -> Result<()> {
    #[cfg(test)]
    fire_panic_hook();
    tx.prepare_cached(sql)?.execute(params)?;
    Ok(())
}

#[cfg(test)]
fn fire_panic_hook() {
    PANIC_AFTER_INSERTS.with(|c| match c.get() {
        Some(0) => panic!("injected panic between inserts"),
        Some(n) => c.set(Some(n - 1)),
        None => {}
    });
}

fn source_name(s: SourceKind) -> &'static str {
    match s {
        SourceKind::Claude => "claude",
        SourceKind::Codex => "codex",
    }
}

/// Component-normalised bytes, so keys compare like `PathBuf` does (`/a/b/` == `/a/b`).
fn path_key(p: &Path) -> Vec<u8> {
    p.components()
        .collect::<PathBuf>()
        .into_os_string()
        .into_vec()
}

/// Each worktree's matched sessions, `ended_at` desc (ties by id).
fn resolve_matches(scan: &ScanResult) -> Result<BTreeMap<Vec<u8>, Vec<&Session>>> {
    let known: HashMap<(SourceKind, &Uuid), &Session> = scan
        .sources
        .iter()
        .flat_map(|s| &s.sessions)
        .map(|s| ((s.source, &s.id), s))
        .collect();
    let mut matches: BTreeMap<Vec<u8>, Vec<&Session>> = BTreeMap::new();
    for (kind, id, worktree) in &scan.matches {
        let Some(worktree) = worktree else { continue };
        let session = known
            .get(&(*kind, id))
            .ok_or_else(|| format!("match names unknown session {}", id.as_str()))?;
        matches.entry(path_key(worktree)).or_default().push(session);
    }
    for sessions in matches.values_mut() {
        sessions.sort_by(|a, b| {
            b.ended_at
                .cmp(&a.ended_at)
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });
    }
    Ok(matches)
}

/// Holds the per-root scan lock until dropped.
#[derive(Debug)]
pub struct ScanLock {
    _file: File,
}

pub fn cache_dir(home: &Path, xdg_cache_home: Option<&Path>) -> PathBuf {
    fs::cache_dir(home, xdg_cache_home)
}

pub fn ensure_cache_dir(dir: &Path) -> Result<()> {
    fs::ensure_cache_dir(dir)
}

pub fn try_scan_lock(cache_dir: &Path, root: &Path) -> Result<Option<ScanLock>> {
    fs::try_scan_lock(cache_dir, root)
}

pub fn clear(cache_dir: &Path) -> Result<()> {
    fs::clear(cache_dir)
}

pub fn effective_uid(cache_dir: &Path) -> Result<u32> {
    fs::effective_uid(cache_dir)
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use rollcall_core::{
        Backing, DoctorDoc, FileState, MemIndex, RepoKind, RepoState, SCHEMA_VERSION,
        SourceDiagnostics, SourceMode, SourceScan, TextField, TextRef, TitleKind, Uuid,
        WorktreeState,
    };

    use super::*;

    const ID1: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
    const ID2: &str = "1f8fad5b-d9cb-469f-a165-70867728950e";
    const ID3: &str = "2f8fad5b-d9cb-469f-a165-70867728950e";
    const CODEX_ID: &str = "3f8fad5b-d9cb-469f-a165-70867728950e";

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn session(source: SourceKind, id: &str, ended: &str, cwd: &str) -> Session {
        let id = Uuid::parse(id).unwrap();
        let backing = match source {
            SourceKind::Claude => Backing::LogFile {
                dev: u64::MAX,
                inode: 2,
                path: PathBuf::from("/log.jsonl"),
            },
            SourceKind::Codex => Backing::CodexThread {
                db_path: PathBuf::from("/codex.db"),
                thread_id: id.clone(),
            },
        };
        Session {
            source,
            id: id.clone(),
            cwd: PathBuf::from(cwd),
            branch: Some("main".into()),
            started_at: ts("2026-09-29T09:00:00.123456789Z"),
            ended_at: ts(ended),
            title: Some((
                TextRef::LogLine {
                    dev: 1,
                    inode: 2,
                    size_at_scan: 300,
                    offset: 10,
                    len: 20,
                    field: TextField::Title,
                },
                TitleKind::Custom,
            )),
            last_prompt: Some(TextRef::CodexThread {
                thread_id: id,
                field: TextField::LastPrompt,
            }),
            message_count: 7,
            backing,
        }
    }

    fn file_state(inode: u64) -> FileState {
        FileState {
            dev: 1,
            inode,
            path: PathBuf::from("/log.jsonl"),
            size: 10,
            mtime: ts("2026-09-29T10:00:00Z"),
            offset: 10,
            head_fp: [1; 16],
            tail_fp: [2; 16],
            parser_version: 1,
        }
    }

    fn source_scan(source: SourceKind, sessions: Vec<Session>, inodes: &[u64]) -> SourceScan {
        SourceScan {
            files: inodes.iter().map(|i| file_state(*i)).collect(),
            diag: SourceDiagnostics {
                source,
                mode: SourceMode::Jsonl,
                db_version: Some(5),
                files: inodes.len() as u32,
                records: 3,
                skipped: Default::default(),
                sessions: sessions.len() as u32,
                excluded_children: 0,
                vanished: 0,
                matched: 0,
                unmatched: Default::default(),
            },
            sessions,
            notes: vec![],
        }
    }

    fn repo_row(path: &str, kind: RepoKind, contains_root: bool) -> RepoRow {
        RepoRow {
            path: PathBuf::from(path),
            name: "r".into(),
            kind,
            contains_root,
            state: RepoState {
                default_branch: Some("main".into()),
                has_remote: true,
                error: Some("boom".into()),
                worktrees: vec![WorktreeState {
                    path: PathBuf::from(path),
                    is_main: true,
                    branch: Some("main".into()),
                    detached: false,
                    head: Some("abc".into()),
                    last_commit_at: Some(ts("2026-09-28T00:00:00Z")),
                    upstream: None,
                    ahead: Some(1),
                    behind: None,
                    unpushed: Some(0),
                    dirty_count: 2,
                    untracked_count: 3,
                    missing: false,
                    merged: false,
                    error: None,
                    locked: false,
                }],
                pending: None,
            },
            submodules: vec![RepoRow {
                path: PathBuf::from(format!("{path}/sub")),
                name: "sub".into(),
                kind: RepoKind::Nested,
                contains_root: false,
                state: RepoState {
                    default_branch: None,
                    has_remote: false,
                    error: None,
                    worktrees: vec![],
                    pending: None,
                },
                submodules: vec![],
            }],
        }
    }

    fn doctor(root: &str, sources: Vec<SourceDiagnostics>) -> DoctorDoc {
        DoctorDoc {
            schema_version: SCHEMA_VERSION,
            root: root.to_owned(),
            scanned_at: Some(ts("2026-09-29T10:00:00Z")),
            walk: Default::default(),
            repos_by_kind: Default::default(),
            worktrees: Default::default(),
            submodules: 0,
            repo_errors: 1,
            untrusted_git_dirs: 0,
            repo_timeouts: 0,
            repo_timeouts_abandoned: 0,
            notes: vec![format!("note for {root}")],
            sources,
        }
    }

    type Match<'a> = (SourceKind, &'a str, Option<&'a str>);

    fn scan(
        root: &str,
        submodules: bool,
        sources: Vec<SourceScan>,
        matches: Vec<Match<'_>>,
        repos: Vec<RepoRow>,
    ) -> ScanResult {
        let diag = sources.iter().map(|s| s.diag.clone()).collect();
        ScanResult {
            scanned_at: ts("2026-09-29T10:00:00Z"),
            submodules,
            partial: false,
            repos,
            matches: matches
                .into_iter()
                .map(|(k, id, wt)| (k, Uuid::parse(id).unwrap(), wt.map(PathBuf::from)))
                .collect(),
            sources,
            diagnostics: doctor(root, diag),
        }
    }

    pub(crate) fn claude_scan(root: &str, submodules: bool, matches: Vec<Match<'_>>) -> ScanResult {
        let sessions = vec![
            session(SourceKind::Claude, ID1, "2026-09-29T09:30:00Z", "/A/wA"),
            session(SourceKind::Claude, ID2, "2026-09-29T09:50:00Z", "/A/wA"),
            session(
                SourceKind::Claude,
                ID3,
                "2026-09-29T09:10:00Z",
                "/elsewhere",
            ),
        ];
        scan(
            root,
            submodules,
            vec![source_scan(SourceKind::Claude, sessions, &[9, 3, 5])],
            matches,
            vec![repo_row(root, RepoKind::Normal, false)],
        )
    }

    /// Every read the trait offers, for the probe roots and worktrees below.
    #[derive(Debug, PartialEq)]
    struct View {
        sources: Vec<SourceState>,
        roots: Vec<RootView>,
    }

    #[derive(Debug, PartialEq)]
    struct RootView {
        scanned_at: Option<Timestamp>,
        submodules: Option<bool>,
        diagnostics: Option<DoctorDoc>,
        repos: Vec<RepoRow>,
        sessions: Vec<Vec<Session>>,
    }

    const ROOTS: [&str; 3] = ["/A", "/B", "/unscanned"];
    const WORKTREES: [&str; 7] = [
        "/A/wA",
        "/A/wA/",
        "/A/w2",
        "/B/wB",
        "/A/wB",
        "/nowhere",
        "/elsewhere",
    ];

    fn view(ix: &impl Index) -> View {
        View {
            sources: [SourceKind::Claude, SourceKind::Codex]
                .into_iter()
                .map(|s| ix.source_state(s))
                .collect(),
            roots: ROOTS
                .iter()
                .map(|r| {
                    let root = Path::new(r);
                    RootView {
                        scanned_at: ix.scanned_at(root),
                        submodules: ix.scanned_submodules(root),
                        diagnostics: ix.last_diagnostics(root),
                        repos: ix.repos_for_root(root),
                        sessions: WORKTREES
                            .iter()
                            .map(|w| ix.sessions_for(root, Path::new(w)))
                            .collect(),
                    }
                })
                .collect(),
        }
    }

    fn open_tmp() -> (tempfile::TempDir, SqliteIndex) {
        let dir = tempfile::tempdir().unwrap();
        let ix = SqliteIndex::open(&dir.path().join("cache")).unwrap();
        (dir, ix)
    }

    #[test]
    fn parity_with_mem_index_on_a_shared_script() {
        let (_dir, mut sql) = open_tmp();
        let mut mem = MemIndex::new();
        assert_eq!(view(&sql), view(&mem), "empty stores agree");

        let mut b_scan = claude_scan(
            "/B",
            true,
            vec![
                (SourceKind::Claude, ID1, Some("/B/wB")),
                (SourceKind::Codex, CODEX_ID, Some("/B/wB")),
            ],
        );
        b_scan.sources.push(source_scan(
            SourceKind::Codex,
            vec![session(
                SourceKind::Codex,
                CODEX_ID,
                "2026-09-29T09:40:00Z",
                "/B/wB",
            )],
            &[],
        ));
        b_scan.repos = vec![
            repo_row("/B", RepoKind::ExternalMain, true),
            repo_row("/A", RepoKind::Normal, false),
        ];

        let script: Vec<(&str, ScanResult)> = vec![
            (
                "/A",
                claude_scan(
                    "/A",
                    false,
                    vec![
                        (SourceKind::Claude, ID1, Some("/A/wA")),
                        (SourceKind::Claude, ID2, Some("/A/wA")),
                        (SourceKind::Claude, ID3, None),
                    ],
                ),
            ),
            ("/B", b_scan),
            (
                "/A",
                claude_scan("/A", true, vec![(SourceKind::Claude, ID1, Some("/A/w2"))]),
            ),
            (
                "/A",
                claude_scan("/A", false, vec![(SourceKind::Claude, ID2, Some("/A/wA"))]),
            ),
            (
                "/A",
                claude_scan(
                    "/A",
                    false,
                    vec![(SourceKind::Claude, CODEX_ID, Some("/A/wA"))],
                ),
            ),
        ];
        for (i, (root, sc)) in script.into_iter().enumerate() {
            let m = mem.commit_scan(Path::new(root), sc.clone());
            let s = sql.commit_scan(Path::new(root), sc);
            assert_eq!(s.is_ok(), m.is_ok(), "step {i}: commit outcome");
            assert_eq!(view(&sql), view(&mem), "step {i}: views differ");
        }
    }

    #[test]
    fn matches_are_keyed_by_root_and_worktree() {
        let (_dir, mut ix) = open_tmp();
        let s1 = session(SourceKind::Claude, ID1, "2026-09-29T09:30:00Z", "/A/wA");
        ix.commit_scan(
            Path::new("/A"),
            claude_scan("/A", false, vec![(SourceKind::Claude, ID1, Some("/A/wA"))]),
        )
        .unwrap();
        ix.commit_scan(
            Path::new("/B"),
            claude_scan("/B", true, vec![(SourceKind::Claude, ID1, Some("/B/wB"))]),
        )
        .unwrap();

        let got = |root: &str, wt: &str| ix.sessions_for(Path::new(root), Path::new(wt));
        assert_eq!(got("/A", "/A/wA"), vec![s1.clone()]);
        assert_eq!(got("/A", "/A/wA/"), vec![s1.clone()]);
        assert_eq!(got("/B", "/B/wB"), vec![s1]);
        assert!(got("/A", "/B/wB").is_empty());
        assert!(got("/unscanned", "/A/wA").is_empty());
        assert_eq!(ix.scanned_submodules(Path::new("/A")), Some(false));
        assert_eq!(ix.scanned_submodules(Path::new("/B")), Some(true));
    }

    #[test]
    fn sessions_for_orders_ended_at_desc() {
        let (_dir, mut ix) = open_tmp();
        ix.commit_scan(
            Path::new("/A"),
            claude_scan(
                "/A",
                false,
                vec![
                    (SourceKind::Claude, ID1, Some("/A/wA")),
                    (SourceKind::Claude, ID2, Some("/A/wA")),
                ],
            ),
        )
        .unwrap();
        let ids: Vec<_> = ix
            .sessions_for(Path::new("/A"), Path::new("/A/wA"))
            .into_iter()
            .map(|s| s.id.as_str().to_owned())
            .collect();
        assert_eq!(ids, [ID2, ID1]);
    }

    #[test]
    fn data_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let sc = claude_scan("/A", true, vec![(SourceKind::Claude, ID1, Some("/A/wA"))]);
        let mut ix = SqliteIndex::open(&cache).unwrap();
        ix.commit_scan(Path::new("/A"), sc.clone()).unwrap();
        drop(ix);

        let ix = SqliteIndex::open(&cache).unwrap();
        assert_eq!(ix.repos_for_root(Path::new("/A")), sc.repos);
        assert_eq!(ix.last_diagnostics(Path::new("/A")), Some(sc.diagnostics));
        assert_eq!(ix.source_state(SourceKind::Claude).sessions.len(), 3);
    }

    #[test]
    fn a_match_naming_an_unknown_session_fails_and_keeps_prior_state() {
        let (_dir, mut ix) = open_tmp();
        ix.commit_scan(
            Path::new("/A"),
            claude_scan("/A", false, vec![(SourceKind::Claude, ID1, Some("/A/wA"))]),
        )
        .unwrap();
        let before = view(&ix);
        let bad = claude_scan(
            "/A",
            true,
            vec![(SourceKind::Codex, CODEX_ID, Some("/A/wA"))],
        );
        assert!(ix.commit_scan(Path::new("/A"), bad).is_err());
        assert_eq!(view(&ix), before);
    }

    #[test]
    fn a_panic_between_inserts_leaves_the_prior_scan_intact() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let mut ix = SqliteIndex::open(&cache).unwrap();
        ix.commit_scan(
            Path::new("/A"),
            claude_scan("/A", false, vec![(SourceKind::Claude, ID1, Some("/A/wA"))]),
        )
        .unwrap();
        let before = view(&ix);

        let second = claude_scan("/A", true, vec![(SourceKind::Claude, ID2, Some("/A/w2"))]);
        PANIC_AFTER_INSERTS.with(|c| c.set(Some(4)));
        let outcome = catch_unwind(AssertUnwindSafe(|| ix.commit_scan(Path::new("/A"), second)));
        let remaining = PANIC_AFTER_INSERTS.with(|c| c.replace(None));
        assert!(outcome.is_err(), "the hook must fire mid-commit");
        assert_eq!(remaining, Some(0), "panicked on the fifth insert");
        assert_eq!(view(&ix), before, "same connection after the unwind");
        drop(ix);

        let reopened = SqliteIndex::open(&cache).unwrap();
        assert_eq!(view(&reopened), before, "fresh connection");
    }

    #[test]
    fn user_version_mismatch_rebuilds_empty_without_error() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let scan_a = || claude_scan("/A", false, vec![(SourceKind::Claude, ID1, Some("/A/wA"))]);
        let mut ix = SqliteIndex::open(&cache).unwrap();
        ix.commit_scan(Path::new("/A"), scan_a()).unwrap();
        drop(ix);

        let raw = rusqlite::Connection::open(cache.join(fs::DB_FILE)).unwrap();
        raw.pragma_update(None, "user_version", 9999).unwrap();
        drop(raw);

        let mut ix = SqliteIndex::open(&cache).unwrap();
        assert_eq!(view(&ix), view(&MemIndex::new()), "rebuilt empty");
        ix.commit_scan(Path::new("/A"), scan_a()).unwrap();
        drop(ix);
        let ix = SqliteIndex::open(&cache).unwrap();
        assert_eq!(ix.scanned_submodules(Path::new("/A")), Some(false));
    }
}
