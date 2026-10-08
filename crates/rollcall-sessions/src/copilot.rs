//! GitHub Copilot CLI source (spec §5.3). Inert until P3 registers it: every scan reports the
//! source absent and no reference resolves.

use rollcall_core::{
    SkippedCounts, SourceDiagnostics, SourceKind, SourceMode, SourceScan, SourceState, TextRef,
    UnmatchedCounts,
};

use crate::SessionSource;

pub struct Copilot;

impl SessionSource for Copilot {
    fn kind(&self) -> SourceKind {
        SourceKind::Copilot
    }

    fn scan(&self, _prior: &SourceState) -> SourceScan {
        SourceScan {
            files: Vec::new(),
            sessions: Vec::new(),
            diag: SourceDiagnostics {
                source: SourceKind::Copilot,
                mode: SourceMode::Absent,
                db_version: None,
                files: 0,
                records: 0,
                skipped: SkippedCounts::default(),
                sessions: 0,
                excluded_children: 0,
                vanished: 0,
                matched: 0,
                unmatched: UnmatchedCounts::default(),
            },
            notes: Vec::new(),
        }
    }

    fn resolve(&self, _r: &TextRef) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use rollcall_core::{
        Backing, FileState, Session, SkippedCounts, SourceKind, SourceMode, SourceState, TextField,
        TextRef, Timestamp, UnmatchedCounts, Uuid,
    };

    use super::*;

    const ID: &str = "0a1b2c3d-0000-4000-8000-0123456789ab";

    fn busy_prior() -> SourceState {
        let ts: Timestamp = "2026-09-29T10:00:00Z".parse().unwrap();
        SourceState {
            files: vec![FileState {
                dev: 1,
                inode: 2,
                path: "/copilot/session-state/a/events.jsonl".into(),
                size: 10,
                mtime: ts,
                offset: 10,
                head_fp: [1; 16],
                tail_fp: [2; 16],
                parser_version: 1,
            }],
            sessions: vec![Session {
                source: SourceKind::Copilot,
                id: Uuid::parse(ID).unwrap(),
                cwd: "/A".into(),
                branch: None,
                started_at: ts,
                ended_at: ts,
                title: None,
                last_prompt: None,
                message_count: 1,
                backing: Backing::LogFile {
                    dev: 1,
                    inode: 2,
                    path: "/copilot/session-state/a/events.jsonl".into(),
                },
            }],
        }
    }

    #[test]
    fn the_inert_source_is_absent_and_resolves_nothing() {
        let src = Copilot;
        assert_eq!(src.kind(), SourceKind::Copilot);
        for prior in [SourceState::default(), busy_prior()] {
            let scan = src.scan(&prior);
            assert!(scan.files.is_empty(), "{:?}", scan.files);
            assert!(scan.sessions.is_empty(), "{:?}", scan.sessions);
            assert!(scan.notes.is_empty(), "{:?}", scan.notes);
            let d = &scan.diag;
            assert_eq!(
                (d.source, d.mode, d.db_version),
                (SourceKind::Copilot, SourceMode::Absent, None)
            );
            assert_eq!(
                (d.files, d.records, d.sessions, d.excluded_children),
                (0, 0, 0, 0)
            );
            assert_eq!((d.vanished, d.matched), (0, 0));
            assert_eq!(d.skipped, SkippedCounts::default());
            assert_eq!(d.unmatched, UnmatchedCounts::default());
        }
        let log = TextRef::LogLine {
            dev: 1,
            inode: 2,
            size_at_scan: 10,
            offset: 0,
            len: 5,
            field: TextField::Title,
        };
        let thread = TextRef::CodexThread {
            thread_id: Uuid::parse(ID).unwrap(),
            field: TextField::LastPrompt,
        };
        assert_eq!(src.resolve(&log), None);
        assert_eq!(src.resolve(&thread), None);
    }
}
