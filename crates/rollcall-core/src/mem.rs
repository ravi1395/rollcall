//! In-memory `Index` for tests and for callers that need no persistence.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::{
    DoctorDoc, Index, RepoRow, Result, ScanDiagnostics, ScanResult, Session, SourceKind,
    SourceState, Timestamp, Uuid,
};

/// Everything one root's last scan recorded (§6.1).
#[derive(Debug)]
struct RootScan {
    scanned_at: Timestamp,
    submodules: bool,
    repos: Vec<RepoRow>,
    matches: HashMap<PathBuf, Vec<Session>>,
    diagnostics: DoctorDoc,
}

#[derive(Debug, Default)]
pub struct MemIndex {
    sources: HashMap<SourceKind, SourceState>,
    roots: HashMap<PathBuf, RootScan>,
}

impl MemIndex {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Index for MemIndex {
    fn source_state(&self, s: SourceKind) -> SourceState {
        self.sources.get(&s).cloned().unwrap_or_default()
    }

    fn repos_for_root(&self, root: &Path) -> Vec<RepoRow> {
        self.roots
            .get(root)
            .map(|r| r.repos.clone())
            .unwrap_or_default()
    }

    fn sessions_for(&self, root: &Path, worktree: &Path) -> Vec<Session> {
        self.roots
            .get(root)
            .and_then(|r| r.matches.get(worktree))
            .cloned()
            .unwrap_or_default()
    }

    fn scanned_at(&self, root: &Path) -> Option<Timestamp> {
        self.roots.get(root).map(|r| r.scanned_at)
    }

    fn scanned_submodules(&self, root: &Path) -> Option<bool> {
        self.roots.get(root).map(|r| r.submodules)
    }

    fn last_diagnostics(&self, root: &Path) -> Option<ScanDiagnostics> {
        self.roots.get(root).map(|r| r.diagnostics.clone())
    }

