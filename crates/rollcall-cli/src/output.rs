use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use ratatui::text::Span;
use rollcall_core::text::{render_text, sanitise, shell_quote};
use rollcall_core::{
    Backing, DoctorDoc, Index, Repo, RepoKind, RepoRow, SCHEMA_VERSION, Session, SourceKind,
    StaleReason, StatusDoc, TextRef, Timestamp, TitleKind, Uuid, Worktree, WorktreeState, rank,
};
use rollcall_sessions::codex_db;
use rollcall_sessions::{Claude, Codex, SessionSource};

use crate::config::Ctx;
use crate::scan::{self, ScanOutcome};

const SECS_PER_MINUTE: i64 = 60;
const SECS_PER_HOUR: i64 = 60 * SECS_PER_MINUTE;
const SECS_PER_DAY: i64 = 24 * SECS_PER_HOUR;

/// What log-derived text may be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPolicy {
    /// Show prompt-derived text: `last_prompt`, and titles of kind `prompt` (MCP `--prompts`;
    /// always true for the CLI).
    pub prompts: bool,
    /// `redact_prompts`: hide all title and prompt text; overrides `prompts`.
    pub redact: bool,
}

/// What `R` and the `rc` function resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeTarget {
    pub source: SourceKind,
    pub id: Uuid,
    pub cwd: PathBuf,
}

/// The §8.1 `Y` line: `cd -- '<cwd>' && claude --resume <id>` (`codex resume <id>` for Codex).
pub fn display_line(target: &ResumeTarget) -> String {
    let agent = match target.source {
        SourceKind::Claude => "claude --resume",
        SourceKind::Codex => "codex resume",
    };
    format!(
        "cd -- {} && {agent} {}",
        shell_quote(&target.cwd),
        target.id.as_str()
    )
}

pub fn render_session(
    session: &Session,
    source: &dyn SessionSource,
    policy: TextPolicy,
) -> rollcall_core::SessionOut {
    // ponytail: the pinned signature carries no config, so email scrubbing is off here;
    // `build_status` (the CLI path) honours `redact_emails` via `render_with`.
    render_with(session, source, policy, false)
}

pub(crate) fn render_with(
    session: &Session,
    source: &dyn SessionSource,
    policy: TextPolicy,
    redact_emails: bool,
) -> rollcall_core::SessionOut {
    let scrub = |t: String| render_text(&t, redact_emails);
    let show = |r: &TextRef| source.resolve(r).map(scrub);
    let title = session
        .title
        .as_ref()
        .filter(|(_, kind)| !policy.redact && (policy.prompts || *kind != TitleKind::Prompt))
        .and_then(|(r, kind)| match (session.source, &session.backing, r) {
            // §6.2: a thread title whose kind changed since the scan (the DB row lost its title,
            // so the text is now the prompt) is a mismatch → null.
            (
                SourceKind::Codex,
                Backing::CodexThread { db_path, thread_id },
                TextRef::CodexThread { .. },
            ) => codex_db::resolve_thread_title(db_path, thread_id)
                .filter(|(_, now)| now == kind)
                .map(|(text, _)| (scrub(text), *kind)),
            _ => show(r).map(|text| (text, *kind)),
        });
    let last_prompt = session
        .last_prompt
        .as_ref()
        .filter(|_| policy.prompts && !policy.redact)
        .and_then(show);
    let (title, title_kind) = match title {
        Some((text, kind)) => (Some(text), Some(kind)),
        None => (None, None),
    };
    let target = ResumeTarget {
        source: session.source,
        id: session.id.clone(),
        cwd: session.cwd.clone(),
    };
    rollcall_core::SessionOut {
        source: session.source,
        id: session.id.as_str().to_owned(),
        branch: session.branch.clone(),
        cwd: session.cwd.to_string_lossy().into_owned(),
        started_at: session.started_at,
        ended_at: session.ended_at,
        title,
        title_kind,
        last_prompt,
        message_count: session.message_count,
        resume: session.cwd.exists().then(|| display_line(&target)),
    }
}

// ---- StatusDoc ----

struct Builder<'a> {
    ctx: &'a Ctx,
    index: &'a dyn Index,
    root: &'a Path,
    policy: TextPolicy,
    submodules: bool,
    sources: [Box<dyn SessionSource>; 2],
}

impl Builder<'_> {
    fn repo(&self, row: &RepoRow) -> Repo {
        let mut worktrees = Vec::new();
        let mut ended = Vec::new();
        for state in &row.state.worktrees {
            let sessions = self.index.sessions_for(self.root, &state.path);
            ended.extend(sessions.iter().map(|s| s.ended_at));
            worktrees.push(self.worktree(state, &sessions));
        }
        // Main first, then linked by path (§7).
        worktrees.sort_by(|a, b| (!a.is_main, &a.path).cmp(&(!b.is_main, &b.path)));
        let last_activity_at =
            rank::last_activity_at(row.state.worktrees.iter().map(|w| w.last_commit_at), ended);
        let attention = rank::repo_attention(
            row.state.error.is_some(),
            worktrees.iter().map(|w| w.attention),
        );
        Repo {
            name: row.name.clone(),
            path: row.path.to_string_lossy().into_owned(),
            kind: row.kind,
            contains_root: row.contains_root,
            default_branch: row.state.default_branch.clone(),
            has_remote: row.state.has_remote,
            attention,
            last_activity_at,
            error: row.state.error.clone(),
            worktrees,
            submodules: self
                .submodules
                .then(|| row.submodules.iter().map(|s| self.repo(s)).collect()),
        }
    }

    fn worktree(&self, state: &WorktreeState, sessions: &[Session]) -> Worktree {
        let stale = rank::stale_reason(state, self.ctx.now, self.ctx.config.stale_days);
        Worktree {
            path: state.path.to_string_lossy().into_owned(),
            is_main: state.is_main,
            external: !canonical_lenient(&state.path).starts_with(self.root),
            branch: state.branch.clone(),
            detached: state.detached,
            head: state.head.clone(),
            last_commit_at: state.last_commit_at,
            upstream: state.upstream.clone(),
            ahead: state.ahead,
            behind: state.behind,
            unpushed: state.unpushed,
            dirty_count: state.dirty_count,
            untracked_count: state.untracked_count,
            stale_reason: stale,
            attention: rank::worktree_attention(state, stale),
            error: state.error.clone(),
            last_session: sessions.first().map(|s| self.session(s)),
            older_sessions: u32::try_from(sessions.len().saturating_sub(1)).unwrap_or(u32::MAX),
        }
    }

    fn session(&self, session: &Session) -> rollcall_core::SessionOut {
        let source = self
            .sources
            .iter()
            .find(|s| s.kind() == session.source)
            .expect("one source per SourceKind");
        render_with(
            session,
            source.as_ref(),
            self.policy,
            self.ctx.config.redact_emails,
        )
    }
}

/// Same as `scan.rs`'s private helper: lexically normalised, with the deepest existing ancestor
/// replaced by its realpath, so "external" here agrees with `doctor`'s count.
// ponytail: duplicated because scan.rs keeps it private; share it if scan.rs is next touched.
fn canonical_lenient(path: &Path) -> PathBuf {
    let mut lexical = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                lexical.pop();
            }
            other => lexical.push(other),
        }
    }
    let mut existing = lexical.as_path();
    let mut tail: Vec<&OsStr> = Vec::new();
    loop {
        if let Ok(real) = fs::canonicalize(existing) {
            return tail.iter().rev().fold(real, |p, part| p.join(part));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                existing = parent;
            }
            _ => return lexical,
        }
    }
}

/// The agent log sources, rooted as `scan.rs` roots them.
pub(crate) fn log_sources(ctx: &Ctx) -> [Box<dyn SessionSource>; 2] {
    let home = ctx
        .env
        .home
        .as_deref()
        .expect("config::load rejects a missing HOME");
    let claude = ctx
        .env
        .claude_config_dir
        .clone()
        .unwrap_or_else(|| home.join(".claude"));
    let codex = ctx
        .env
        .codex_home
        .clone()
        .unwrap_or_else(|| home.join(".codex"));
    [
        Box::new(Claude::new(claude.join("projects"))),
        Box::new(Codex::new(codex)),
    ]
}

pub fn build_status(
    ctx: &Ctx,
    index: &dyn Index,
    root: &Path,
    stale: bool,
    policy: TextPolicy,
) -> StatusDoc {
    // The cache is keyed by the realpath of the root (scan.rs).
    let root = scan::canonical_root(root);
    let scanned_submodules = index.scanned_submodules(&root);
    let builder = Builder {
        ctx,
        index,
        root: &root,
        policy,
        submodules: scanned_submodules == Some(true),
        sources: log_sources(ctx),
    };
    let mut repos: Vec<Repo> = scan::visible_rows(ctx, &root, index.repos_for_root(&root))
        .iter()
        .map(|row| builder.repo(row))
        .collect();
    rank::sort_repos(&mut repos);
    StatusDoc {
        schema_version: SCHEMA_VERSION,
        root: root.to_string_lossy().into_owned(),
        scanned_at: index.scanned_at(&root),
        // §6.1: rows scanned with other --submodules settings do not answer this invocation.
        stale: stale || scanned_submodules.is_some_and(|s| s != ctx.config.submodules),
        partial: index
            .last_diagnostics(&root)
            .is_some_and(|d| d.walk.budget_hit),
        repos,
    }
}

// ---- commands ----

