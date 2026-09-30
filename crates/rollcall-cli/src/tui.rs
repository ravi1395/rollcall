use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use rollcall_core::text::sanitise;
use rollcall_core::{
    Repo, RepoKind, SessionOut, SourceKind, StaleReason, StatusDoc, Timestamp, Worktree, rank,
};

/// Which pane has focus (`tab` switches).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Pane {
    #[default]
    Repos,
    Worktrees,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub doc: StatusDoc,
    /// Cursor in the Repos pane: index into the repos that pass `filter`.
    pub selected: usize,
    /// Cursor in the worktrees pane: index into the selected repo's worktrees.
    pub selected_worktree: usize,
    /// `enter`: the selected worktree's older sessions are expanded.
    pub expanded: bool,
    pub pane: Pane,
    /// Case-insensitive substring over repo names and branches.
    pub filter: Option<String>,
    /// `/` was pressed and keys currently edit `filter`.
    pub filter_editing: bool,
    pub status_message: Option<String>,
    pub show_help: bool,
    pub no_color: bool,
    pub now: Timestamp,
    /// Repos found so far during a first scan; `None` once rows are available.
    pub scanning: Option<usize>,
}

impl Model {
    pub fn new(doc: StatusDoc, now: Timestamp, no_color: bool) -> Model {
        Model {
            doc,
            selected: 0,
            selected_worktree: 0,
            expanded: false,
            pane: Pane::Repos,
            filter: None,
            filter_editing: false,
            status_message: None,
            show_help: false,
            no_color,
            now,
            scanning: None,
        }
    }
}

const REPOS_PANE_WIDTH: u16 = 32;
const OLDER_PANE_HEIGHT: u16 = 5;
const HELP_WIDTH: u16 = 72;
const REPO_NAME_COLUMN: usize = 12;
const BRANCH_COLUMN: usize = 30;
const STATE_COLUMN: usize = 22;
const SECS_PER_MINUTE: i64 = 60;
const SECS_PER_HOUR: i64 = 3_600;
const SECS_PER_DAY: i64 = 86_400;

// §8.1 colours: unpushed yellow, dirty blue, stale / missing dim, error red.
const YELLOW: Style = Style::new().fg(Color::Yellow);
const BLUE: Style = Style::new().fg(Color::Blue);
const RED: Style = Style::new().fg(Color::Red);
const DIM: Style = Style::new().add_modifier(Modifier::DIM);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
const FOCUS: Style = Style::new().fg(Color::Cyan);

/// key, key-line label, help text, only shown when some source has data (§8.1).
const KEYS: [(&str, &str, &str, bool); 11] = [
    ("j/k", "move", "move the cursor (arrow keys too)", false),
    (
        "enter",
        "expand",
        "expand / collapse the selected worktree's sessions",
        false,
    ),
    (
        "tab",
        "pane",
        "switch between the repo and worktree panes",
        false,
    ),
    (
        "/",
        "filter",
        "filter repos and branches (esc clears)",
        false,
    ),
    (
        "R",
        "resume",
        "resume the selected worktree's last session",
        true,
    ),
    ("Y", "copy resume", "copy the resume command", true),
    ("c", "copy cd", "copy a cd command for the worktree", false),
    ("o", "open", "open the worktree in $VISUAL / $EDITOR", false),
    ("r", "rescan", "rescan the root", false),
    ("?", "help", "toggle this help", false),
    ("q", "quit", "quit", false),
];

/// `no_color` strips every style, including modifiers.
fn paint(model: &Model, style: Style) -> Style {
    if model.no_color {
        Style::default()
    } else {
        style
    }
}

fn span(model: &Model, text: impl Into<String>, style: Style) -> Span<'static> {
    Span::styled(text.into(), paint(model, style))
}

