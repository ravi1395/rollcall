//! Domain types, the `Index` trait and the §7 output documents. No I/O.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub mod matching;
pub mod mem;
pub mod rank;
pub mod text;

pub use mem::MemIndex;

pub type Timestamp = jiff::Timestamp;
pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

/// `schema_version` of every §7 document.
pub const SCHEMA_VERSION: u32 = 1;

// ---- Uuid (§5.0) ----

/// A session id matching `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-…-[0-9a-fA-F]{12}$`; case preserved.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Uuid(String);

const UUID_LEN: usize = 36;
const UUID_HYPHENS: [usize; 4] = [8, 13, 18, 23];

impl Uuid {
    pub fn parse(s: &str) -> Option<Uuid> {
        let valid = s.len() == UUID_LEN
            && s.bytes().enumerate().all(|(i, b)| {
                if UUID_HYPHENS.contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit()
                }
            });
        valid.then(|| Uuid(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Uuid {
    type Error = String;
    fn try_from(s: String) -> std::result::Result<Self, String> {
        Uuid::parse(&s).ok_or_else(|| format!("invalid session id: {s:?}"))
    }
}

impl From<Uuid> for String {
    fn from(u: Uuid) -> String {
        u.0
    }
}

// ---- §3.4 session model ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Claude,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextField {
    Title,
    LastPrompt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextRef {
    LogLine {
        dev: u64,
        inode: u64,
        size_at_scan: u64,
        offset: u64,
        len: u32,
        field: TextField,
    },
    CodexThread {
        thread_id: Uuid,
        field: TextField,
    },
}

/// Provenance of a session title, used by MCP (§8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitleKind {
    Custom,
    Ai,
    Thread,
    Prompt,
}

/// The file or thread row a session lives in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Backing {
    LogFile { dev: u64, inode: u64, path: PathBuf },
    CodexThread { db_path: PathBuf, thread_id: Uuid },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub source: SourceKind,
    pub id: Uuid,
    pub cwd: PathBuf,
    pub branch: Option<String>,
    pub started_at: Timestamp,
    pub ended_at: Timestamp,
    pub title: Option<(TextRef, TitleKind)>,
    pub last_prompt: Option<TextRef>,
    pub message_count: u32,
    pub backing: Backing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileState {
    pub dev: u64,
    pub inode: u64,
    pub path: PathBuf,
    pub size: u64,
    pub mtime: Timestamp,
    pub offset: u64,
    pub head_fp: [u8; 16],
    pub tail_fp: [u8; 16],
    pub parser_version: u32,
}

/// Prior scan of one source.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceState {
    pub files: Vec<FileState>,
    pub sessions: Vec<Session>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceScan {
    pub files: Vec<FileState>,
    pub sessions: Vec<Session>,
    pub diag: SourceDiagnostics,
    /// Flow into `DoctorDoc.notes`.
    pub notes: Vec<String>,
}

pub trait Index {
    fn source_state(&self, s: SourceKind) -> SourceState;
    /// Via the root-membership table (§6.1).
    fn repos_for_root(&self, root: &Path) -> Vec<RepoRow>;
    /// Sessions matched by this root's last scan, `ended_at` desc.
    fn sessions_for(&self, root: &Path, worktree: &Path) -> Vec<Session>;
    fn scanned_at(&self, root: &Path) -> Option<Timestamp>;
    /// The `--submodules` flag of that scan (§6.1).
    fn scanned_submodules(&self, root: &Path) -> Option<bool>;
    /// For `doctor` under contention.
    fn last_diagnostics(&self, root: &Path) -> Option<ScanDiagnostics>;
    /// One transaction.
    fn commit_scan(&mut self, root: &Path, scan: ScanResult) -> Result<()>;
}

pub fn match_sessions(worktrees: &[WorktreeKey], sessions: &[SessionLocation]) -> Vec<MatchResult> {
    matching::match_sessions(worktrees, sessions)
}

// ---- matching inputs/outputs (§6.5) ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeKey {
    pub canonical_path: PathBuf,
    pub hidden: bool,
    pub missing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLocation {
    pub cwd_canonical: PathBuf,
    pub cwd_exists: bool,
    pub under_root: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchResult {
    /// Index into the `worktrees` slice.
    Matched(usize),
    OutsideRoot,
    PathGone,
    NonRepo,
}

// ---- git state ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoKind {
    Normal,
    Nested,
    Bare,
    ExternalMain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StaleReason {
    Missing,
    Merged,
    Inactive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeState {
    pub path: PathBuf,
    pub is_main: bool,
    pub branch: Option<String>,
    pub detached: bool,
    pub head: Option<String>,
    pub last_commit_at: Option<Timestamp>,
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub unpushed: Option<u32>,
    pub dirty_count: u32,
    pub untracked_count: u32,
    pub missing: bool,
    pub merged: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoState {
    pub default_branch: Option<String>,
    pub has_remote: bool,
    pub error: Option<String>,
    pub worktrees: Vec<WorktreeState>,
    /// Set only for a timed-out repo; the abandoned worker stores `true` when it finishes.
    #[serde(skip)]
    pub pending: Option<Arc<AtomicBool>>,
}

// `AtomicBool` has no `PartialEq`; two states are equal when they describe the same
// repo and share (or both lack) the same pending flag.
impl PartialEq for RepoState {
    fn eq(&self, o: &Self) -> bool {
        let same_pending = match (&self.pending, &o.pending) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        same_pending
            && self.default_branch == o.default_branch
            && self.has_remote == o.has_remote
            && self.error == o.error
            && self.worktrees == o.worktrees
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoRow {
    pub path: PathBuf,
    pub name: String,
    pub kind: RepoKind,
    pub contains_root: bool,
    pub state: RepoState,
    pub submodules: Vec<RepoRow>,
}

// ---- discovery ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkOptions {
    pub home: PathBuf,
    pub confirmed: bool,
    pub follow_symlinks: bool,
    pub submodules: bool,
    pub skip_dirs: Vec<String>,
    pub budget_entries: u64,
    pub budget: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitOptions {
    pub root: PathBuf,
    pub home: PathBuf,
    /// §4.2 path skips only; name skips are not a trust boundary.
    pub skip_paths: Vec<PathBuf>,
    pub repo_budget: Duration,
    pub excludes_file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredRepo {
    pub git_dir: PathBuf,
    pub workdir: Option<PathBuf>,
    pub kind: RepoKind,
    pub contains_root: bool,
    pub submodule_of: Option<usize>,
    pub linked: Vec<LinkedWorktree>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedWorktree {
    pub path: PathBuf,
    pub admin_dir: PathBuf,
    pub missing: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Discovery {
    pub repos: Vec<DiscoveredRepo>,
    pub walk: WalkDiag,
    pub partial: bool,
    pub notes: Vec<String>,
}

// ---- scan result ----

#[derive(Debug, Clone, PartialEq)]
pub struct ScanResult {
    pub scanned_at: Timestamp,
    pub submodules: bool,
    pub partial: bool,
    pub repos: Vec<RepoRow>,
    pub sources: Vec<SourceScan>,
    /// Each session's matched worktree for this root (`None`: unmatched).
    pub matches: Vec<(SourceKind, Uuid, Option<PathBuf>)>,
    pub diagnostics: DoctorDoc,
}

pub type ScanDiagnostics = DoctorDoc;

// ---- §7 documents ----

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusDoc {
    pub schema_version: u32,
    pub root: String,
    pub scanned_at: Option<Timestamp>,
    pub stale: bool,
    pub partial: bool,
    pub repos: Vec<Repo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Repo {
    pub name: String,
    pub path: String,
    pub kind: RepoKind,
    pub contains_root: bool,
    pub default_branch: Option<String>,
    pub has_remote: bool,
    pub attention: u8,
    pub last_activity_at: Option<Timestamp>,
    pub error: Option<String>,
    pub worktrees: Vec<Worktree>,
    /// Present only with `--submodules`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submodules: Option<Vec<Repo>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Worktree {
    pub path: String,
    pub is_main: bool,
    pub external: bool,
    pub branch: Option<String>,
    pub detached: bool,
    pub head: Option<String>,
    pub last_commit_at: Option<Timestamp>,
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub unpushed: Option<u32>,
    pub dirty_count: u32,
    pub untracked_count: u32,
    pub stale_reason: Option<StaleReason>,
    pub attention: u8,
    pub error: Option<String>,
    pub last_session: Option<SessionOut>,
    /// Count of other matched sessions.
    pub older_sessions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionOut {
    pub source: SourceKind,
    pub id: String,
    pub branch: Option<String>,
    pub cwd: String,
    pub started_at: Timestamp,
    pub ended_at: Timestamp,
    pub title: Option<String>,
    /// `None` iff `title` is `None`.
    pub title_kind: Option<TitleKind>,
    pub last_prompt: Option<String>,
    pub message_count: u32,
    /// The §8.1 display line; `None` if the cwd is gone.
    pub resume: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WhereWasIDoc {
    pub schema_version: u32,
    pub root: String,
    pub scanned_at: Option<Timestamp>,
    pub stale: bool,
    pub partial: bool,
    pub truncated: bool,
    pub repos: Vec<Repo>,
    pub recent_sessions: Vec<SessionOut>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaleItem {
    pub repo_name: String,
    pub repo_path: String,
    pub worktree_path: String,
    pub branch: Option<String>,
    pub stale_reason: StaleReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListStaleDoc {
    pub schema_version: u32,
    pub root: String,
    pub scanned_at: Option<Timestamp>,
    pub stale: bool,
    pub partial: bool,
    pub truncated: bool,
    pub items: Vec<StaleItem>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalkDiag {
    pub entries: u64,
    pub budget_hit: bool,
    pub stopped_at: Option<String>,
    pub unreadable_dirs: u32,
    pub skipped_dirs: u32,
    pub skipped_mounts: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReposByKind {
    pub normal: u32,
    pub nested: u32,
    pub bare: u32,
    pub external_main: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeCounts {
    pub total: u32,
    pub external: u32,
    pub missing: u32,
    pub untrusted_path: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceMode {
    Jsonl,
    Db,
    Rollouts,
    /// Source root missing or containing no session files/DB.
    Absent,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedCounts {
    pub malformed: u32,
    pub unknown_type: u32,
    pub oversize: u32,
    pub non_uuid: u32,
    pub bad_rollout_path: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnmatchedCounts {
    pub outside_root: u32,
    pub path_gone: u32,
    pub non_repo: u32,
}

/// One `DoctorDoc.sources[]` entry. Sources leave `matched` / `unmatched` at 0; P30 fills them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceDiagnostics {
    pub source: SourceKind,
    pub mode: SourceMode,
    pub db_version: Option<u32>,
    pub files: u32,
    pub records: u64,
    pub skipped: SkippedCounts,
    pub sessions: u32,
    pub excluded_children: u32,
    pub vanished: u32,
    pub matched: u32,
    pub unmatched: UnmatchedCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorDoc {
    pub schema_version: u32,
    pub root: String,
    pub scanned_at: Option<Timestamp>,
    pub walk: WalkDiag,
    pub repos_by_kind: ReposByKind,
    pub worktrees: WorktreeCounts,
    pub submodules: u32,
    pub repo_errors: u32,
    pub untrusted_git_dirs: u32,
    pub repo_timeouts: u32,
    pub repo_timeouts_abandoned: u32,
    pub notes: Vec<String>,
    pub sources: Vec<SourceDiagnostics>,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};

    use super::*;

    const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    /// Keys of the JSON object at `v`, asserting it is an object.
    fn keys(v: &Value) -> BTreeSet<&str> {
        v.as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect()
    }

    fn assert_keys(v: &Value, expected: &[&str], what: &str) {
        assert_eq!(
            keys(v),
            expected.iter().copied().collect::<BTreeSet<_>>(),
            "{what}"
        );
    }

    fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(doc: &T) -> Value {
        let v = serde_json::to_value(doc).unwrap();
        let back: T = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(&back, doc);
        v
    }

    fn session_out() -> SessionOut {
        SessionOut {
            source: SourceKind::Codex,
            id: ID.into(),
            branch: Some("main".into()),
            cwd: "/r".into(),
            started_at: ts("2026-09-29T09:00:00Z"),
            ended_at: ts("2026-09-29T09:30:00.5Z"),
            title: Some("t".into()),
            title_kind: Some(TitleKind::Thread),
            last_prompt: Some("p".into()),
            message_count: 4,
            resume: Some("cd /r && codex resume".into()),
        }
    }

    fn worktree() -> Worktree {
        Worktree {
            path: "/r".into(),
            is_main: true,
            external: false,
            branch: Some("main".into()),
            detached: false,
            head: Some("abc123".into()),
            last_commit_at: Some(ts("2026-09-28T00:00:00Z")),
            upstream: Some("origin/main".into()),
            ahead: Some(1),
            behind: Some(2),
            unpushed: Some(1),
            dirty_count: 3,
            untracked_count: 4,
            stale_reason: Some(StaleReason::Inactive),
            attention: 3,
            error: None,
            last_session: Some(session_out()),
            older_sessions: 2,
        }
    }

    fn repo(submodules: Option<Vec<Repo>>) -> Repo {
        Repo {
            name: "r".into(),
            path: "/r".into(),
            kind: RepoKind::ExternalMain,
            contains_root: true,
            default_branch: Some("main".into()),
            has_remote: true,
            attention: 3,
            last_activity_at: Some(ts("2026-09-29T09:30:00Z")),
            error: None,
            worktrees: vec![worktree()],
            submodules,
        }
    }

    const REPO_KEYS: &[&str] = &[
        "name",
        "path",
        "kind",
        "contains_root",
        "default_branch",
        "has_remote",
        "attention",
        "last_activity_at",
        "error",
        "worktrees",
    ];
    const WORKTREE_KEYS: &[&str] = &[
        "path",
        "is_main",
        "external",
        "branch",
        "detached",
        "head",
        "last_commit_at",
        "upstream",
        "ahead",
        "behind",
        "unpushed",
        "dirty_count",
        "untracked_count",
        "stale_reason",
        "attention",
        "error",
        "last_session",
        "older_sessions",
    ];
    const SESSION_KEYS: &[&str] = &[
        "source",
        "id",
        "branch",
        "cwd",
        "started_at",
        "ended_at",
        "title",
        "title_kind",
        "last_prompt",
        "message_count",
        "resume",
    ];

    #[test]
    fn status_doc_has_exactly_the_spec_keys_and_round_trips() {
        let doc = StatusDoc {
            schema_version: SCHEMA_VERSION,
            root: "/r".into(),
            scanned_at: Some(ts("2026-09-29T10:00:00Z")),
            stale: false,
            partial: true,
            repos: vec![repo(Some(vec![repo(None)]))],
        };
        let v = round_trip(&doc);

        assert_keys(
            &v,
            &[
                "schema_version",
                "root",
                "scanned_at",
                "stale",
                "partial",
                "repos",
            ],
            "StatusDoc",
        );
        assert_eq!(v["schema_version"], 1);
        let r = &v["repos"][0];
        let mut with_sub = REPO_KEYS.to_vec();
        with_sub.push("submodules");
        assert_keys(r, &with_sub, "Repo with submodules");
        assert_keys(
            &r["submodules"][0],
            REPO_KEYS,
            "submodule Repo (submodules omitted when None)",
        );
        assert_keys(&r["worktrees"][0], WORKTREE_KEYS, "Worktree");
        assert_keys(&r["worktrees"][0]["last_session"], SESSION_KEYS, "Session");
        assert_eq!(r["kind"], "external_main");
        assert_eq!(r["last_activity_at"], "2026-09-29T09:30:00Z");
        assert_eq!(
            r["worktrees"][0]["last_session"]["ended_at"],
            "2026-09-29T09:30:00.5Z"
        );
        assert_eq!(v["scanned_at"], "2026-09-29T10:00:00Z");
    }

    #[test]
    fn status_doc_nulls_serialise_as_null_not_omitted() {
        let mut w = worktree();
        w.last_session = None;
        w.stale_reason = None;
        w.upstream = None;
        let mut r = repo(None);
        r.worktrees = vec![w];
        let doc = StatusDoc {
            schema_version: SCHEMA_VERSION,
            root: "/r".into(),
            scanned_at: None,
            stale: true,
            partial: false,
            repos: vec![r],
        };
        let v = round_trip(&doc);
        assert_eq!(v["scanned_at"], Value::Null);
        let w = &v["repos"][0]["worktrees"][0];
        assert_keys(w, WORKTREE_KEYS, "Worktree with nulls");
        assert_eq!(w["last_session"], Value::Null);
        assert_eq!(w["stale_reason"], Value::Null);
    }

    #[test]
    fn where_was_i_and_list_stale_have_the_spec_keys() {
        let wwi = WhereWasIDoc {
            schema_version: SCHEMA_VERSION,
            root: "/r".into(),
            scanned_at: None,
            stale: false,
            partial: false,
            truncated: true,
            repos: vec![repo(None)],
            recent_sessions: vec![session_out()],
        };
        let v = round_trip(&wwi);
        assert_keys(
            &v,
            &[
                "schema_version",
                "root",
                "scanned_at",
                "stale",
                "partial",
                "truncated",
                "repos",
                "recent_sessions",
            ],
            "WhereWasIDoc",
        );
        assert_keys(&v["recent_sessions"][0], SESSION_KEYS, "recent Session");

        let ls = ListStaleDoc {
            schema_version: SCHEMA_VERSION,
            root: "/r".into(),
            scanned_at: Some(ts("2026-09-29T10:00:00Z")),
            stale: false,
            partial: false,
            truncated: false,
            items: vec![StaleItem {
                repo_name: "r".into(),
                repo_path: "/r".into(),
                worktree_path: "/r/w".into(),
                branch: None,
                stale_reason: StaleReason::Merged,
            }],
        };
        let v = round_trip(&ls);
        assert_keys(
            &v,
            &[
                "schema_version",
                "root",
                "scanned_at",
                "stale",
                "partial",
                "truncated",
                "items",
            ],
            "ListStaleDoc",
        );
        assert_keys(
            &v["items"][0],
            &[
                "repo_name",
                "repo_path",
                "worktree_path",
                "branch",
                "stale_reason",
            ],
            "ListStaleDoc item",
        );
        assert_eq!(v["items"][0]["stale_reason"], "merged");
    }

    #[test]
    fn doctor_doc_has_exactly_the_spec_keys_and_round_trips() {
        let doc = DoctorDoc {
            schema_version: SCHEMA_VERSION,
            root: "/r".into(),
            scanned_at: Some(ts("2026-09-29T10:00:00Z")),
            walk: WalkDiag {
                entries: 10,
                budget_hit: true,
                stopped_at: Some("/r/x".into()),
                unreadable_dirs: 1,
                skipped_dirs: 2,
                skipped_mounts: vec!["/mnt".into()],
            },
            repos_by_kind: ReposByKind {
                normal: 1,
                nested: 2,
                bare: 3,
                external_main: 4,
            },
            worktrees: WorktreeCounts {
                total: 5,
                external: 1,
                missing: 1,
                untrusted_path: 1,
            },
            submodules: 1,
            repo_errors: 1,
            untrusted_git_dirs: 1,
            repo_timeouts: 1,
            repo_timeouts_abandoned: 1,
            notes: vec!["n".into()],
            sources: vec![SourceDiagnostics {
                source: SourceKind::Codex,
                mode: SourceMode::Rollouts,
                db_version: Some(5),
                files: 2,
                records: 9,
                skipped: SkippedCounts {
                    malformed: 1,
                    unknown_type: 2,
                    oversize: 3,
                    non_uuid: 4,
                    bad_rollout_path: 5,
                },
                sessions: 2,
                excluded_children: 1,
                vanished: 1,
                matched: 1,
                unmatched: UnmatchedCounts {
                    outside_root: 1,
                    path_gone: 2,
                    non_repo: 3,
                },
            }],
        };
        let v = round_trip(&doc);

        assert_keys(
            &v,
            &[
                "schema_version",
                "root",
                "scanned_at",
                "walk",
                "repos_by_kind",
                "worktrees",
                "submodules",
                "repo_errors",
                "untrusted_git_dirs",
                "repo_timeouts",
                "repo_timeouts_abandoned",
                "notes",
                "sources",
            ],
            "DoctorDoc",
        );
        assert_keys(
            &v["walk"],
            &[
                "entries",
                "budget_hit",
                "stopped_at",
                "unreadable_dirs",
                "skipped_dirs",
                "skipped_mounts",
            ],
            "walk",
        );
        assert_keys(
            &v["repos_by_kind"],
            &["normal", "nested", "bare", "external_main"],
            "repos_by_kind",
        );
        assert_keys(
            &v["worktrees"],
            &["total", "external", "missing", "untrusted_path"],
            "worktrees",
        );
        let s = &v["sources"][0];
        assert_keys(
            s,
            &[
                "source",
                "mode",
                "db_version",
                "files",
                "records",
                "skipped",
                "sessions",
                "excluded_children",
                "vanished",
                "matched",
                "unmatched",
            ],
            "source",
        );
        assert_keys(
            &s["skipped"],
            &[
                "malformed",
                "unknown_type",
                "oversize",
                "non_uuid",
                "bad_rollout_path",
            ],
            "skipped",
        );
        assert_keys(
            &s["unmatched"],
            &["outside_root", "path_gone", "non_repo"],
            "unmatched",
        );
        assert_eq!(s["mode"], "rollouts");
        assert_eq!(s["source"], "codex");
    }

    #[test]
    fn enums_serialise_to_the_spec_strings() {
        let s = |v: &dyn erased::Ser| v.to_json();
        for (got, want) in [
            (s(&TitleKind::Custom), "custom"),
            (s(&TitleKind::Ai), "ai"),
            (s(&TitleKind::Thread), "thread"),
            (s(&TitleKind::Prompt), "prompt"),
            (s(&StaleReason::Missing), "missing"),
            (s(&StaleReason::Merged), "merged"),
            (s(&StaleReason::Inactive), "inactive"),
            (s(&RepoKind::Normal), "normal"),
            (s(&RepoKind::Nested), "nested"),
            (s(&RepoKind::Bare), "bare"),
            (s(&RepoKind::ExternalMain), "external_main"),
            (s(&SourceKind::Claude), "claude"),
            (s(&SourceKind::Codex), "codex"),
            (s(&SourceMode::Absent), "absent"),
        ] {
            assert_eq!(got, json!(want));
        }
    }

    mod erased {
        pub trait Ser {
            fn to_json(&self) -> serde_json::Value;
        }
        impl<T: serde::Serialize> Ser for T {
            fn to_json(&self) -> serde_json::Value {
                serde_json::to_value(self).unwrap()
            }
        }
    }

    #[test]
    fn uuid_accepts_either_case_and_keeps_it() {
        assert_eq!(Uuid::parse(ID).unwrap().as_str(), ID);
        let upper = ID.to_uppercase();
        assert_eq!(Uuid::parse(&upper).unwrap().as_str(), upper);
        assert!(Uuid::parse("0F8fad5b-D9CB-469f-a165-70867728950E").is_some());
    }

    #[test]
    fn uuid_rejects_malformed_ids() {
        let short = &ID[..35];
        let long = format!("{ID}0");
        let moved_hyphen = "0f8fad5bd-9cb-469f-a165-70867728950e";
        let non_hex = "0f8fad5b-d9cb-469f-a165-70867728950g";
        let no_hyphens = "0f8fad5bd9cb469fa16570867728950e0000";
        assert_eq!(
            (short.len(), long.len(), moved_hyphen.len(), non_hex.len()),
            (35, 37, 36, 36)
        );
        for bad in [
            short,
            long.as_str(),
            moved_hyphen,
            non_hex,
            no_hyphens,
            "",
            "é8fad5b-d9cb-469f-a165-70867728950e",
        ] {
            assert!(Uuid::parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn uuid_deserialisation_validates() {
        assert!(serde_json::from_value::<Uuid>(json!(ID)).is_ok());
        assert!(serde_json::from_value::<Uuid>(json!("nope")).is_err());
    }

    #[test]
    fn repo_state_pending_is_not_serialised() {
        let st = RepoState {
            default_branch: None,
            has_remote: false,
            error: None,
            worktrees: vec![],
            pending: Some(Arc::new(AtomicBool::new(false))),
        };
        let v = serde_json::to_value(&st).unwrap();
        assert_keys(
            &v,
            &["default_branch", "has_remote", "error", "worktrees"],
            "RepoState",
        );
        let back: RepoState = serde_json::from_value(v).unwrap();
        assert!(back.pending.is_none());
    }
}
