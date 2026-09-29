//! Stale, attention, activity, sort and quiet rules (§3.3). Pure; no I/O.

use jiff::SignedDuration;

use crate::{Repo, StaleReason, Timestamp, WorktreeState};

const SECS_PER_DAY: i64 = 24 * 60 * 60;
const ATTENTION_ERROR: u8 = 4;

/// §3.3 stale_reason. Linked worktrees only: a main worktree is never stale.
///
/// `inactive` needs `last_commit_at` strictly older than `stale_days` whole days: a commit
/// exactly `stale_days` old is not yet stale. A worktree without a commit time (or with one in
/// the future) is never inactive, because its age is unknown.
pub fn stale_reason(wt: &WorktreeState, now: Timestamp, stale_days: u32) -> Option<StaleReason> {
    if wt.is_main {
        return None;
    }
    if wt.missing {
        return Some(StaleReason::Missing);
    }
    if wt.merged {
        return Some(StaleReason::Merged);
    }
    let quiet_tree =
        wt.dirty_count == 0 && wt.untracked_count == 0 && wt.unpushed.unwrap_or(0) == 0;
    let threshold = SignedDuration::from_secs(i64::from(stale_days) * SECS_PER_DAY);
    let old = wt
        .last_commit_at
        .is_some_and(|t| now.duration_since(t) > threshold);
    (quiet_tree && old).then_some(StaleReason::Inactive)
}

/// §3.3 worktree attention: 4 error, 3 unpushed, 2 dirty or untracked, 1 stale, else 0.
pub fn worktree_attention(wt: &WorktreeState, stale: Option<StaleReason>) -> u8 {
    if wt.error.is_some() {
        ATTENTION_ERROR
    } else if wt.unpushed.unwrap_or(0) > 0 {
        3
    } else if wt.dirty_count > 0 || wt.untracked_count > 0 {
        2
    } else {
        u8::from(stale.is_some())
    }
}

/// §3.3 repo attention: the repo's own `error` gives 4, else the max over its worktrees.
pub fn repo_attention(repo_error: bool, worktree_attentions: impl IntoIterator<Item = u8>) -> u8 {
    if repo_error {
        return ATTENTION_ERROR;
    }
    worktree_attentions.into_iter().max().unwrap_or(0)
}

/// §3.3 last_activity_at: the latest worktree commit time or matched session `ended_at`.
pub fn last_activity_at(
    commit_times: impl IntoIterator<Item = Option<Timestamp>>,
    session_ended_ats: impl IntoIterator<Item = Timestamp>,
) -> Option<Timestamp> {
    commit_times
        .into_iter()
        .flatten()
        .chain(session_ended_ats)
        .max()
}

/// §3.3 repo sort: attention desc, then `last_activity_at` desc with `None` last, then path asc.
pub fn sort_repos(repos: &mut [Repo]) {
    // `Option`'s order puts `None` first, so reversing the activity comparison puts it last.
    repos.sort_by(|a, b| {
        b.attention
            .cmp(&a.attention)
            .then_with(|| b.last_activity_at.cmp(&a.last_activity_at))
            .then_with(|| a.path.cmp(&b.path))
    });
}