/// Repos that pass the filter: case-insensitive substring over names and branches.
pub fn visible_repos(model: &Model) -> Vec<&Repo> {
    let needle = model.filter.as_deref().map(str::to_lowercase);
    let matches = |text: &str| {
        needle
            .as_ref()
            .is_none_or(|n| text.to_lowercase().contains(n))
    };
    model
        .doc
        .repos
        .iter()
        .filter(|r| {
            matches(&r.name)
                || r.worktrees
                    .iter()
                    .any(|w| w.branch.as_deref().is_some_and(matches))
        })
        .collect()
}

/// §8.1: agent columns, older sessions, `R` and `Y` exist only when some source has data.
fn has_sessions(doc: &StatusDoc) -> bool {
    doc.repos
        .iter()
        .flat_map(|r| &r.worktrees)
        .any(|w| w.last_session.is_some() || w.older_sessions > 0)
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

pub fn draw(frame: &mut Frame, model: &Model) {
    let [header, body, keys] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let sessions = has_sessions(&model.doc);
    frame.render_widget(header_line(model), header);
    frame.render_widget(Paragraph::new("[r]escan").right_aligned(), header);
    match model.scanning {
        Some(found) if model.doc.repos.is_empty() => frame.render_widget(
            Paragraph::new(format!(" scanning… {found} repos found")),
            body,
        ),
        _ => draw_panes(frame, model, body, sessions),
    }
    frame.render_widget(bottom_line(model, sessions, keys.width), keys);
    if model.show_help {
        draw_help(frame, model, body, sessions);
    }
}

fn header_line(model: &Model) -> Paragraph<'static> {
    let doc = &model.doc;
    let worktrees: usize = doc.repos.iter().map(|r| r.worktrees.len()).sum();
    let scanned = match doc.scanned_at {
        Some(t) => format!("scanned {} ago", age(model.now, t)),
        None => "not scanned yet".to_owned(),
    };
    let mut spans = vec![Span::raw(format!(
        " rollcall ─ {} ─ {} repos · {worktrees} worktrees · {scanned}",
        sanitise(&doc.root),
        doc.repos.len()
    ))];
    if doc.stale {
        spans.push(span(model, " · stale", DIM));
    }
    Paragraph::new(Line::from(spans))
}

fn bottom_line(model: &Model, sessions: bool, width: u16) -> Paragraph<'static> {
    let text = if model.filter_editing {
        format!(
            " /{}",
            sanitise(model.filter.as_deref().unwrap_or_default())
        )
    } else if let Some(message) = &model.status_message {
        format!(" {}", sanitise(message))
    } else {
        let labels: Vec<String> = KEYS
            .iter()
            .filter(|(.., agent_only)| sessions || !agent_only)
            .map(|(key, label, ..)| format!("{key} {label}"))
            .collect();
        // The spec's dotted line is 123 columns, wider than the 120-column snapshot size.
        let dotted = format!(" {}", labels.join(" · "));
        if dotted.chars().count() <= usize::from(width) {
            dotted
        } else {
            format!(" {}", labels.join("  "))
        }
    };
    Paragraph::new(text)
}

