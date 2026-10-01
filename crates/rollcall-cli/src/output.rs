use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use rollcall_core::text::{render_text, sanitise, shell_quote};
use rollcall_core::{
    DoctorDoc, Index, Repo, RepoKind, RepoRow, SCHEMA_VERSION, Session, SourceKind, StaleReason,
    StatusDoc, TextRef, Timestamp, TitleKind, Uuid, Worktree, WorktreeState, rank,
};
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
    let show = |r: &TextRef| source.resolve(r).map(|t| render_text(&t, redact_emails));
    let title = session
        .title
        .as_ref()
        .filter(|(_, kind)| !policy.redact && (policy.prompts || *kind != TitleKind::Prompt))
        .and_then(|(r, kind)| show(r).map(|text| (text, *kind)));
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
    let mut repos: Vec<Repo> = index
        .repos_for_root(&root)
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

/// §8.2: a line per repo, then an indented line per worktree. Every text that came from a repo
/// or a log is sanitised; JSON keeps it exact.
fn plain_status(doc: &StatusDoc, now: Timestamp) -> String {
    let mut text = String::new();
    for repo in &doc.repos {
        text.push_str(&repo_line(repo, now));
        text.push('\n');
        for worktree in &repo.worktrees {
            text.push_str(&worktree_line(worktree, now));
            text.push('\n');
        }
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

fn repo_line(repo: &Repo, now: Timestamp) -> String {
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
    if repo.attention == 0 {
        badges.push(rank::quiet(repo.last_activity_at, now));
    } else {
        let activity = repo
            .last_activity_at
            .map_or("—".to_owned(), |t| age(now, t));
        badges.push(activity);
    }
    format!("{}  {}", sanitise(&repo.name), badges.join(" "))
}

fn worktree_label(w: &Worktree) -> String {
    let label = match (&w.branch, w.detached, &w.head) {
        (Some(branch), ..) => branch.clone(),
        (None, true, Some(head)) => {
            format!("(detached {})", head.chars().take(7).collect::<String>())
        }
        _ => w.path.clone(),
    };
    sanitise(&label)
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

fn worktree_line(w: &Worktree, now: Timestamp) -> String {
    let mut parts = vec![worktree_label(w)];
    parts.extend(state_badges(w));
    let mut line = format!("  {}", parts.join("  "));
    if let Some(s) = &w.last_session {
        line.push_str("  ");
        line.push_str(&session_text(s, now));
    }
    line
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
    finish(out, &text, outcome.exit)
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
    _ctx: &Ctx,
    outcome: &ScanOutcome,
    root: &Path,
    elapsed: Duration,
    err: &mut dyn Write,
) -> ExitCode {
    let root = scan::canonical_root(root);
    let rows = outcome.index.repos_for_root(&root);
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
}
