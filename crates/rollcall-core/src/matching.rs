use crate::{MatchResult, SessionLocation, WorktreeKey};

/// One result per session, in order (§6.5). `outside_root` is checked first: worktrees are
/// under the root, so a cwd outside it can never match one, even a missing worktree at that path.
pub fn match_sessions(worktrees: &[WorktreeKey], sessions: &[SessionLocation]) -> Vec<MatchResult> {
    sessions
        .iter()
        .map(|session| match_one(worktrees, session))
        .collect()
}

fn match_one(worktrees: &[WorktreeKey], session: &SessionLocation) -> MatchResult {
    if !session.under_root {
        return MatchResult::OutsideRoot;
    }
    let cwd = session.cwd_canonical.as_path();
    if !session.cwd_exists {
        // Only a missing worktree at exactly this path claims a vanished cwd.
        return worktrees
            .iter()
            .position(|w| !w.hidden && w.missing && w.canonical_path == cwd)
            .map_or(MatchResult::PathGone, MatchResult::Matched);
    }
    // Strict `>` keeps the first of equal-length candidates (duplicate paths).
    let mut best: Option<(usize, usize)> = None;
    for (index, worktree) in worktrees.iter().enumerate() {
        let is_candidate =
            !worktree.hidden && (!worktree.missing || worktree.canonical_path == cwd);
        if !is_candidate || !cwd.starts_with(&worktree.canonical_path) {
            continue;
        }
        let depth = worktree.canonical_path.components().count();
        if best.is_none_or(|(_, best_depth)| depth > best_depth) {
            best = Some((index, depth));
        }
    }
    best.map_or(MatchResult::NonRepo, |(index, _)| {
        MatchResult::Matched(index)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn wt(path: &str) -> WorktreeKey {
        WorktreeKey {
            canonical_path: PathBuf::from(path),
            hidden: false,
            missing: false,
        }
    }

    fn hidden(path: &str) -> WorktreeKey {
        WorktreeKey {
            hidden: true,
            ..wt(path)
        }
    }

    fn missing(path: &str) -> WorktreeKey {
        WorktreeKey {
            missing: true,
            ..wt(path)
        }
    }

    fn session(cwd: &str) -> SessionLocation {
        SessionLocation {
            cwd_canonical: PathBuf::from(cwd),
            cwd_exists: true,
            under_root: true,
        }
    }

    fn one(worktrees: &[WorktreeKey], s: SessionLocation) -> MatchResult {
        match_sessions(worktrees, &[s])[0]
    }

    #[test]
    fn prefix_is_component_wise_not_string_prefix() {
        assert_eq!(one(&[wt("/a/b")], session("/a/bc")), MatchResult::NonRepo);
        assert_eq!(
            one(&[wt("/a/b")], session("/a/b/c")),
            MatchResult::Matched(0)
        );
    }

    #[test]
    fn longest_prefix_wins_regardless_of_order() {
        let outer_first = [wt("/a"), wt("/a/b")];
        assert_eq!(
            one(&outer_first, session("/a/b/c")),
            MatchResult::Matched(1)
        );
        let inner_first = [wt("/a/b"), wt("/a")];
        assert_eq!(
            one(&inner_first, session("/a/b/c")),
            MatchResult::Matched(0)
        );
        assert_eq!(one(&outer_first, session("/a/x")), MatchResult::Matched(0));
    }

    #[test]
    fn cwd_equal_to_worktree_matches() {
        assert_eq!(one(&[wt("/a/b")], session("/a/b")), MatchResult::Matched(0));
    }

    #[test]
    fn hidden_submodule_is_skipped_and_session_falls_to_parent() {
        let worktrees = [wt("/a"), hidden("/a/sub")];
        assert_eq!(
            one(&worktrees, session("/a/sub/x")),
            MatchResult::Matched(0)
        );
        assert_eq!(one(&worktrees, session("/a/sub")), MatchResult::Matched(0));
        assert_eq!(
            one(&[hidden("/a/sub")], session("/a/sub")),
            MatchResult::NonRepo
        );
    }

    #[test]
    fn missing_worktree_matches_only_an_equal_cwd() {
        let worktrees = [wt("/a"), missing("/a/gone")];
        assert_eq!(
            one(&worktrees, session("/a/gone/x")),
            MatchResult::Matched(0)
        );
        assert_eq!(one(&worktrees, session("/a/gone")), MatchResult::Matched(1));
        assert_eq!(
            one(&[missing("/a/gone")], session("/a/gone/x")),
            MatchResult::NonRepo
        );
    }

    #[test]
    fn nonexistent_cwd_matches_equal_missing_worktree() {
        let mut s = session("/a/gone");
        s.cwd_exists = false;
        assert_eq!(
            one(&[wt("/a"), missing("/a/gone")], s),
            MatchResult::Matched(1)
        );
    }

    #[test]
    fn nonexistent_cwd_is_path_gone_otherwise() {
        let mut s = session("/a/b/deleted");
        s.cwd_exists = false;
        // A live parent worktree does not claim a vanished cwd.
        assert_eq!(one(&[wt("/a/b")], s.clone()), MatchResult::PathGone);
        assert_eq!(one(&[], s.clone()), MatchResult::PathGone);
        // A non-missing worktree with an equal path does not rescue it either.
        s.cwd_canonical = PathBuf::from("/a/b");
        assert_eq!(one(&[wt("/a/b")], s), MatchResult::PathGone);
    }

    #[test]
    fn outside_root_wins_over_everything() {
        let mut s = session("/elsewhere/x");
        s.under_root = false;
        assert_eq!(
            one(&[wt("/elsewhere")], s.clone()),
            MatchResult::OutsideRoot
        );
        s.cwd_exists = false;
        assert_eq!(one(&[missing("/elsewhere/x")], s), MatchResult::OutsideRoot);
    }

    #[test]
    fn no_worktrees_is_non_repo() {
        assert_eq!(one(&[], session("/a/b")), MatchResult::NonRepo);
    }

    #[test]
    fn results_are_one_per_session_in_order() {
        let worktrees = [wt("/a"), wt("/b")];
        let mut outside = session("/z");
        outside.under_root = false;
        let sessions = [session("/b/x"), outside, session("/a"), session("/c")];
        assert_eq!(
            match_sessions(&worktrees, &sessions),
            vec![
                MatchResult::Matched(1),
                MatchResult::OutsideRoot,
                MatchResult::Matched(0),
                MatchResult::NonRepo,
            ]
        );
        assert!(match_sessions(&worktrees, &[]).is_empty());
    }
}