fn draw_help(frame: &mut Frame, model: &Model, body: Rect, sessions: bool) {
    let lines: Vec<Line> = KEYS
        .iter()
        .filter(|(.., agent_only)| sessions || !agent_only)
        .map(|(key, _, help, _)| Line::from(format!(" {key:<7}{help}")))
        .collect();
    let height = (lines.len() as u16 + 2).min(body.height);
    let popup = Rect {
        x: body.x + body.width.saturating_sub(HELP_WIDTH) / 2,
        y: body.y + body.height.saturating_sub(height) / 2,
        width: HELP_WIDTH.min(body.width),
        height,
    };
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .title(" Help ")
        .border_style(paint(model, FOCUS));
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

fn draw_panes(frame: &mut Frame, model: &Model, body: Rect, sessions: bool) {
    let [left, right] =
        Layout::horizontal([Constraint::Length(REPOS_PANE_WIDTH), Constraint::Min(0)]).areas(body);
    let repos = visible_repos(model);
    let selected = model.selected.min(repos.len().saturating_sub(1));
    let border = |pane| {
        paint(
            model,
            if model.pane == pane {
                FOCUS
            } else {
                Style::new()
            },
        )
    };

    let title = match &model.filter {
        Some(f) => format!(" Repos /{} ", sanitise(f)),
        None => " Repos ".to_owned(),
    };
    let lines: Vec<Line> = if repos.is_empty() {
        let empty = if model.filter.is_some() {
            " no repos match"
        } else {
            " no repos found"
        };
        vec![Line::from(empty)]
    } else {
        repos
            .iter()
            .enumerate()
            .map(|(i, r)| repo_line(model, r, i == selected))
            .collect()
    };
    let block = Block::bordered()
        .title(title)
        .border_style(border(Pane::Repos));
    frame.render_widget(Paragraph::new(lines).block(block), left);

    let Some(repo) = repos.get(selected) else {
        frame.render_widget(
            Block::bordered().border_style(border(Pane::Worktrees)),
            right,
        );
        return;
    };
    let block = Block::bordered()
        .title(format!(" {} ", sanitise(&repo.name)))
        .border_style(border(Pane::Worktrees));
    let inner = block.inner(right);
    frame.render_widget(block, right);

    let wt_index = model
        .selected_worktree
        .min(repo.worktrees.len().saturating_sub(1));
    let older_for = repo
        .worktrees
        .get(wt_index)
        .filter(|w| sessions && w.older_sessions > 0);
    let older_height = if older_for.is_some() {
        OLDER_PANE_HEIGHT
    } else {
        0
    };
    let [list, older] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(older_height)]).areas(inner);

    let mut lines: Vec<Line> = Vec::new();
    if let Some(error) = &repo.error {
        lines.push(Line::from(span(
            model,
            format!(" error: {}", sanitise(error)),
            RED,
        )));
    }
    lines.extend(
        repo.worktrees
            .iter()
            .enumerate()
            .map(|(i, w)| worktree_line(model, w, model.pane == Pane::Worktrees && i == wt_index)),
    );
    frame.render_widget(Paragraph::new(lines), list);
    if let Some(w) = older_for {
        frame.render_widget(Paragraph::new(older_lines(model, w)), older);
    }
}

fn joined(parts: Vec<Span<'static>>) -> Vec<Span<'static>> {
    let mut out = Vec::with_capacity(parts.len() * 2);
    for part in parts {
        if !out.is_empty() {
            out.push(Span::raw(" "));
        }
        out.push(part);
    }
    out
}

fn count_badges(model: &Model, dirty: u32, unpushed: u32) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    if dirty > 0 {
        out.push(span(model, format!("●{dirty}"), BLUE));
    }
    if unpushed > 0 {
        out.push(span(model, format!("↑{unpushed}"), YELLOW));
    }
    out
}

/// No upstream but a remote exists (§3.3: `unpushed` is then counted, not null).
fn no_upstream(w: &Worktree) -> bool {
    w.upstream.is_none() && w.unpushed.is_some()
}

fn repo_line(model: &Model, repo: &Repo, selected: bool) -> Line<'static> {
    let wts = &repo.worktrees;
    let dirty = wts.iter().map(|w| w.dirty_count + w.untracked_count).sum();
    let unpushed = wts.iter().filter_map(|w| w.unpushed).sum();
    let stale = wts.iter().filter(|w| w.stale_reason.is_some()).count();
    let mut badges = Vec::new();
    if repo.error.is_some() {
        badges.push(span(model, "error", RED));
    }
    match repo.kind {
        RepoKind::Nested => badges.push(span(model, "nested", DIM)),
        RepoKind::Bare => badges.push(span(model, "bare", DIM)),
        RepoKind::Normal | RepoKind::ExternalMain => {}
    }
    badges.extend(count_badges(model, dirty, unpushed));
    if stale > 0 {
        badges.push(span(model, format!("{stale} stale"), DIM));
    }
    if wts.iter().any(no_upstream) {
        badges.push(span(model, "no-upstream", DIM));
    }
    if repo.attention == 0 {
        badges.push(span(
            model,
            rank::quiet(repo.last_activity_at, model.now),
            DIM,
        ));
    }
    let marker = if selected { "▶ " } else { "  " };
    let name = sanitise(&repo.name);
    let mut spans = vec![Span::raw(format!("{marker}{name:<REPO_NAME_COLUMN$} "))];
    spans.extend(joined(badges));
    row(model, spans, selected)
}