    fn commit_scan(&mut self, root: &Path, scan: ScanResult) -> Result<()> {
        // Resolve every match before mutating, so a bad scan leaves the prior state intact.
        let known: HashMap<(SourceKind, &Uuid), &Session> = scan
            .sources
            .iter()
            .flat_map(|s| &s.sessions)
            .map(|s| ((s.source, &s.id), s))
            .collect();
        let mut matches: HashMap<PathBuf, Vec<Session>> = HashMap::new();
        for (kind, id, worktree) in &scan.matches {
            let Some(worktree) = worktree else { continue };
            let session = known
                .get(&(*kind, id))
                .ok_or_else(|| format!("match names unknown session {}", id.as_str()))?;
            matches
                .entry(worktree.clone())
                .or_default()
                .push((*session).clone());
        }
        for sessions in matches.values_mut() {
            sessions.sort_by(|a, b| {
                b.ended_at
                    .cmp(&a.ended_at)
                    .then_with(|| a.id.as_str().cmp(b.id.as_str()))
            });
        }

        for source in scan.sources {
            let state = SourceState {
                files: source.files,
                sessions: source.sessions,
            };
            self.sources.insert(source.diag.source, state);
        }
        self.roots.insert(
            root.to_path_buf(),
            RootScan {
                scanned_at: scan.scanned_at,
                submodules: scan.submodules,
                repos: scan.repos,
                matches,
                diagnostics: scan.diagnostics,
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Backing, FileState, RepoKind, RepoState, SCHEMA_VERSION, SourceDiagnostics, SourceMode,
        SourceScan, Uuid,
    };

    const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
    const OTHER_ID: &str = "1f8fad5b-d9cb-469f-a165-70867728950e";

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn session(id: &str, ended: &str) -> Session {
        Session {
            source: SourceKind::Claude,
            id: Uuid::parse(id).unwrap(),
            cwd: PathBuf::from("/w"),
            branch: None,
            started_at: ts("2026-09-29T09:00:00Z"),
            ended_at: ts(ended),
            title: None,
            last_prompt: None,
            message_count: 1,
            backing: Backing::LogFile {
                dev: 1,
                inode: 2,
                path: PathBuf::from("/log.jsonl"),
            },
        }
    }

    fn file_state() -> FileState {
        FileState {
            dev: 1,
            inode: 2,
            path: PathBuf::from("/log.jsonl"),
            size: 10,
            mtime: ts("2026-09-29T10:00:00Z"),
            offset: 10,
            head_fp: [1; 16],
            tail_fp: [2; 16],
            parser_version: 1,
        }
    }

    fn diag(root: &str, source: SourceKind) -> (DoctorDoc, SourceDiagnostics) {
        let sd = SourceDiagnostics {
            source,
            mode: SourceMode::Jsonl,
            db_version: None,
            files: 1,
            records: 3,
            skipped: Default::default(),
            sessions: 1,
            excluded_children: 0,
            vanished: 0,
            matched: 0,
            unmatched: Default::default(),
        };
        let doc = DoctorDoc {
            schema_version: SCHEMA_VERSION,
            root: root.to_owned(),
            scanned_at: Some(ts("2026-09-29T10:00:00Z")),
            walk: Default::default(),
            repos_by_kind: Default::default(),
            worktrees: Default::default(),
            submodules: 0,
            repo_errors: 0,
            untrusted_git_dirs: 0,
            repo_timeouts: 0,
            repo_timeouts_abandoned: 0,
            notes: vec![format!("note for {root}")],
            sources: vec![sd.clone()],
        };
        (doc, sd)
    }

    fn repo_row(path: &str) -> RepoRow {
        RepoRow {
            path: PathBuf::from(path),
            name: "r".into(),
            kind: RepoKind::Normal,
            contains_root: false,
            state: RepoState {
                default_branch: Some("main".into()),
                has_remote: false,
                error: None,
                worktrees: vec![],
                pending: None,
            },
            submodules: vec![],
        }
    }

    /// A scan of `root` whose sessions are `sessions`, each matched to the given worktree.
    fn scan(
        root: &str,
        submodules: bool,
        sessions: Vec<Session>,
        matches: Vec<(&str, Option<&str>)>,
    ) -> ScanResult {
        let (diagnostics, diag) = diag(root, SourceKind::Claude);
        ScanResult {
            scanned_at: ts("2026-09-29T10:00:00Z"),
            submodules,
            partial: false,
            repos: vec![repo_row(root)],
            matches: matches
                .into_iter()
                .map(|(id, wt)| {
                    (
                        SourceKind::Claude,
                        Uuid::parse(id).unwrap(),
                        wt.map(PathBuf::from),
                    )
                })
                .collect(),
            sources: vec![SourceScan {
                files: vec![file_state()],
                sessions,
                diag,
                notes: vec![],
            }],
            diagnostics,
        }
    }

    #[test]
    fn matches_are_per_root_and_per_worktree() {
        let mut ix = MemIndex::new();
        let s = session(ID, "2026-09-29T09:30:00Z");
        ix.commit_scan(
            Path::new("/A"),
            scan("/A", false, vec![s.clone()], vec![(ID, Some("/A/wA"))]),
        )
        .unwrap();
        ix.commit_scan(
            Path::new("/B"),
            scan("/B", true, vec![s.clone()], vec![(ID, Some("/B/wB"))]),
        )
        .unwrap();

        assert_eq!(
            ix.sessions_for(Path::new("/A"), Path::new("/A/wA")),
            vec![s.clone()]
        );
        assert_eq!(
            ix.sessions_for(Path::new("/B"), Path::new("/B/wB")),
            vec![s]
        );
        assert!(
            ix.sessions_for(Path::new("/A"), Path::new("/B/wB"))
                .is_empty()
        );
        assert!(
            ix.sessions_for(Path::new("/A"), Path::new("/nowhere"))
                .is_empty()
        );
        assert!(
            ix.sessions_for(Path::new("/unscanned"), Path::new("/A/wA"))
                .is_empty()
        );
    }

    #[test]
    fn unmatched_sessions_attach_nowhere() {
        let mut ix = MemIndex::new();
        let s = session(ID, "2026-09-29T09:30:00Z");
        ix.commit_scan(
            Path::new("/A"),
            scan("/A", false, vec![s], vec![(ID, None)]),
        )
        .unwrap();
        assert!(ix.sessions_for(Path::new("/A"), Path::new("/w")).is_empty());
    }

    #[test]
    fn sessions_for_orders_ended_at_desc() {
        let mut ix = MemIndex::new();
        let old = session(ID, "2026-09-29T09:10:00Z");
        let new = session(OTHER_ID, "2026-09-29T09:50:00Z");
        ix.commit_scan(
            Path::new("/A"),
            scan(
                "/A",
                false,
                vec![old.clone(), new.clone()],
                vec![(ID, Some("/A/w")), (OTHER_ID, Some("/A/w"))],
            ),
        )
        .unwrap();
        assert_eq!(
            ix.sessions_for(Path::new("/A"), Path::new("/A/w")),
            vec![new, old]
        );
    }

    #[test]
    fn match_naming_an_unknown_session_is_an_error() {
        let mut ix = MemIndex::new();
        let r = ix.commit_scan(
            Path::new("/A"),
            scan("/A", false, vec![], vec![(ID, Some("/A/w"))]),
        );
        assert!(r.is_err());
        assert_eq!(
            ix.scanned_at(Path::new("/A")),
            None,
            "failed commit leaves prior state"
        );
    }

    #[test]
    fn metadata_round_trips() {
        let mut ix = MemIndex::new();
        assert_eq!(ix.scanned_at(Path::new("/A")), None);
        assert_eq!(ix.scanned_submodules(Path::new("/A")), None);
        assert_eq!(ix.last_diagnostics(Path::new("/A")), None);
        assert_eq!(ix.source_state(SourceKind::Claude), SourceState::default());

        let s = session(ID, "2026-09-29T09:30:00Z");
        let sc = scan("/A", true, vec![s.clone()], vec![]);
        let expected_diag = sc.diagnostics.clone();
        let expected_repos = sc.repos.clone();
        ix.commit_scan(Path::new("/A"), sc).unwrap();

        assert_eq!(
            ix.scanned_at(Path::new("/A")),
            Some(ts("2026-09-29T10:00:00Z"))
        );
        assert_eq!(ix.last_diagnostics(Path::new("/A")), Some(expected_diag));
        assert_eq!(ix.repos_for_root(Path::new("/A")), expected_repos);
        assert_eq!(
            ix.source_state(SourceKind::Claude),
            SourceState {
                files: vec![file_state()],
                sessions: vec![s]
            }
        );
        assert_eq!(ix.source_state(SourceKind::Codex), SourceState::default());
    }

    #[test]
    fn submodules_flag_is_stored_per_root() {
        let mut ix = MemIndex::new();
        ix.commit_scan(Path::new("/A"), scan("/A", true, vec![], vec![]))
            .unwrap();
        ix.commit_scan(Path::new("/B"), scan("/B", false, vec![], vec![]))
            .unwrap();
        assert_eq!(ix.scanned_submodules(Path::new("/A")), Some(true));
        assert_eq!(ix.scanned_submodules(Path::new("/B")), Some(false));

        ix.commit_scan(Path::new("/A"), scan("/A", false, vec![], vec![]))
            .unwrap();
        assert_eq!(
            ix.scanned_submodules(Path::new("/A")),
            Some(false),
            "rescan replaces the flag"
        );
    }

    #[test]
    fn rescan_replaces_the_roots_matches() {
        let mut ix = MemIndex::new();
        let s = session(ID, "2026-09-29T09:30:00Z");
        ix.commit_scan(
            Path::new("/A"),
            scan("/A", false, vec![s.clone()], vec![(ID, Some("/A/w1"))]),
        )
        .unwrap();
        ix.commit_scan(
            Path::new("/A"),
            scan("/A", false, vec![s.clone()], vec![(ID, Some("/A/w2"))]),
        )
        .unwrap();
        assert!(
            ix.sessions_for(Path::new("/A"), Path::new("/A/w1"))
                .is_empty()
        );
        assert_eq!(
            ix.sessions_for(Path::new("/A"), Path::new("/A/w2")),
            vec![s]
        );
    }
}