/// Writes `text` whole; a write failure (a closed pipe) is reported and fails the command.
fn finish(out: &mut dyn Write, text: &str, exit: ExitCode) -> ExitCode {
    match out.write_all(text.as_bytes()) {
        Ok(()) => exit,
        Err(e) => {
            eprintln!("rollcall: cannot write output: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `serde_json` failing on these plain structs would be a bug, not an input problem.
fn json_line<T: serde::Serialize>(doc: &T) -> String {
    let mut text = serde_json::to_string_pretty(doc).expect("documents serialise");
    text.push('\n');
    text
}

pub fn status(
    ctx: &Ctx,
    outcome: &ScanOutcome,
    root: &Path,
    json: bool,
    out: &mut dyn Write,
) -> ExitCode {
    let policy = TextPolicy {
        prompts: true,
        redact: ctx.config.redact_prompts,
    };
    let doc = build_status(ctx, &outcome.index, root, outcome.stale, policy);
    let text = if json {
        json_line(&doc)
    } else {
        plain_status(&doc, ctx.now)
    };
    finish(out, &text, outcome.exit)
}

/// §8.2: a line per repo, then an indented line per worktree, columns aligned like `ls`. Every
/// text that came from a repo or a log is sanitised; JSON keeps it exact.
fn plain_status(doc: &StatusDoc, now: Timestamp) -> String {
    let mut rows = Vec::new();
    for repo in &doc.repos {
        rows.push(repo_cells(repo, now));
        rows.extend(repo.worktrees.iter().map(|w| worktree_cells(w, now)));
    }
    align(&rows)
}

/// Widest a column may grow from one cell; wider cells overflow on their own row.
const COL_CAP: usize = 40;

/// Render `[name, badges, last]` rows on tab stops: the badge column starts 2 past the widest
/// in-cap name, the last column 2 past the widest in-cap badge cell after that (display columns).
/// A non-empty cell starts at its stop or 2 past the text before it, whichever is later, so an
/// over-cap cell shifts only its own line; an empty cell adds nothing.
fn align(rows: &[[String; 3]]) -> String {
    let width = |cell: &str| Span::raw(cell).width();
    let widest = |col: usize| {
        let fits = rows
            .iter()
            .map(|r| width(&r[col]))
            .filter(|&w| w <= COL_CAP);
        fits.max().unwrap_or(0)
    };
    let stop2 = widest(0) + 2;
    let stops = [stop2, stop2 + widest(1) + 2];
    let mut text = String::new();
    for row in rows {
        let mut line = row[0].clone();
        for (cell, stop) in row[1..].iter().zip(stops) {
            if cell.is_empty() {
                continue;
            }
            line.truncate(line.trim_end().len());
            let end = width(&line);
            line.push_str(&" ".repeat(stop.max(end + 2) - end));
            line.push_str(cell);
        }
        text.push_str(line.trim_end());
        text.push('\n');
    }
    text
}

fn age(now: Timestamp, then: Timestamp) -> String {
    let secs = (now.as_second() - then.as_second()).max(0);
    match secs {
        s if s < SECS_PER_MINUTE => format!("{s}s"),
        s if s < SECS_PER_HOUR => format!("{}m", s / SECS_PER_MINUTE),
        s if s < SECS_PER_DAY => format!("{}h", s / SECS_PER_HOUR),
        s => format!("{}d", s / SECS_PER_DAY),
    }
}

fn count_badges(dirty: u32, unpushed: u32) -> Vec<String> {
    let mut badges = Vec::new();
    if dirty > 0 {
        badges.push(format!("●{dirty}"));
    }
    if unpushed > 0 {
        badges.push(format!("↑{unpushed}"));
    }
    badges
}

/// No upstream but a remote exists (§3.3: `unpushed` is then counted, not null).
fn no_upstream(w: &Worktree) -> bool {
    w.upstream.is_none() && w.unpushed.is_some()
}

/// `[name, badges, last activity]` of a repo row.
fn repo_cells(repo: &Repo, now: Timestamp) -> [String; 3] {
    let wts = &repo.worktrees;
    let mut badges = Vec::new();
    if let Some(error) = &repo.error {
        badges.push(format!("error: {}", sanitise(error)));
    }
    match repo.kind {
        RepoKind::Nested => badges.push("nested".to_owned()),
        RepoKind::Bare => badges.push("bare".to_owned()),
        RepoKind::ExternalMain => badges.push("ext".to_owned()),
        RepoKind::Normal => {}
    }
    badges.extend(count_badges(
        wts.iter().map(|w| w.dirty_count + w.untracked_count).sum(),
        wts.iter().filter_map(|w| w.unpushed).sum(),
    ));
    let stale = wts.iter().filter(|w| w.stale_reason.is_some()).count();
    if stale > 0 {
        badges.push(format!("{stale} stale"));
    }
    if wts.iter().any(no_upstream) {
        badges.push("no-upstream".to_owned());
    }
    let activity = if repo.attention == 0 {
        rank::quiet(repo.last_activity_at, now)
    } else {
        repo.last_activity_at
            .map_or("—".to_owned(), |t| age(now, t))
    };
    // A repo line never starts with whitespace, so it can't pass for a worktree line.
    let name = match sanitise(&repo.name).trim_start() {
        "" => sanitise(&repo.path),
        name => name.to_owned(),
    };
    [name, badges.join(" "), activity]
}

fn worktree_label(w: &Worktree) -> String {
    let label = match (&w.branch, w.detached, &w.head) {
        (Some(branch), ..) => branch.clone(),
        (None, true, Some(head)) => {
            format!("(detached {})", head.chars().take(7).collect::<String>())
        }
        _ => w.path.clone(),
    };
    match sanitise(&label) {
        label if label.trim().is_empty() => sanitise(&w.path),
        label => label,
    }
}

fn state_badges(w: &Worktree) -> Vec<String> {
    if let Some(error) = &w.error {
        return vec![format!("error: {}", sanitise(error))];
    }
    let mut badges = Vec::new();
    if w.external {
        badges.push("ext".to_owned());
    }
    if w.stale_reason == Some(StaleReason::Missing) {
        badges.push("missing".to_owned());
        return badges;
    }
    let counts = count_badges(w.dirty_count + w.untracked_count, w.unpushed.unwrap_or(0));
    if counts.is_empty() && w.stale_reason.is_none() {
        badges.push("✓ clean".to_owned());
    }
    badges.extend(counts);
    match w.stale_reason {
        Some(StaleReason::Merged) => badges.push("merged·stale".to_owned()),
        Some(StaleReason::Inactive) => badges.push("stale".to_owned()),
        Some(StaleReason::Missing) | None => {}
    }
    if no_upstream(w) {
        badges.push("no-upstream".to_owned());
    }
    badges
}

fn agent_name(source: SourceKind) -> &'static str {
    match source {
        SourceKind::Claude => "claude",
        SourceKind::Codex => "codex",
    }
}

/// `last_session` inline (§8.1): agent, age, quoted title (the prompt when untitled). The text
/// is already sanitised by `render_text`; sanitising again keeps this safe on its own.
fn session_text(s: &rollcall_core::SessionOut, now: Timestamp) -> String {
    let what = s.title.as_deref().or(s.last_prompt.as_deref());
    let quoted = what
        .map(|t| format!(" \"{}\"", sanitise(t)))
        .unwrap_or_default();
    format!("{} {}{quoted}", agent_name(s.source), age(now, s.ended_at))
}

/// `[indented branch, state badges, last session]` of a worktree row.
fn worktree_cells(w: &Worktree, now: Timestamp) -> [String; 3] {
    [
        format!("  {}", worktree_label(w)),
        state_badges(w).join(" "),
        w.last_session
            .as_ref()
            .map_or(String::new(), |s| session_text(s, now)),
    ]
}

pub fn doctor(
    _ctx: &Ctx,
    outcome: &ScanOutcome,
    root: &Path,
    json: bool,
    out: &mut dyn Write,
) -> ExitCode {
    let root = scan::canonical_root(root);
    let doc = outcome
        .index
        .last_diagnostics(&root)
        .unwrap_or_else(|| empty_doctor(&root));
    let text = if json {
        json_line(&doc)
    } else {
        plain_doctor(&doc)
    };
    // §3.1: error rows are exit 4 for `status` and `scan` only; `doctor` keeps 1.
    let exit = if outcome.exit == ExitCode::from(scan::EXIT_ERROR_ROWS) {
        ExitCode::FAILURE
    } else {
        outcome.exit
    };
    finish(out, &text, exit)
}

/// No scan ever completed for the root: the §7 shape with nothing in it.
fn empty_doctor(root: &Path) -> DoctorDoc {
    DoctorDoc {
        schema_version: SCHEMA_VERSION,
        root: root.to_string_lossy().into_owned(),
        scanned_at: None,
        walk: Default::default(),
        repos_by_kind: Default::default(),
        worktrees: Default::default(),
        submodules: 0,
        repo_errors: 0,
        untrusted_git_dirs: 0,
        repo_timeouts: 0,
        repo_timeouts_abandoned: 0,
        notes: Vec::new(),
        sources: Vec::new(),
    }
}

fn plain_doctor(doc: &DoctorDoc) -> String {
    let mut lines = vec![
        format!("root: {}", sanitise(&doc.root)),
        format!(
            "scanned_at: {}",
            doc.scanned_at.map_or("never".to_owned(), |t| t.to_string())
        ),
        format!(
            "walk: {} entries, budget hit: {}, stopped at: {}, {} unreadable dirs, {} skipped dirs",
            doc.walk.entries,
            doc.walk.budget_hit,
            doc.walk
                .stopped_at
                .as_deref()
                .map_or("—".to_owned(), sanitise),
            doc.walk.unreadable_dirs,
            doc.walk.skipped_dirs,
        ),
    ];
    lines.extend(
        doc.walk
            .skipped_mounts
            .iter()
            .map(|m| format!("skipped mount: {}", sanitise(m))),
    );
    let kinds = &doc.repos_by_kind;
    lines.push(format!(
        "repos: {} normal, {} nested, {} bare, {} external_main",
        kinds.normal, kinds.nested, kinds.bare, kinds.external_main
    ));
    let wts = &doc.worktrees;
    lines.push(format!(
        "worktrees: {} total, {} external, {} missing, {} untrusted_path",
        wts.total, wts.external, wts.missing, wts.untrusted_path
    ));
    lines.push(format!("submodules: {}", doc.submodules));
    lines.push(format!("repo errors: {}", doc.repo_errors));
    lines.push(format!("untrusted git dirs: {}", doc.untrusted_git_dirs));
    lines.push(format!(
        "repo timeouts: {} ({} abandoned)",
        doc.repo_timeouts, doc.repo_timeouts_abandoned
    ));
    for s in &doc.sources {
        let skipped = &s.skipped;
        lines.push(format!(
            "{}: mode {}, db version {}, {} files, {} records, {} sessions, {} excluded children, {} vanished, {} matched, unmatched {}/{}/{} (outside root/path gone/non-repo), skipped {}/{}/{}/{}/{} (malformed/unknown type/oversize/non-uuid/bad rollout path)",
            agent_name(s.source),
            format!("{:?}", s.mode).to_lowercase(),
            s.db_version.map_or("—".to_owned(), |v| v.to_string()),
            s.files,
            s.records,
            s.sessions,
            s.excluded_children,
            s.vanished,
            s.matched,
            s.unmatched.outside_root,
            s.unmatched.path_gone,
            s.unmatched.non_repo,
            skipped.malformed,
            skipped.unknown_type,
            skipped.oversize,
            skipped.non_uuid,
            skipped.bad_rollout_path,
        ));
    }
    lines.extend(doc.notes.iter().map(|n| format!("note: {}", sanitise(n))));
    lines.push(String::new());
    lines.join("\n")
}

fn flatten<'a>(rows: &'a [RepoRow], out: &mut Vec<&'a RepoRow>) {
    for row in rows {
        out.push(row);
        flatten(&row.submodules, out);
    }
}

/// Prints the one-line `scanned <root>: …` summary (§3.1) to `err`.
pub fn scan_summary(
    ctx: &Ctx,
    outcome: &ScanOutcome,
    root: &Path,
    elapsed: Duration,
    err: &mut dyn Write,
) -> ExitCode {
    let root = scan::canonical_root(root);
    let rows = scan::visible_rows(ctx, &root, outcome.index.repos_for_root(&root));
    let mut all = Vec::new();
    flatten(&rows, &mut all);
    let worktrees: usize = all.iter().map(|r| r.state.worktrees.len()).sum();
    let errors = all
        .iter()
        .map(|r| {
            usize::from(r.state.error.is_some())
                + r.state
                    .worktrees
                    .iter()
                    .filter(|w| w.error.is_some())
                    .count()
        })
        .sum::<usize>();
    let sessions: u32 = outcome
        .index
        .last_diagnostics(&root)
        .map_or(0, |d| d.sources.iter().map(|s| s.sessions).sum());
    let line = format!(
        "scanned {}: {} repos, {worktrees} worktrees, {sessions} sessions in {:.2}s ({errors} errors)\n",
        sanitise(&root.to_string_lossy()),
        all.len(),
        elapsed.as_secs_f64(),
    );
    finish(err, &line, outcome.exit)
}

pub fn cache_clear(ctx: &Ctx, out: &mut dyn Write) -> ExitCode {
    if let Err(e) = rollcall_store::clear(&ctx.cache_dir) {
        eprintln!(
            "rollcall: cannot clear the cache {}: {e}",
            ctx.cache_dir.display()
        );
        return ExitCode::FAILURE;
    }
    let line = format!("cleared {}\n", sanitise(&ctx.cache_dir.to_string_lossy()));
    finish(out, &line, ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::Command;
    use std::sync::mpsc;

    use rollcall_core::{
        Backing, DoctorDoc, RepoKind, RepoRow, RepoState, ScanResult, SourceScan, SourceState,
        TextField, TextRef, TitleKind, WorktreeState,
    };
    use rollcall_store::SqliteIndex;
    use serde_json::Value;
    use tempfile::TempDir;

    use super::*;
    use crate::config::{Config, EnvPaths};
    use crate::scan::{self, ScanMode};

    const NOW: &str = "2026-09-29T12:00:00Z";
    const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
    const WATCHDOG: Duration = Duration::from_secs(60);
    /// ESC CSI, C1 CSI, right-to-left override, zero-width space: every §9 payload class.
    const PAYLOADS: [&str; 4] = ["\x1b[2J", "\u{9b}", "\u{202e}", "\u{200b}"];
    const POLICY: TextPolicy = TextPolicy {
        prompts: true,
        redact: false,
    };

    fn within<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(WATCHDOG).expect("watchdog expired")
    }

    fn succeeded(exit: ExitCode) -> bool {
        format!("{exit:?}") == format!("{:?}", ExitCode::SUCCESS)
    }

    struct World {
        _dir: TempDir,
        home: PathBuf,
        root: PathBuf,
        ctx: Ctx,
    }

    /// A HOME with empty agent dirs and a `work/` root inside it.
    fn world() -> World {
        let dir = TempDir::new().unwrap();
        let home = std::fs::canonicalize(dir.path()).unwrap();
        let root = home.join("work");
        std::fs::create_dir_all(&root).unwrap();
        let ctx = Ctx {
            config: Config::default(),
            now: NOW.parse().unwrap(),
            env: EnvPaths {
                home: Some(home.clone()),
                claude_config_dir: Some(home.join("claude")),
                codex_home: Some(home.join("codex")),
                ..EnvPaths::default()
            },
            cache_dir: home.join("cache"),
        };
        World {
            _dir: dir,
            home,
            root,
            ctx,
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_DATE", "2026-09-10T09:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-09-10T09:00:00Z")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    fn init_repo(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        git(path, &["init", "-q", "-b", "main"]);
        std::fs::write(path.join("f"), "x").unwrap();
        git(path, &["add", "f"]);
        git(path, &["commit", "-q", "-m", "c"]);
    }

    fn write_claude_session(w: &World, cwd: &Path, prompt: &str) {
        let dir = w.home.join("claude/projects/p");
        std::fs::create_dir_all(&dir).unwrap();
        let record = serde_json::json!({
            "type": "user", "sessionId": ID, "timestamp": "2026-09-28T18:04:11.532Z",
            "cwd": cwd, "gitBranch": "main", "isSidechain": false,
            "message": {"role": "user", "content": prompt}
        });
        std::fs::write(dir.join(format!("{ID}.jsonl")), format!("{record}\n")).unwrap();
    }

    fn scan_root(w: &World, mode: ScanMode) -> scan::ScanOutcome {
        let (ctx, root) = (w.ctx.clone(), w.root.clone());
        within(move || scan::scan(&ctx, &root, mode))
    }

    fn run_status(w: &World, outcome: &scan::ScanOutcome, json: bool) -> String {
        let mut out = Vec::new();
        status(&w.ctx, outcome, &w.root, json, &mut out);
        String::from_utf8(out).unwrap()
    }

    fn run_doctor(w: &World, outcome: &scan::ScanOutcome, json: bool) -> String {
        let mut out = Vec::new();
        doctor(&w.ctx, outcome, &w.root, json, &mut out);
        String::from_utf8(out).unwrap()
    }

    fn status_json(w: &World, outcome: &scan::ScanOutcome) -> Value {
        serde_json::from_str(&run_status(w, outcome, true)).unwrap()
    }

    fn validate(schema: &str, doc: &Value) {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema");
        let load = |name: &str| -> Value {
            let text = std::fs::read_to_string(dir.join(format!("{name}.json"))).unwrap();
            serde_json::from_str(&text).unwrap()
        };
        let registry = ["status", "doctor"]
            .iter()
            .fold(jsonschema::Registry::new(), |b, name| {
                b.add(format!("rollcall:{name}.json"), load(name)).unwrap()
            })
            .prepare()
            .unwrap();
        let validator = jsonschema::options()
            .with_registry(&registry)
            .build(&load(schema))
            .unwrap();
        let errors: Vec<String> = validator.iter_errors(doc).map(|e| e.to_string()).collect();
        assert!(errors.is_empty(), "{schema}: {errors:?}");
    }

    /// Commits a hand-built scan of `w.root` so tests can plant hostile text in it.
    fn commit(w: &World, repos: Vec<RepoRow>, diagnostics: DoctorDoc) -> scan::ScanOutcome {
        let mut index = SqliteIndex::open(&w.ctx.cache_dir).unwrap();
        index
            .commit_scan(
                &scan::canonical_root(&w.root),
                ScanResult {
                    scanned_at: NOW.parse().unwrap(),
                    submodules: false,
                    partial: false,
                    repos,
                    sources: Vec::new(),
                    matches: Vec::new(),
                    diagnostics,
                },
            )
            .unwrap();
        scan::ScanOutcome {
            index,
            stale: false,
            exit: ExitCode::SUCCESS,
        }
    }

    fn empty_doctor(w: &World) -> DoctorDoc {
        DoctorDoc {
            schema_version: 1,
            root: w.root.to_string_lossy().into_owned(),
            scanned_at: Some(NOW.parse().unwrap()),
            walk: Default::default(),
            repos_by_kind: Default::default(),
            worktrees: Default::default(),
            submodules: 0,
            repo_errors: 0,
            untrusted_git_dirs: 0,
            repo_timeouts: 0,
            repo_timeouts_abandoned: 0,
            notes: Vec::new(),
            sources: Vec::new(),
        }
    }

    fn repo_row(w: &World, branch: &str) -> RepoRow {
        let path = w.root.join("hostile");
        let worktree = WorktreeState {
            path: path.clone(),
            is_main: true,
            branch: Some(branch.to_owned()),
            detached: false,
            head: None,
            last_commit_at: Some("2026-09-28T00:00:00Z".parse().unwrap()),
            upstream: None,
            ahead: None,
            behind: None,
            unpushed: None,
            dirty_count: 0,
            untracked_count: 0,
            missing: false,
            merged: false,
            error: None,
        };
        RepoRow {
            path,
            name: "hostile".to_owned(),
            kind: RepoKind::Normal,
            contains_root: false,
            state: RepoState {
                default_branch: None,
                has_remote: false,
                error: None,
                worktrees: vec![worktree],
                pending: None,
            },
            submodules: Vec::new(),
        }
    }

    // ---- schema, shape, order ----

    #[test]
    fn status_json_validates_and_repos_are_sorted_by_attention() {
        let w = world();
        init_repo(&w.root.join("a"));
        init_repo(&w.root.join("b"));
        std::fs::write(w.root.join("b/new"), "u").unwrap();
        write_claude_session(&w, &w.root.join("a"), "hello there");
        let outcome = scan_root(&w, ScanMode::Fresh);

        let json = status_json(&w, &outcome);
        validate("status", &json);
        assert_eq!(json["stale"], false);
        let names: Vec<_> = json["repos"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["b", "a"], "an untracked file lifts b above a");
        let session = &json["repos"][1]["worktrees"][0]["last_session"];
        assert_eq!(session["id"], ID);
        assert_eq!(session["title"], "hello there");
        assert!(session["resume"].as_str().unwrap().ends_with(ID));
        assert_eq!(
            json["repos"][1]["last_activity_at"],
            "2026-09-28T18:04:11.532Z"
        );
    }

    #[test]
    fn plain_status_is_one_line_per_repo_then_one_indented_line_per_worktree() {
        let w = world();
        init_repo(&w.root.join("a"));
        init_repo(&w.root.join("b"));
        std::fs::write(w.root.join("b/new"), "u").unwrap();
        write_claude_session(&w, &w.root.join("a"), "hello there");
        let outcome = scan_root(&w, ScanMode::Fresh);

        let text = run_status(&w, &outcome, false);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert!(
            lines[0].starts_with('b') && lines[0].contains("●1"),
            "{text}"
        );
        assert!(
            lines[1].starts_with("  main") && lines[1].contains("●1"),
            "{text}"
        );
        assert!(
            lines[2].starts_with('a') && lines[2].contains("quiet"),
            "{text}"
        );
        assert!(
            lines[3].contains("✓ clean") && lines[3].contains("claude"),
            "{text}"
        );
        assert!(lines[3].contains("\"hello there\""), "{text}");
    }

    fn cells(rows: &[[&str; 3]]) -> Vec<[String; 3]> {
        rows.iter().map(|r| r.map(str::to_owned)).collect()
    }

    fn no_trailing_space(text: &str) {
        for line in text.lines() {
            assert_eq!(line, line.trim_end(), "{text:?}");
        }
    }

    #[test]
    fn align_pads_columns_one_and_two_to_the_widest_cell() {
        let text = align(&cells(&[["alpha", "a b", "1m"], ["be", "●1", "2h"]]));
        assert_eq!(text, "alpha  a b  1m\nbe     ●1   2h\n");
    }

    #[test]
    fn align_keeps_col3_aligned_over_empty_cells_without_trailing_space() {
        let text = align(&cells(&[
            ["a", "", "3m"],
            ["bb", "xx", "1m"],
            ["bb", "xx", ""],
            ["c", "", ""],
        ]));
        assert_eq!(text, "a       3m\nbb  xx  1m\nbb  xx\nc\n");
        no_trailing_space(&text);
    }

    #[test]
    fn align_lets_an_over_cap_cell_overflow_without_widening_the_column() {
        let long1 = "x".repeat(COL_CAP + 1);
        let text = align(&cells(&[
            [&long1, "b", "c"],
            ["a", "b", "c"],
            ["bb", "b", "c"],
        ]));
        assert_eq!(text, format!("{long1}  b  c\na   b  c\nbb  b  c\n"));

        let long2 = "y".repeat(COL_CAP + 1);
        let text = align(&cells(&[
            ["a", &long2, "c"],
            ["bb", "b", "c"],
            ["a", "bb", "c"],
        ]));
        assert_eq!(text, format!("a   {long2}  c\nbb  b   c\na   bb  c\n"));
    }

    #[test]
    fn align_counts_a_cell_of_exactly_the_cap_toward_the_width() {
        let edge = "x".repeat(COL_CAP);
        let text = align(&cells(&[[&edge, "b", "c"], ["a", "b", "c"]]));
        assert_eq!(
            text,
            format!("{edge}  b  c\na{}  b  c\n", " ".repeat(COL_CAP - 1))
        );
    }

    #[test]
    fn align_measures_display_columns_not_chars_or_bytes() {
        let text = align(&cells(&[["漢字", "b", "c"], ["ab", "b", "c"]]));
        assert_eq!(text, "漢字  b  c\nab    b  c\n");
        // at the cap boundary: 20 CJK chars = 40 columns fit, 21 = 42 overflow.
        let fits = "漢".repeat(COL_CAP / 2);
        let over = "漢".repeat(COL_CAP / 2 + 1);
        let text = align(&cells(&[
            [&fits, "b", "c"],
            [&over, "b", "c"],
            ["a", "b", "c"],
        ]));
        assert_eq!(
            text,
            format!(
                "{fits}  b  c\n{over}  b  c\na{}  b  c\n",
                " ".repeat(COL_CAP - 1)
            )
        );
    }

    #[test]
    fn align_places_col3_after_an_over_cap_col1_with_empty_col2_at_stop_or_end_plus_two() {
        // w1 = 2, w2 = 2: stops at 4 and 8; the over-cap name ends at 45, so col3 is at 47.
        let long = "x".repeat(COL_CAP + 5);
        let text = align(&cells(&[
            ["a", "bb", "c"],
            ["bb", "x", "c"],
            [&long, "", "q"],
        ]));
        assert_eq!(text, format!("a   bb  c\nbb  x   c\n{long}  q\n"));

        // w1 = 1, w2 = 40: stops at 3 and 45; a 41-column name ends at 41, so col3 rejoins 45.
        let short = "x".repeat(COL_CAP + 1);
        let text = align(&cells(&[
            ["a", &"w".repeat(COL_CAP), "1"],
            [&short, "", "q"],
        ]));
        let wide = "w".repeat(COL_CAP);
        assert_eq!(text, format!("a  {wide}  1\n{short}    q\n"));
    }

    #[test]
    fn align_rejoins_col3_when_an_over_cap_col1_leaves_room() {
        // w1 = 40, w2 = 10: stops at 42 and 54. The 41-column name pushes col2 to 43 (end 44),
        // and col3 is back on its stop at 54.
        let edge = "x".repeat(COL_CAP);
        let over = "y".repeat(COL_CAP + 1);
        let text = align(&cells(&[[&edge, "0123456789", "1"], [&over, "b", "c"]]));
        assert_eq!(
            text,
            format!("{edge}  0123456789  1\n{over}  b{}c\n", " ".repeat(10))
        );
    }

    #[test]
    fn align_trims_trailing_spaces_before_measuring_where_a_line_ends() {
        // "x…x " is 42 columns wide, over the cap; its end without the space is 41, so col2 is
        // at 43, not 45.
        let over = format!("{} ", "x".repeat(COL_CAP + 1));
        let text = align(&cells(&[
            ["a", "bb", "c"],
            ["bb", "x", "c"],
            [&over, "b", "c"],
        ]));
        let name = over.trim_end();
        assert_eq!(text, format!("a   bb  c\nbb  x   c\n{name}  b  c\n"));
    }

    #[test]
    fn align_measures_an_over_cap_cell_in_display_columns_when_placing_the_next_one() {
        // 21 × 漢 = 42 columns (over the cap), 21 chars, 63 bytes; col3 goes at 44, not at the
        // stop (35) and not at a char- or byte-based end.
        let over = "漢".repeat(COL_CAP / 2 + 1);
        let text = align(&cells(&[["a", &"x".repeat(30), "1"], [&over, "", "q"]]));
        assert_eq!(text, format!("a  {}  1\n{over}  q\n", "x".repeat(30)));
    }

    /// Display column where each non-empty col2/col3 cell of `line` starts, found by walking the
    /// rendered text rather than by re-deriving the layout.
    fn starts(row: &[String; 3], line: &str) -> [Option<usize>; 2] {
        let mut rest = line.strip_prefix(row[0].trim_end()).expect("col1 first");
        let mut at = breaker_width(row[0].trim_end());
        let mut out = [None, None];
        for (slot, cell) in out.iter_mut().zip(&row[1..]) {
            if cell.is_empty() {
                continue;
            }
            let gap = rest.len() - rest.trim_start_matches(' ').len();
            rest = rest[gap..].strip_prefix(cell.as_str()).expect("cell next");
            at += gap;
            *slot = Some(at);
            at += breaker_width(cell);
        }
        assert!(rest.is_empty(), "{line:?} has text past the last cell");
        out
    }

    #[test]
    fn align_starts_every_cell_at_its_stop_or_two_past_the_text_before_it() {
        let x = |n: usize| "x".repeat(n);
        let han = |n: usize| "漢".repeat(n);
        let table: Vec<[String; 3]> = vec![
            [x(5), "●1".into(), "3h".into()],
            [x(40), x(10), "c".into()],
            [x(41), String::new(), "q".into()],
            [x(41), "b".into(), "c".into()],
            [x(41), x(40), "c".into()],
            [x(50), x(41), "c".into()],
            [han(20), String::new(), "c".into()],
            [han(21), "b".into(), String::new()],
            [han(21), String::new(), "q".into()],
            ["  wt".into(), "✓ clean".into(), "claude 1d".into()],
            [String::new(), "b".into(), "c".into()],
            ["only".into(), String::new(), String::new()],
        ];
        let text = align(&table);
        let width = breaker_width;
        // w1 = 40 (x(40), han(20)); w2 = 40 (x(40)).
        let (s2, s3) = (42, 42 + 40 + 2);
        for (row, line) in table.iter().zip(text.lines()) {
            assert_eq!(line, line.trim_end(), "trailing space: {line:?}");
            let [at2, at3] = starts(row, line);
            let mut end = width(row[0].trim_end());
            if let Some(at) = at2 {
                assert_eq!(at, s2.max(end + 2), "col2: {line:?}");
                end = at + width(&row[1]);
            }
            if let Some(at) = at3 {
                assert_eq!(at, s3.max(end + 2), "col3: {line:?}");
            }
        }
    }

    #[test]
    fn plain_status_columns_line_up_across_repos_and_worktrees() {
        let w = world();
        init_repo(&w.root.join("b"));
        init_repo(&w.root.join("longer-name"));
        std::fs::write(w.root.join("b/new"), "u").unwrap();
        write_claude_session(&w, &w.root.join("longer-name"), "hello there");
        let outcome = scan_root(&w, ScanMode::Fresh);

        let text = run_status(&w, &outcome, false);
        no_trailing_space(&text);
        let lines: Vec<Vec<char>> = text.lines().map(|l| l.chars().collect()).collect();
        assert_eq!(lines.len(), 4, "{text}");
        // col1 = "longer-name" (11), col2 = "✓ clean" (7): col2 starts at column 13, col3 at 22.
        let at = |line: usize, col: usize| lines[line].get(col).copied().unwrap_or(' ');
        for i in [0, 1, 3] {
            assert_eq!([at(i, 11), at(i, 12)], [' ', ' '], "{text}");
            assert_ne!(at(i, 13), ' ', "{text}");
        }
        for i in [0, 2, 3] {
            assert_eq!([at(i, 20), at(i, 21)], [' ', ' '], "{text}");
            assert_ne!(at(i, 22), ' ', "{text}");
        }
        eprintln!("{text}");
    }

    #[test]
    fn a_repo_whose_name_sanitises_to_empty_shows_its_sanitised_path() {
        let w = world();
        let mut rows = Vec::new();
        for (name, dir) in [
            ("", "blank"),
            ("\u{200b}\u{7}", "invisible"),
            ("ok", "named"),
        ] {
            let mut r = repo_row(&w, "main");
            r.name = name.to_owned();
            r.path = w.root.join(dir);
            r.state.worktrees[0].path = r.path.clone();
            rows.push(r);
        }
        let outcome = commit(&w, rows, empty_doctor(&w));
        let text = run_status(&w, &outcome, false);
        let doc = build_status(&w.ctx, &outcome.index, &w.root, outcome.stale, POLICY);

        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 6, "{text}");
        for repo in &doc.repos {
            let shown = if repo.name == "ok" {
                "ok".to_owned()
            } else {
                sanitise(&repo.path)
            };
            let line = lines
                .iter()
                .find(|l| !l.starts_with(' ') && l.starts_with(shown.as_str()))
                .unwrap_or_else(|| panic!("no line for {shown:?}: {text}"));
            assert!(
                shown == "ok" || shown.starts_with('/') && shown.len() > 1,
                "{shown:?}"
            );
            assert_eq!(repo_cells(repo, w.ctx.now)[0], shown, "{text}");
            assert!(!line.starts_with(' '), "{line:?}");
        }
        assert!(
            lines.iter().filter(|l| !l.starts_with(' ')).count() == 3,
            "{text}"
        );
    }

    #[test]
    fn doctor_json_validates_and_reports_what_the_scan_saw() {
        let w = world();
        init_repo(&w.root.join("a"));
        write_claude_session(&w, &w.root.join("a"), "hi");
        let outcome = scan_root(&w, ScanMode::Fresh);

        let json: Value = serde_json::from_str(&run_doctor(&w, &outcome, true)).unwrap();
        validate("doctor", &json);
        assert_eq!(json["repos_by_kind"]["normal"], 1);
        assert_eq!(json["worktrees"]["total"], 1);
        assert_eq!(json["sources"][0]["source"], "claude");
        assert_eq!(json["sources"][0]["sessions"], 1);
    }

    #[test]
    fn doctor_without_any_scan_is_a_valid_empty_document() {
        let w = world();
        let outcome = scan_root(&w, ScanMode::NoScan);
        let json: Value = serde_json::from_str(&run_doctor(&w, &outcome, true)).unwrap();
        validate("doctor", &json);
        assert_eq!(json["scanned_at"], Value::Null);
        assert_eq!(json["worktrees"]["total"], 0);
    }

    #[test]
    fn plain_doctor_lists_the_counts_and_notes() {
        let w = world();
        let mut doc = empty_doctor(&w);
        doc.repo_timeouts = 2;
        doc.notes = vec!["codex: db missing".to_owned()];
        let outcome = commit(&w, Vec::new(), doc);
        let text = run_doctor(&w, &outcome, false);
        assert!(text.contains("repo timeouts: 2"), "{text}");
        assert!(text.contains("codex: db missing"), "{text}");
    }

    #[test]
    fn scan_summary_is_the_spec_line() {
        let w = world();
        init_repo(&w.root.join("a"));
        init_repo(&w.root.join("b"));
        write_claude_session(&w, &w.root.join("a"), "hi");
        let outcome = scan_root(&w, ScanMode::Fresh);

        let mut err = Vec::new();
        scan_summary(
            &w.ctx,
            &outcome,
            &w.root,
            Duration::from_millis(1234),
            &mut err,
        );
        let line = String::from_utf8(err).unwrap();
        let line = line.strip_suffix('\n').expect("newline-terminated");
        assert!(!line.contains('\n'), "{line}");
        let rest = line.strip_prefix("scanned ").unwrap();
        let (root, counts) = rest.rsplit_once(": ").unwrap();
        assert_eq!(root, w.root.to_string_lossy());
        assert_eq!(
            counts,
            "2 repos, 2 worktrees, 1 sessions in 1.23s (0 errors)"
        );
    }

    #[test]
    fn scan_summary_counts_error_rows() {
        let w = world();
        let mut row = repo_row(&w, "main");
        row.state.error = Some("boom".to_owned());
        row.state.worktrees[0].error = Some("boom".to_owned());
        let outcome = commit(&w, vec![row], empty_doctor(&w));
        let mut err = Vec::new();
        scan_summary(&w.ctx, &outcome, &w.root, Duration::ZERO, &mut err);
        let line = String::from_utf8(err).unwrap();
        assert!(line.contains("1 repos, 1 worktrees"), "{line}");
        assert!(line.trim_end().ends_with("(2 errors)"), "{line}");
    }

    // ---- sanitising (§9) ----

    #[test]
    fn a_branch_payload_is_absent_from_plain_output_and_exact_in_json() {
        let w = world();
        for payload in PAYLOADS {
            let branch = format!("pre{payload}post");
            let outcome = commit(&w, vec![repo_row(&w, &branch)], empty_doctor(&w));

            let plain = run_status(&w, &outcome, false);
            assert!(plain.contains("prepost"), "{payload:?}: {plain:?}");
            assert!(!plain.contains(payload), "{payload:?}: {plain:?}");

            let raw = run_status(&w, &outcome, true);
            assert!(!raw.contains('\x1b'), "{payload:?}: raw ESC in JSON");
            let json: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(json["repos"][0]["worktrees"][0]["branch"], branch.as_str());
        }
    }

    #[test]
    fn an_error_payload_is_absent_from_plain_output_and_exact_in_json() {
        let w = world();
        let mut row = repo_row(&w, "main");
        row.state.error = Some("bad \x1b[2J repo".to_owned());
        row.state.worktrees[0].error = Some("bad \u{202e} tree".to_owned());
        let outcome = commit(&w, vec![row], empty_doctor(&w));

        let plain = run_status(&w, &outcome, false);
        assert!(plain.contains("bad"), "{plain:?}");
        assert!(
            !plain.contains('\x1b') && !plain.contains('\u{202e}'),
            "{plain:?}"
        );
        let json = status_json(&w, &outcome);
        assert_eq!(json["repos"][0]["error"], "bad \x1b[2J repo");
    }

    #[test]
    fn a_prompt_payload_is_absent_from_plain_and_json_output() {
        let w = world();
        init_repo(&w.root.join("a"));
        let prompt = format!("hello {} world", PAYLOADS.join(" "));
        write_claude_session(&w, &w.root.join("a"), &prompt);
        let outcome = scan_root(&w, ScanMode::Fresh);

        let plain = run_status(&w, &outcome, false);
        assert!(plain.contains("hello"), "the session is shown: {plain:?}");
        let raw = run_status(&w, &outcome, true);
        let json: Value = serde_json::from_str(&raw).unwrap();
        let session = &json["repos"][0]["worktrees"][0]["last_session"];
        let title = session["title"].as_str().expect("title");
        let last_prompt = session["last_prompt"].as_str().expect("last_prompt");
        assert!(title.starts_with("hello"), "{session}");
        assert!(last_prompt.starts_with("hello"), "{session}");
        for payload in PAYLOADS {
            assert!(!plain.contains(payload), "plain {payload:?}");
            assert!(!raw.contains(payload), "json {payload:?}");
            assert!(!title.contains(payload), "title {payload:?}");
            assert!(!last_prompt.contains(payload), "prompt {payload:?}");
        }
        assert!(!raw.contains("u001b"), "an ESC survived into the JSON text");
    }

    #[test]
    fn a_doctor_note_payload_is_absent_from_plain_output_and_exact_in_json() {
        let w = world();
        let mut doc = empty_doctor(&w);
        doc.notes = vec!["ps failed: \x1b[2J oops".to_owned()];
        doc.walk.skipped_mounts = vec!["/mnt/\u{202e}x".to_owned()];
        let outcome = commit(&w, Vec::new(), doc);

        let plain = run_doctor(&w, &outcome, false);
        assert!(plain.contains("ps failed:"), "{plain:?}");
        assert!(
            !plain.contains('\x1b') && !plain.contains('\u{202e}'),
            "{plain:?}"
        );
        let json: Value = serde_json::from_str(&run_doctor(&w, &outcome, true)).unwrap();
        assert_eq!(json["notes"][0], "ps failed: \x1b[2J oops");
    }

    // ---- freshness, cache, resume ----

    #[test]
    fn no_scan_with_a_cache_is_stale_and_a_fresh_scan_is_not() {
        let w = world();
        init_repo(&w.root.join("a"));
        let fresh = status_json(&w, &scan_root(&w, ScanMode::Fresh));
        assert_eq!(fresh["stale"], false);
        assert_eq!(fresh["repos"].as_array().unwrap().len(), 1);

        let cached = status_json(&w, &scan_root(&w, ScanMode::NoScan));
        assert_eq!(cached["stale"], true);
        assert_eq!(cached["repos"].as_array().unwrap().len(), 1);
        assert_eq!(cached["scanned_at"], NOW);
    }

    #[test]
    fn a_cache_scanned_with_other_submodule_settings_is_stale() {
        let mut w = world();
        init_repo(&w.root.join("a"));
        let outcome = scan_root(&w, ScanMode::Fresh);
        w.ctx.config.submodules = true;
        let doc = build_status(&w.ctx, &outcome.index, &w.root, false, POLICY);
        assert!(doc.stale);
        assert!(doc.repos[0].submodules.is_none(), "rows hold no submodules");
    }

    #[test]
    fn a_session_whose_cwd_was_deleted_has_no_resume_line() {
        let w = world();
        init_repo(&w.root.join("a"));
        write_claude_session(&w, &w.root.join("a"), "hi");
        scan_root(&w, ScanMode::Fresh);
        std::fs::remove_dir_all(w.root.join("a")).unwrap();

        let json = status_json(&w, &scan_root(&w, ScanMode::NoScan));
        let session = &json["repos"][0]["worktrees"][0]["last_session"];
        assert_eq!(session["id"], ID, "still shown: {json}");
        assert_eq!(session["resume"], Value::Null);
    }

    #[test]
    fn cache_clear_removes_the_cache_directory_and_tolerates_its_absence() {
        let w = world();
        init_repo(&w.root.join("a"));
        scan_root(&w, ScanMode::Fresh);
        assert!(w.ctx.cache_dir.is_dir());

        let mut out = Vec::new();
        assert!(succeeded(cache_clear(&w.ctx, &mut out)));
        assert!(!w.ctx.cache_dir.exists());
        assert!(!out.is_empty(), "says what it did");
        assert!(succeeded(cache_clear(&w.ctx, &mut out)));
    }

    #[test]
    fn a_failing_writer_is_an_error_not_a_success() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let w = world();
        let outcome = commit(&w, vec![repo_row(&w, "main")], empty_doctor(&w));
        assert!(!succeeded(status(
            &w.ctx,
            &outcome,
            &w.root,
            true,
            &mut Broken
        )));
    }

    // ---- render_session ----

    struct Canned;
    impl SessionSource for Canned {
        fn kind(&self) -> SourceKind {
            SourceKind::Claude
        }
        fn scan(&self, _: &SourceState) -> SourceScan {
            unreachable!("render_session never scans")
        }
        fn resolve(&self, r: &TextRef) -> Option<String> {
            match r {
                TextRef::CodexThread {
                    field: TextField::Title,
                    ..
                } => Some("the title".into()),
                TextRef::CodexThread {
                    field: TextField::LastPrompt,
                    ..
                } => Some("the prompt \x1b[2J sk-abcdefghijklmnopqrstuv".into()),
                TextRef::LogLine { .. } => None,
            }
        }
    }

    fn session(cwd: &Path, kind: TitleKind) -> Session {
        let thread_id = Uuid::parse(ID).unwrap();
        let text = |field| TextRef::CodexThread {
            thread_id: thread_id.clone(),
            field,
        };
        Session {
            source: SourceKind::Claude,
            id: thread_id.clone(),
            cwd: cwd.to_path_buf(),
            branch: Some("main".into()),
            started_at: NOW.parse().unwrap(),
            ended_at: NOW.parse().unwrap(),
            title: Some((text(TextField::Title), kind)),
            last_prompt: Some(text(TextField::LastPrompt)),
            message_count: 3,
            backing: Backing::CodexThread {
                db_path: PathBuf::from("/db"),
                thread_id: thread_id.clone(),
            },
        }
    }

    #[test]
    fn render_session_sanitises_scrubs_and_resolves_the_resume_line() {
        let dir = TempDir::new().unwrap();
        let out = render_session(&session(dir.path(), TitleKind::Custom), &Canned, POLICY);
        assert_eq!(out.title.as_deref(), Some("the title"));
        assert_eq!(out.title_kind, Some(TitleKind::Custom));
        assert_eq!(out.last_prompt.as_deref(), Some("the prompt  [redacted]"));
        assert_eq!(out.message_count, 3);
        assert_eq!(
            out.resume,
            Some(format!(
                "cd -- '{}' && claude --resume {ID}",
                dir.path().display()
            ))
        );
        let gone = session(&dir.path().join("gone"), TitleKind::Custom);
        assert_eq!(render_session(&gone, &Canned, POLICY).resume, None);
    }

    #[test]
    fn render_session_withholds_prompt_text_per_policy() {
        let cwd = Path::new("/");
        let no_prompts = TextPolicy {
            prompts: false,
            redact: false,
        };
        let prompt_titled = render_session(&session(cwd, TitleKind::Prompt), &Canned, no_prompts);
        assert_eq!(prompt_titled.title, None);
        assert_eq!(prompt_titled.title_kind, None);
        assert_eq!(prompt_titled.last_prompt, None);
        let ai = render_session(&session(cwd, TitleKind::Ai), &Canned, no_prompts);
        assert_eq!(ai.title.as_deref(), Some("the title"));
        assert_eq!(ai.title_kind, Some(TitleKind::Ai));
        assert_eq!(ai.last_prompt, None);

        let redact = TextPolicy {
            prompts: true,
            redact: true,
        };
        let hidden = render_session(&session(cwd, TitleKind::Custom), &Canned, redact);
        assert_eq!(hidden.title, None);
        assert_eq!(hidden.title_kind, None);
        assert_eq!(hidden.last_prompt, None);
    }

    #[test]
    fn an_unresolvable_title_leaves_title_and_kind_null_together() {
        let mut s = session(Path::new("/"), TitleKind::Custom);
        s.title = Some((
            TextRef::LogLine {
                dev: 0,
                inode: 0,
                size_at_scan: 0,
                offset: 0,
                len: 0,
                field: TextField::Title,
            },
            TitleKind::Custom,
        ));
        let out = render_session(&s, &Canned, POLICY);
        assert_eq!((out.title, out.title_kind), (None, None));
    }

    // ---- display_line ----

    fn target(cwd: &Path, source: SourceKind) -> ResumeTarget {
        ResumeTarget {
            source,
            id: Uuid::parse(ID).unwrap(),
            cwd: cwd.to_path_buf(),
        }
    }

    #[test]
    fn display_line_names_the_agent_and_its_resume_verb() {
        assert_eq!(
            display_line(&target(Path::new("/r/CRM"), SourceKind::Claude)),
            format!("cd -- '/r/CRM' && claude --resume {ID}")
        );
        assert_eq!(
            display_line(&target(Path::new("/r/it's"), SourceKind::Codex)),
            format!("cd -- '/r/it'\\''s' && codex resume {ID}")
        );
    }

    #[test]
    fn display_line_round_trips_every_shell_hostile_cwd_through_sh() {
        const HOSTILE: [&str; 8] = [
            "a b",
            "a$HOME",
            "a`echo pwned`",
            "a;echo pwned",
            "it's",
            "a\nb",
            "$(echo pwned)",
            "'; echo pwned; '",
        ];
        let dir = TempDir::new().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let bin = base.join("bin");
        std::fs::create_dir(&bin).unwrap();
        for agent in ["claude", "codex"] {
            let script = bin.join(agent);
            std::fs::write(&script, "#!/bin/sh\npwd -P\nprintf '%s\\n' \"$@\"\n").unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        for (source, args) in [
            (SourceKind::Claude, "--resume"),
            (SourceKind::Codex, "resume"),
        ] {
            for name in HOSTILE {
                let cwd = base.join(name);
                std::fs::create_dir_all(&cwd).unwrap();
                let out = Command::new("/bin/sh")
                    .arg("-c")
                    .arg(display_line(&target(&cwd, source)))
                    .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                    .output()
                    .unwrap();
                assert!(out.status.success(), "{name:?}: {out:?}");
                assert_eq!(
                    String::from_utf8(out.stdout).unwrap(),
                    format!("{}\n{args}\n{ID}\n", cwd.display()),
                    "{name:?}"
                );
            }
        }
    }

    // ---- breaker: column alignment (§8.2) ----

    fn breaker_width(s: &str) -> usize {
        Span::raw(s).width()
    }

    /// Checks AC1/AC2/AC4 of `align` output against its rows, measuring each cell's start as
    /// the display width of the rendered line before it.
    fn breaker_check(rows: &[[String; 3]], text: &str) {
        let lines: Vec<&str> = text.split_terminator('\n').collect();
        assert_eq!(lines.len(), rows.len(), "{text:?}");
        let (mut col2, mut col3) = (Vec::new(), Vec::new());
        for (row, line) in rows.iter().zip(&lines) {
            assert_eq!(*line, line.trim_end(), "trailing space: {line:?}");
            let [c1, c2, c3] = row;
            assert!(line.starts_with(c1.trim_end()), "{line:?} / {row:?}");
            let (w1, w2) = (breaker_width(c1), breaker_width(c2));
            let mut gap_after = c1.len();
            if !c2.is_empty() {
                let at = c1.len() + line[c1.len()..].len() - line[c1.len()..].trim_start().len();
                assert!(line[at..].starts_with(c2.as_str()), "{line:?} / {row:?}");
                assert!(at - c1.len() >= 2, "col1->col2 gap < 2: {line:?}");
                if w1 <= COL_CAP {
                    col2.push((breaker_width(&line[..at]), line.to_owned()));
                } else {
                    assert_eq!(&line[c1.len()..at], "  ", "AC2 col1 overflow: {line:?}");
                }
                gap_after = at + c2.len();
            }
            if !c3.is_empty() {
                assert!(line.ends_with(c3.as_str()), "{line:?} / {row:?}");
                let at = line.len() - c3.len();
                let gap = &line[gap_after..at];
                assert!(
                    gap.len() >= 2 && gap.trim().is_empty(),
                    "gap {gap:?}: {line:?}"
                );
                if w1 <= COL_CAP && w2 <= COL_CAP {
                    col3.push((breaker_width(&line[..at]), line.to_owned()));
                } else if w2 > COL_CAP {
                    assert_eq!(gap, "  ", "AC2 col2 overflow: {line:?}");
                }
            }
        }
        for col in [col2, col3] {
            if let Some((first, _)) = col.first() {
                assert!(col.iter().all(|(w, _)| w == first), "misaligned: {col:#?}");
            }
        }
    }

    #[test]
    fn breaker_align_holds_ac1_ac2_ac4_over_an_adversarial_table() {
        let x = |n: usize| "x".repeat(n);
        let han = |n: usize| "漢".repeat(n);
        let table: Vec<[String; 3]> = vec![
            [x(39), "b".into(), "c".into()],
            [x(40), "bb".into(), "c".into()],
            [x(41), "b".into(), "c".into()],
            [format!("  {}", x(38)), "✓ clean".into(), String::new()],
            [
                format!("  {}", x(39)),
                "✓ clean".into(),
                "claude 1d \"t\"".into(),
            ],
            [han(20), "●1 ↑2".into(), "3h".into()],
            [han(21), "●1".into(), "3h".into()],
            [format!("{}x", han(19)), "merged·stale".into(), "—".into()],
            [format!("{}x", han(20)), "b".into(), "c".into()],
            ["e\u{301}".into(), x(40), "c".into()],
            ["a".into(), x(41), "c".into()],
            ["a".into(), han(21), "c".into()],
            [x(50), x(50), "c".into()],
            [
                "😀👍🏽".into(),
                "❤\u{fe0f}".into(),
                "1\u{fe0f}\u{20e3}".into(),
            ],
            ["🇯🇵 flag".into(), "\u{644}\u{627}".into(), "c".into()],
            ["a\u{a0}".into(), "b".into(), "c".into()],
            ["a\u{3000}".into(), String::new(), "quiet 3d".into()],
            ["\u{301}lead".into(), "b".into(), "c".into()],
            [String::new(), String::new(), "c".into()],
            ["only".into(), String::new(), String::new()],
            [x(41), String::new(), "quiet 1d".into()],
            ["  wt".into(), "b".into(), String::new()],
        ];
        let text = align(&table);
        breaker_check(&table, &text);
        // every row over the cap: widths fall to 0, cells still two spaces apart
        let over = vec![
            [x(41), x(41), "c".into()],
            [han(21), "b".into(), "c".into()],
        ];
        let text = align(&over);
        breaker_check(&over, &text);
        assert_eq!(align(&[]), "");
    }

    /// A doc of hostile/odd rows planted through the index, rendered by the real `status`.
    #[test]
    fn breaker_plain_status_aligns_hostile_rows_end_to_end() {
        let w = world();
        let mut rows = Vec::new();
        let named = |name: &str, branch: &str| {
            let mut r = repo_row(&w, branch);
            r.name = name.to_owned();
            r.path = w.root.join(format!("r{}", name.len()));
            r.state.worktrees[0].path = r.path.clone();
            r
        };
        rows.push(named("ok", "main"));
        rows.push(named("tab\there\u{200b}", &"b".repeat(38)));
        rows.push(named(&"n".repeat(40), &"b".repeat(39)));
        rows.push(named(&"漢".repeat(21), "feature/漢字"));
        let mut err = named("errored", "main");
        err.state.error = Some(format!("gix: {}", "e".repeat(60)));
        err.state.worktrees[0].error = Some("worktree\nbroken\twide".to_owned());
        rows.push(err);
        let mut det = named("detached-one", "x");
        det.state.worktrees[0].branch = None;
        det.state.worktrees[0].detached = true;
        det.state.worktrees[0].head = Some("0123456789abcdef0123456789abcdef01234567".into());
        let mut ext = det.state.worktrees[0].clone();
        ext.detached = false;
        ext.head = None;
        ext.is_main = false;
        ext.path = PathBuf::from(format!("/elsewhere/{}/wt", "deep".repeat(12)));
        let mut missing = ext.clone();
        missing.path = w.root.join("gone");
        missing.branch = Some("gone-branch".into());
        missing.missing = true;
        det.state.worktrees.extend([ext, missing]);
        det.state.worktrees[0].dirty_count = 3;
        rows.push(det);
        let n = rows.len();
        let outcome = commit(&w, rows, empty_doctor(&w));

        let text = run_status(&w, &outcome, false);
        let doc = build_status(&w.ctx, &outcome.index, &w.root, outcome.stale, POLICY);
        assert_eq!(doc.repos.len(), n, "{text}");
        let mut cells = Vec::new();
        let mut is_repo = Vec::new();
        for repo in &doc.repos {
            cells.push(repo_cells(repo, w.ctx.now));
            is_repo.push(true);
            cells.extend(repo.worktrees.iter().map(|t| worktree_cells(t, w.ctx.now)));
            is_repo.extend(repo.worktrees.iter().map(|_| false));
        }
        assert_eq!(text, align(&cells));
        breaker_check(&cells, &text);
        // AC4: one unindented line per repo, then one indented line per worktree.
        for (line, repo) in text.lines().zip(is_repo) {
            let indented = line.starts_with("  ") && !line[2..].starts_with(' ');
            let shaped = if repo {
                !line.starts_with(' ')
            } else {
                indented
            };
            assert!(shaped, "{line:?}\n{text}");
        }
    }

    // ---- breaker2: tab stops, empty-name fallback, U+2028/U+2029 (§8.2, §9 revision 15) ----

    /// Trailing spaces count toward a cell's width and the cap but not toward where its text
    /// ends; a blank-but-non-empty cell adds no visible text.
    #[test]
    fn breaker2_align_trailing_and_blank_cells_follow_the_stop_formula() {
        let x = |n: usize| "x".repeat(n);
        // w1 = 40 (39 x + a space), w2 = 2: S2 = 42, S3 = 46.
        let text = align(&cells(&[
            [&format!("{} ", x(39)), "b", "c"],
            ["a", "bb", "c"],
        ]));
        let want = format!("{}   b   c\na{}bb  c\n", x(39), " ".repeat(41));
        assert_eq!(text, want);
        // 41 x + a space is over the cap (42): w1 = 1, w2 = 2, S2 = 3, S3 = 7. E = 41.
        let text = align(&cells(&[
            [&format!("{} ", x(41)), "", "q"],
            [&format!("{} ", x(41)), "b", "c"],
            ["a", "bb", "c"],
        ]));
        let want = format!("{0}  q\n{0}  b  c\na  bb  c\n", x(41));
        assert_eq!(text, want);
        // A col2 of only spaces: w2 = 3 (S3 = 9) but nothing visible before col3.
        let text = align(&cells(&[
            ["a", "   ", "c"],
            ["bb", "x", "c"],
            ["c", "   ", ""],
        ]));
        assert_eq!(text, "a        c\nbb  x    c\nc\n");
        // A col2 with a trailing space: col3 still on its stop.
        let text = align(&cells(&[["a", "b ", "c"], ["a", "bb", "c"]]));
        assert_eq!(text, "a  b   c\na  bb  c\n");
        no_trailing_space(&text);
    }

    /// AC6 for repos that also carry badges: an errored, nested and bare repo whose names
    /// sanitise to empty, with hostile path segments, render their sanitised path as col1,
    /// stay aligned, and `--json` keeps name and path exact.
    #[test]
    fn breaker2_empty_name_fallback_for_nested_bare_and_errored_repos() {
        let w = world();
        let mut rows = Vec::new();
        for (i, (name, kind)) in [
            ("", RepoKind::Nested),
            ("\u{200b}\x1b[2J", RepoKind::Bare),
            ("\u{7f}", RepoKind::Normal),
            ("ok", RepoKind::Normal),
        ]
        .into_iter()
        .enumerate()
        {
            let mut r = repo_row(&w, "main");
            r.name = name.to_owned();
            r.kind = kind;
            r.path = w.root.join(format!("p{i}\u{2028}\n\x1b[31m\u{202e}x"));
            r.state.worktrees[0].path = r.path.clone();
            if i == 2 {
                r.state.error = Some("bad\u{2029}thing".into());
            }
            rows.push(r);
        }
        let outcome = commit(&w, rows, empty_doctor(&w));
        let text = run_status(&w, &outcome, false);
        let doc = build_status(&w.ctx, &outcome.index, &w.root, outcome.stale, POLICY);
        assert_eq!(doc.repos.len(), 4, "{text}");
        assert!(
            !text.contains(['\u{2028}', '\u{2029}', '\x1b', '\u{202e}']),
            "{text:?}"
        );

        let mut rows = Vec::new();
        for repo in &doc.repos {
            rows.push(repo_cells(repo, w.ctx.now));
            rows.extend(repo.worktrees.iter().map(|t| worktree_cells(t, w.ctx.now)));
        }
        assert_eq!(text, align(&rows));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 8, "{text}");
        let mut i = 0;
        for repo in &doc.repos {
            let line = lines[i];
            let shown = if repo.name == "ok" {
                "ok".to_owned()
            } else {
                sanitise(&repo.path)
            };
            assert!(
                shown.starts_with("ok") || shown.starts_with('/'),
                "{shown:?}"
            );
            assert!(
                !line.starts_with(' ') && line.starts_with(&shown),
                "{line:?}"
            );
            let badge = match (repo.kind, &repo.error) {
                (_, Some(_)) => "error: bad thing",
                (RepoKind::Nested, _) => "nested",
                (RepoKind::Bare, _) => "bare",
                _ => "",
            };
            assert!(line.contains(badge), "{line:?}");
            // An over-cap path: the badge sits two past it.
            if !badge.is_empty() {
                assert_eq!(
                    &line[shown.len()..shown.len() + 2 + badge.len()],
                    format!("  {badge}")
                );
            }
            assert!(lines[i + 1].starts_with("  main"), "{text}");
            i += 2;
        }

        let json = status_json(&w, &outcome);
        validate("status", &json);
        let mut names: Vec<&str> = json["repos"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["", "ok", "\u{7f}", "\u{200b}\x1b[2J"]);
        for r in json["repos"].as_array().unwrap() {
            assert!(
                r["path"]
                    .as_str()
                    .unwrap()
                    .ends_with("\u{2028}\n\x1b[31m\u{202e}x"),
                "{r}"
            );
        }
        let errors: Vec<&Value> = json["repos"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| &r["error"])
            .filter(|e| !e.is_null())
            .collect();
        assert_eq!(errors, [&Value::from("bad\u{2029}thing")]);
    }

    /// U+2028 through the real git and Claude-log parsers: plain output never carries it (branch
    /// and title become a space); JSON keeps the branch exact and sanitises the title and prompt.
    #[test]
    fn breaker2_separators_flow_through_plain_and_json_from_a_real_scan() {
        let w = world();
        let repo = w.root.join("sep");
        init_repo(&repo);
        git(&repo, &["checkout", "-q", "-b", "feat\u{2028}x\u{2029}y"]);
        write_claude_session(&w, &repo, "hello\u{2028}there\u{2029}now");
        let outcome = scan_root(&w, ScanMode::Fresh);

        let text = run_status(&w, &outcome, false);
        assert!(!text.contains(['\u{2028}', '\u{2029}']), "{text:?}");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines[0].starts_with("sep  "), "{text}");
        assert!(lines[1].starts_with("  feat x y  "), "{text}");
        assert!(lines[1].ends_with("\"hello there now\""), "{text}");

        let json = status_json(&w, &outcome);
        let wt = &json["repos"][0]["worktrees"][0];
        assert_eq!(wt["branch"], "feat\u{2028}x\u{2029}y");
        let s = &wt["last_session"];
        assert_eq!(s["title"], "hello there now", "{s}");
        assert_eq!(s["last_prompt"], "hello there now", "{s}");
    }

    // ---- blank names fall back to the path; every Unicode space is trimmed before a stop (§8.2) ----

    /// One real `hostile` repo (one worktree) as a status doc, for rewriting its name and branch.
    fn blank_doc() -> (World, StatusDoc) {
        let w = world();
        let outcome = commit(&w, vec![repo_row(&w, "main")], empty_doctor(&w));
        let doc = build_status(&w.ctx, &outcome.index, &w.root, outcome.stale, POLICY);
        assert_eq!(doc.repos.len(), 1);
        (w, doc)
    }

    #[test]
    fn a_repo_name_that_sanitises_to_whitespace_shows_the_sanitised_path() {
        let (w, mut doc) = blank_doc();
        for name in [" ", "\t", "\n", "\u{2028}", "\u{200b}"] {
            doc.repos[0].name = name.to_owned();
            let want = sanitise(&doc.repos[0].path);
            assert!(want.starts_with('/'), "{want:?}");
            let cells = repo_cells(&doc.repos[0], w.ctx.now);
            assert_eq!(cells[0], want, "{name:?}");
            let text = plain_status(&doc, w.ctx.now);
            let first = text.lines().next().unwrap();
            assert!(first.starts_with(&want), "{name:?}: {text:?}");
            assert!(
                !first.starts_with(char::is_whitespace),
                "{name:?}: {text:?}"
            );
        }
    }

    #[test]
    fn a_repo_name_with_leading_whitespace_starts_its_line_with_the_name() {
        let (w, mut doc) = blank_doc();
        for name in ["\tfoo", " foo", "\u{2028}foo", "\n \u{200b}foo"] {
            doc.repos[0].name = name.to_owned();
            assert_eq!(repo_cells(&doc.repos[0], w.ctx.now)[0], "foo", "{name:?}");
            let text = plain_status(&doc, w.ctx.now);
            assert!(text.starts_with("foo  "), "{name:?}: {text:?}");
        }
    }

    #[test]
    fn a_worktree_branch_that_sanitises_to_whitespace_shows_the_indented_sanitised_path() {
        let (w, mut doc) = blank_doc();
        let want = format!("  {}", sanitise(&doc.repos[0].worktrees[0].path));
        for branch in [" ", "\u{2029}", "\t", "\u{200b}", ""] {
            doc.repos[0].worktrees[0].branch = Some(branch.to_owned());
            let cells = worktree_cells(&doc.repos[0].worktrees[0], w.ctx.now);
            assert_eq!(cells[0], want, "{branch:?}");
            let text = plain_status(&doc, w.ctx.now);
            assert!(
                text.lines().nth(1).unwrap().starts_with(&want),
                "{branch:?}: {text:?}"
            );
        }
        // Not blank once sanitised: U+2028 becomes a space but `feat` keeps the label.
        doc.repos[0].worktrees[0].branch = Some("\u{2028}feat".to_owned());
        let cells = worktree_cells(&doc.repos[0].worktrees[0], w.ctx.now);
        assert_eq!(cells[0], "   feat");
        let text = plain_status(&doc, w.ctx.now);
        assert!(
            text.lines().nth(1).unwrap().starts_with("   feat  "),
            "{text:?}"
        );
    }

    /// I3-6 (and I3-7 for the TUI's first paint, which answers from the cache): the snapshot of
    /// a root whose cached scan holds six repos lists only the two that `ignore` leaves.
    #[test]
    fn the_tui_snapshot_lists_only_the_repos_that_ignore_leaves() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut w = world();
        for name in ["a", "app", "app2", "grp/x", "grp/y", "z"] {
            init_repo(&w.root.join(name));
        }
        let render = |w: &World, outcome: &scan::ScanOutcome| -> Vec<String> {
            let doc = build_status(&w.ctx, &outcome.index, &w.root, outcome.stale, POLICY);
            let model = crate::tui::Model::new(doc, w.ctx.now, true);
            let mut terminal = Terminal::new(TestBackend::new(110, 16)).unwrap();
            terminal.draw(|f| crate::tui::draw(f, &model)).unwrap();
            let buf = terminal.backend().buffer().clone();
            (0..buf.area.height)
                .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
                .collect()
        };
        let all = render(&w, &scan_root(&w, ScanMode::Fresh));
        assert!(all[0].contains("6 repos"), "control: {all:#?}");

        w.ctx.config.ignore = ["grp", "app", "z"].map(|n| w.root.join(n)).to_vec();
        let shown = render(&w, &scan_root(&w, ScanMode::NoScan));
        assert!(shown[0].contains("2 repos"), "{shown:#?}");
        // The list pane's first column: one row per visible repo, in §3.3 order.
        let listed: Vec<&str> = shown[1..]
            .iter()
            .filter_map(|line| {
                line.trim_start_matches(['│', '▶', ' '])
                    .split_whitespace()
                    .next()
            })
            .filter(|word| ["a", "app", "app2", "x", "y", "z"].contains(word))
            .collect();
        assert_eq!(listed, ["a", "app2"], "{shown:#?}");
    }

    /// E ignores trailing Unicode whitespace of any kind, so the next cell sits two past the
    /// text, not two past a no-break or ideographic space.
    #[test]
    fn align_trims_trailing_unicode_whitespace_before_measuring_where_a_line_ends() {
        let x41 = "x".repeat(41);
        for space in ["\u{a0}", "\u{3000}"] {
            // col1 is over the cap (w >= 42), so stop2 = 3 and only E + 2 matters.
            let col1 = format!("{x41}{space}");
            let text = align(&cells(&[[&col1, "b", "c"], ["a", "bb", "c"]]));
            assert_eq!(text, format!("{x41}  b  c\na  bb  c\n"), "{space:?}");
            // An empty col2 does not move col3 off E + 2 either.
            let text = align(&cells(&[[&col1, "", "q"], ["a", "bb", "c"]]));
            assert_eq!(text, format!("{x41}  q\na  bb  c\n"), "{space:?}");
        }
    }
}