fn row(model: &Model, spans: Vec<Span<'static>>, selected: bool) -> Line<'static> {
    let line = Line::from(spans);
    if selected {
        line.style(paint(model, BOLD))
    } else {
        line
    }
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

fn state_badges(model: &Model, w: &Worktree) -> Vec<Span<'static>> {
    if let Some(error) = &w.error {
        return vec![span(model, format!("error: {}", sanitise(error)), RED)];
    }
    let mut badges = Vec::new();
    if w.external {
        badges.push(span(model, "ext", DIM));
    }
    if w.stale_reason == Some(StaleReason::Missing) {
        badges.push(span(model, "missing", DIM));
        return badges;
    }
    let counts = count_badges(
        model,
        w.dirty_count + w.untracked_count,
        w.unpushed.unwrap_or(0),
    );
    if counts.is_empty() && w.stale_reason.is_none() {
        badges.push(Span::raw("✓ clean"));
    }
    badges.extend(counts);
    match w.stale_reason {
        Some(StaleReason::Merged) => badges.push(span(model, "merged·stale", DIM)),
        Some(StaleReason::Inactive) => badges.push(span(model, "stale", DIM)),
        Some(StaleReason::Missing) | None => {}
    }
    if no_upstream(w) {
        badges.push(span(model, "no-upstream", DIM));
    }
    badges
}

fn agent_name(source: SourceKind) -> &'static str {
    match source {
        SourceKind::Claude => "claude",
        SourceKind::Codex => "codex",
    }
}

/// `last_session` inline (§8.1): agent, age, quoted title (the prompt when untitled).
fn session_text(model: &Model, s: &SessionOut) -> String {
    let what = s.title.as_deref().or(s.last_prompt.as_deref());
    let quoted = what
        .map(|t| format!(" \"{}\"", sanitise(t)))
        .unwrap_or_default();
    format!(
        "{} {}{quoted}",
        agent_name(s.source),
        age(model.now, s.ended_at)
    )
}

fn worktree_line(model: &Model, w: &Worktree, selected: bool) -> Line<'static> {
    let marker = if selected { "▶ " } else { "  " };
    let mut spans = vec![Span::raw(format!(
        "{marker}{:<BRANCH_COLUMN$} ",
        worktree_label(w)
    ))];
    let state = joined(state_badges(model, w));
    let used: usize = state.iter().map(|s| s.content.chars().count()).sum();
    spans.extend(state);
    if let Some(s) = &w.last_session {
        spans.push(Span::raw(" ".repeat(STATE_COLUMN.saturating_sub(used) + 1)));
        spans.push(Span::raw(session_text(model, s)));
    }
    row(model, spans, selected)
}