/// §3.3 quiet label: `quiet Nd` with N whole days (floor) since `last_activity`, or `quiet —`.
/// A future `last_activity` (clock skew) reads as `quiet 0d`.
pub fn quiet(last_activity: Option<Timestamp>, now: Timestamp) -> String {
    let Some(t) = last_activity else {
        return "quiet —".to_owned();
    };
    let days = (now.duration_since(t).as_secs() / SECS_PER_DAY).max(0);
    format!("quiet {days}d")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::RepoKind;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn now() -> Timestamp {
        ts("2026-09-29T00:00:00Z")
    }

    const STALE_DAYS: u32 = 14;
    const OLD: &str = "2026-08-01T00:00:00Z";
    const RECENT: &str = "2026-09-28T00:00:00Z";

    /// A clean, linked, old-commit worktree: inactive unless a test disturbs it.
    fn wt() -> WorktreeState {
        WorktreeState {
            path: PathBuf::from("/r/w"),
            is_main: false,
            branch: Some("feat".into()),
            detached: false,
            head: Some("abc".into()),
            last_commit_at: Some(ts(OLD)),
            upstream: Some("origin/feat".into()),
            ahead: Some(0),
            behind: Some(0),
            unpushed: Some(0),
            dirty_count: 0,
            untracked_count: 0,
            missing: false,
            merged: false,
            error: None,
        }
    }

    fn stale(w: &WorktreeState) -> Option<StaleReason> {
        stale_reason(w, now(), STALE_DAYS)
    }

    // ---- stale_reason ----

    #[test]
    fn missing_is_stale_missing_even_when_merged() {
        let mut w = wt();
        w.missing = true;
        w.merged = true;
        assert_eq!(stale(&w), Some(StaleReason::Missing));
    }

    #[test]
    fn merged_is_stale_merged_even_when_dirty_and_recent() {
        let mut w = wt();
        w.merged = true;
        w.dirty_count = 3;
        w.last_commit_at = Some(ts(RECENT));
        assert_eq!(stale(&w), Some(StaleReason::Merged));
    }

    #[test]
    fn old_clean_pushed_worktree_is_inactive() {
        assert_eq!(stale(&wt()), Some(StaleReason::Inactive));
    }

    #[test]
    fn unpushed_null_still_allows_inactive() {
        let mut w = wt();
        w.unpushed = None;
        assert_eq!(stale(&w), Some(StaleReason::Inactive));
    }

    #[test]
    fn dirty_blocks_inactive() {
        let mut w = wt();
        w.dirty_count = 1;
        assert_eq!(stale(&w), None);
    }

    #[test]
    fn untracked_blocks_inactive() {
        let mut w = wt();
        w.untracked_count = 1;
        assert_eq!(stale(&w), None);
    }

    #[test]
    fn unpushed_commits_block_inactive() {
        let mut w = wt();
        w.unpushed = Some(1);
        assert_eq!(stale(&w), None);
    }

    #[test]
    fn recent_commit_blocks_inactive() {
        let mut w = wt();
        w.last_commit_at = Some(ts(RECENT));
        assert_eq!(stale(&w), None);
    }

    #[test]
    fn inactive_boundary_is_strictly_older_than_stale_days() {
        let mut w = wt();
        w.last_commit_at = Some(ts("2026-09-15T00:00:00Z")); // exactly 14 days
        assert_eq!(stale(&w), None);
        w.last_commit_at = Some(ts("2026-09-14T23:59:59Z")); // 14 days and 1 s
        assert_eq!(stale(&w), Some(StaleReason::Inactive));
    }

    #[test]
    fn unknown_or_future_commit_time_is_not_inactive() {
        let mut w = wt();
        w.last_commit_at = None;
        assert_eq!(stale(&w), None);
        w.last_commit_at = Some(ts("2026-10-30T00:00:00Z"));
        assert_eq!(stale(&w), None);
    }

    #[test]
    fn main_worktree_is_never_stale() {
        let mut w = wt();
        w.is_main = true;
        assert_eq!(stale(&w), None, "old, clean main");
        w.merged = true;
        w.missing = true;
        assert_eq!(stale(&w), None, "merged and missing main");
    }

    // ---- worktree_attention ----

    #[test]
    fn worktree_attention_precedence_one_case_each() {
        let mut w = wt();
        assert_eq!(worktree_attention(&w, None), 0);
        assert_eq!(worktree_attention(&w, Some(StaleReason::Merged)), 1);
        w.untracked_count = 1;
        assert_eq!(
            worktree_attention(&w, Some(StaleReason::Merged)),
            2,
            "untracked"
        );
        w.untracked_count = 0;
        w.dirty_count = 1;
        assert_eq!(worktree_attention(&w, None), 2, "dirty");
        w.unpushed = Some(1);
        assert_eq!(worktree_attention(&w, None), 3, "unpushed beats dirty");
        w.error = Some("boom".into());
        assert_eq!(worktree_attention(&w, None), 4, "error beats unpushed");
    }

    #[test]
    fn worktree_attention_null_unpushed_is_not_attention() {
        let mut w = wt();
        w.unpushed = None;
        assert_eq!(worktree_attention(&w, None), 0);
    }

    // ---- repo_attention ----

    #[test]
    fn repo_attention_is_the_max_of_its_worktrees() {
        assert_eq!(repo_attention(false, [1, 3, 2]), 3);
        assert_eq!(repo_attention(false, []), 0);
    }

    #[test]
    fn repo_error_is_attention_four() {
        assert_eq!(repo_attention(true, []), 4);
        assert_eq!(repo_attention(true, [0, 1]), 4);
    }

    // ---- last_activity_at ----

    #[test]
    fn last_activity_is_the_max_over_commits_and_sessions() {
        let commits = [
            Some(ts("2026-09-01T00:00:00Z")),
            None,
            Some(ts("2026-09-10T00:00:00Z")),
        ];
        let ended = [ts("2026-09-05T00:00:00Z")];
        assert_eq!(
            last_activity_at(commits, ended),
            Some(ts("2026-09-10T00:00:00Z")),
            "commit wins"
        );
        let ended_later = [ts("2026-09-20T00:00:00Z")];
        assert_eq!(
            last_activity_at(commits, ended_later),
            Some(ts("2026-09-20T00:00:00Z")),
            "session wins"
        );
        assert_eq!(
            last_activity_at([None], ended),
            Some(ts("2026-09-05T00:00:00Z")),
            "session only"
        );
    }

    #[test]
    fn last_activity_is_none_without_any_time() {
        assert_eq!(last_activity_at([None, None], []), None);
        assert_eq!(last_activity_at([], []), None);
    }

    // ---- sort_repos ----

    fn repo(path: &str, attention: u8, activity: Option<&str>) -> Repo {
        Repo {
            name: path.rsplit('/').next().unwrap().into(),
            path: path.into(),
            kind: RepoKind::Normal,
            contains_root: false,
            default_branch: None,
            has_remote: false,
            attention,
            last_activity_at: activity.map(ts),
            error: None,
            worktrees: vec![],
            submodules: None,
        }
    }

    fn order(repos: &[Repo]) -> Vec<&str> {
        repos.iter().map(|r| r.path.as_str()).collect()
    }

    #[test]
    fn sort_by_attention_desc_first() {
        let mut v = vec![
            repo("/a", 0, Some("2026-09-28T00:00:00Z")),
            repo("/b", 3, Some("2026-01-01T00:00:00Z")),
            repo("/c", 1, None),
        ];
        sort_repos(&mut v);
        assert_eq!(order(&v), ["/b", "/c", "/a"]);
    }

    #[test]
    fn sort_ties_on_activity_desc_with_null_last() {
        let mut v = vec![
            repo("/a", 2, None),
            repo("/b", 2, Some("2026-09-01T00:00:00Z")),
            repo("/c", 2, Some("2026-09-10T00:00:00Z")),
        ];
        sort_repos(&mut v);
        assert_eq!(order(&v), ["/c", "/b", "/a"]);
    }

    #[test]
    fn sort_final_tie_break_is_path_ascending() {
        let same = Some("2026-09-01T00:00:00Z");
        let mut v = vec![
            repo("/z", 2, same),
            repo("/a", 2, same),
            repo("/m", 2, None),
            repo("/b", 2, None),
        ];
        sort_repos(&mut v);
        assert_eq!(order(&v), ["/a", "/z", "/b", "/m"]);
    }

    // ---- quiet ----

    #[test]
    fn quiet_floors_whole_days() {
        let last = |s| Some(ts(s));
        assert_eq!(
            quiet(last("2026-09-28T00:00:01Z"), now()),
            "quiet 0d",
            "23h59m59s"
        );
        assert_eq!(
            quiet(last("2026-09-28T00:01:00Z"), now()),
            "quiet 0d",
            "23h59m"
        );
        assert_eq!(
            quiet(last("2026-09-28T00:00:00Z"), now()),
            "quiet 1d",
            "24h"
        );
        assert_eq!(
            quiet(last("2026-09-26T12:00:00Z"), now()),
            "quiet 2d",
            "2.5 days"
        );
        assert_eq!(
            quiet(last("2026-09-29T00:00:00Z"), now()),
            "quiet 0d",
            "just now"
        );
    }

    #[test]
    fn quiet_null_is_em_dash() {
        assert_eq!(quiet(None, now()), "quiet —");
    }

    #[test]
    fn quiet_future_activity_reads_zero_days() {
        assert_eq!(quiet(Some(ts("2026-09-30T00:00:00Z")), now()), "quiet 0d");
    }
}