/// `StatusDoc` carries only a count of older sessions, so the pane shows that count; `enter`
/// adds the last session's details.
fn older_lines(model: &Model, w: &Worktree) -> Vec<Line<'static>> {
    let branch = worktree_label(w);
    let mut lines = vec![
        Line::from(span(model, format!(" Older sessions ({branch})"), BOLD)),
        Line::from(format!("  {} more", w.older_sessions)),
    ];
    if let (true, Some(s)) = (model.expanded, &w.last_session) {
        lines.push(Line::from(format!(
            "  last: {} {}",
            agent_name(s.source),
            s.id
        )));
        let prompt = s.last_prompt.as_deref().map(sanitise).unwrap_or_default();
        lines.push(Line::from(format!(
            "  {} messages \"{prompt}\"",
            s.message_count
        )));
    }
    lines
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::{Color, Modifier, Style};
    use rollcall_core::{
        Repo, RepoKind, SCHEMA_VERSION, SessionOut, SourceKind, StaleReason, TitleKind, Worktree,
    };

    use super::*;

    const WIDTH: u16 = 120;
    const HEIGHT: u16 = 32;
    const NOW: &str = "2026-09-29T00:00:00Z";

    fn now() -> Timestamp {
        NOW.parse().unwrap()
    }

    fn wt(branch: &str) -> Worktree {
        Worktree {
            path: format!("/srv/demo/{branch}"),
            is_main: false,
            external: false,
            branch: Some(branch.to_owned()),
            detached: false,
            head: Some("abc1234".to_owned()),
            last_commit_at: Some(now()),
            upstream: Some(format!("origin/{branch}")),
            ahead: Some(0),
            behind: Some(0),
            unpushed: Some(0),
            dirty_count: 0,
            untracked_count: 0,
            stale_reason: None,
            attention: 0,
            error: None,
            last_session: None,
            older_sessions: 0,
        }
    }

    fn repo(name: &str, worktrees: Vec<Worktree>) -> Repo {
        Repo {
            name: name.to_owned(),
            path: format!("/srv/demo/{name}"),
            kind: RepoKind::Normal,
            contains_root: false,
            default_branch: Some("main".to_owned()),
            has_remote: true,
            attention: 0,
            last_activity_at: Some(now()),
            error: None,
            worktrees,
            submodules: None,
        }
    }

    fn session(source: SourceKind, age_secs: i64, title: &str) -> SessionOut {
        let ended_at = Timestamp::from_second(now().as_second() - age_secs).unwrap();
        SessionOut {
            source,
            id: "11111111-2222-3333-4444-555555555555".to_owned(),
            branch: None,
            cwd: "/srv/demo/crm".to_owned(),
            started_at: ended_at,
            ended_at,
            title: Some(title.to_owned()),
            title_kind: Some(TitleKind::Ai),
            last_prompt: Some("please fix the flaky e2e".to_owned()),
            message_count: 12,
            resume: Some("cd -- '/srv/demo/crm' && claude --resume x".to_owned()),
        }
    }

    fn doc(repos: Vec<Repo>) -> StatusDoc {
        StatusDoc {
            schema_version: SCHEMA_VERSION,
            root: "/srv/demo".to_owned(),
            scanned_at: Some(now()),
            stale: false,
            partial: false,
            repos,
        }
    }

    fn with_sessions() -> Model {
        let mut main = wt("main");
        main.is_main = true;
        main.last_session = Some(session(SourceKind::Codex, 86_400, "fix flaky e2e"));
        let mut dirty = wt("ui-refine/css");
        dirty.dirty_count = 2;
        dirty.unpushed = Some(3);
        dirty.last_session = Some(session(SourceKind::Claude, 7_200, "consolidate dark mode"));
        dirty.older_sessions = 2;
        let mut merged = wt("chore/post-pr1");
        merged.stale_reason = Some(StaleReason::Merged);
        merged.last_session = Some(session(SourceKind::Claude, 259_200, "post-PR1 cleanup"));
        let mut gone = wt("spike");
        gone.path = "../tmp/crm-spike".to_owned();
        gone.external = true;
        gone.stale_reason = Some(StaleReason::Missing);
        let mut crm = repo("CRM", vec![main, dirty, merged, gone]);
        crm.attention = 3;
        let mut sutra_main = wt("main");
        sutra_main.is_main = true;
        sutra_main.upstream = None;
        sutra_main.dirty_count = 5;
        let mut sutra = repo("sutra", vec![sutra_main]);
        sutra.attention = 2;
        let mut agentrec_main = wt("main");
        agentrec_main.unpushed = Some(1);
        let mut agentrec = repo("agentrec", vec![agentrec_main]);
        agentrec.attention = 3;
        let mut games = repo("games", vec![wt("main")]);
        games.last_activity_at =
            Some(Timestamp::from_second(now().as_second() - 9 * 86_400).unwrap());
        let mut uv = repo("uv", vec![wt("main")]);
        uv.last_activity_at =
            Some(Timestamp::from_second(now().as_second() - 21 * 86_400).unwrap());
        let mut model = Model::new(doc(vec![crm, sutra, agentrec, games, uv]), now(), false);
        model.selected_worktree = 1;
        model
    }

    fn without_sessions() -> Model {
        let mut m = with_sessions();
        for r in &mut m.doc.repos {
            for w in &mut r.worktrees {
                w.last_session = None;
                w.older_sessions = 0;
            }
        }
        m
    }

    fn filter_active() -> Model {
        let mut m = with_sessions();
        m.filter = Some("SUT".to_owned());
        m
    }

    fn help_open() -> Model {
        let mut m = with_sessions();
        m.show_help = true;
        m
    }

    fn stale_header() -> Model {
        let mut m = with_sessions();
        m.doc.stale = true;
        m
    }

    fn scanning() -> Model {
        let mut m = Model::new(doc(vec![]), now(), false);
        m.doc.scanned_at = None;
        m.scanning = Some(7);
        m
    }

    fn render(model: &Model) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
        terminal.draw(|f| draw(f, model)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn rows(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| {
                let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
                line.trim_end().to_owned()
            })
            .collect()
    }

    fn text(buf: &Buffer) -> String {
        rows(buf).join("\n")
    }

    /// Style of the first cell of the first occurrence of `needle` (one cell per char).
    fn style_of(buf: &Buffer, needle: &str) -> Style {
        let want: Vec<String> = needle.chars().map(String::from).collect();
        for y in 0..buf.area.height {
            let syms: Vec<&str> = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
            for x in 0..=syms.len() - want.len() {
                if syms[x..x + want.len()]
                    .iter()
                    .zip(&want)
                    .all(|(a, b)| a == b)
                {
                    return buf[(x as u16, y)].style();
                }
            }
        }
        panic!("{needle:?} not on screen:\n{}", text(buf));
    }

    fn is_default_style(cell: &ratatui::buffer::Cell) -> bool {
        cell.fg == Color::Reset && cell.bg == Color::Reset && cell.modifier.is_empty()
    }

    fn all_models() -> Vec<Model> {
        vec![
            with_sessions(),
            without_sessions(),
            filter_active(),
            help_open(),
            stale_header(),
            scanning(),
        ]
    }

    #[test]
    fn snapshot_with_sessions() {
        insta::assert_snapshot!(text(&render(&with_sessions())));
    }

    #[test]
    fn snapshot_without_sessions() {
        insta::assert_snapshot!(text(&render(&without_sessions())));
    }

    #[test]
    fn snapshot_filter_active() {
        insta::assert_snapshot!(text(&render(&filter_active())));
    }

    #[test]
    fn snapshot_help_open() {
        insta::assert_snapshot!(text(&render(&help_open())));
    }

    #[test]
    fn snapshot_stale_header() {
        insta::assert_snapshot!(text(&render(&stale_header())));
    }

    #[test]
    fn snapshot_scanning() {
        insta::assert_snapshot!(text(&render(&scanning())));
    }

    #[test]
    fn colours_are_reset_or_named() {
        for m in all_models() {
            for cell in render(&m).content() {
                for c in [cell.fg, cell.bg] {
                    assert!(!matches!(c, Color::Rgb(..) | Color::Indexed(..)), "{c:?}");
                }
            }
        }
    }

    #[test]
    fn no_color_makes_every_style_default() {
        for mut m in all_models() {
            let coloured = render(&m);
            m.no_color = true;
            let plain = render(&m);
            assert_eq!(
                text(&coloured),
                text(&plain),
                "no_color changes styles only"
            );
            assert!(plain.content().iter().all(is_default_style));
        }
        let coloured = render(&with_sessions());
        assert!(!coloured.content().iter().all(is_default_style));
    }

    #[test]
    fn no_sessions_hides_agent_ui() {
        let buf = render(&without_sessions());
        let all = text(&buf);
        for absent in ["claude", "codex", "Older sessions"] {
            assert!(!all.contains(absent), "{absent} on screen:\n{all}");
        }
        let keys = rows(&buf).pop().unwrap();
        assert!(keys.contains("j/k move"), "{keys}");
        assert!(!keys.contains('R') && !keys.contains('Y'), "{keys}");
    }

    #[test]
    fn sessions_show_agents_older_and_resume_keys() {
        let buf = render(&with_sessions());
        let all = text(&buf);
        for present in ["claude 2h", "codex 1d", "Older sessions (ui-refine/css)"] {
            assert!(all.contains(present), "{present} missing:\n{all}");
        }
        let keys = rows(&buf).pop().unwrap();
        assert!(
            keys.contains("R resume") && keys.contains("Y copy resume"),
            "{keys}"
        );
        assert!(keys.ends_with("q quit"), "key line clipped: {keys}");
    }

    #[test]
    fn badge_colours() {
        let buf = render(&with_sessions());
        assert_eq!(style_of(&buf, "↑3").fg, Some(Color::Yellow));
        assert_eq!(style_of(&buf, "●2").fg, Some(Color::Blue));
        assert!(
            style_of(&buf, "merged·stale")
                .add_modifier
                .contains(Modifier::DIM)
        );
        assert!(
            style_of(&buf, "missing")
                .add_modifier
                .contains(Modifier::DIM)
        );
    }

    #[test]
    fn error_badge_is_red() {
        let mut m = with_sessions();
        m.doc.repos[0].worktrees[0].error = Some("open failed: boom".to_owned());
        let buf = render(&m);
        assert_eq!(style_of(&buf, "open failed: boom").fg, Some(Color::Red));
    }

    #[test]
    fn sanitising_payloads_never_reach_the_buffer() {
        let payload = "a\x1b[2Jb\u{9b}c\u{202e}d\u{200b}e";
        let mut m = with_sessions();
        m.doc.repos[0].worktrees[1].branch = Some(payload.to_owned());
        m.doc.repos[0].worktrees[1]
            .last_session
            .as_mut()
            .unwrap()
            .title = Some(payload.to_owned());
        let all = text(&render(&m));
        for bad in ["\x1b", "\u{9b}", "\u{202e}", "\u{200b}", "[2J"] {
            assert!(!all.contains(bad), "{bad:?} leaked:\n{all}");
        }
        assert!(all.contains("abcde"), "sanitised text missing:\n{all}");
    }

    #[test]
    fn stale_and_scanning_headers() {
        let stale = rows(&render(&stale_header()))[0].clone();
        assert!(
            stale.contains("scanned 0s ago") && stale.contains("stale"),
            "{stale}"
        );
        let fresh = rows(&render(&with_sessions()))[0].clone();
        assert!(
            fresh.contains("5 repos · 8 worktrees · scanned 0s ago"),
            "{fresh}"
        );
        assert!(!fresh.contains("stale"), "{fresh}");
        assert!(text(&render(&scanning())).contains("scanning… 7 repos found"));
    }

    #[test]
    fn filter_prompt_and_status_message_use_the_bottom_line() {
        let mut m = filter_active();
        m.filter_editing = true;
        assert_eq!(rows(&render(&m)).pop().unwrap(), " /SUT");
        m.filter_editing = false;
        m.status_message = Some("clipboard unavailable".to_owned());
        assert_eq!(rows(&render(&m)).pop().unwrap(), " clipboard unavailable");
    }

    #[test]
    fn filter_matches_repo_names_and_branches_case_insensitively() {
        let mut m = with_sessions();
        m.filter = Some("CSS".to_owned());
        let names: Vec<_> = visible_repos(&m).iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["CRM"]);
        m.filter = Some("utr".to_owned());
        let names: Vec<_> = visible_repos(&m).iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["sutra"]);
        m.filter = Some("zzz".to_owned());
        assert!(visible_repos(&m).is_empty());
        assert!(text(&render(&m)).contains("no repos match"));
    }

    #[test]
    fn out_of_range_selection_does_not_panic() {
        let mut m = with_sessions();
        m.selected = 99;
        m.selected_worktree = 99;
        m.pane = Pane::Worktrees;
        render(&m);
    }
}
