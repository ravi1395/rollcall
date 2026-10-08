//! Gates for the session sources, matching and incremental reads (spec 12.1 #4, #5, #6, #7).
//!
//! #4 / #5: per-session fields come from `Claude` / `Codex` `scan` + `output::render_session` in-process (the
//! path `status` uses, plan § Fixture manifest schema), judged by `tests/expected/{claude,codex_db,codex_rollout}.json`;
//! source counts come from `doctor --json` of a real CLI scan. #6: `status --json` + `doctor --json` over the
//! `matching` corpus alone. #7: CLI scans with edits between, offsets read back from the cache through
//! `SqliteIndex::source_state`.
//!
//! Units that #7 cites rather than repeats: inode reuse and `parser_version` bumps are P22 unit tests that inject a
//! prior `FileState` (`jsonl::tests::plan_rereads_for_a_different_inode`, `plan_rereads_when_head_changes`,
//! `plan_rereads_on_older_parser_version`, `resolve_is_none_for_a_new_inode`).
use super::build_sessions::{CodexMode, build_big_log, build_corpora, log_bytes};
use super::support::{
    Corpus, Expect, Op, Run, Step, StepExpect, StepTo, Tokens, Which, World, expected, fresh,
    manifest, peak_rss, require_tool, run_rollcall, validate,
};
use rollcall_cli::output::{TextPolicy, render_session};
use rollcall_core::{Backing, Index, SourceKind, SourceState, TextRef};
use rollcall_sessions::{Claude, Codex, SessionSource};
use rollcall_store::SqliteIndex;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

const POLICY: TextPolicy = TextPolicy {
    prompts: true,
    redact: false,
};
/// Corpora of the #4 / #5 worlds: every corpus the source oracles count (`[expect_counts]`).
const RULES_AND_MORE: [Corpus; 4] = [
    Corpus::Matching,
    Corpus::ClaudeRules,
    Corpus::CodexRules,
    Corpus::Incremental,
];
const FALLBACK_CORPORA: [Corpus; 2] = [Corpus::CodexRules, Corpus::Incremental];
/// The per-session fields the source oracles pin (spec 12.1 #4).
const SESSION_FIELDS: [&str; 8] = [
    "title",
    "title_kind",
    "last_prompt",
    "branch",
    "cwd",
    "message_count",
    "started_at",
    "ended_at",
];
const TIME_TOOL: &str = "/usr/bin/time";
/// 12.1 #7: "< 48 MB", read as decimal megabytes (the stricter reading of MB / MiB).
const RSS_LIMIT_BYTES: u64 = 48_000_000;
const SCAN_DEADLINE: Duration = Duration::from_secs(120);
/// Codex DB file `state_5.sqlite`.
const CODEX_DB_VERSION: u64 = 5;

// ------------------------------------------------------------------ helpers

fn claude_source(w: &World) -> Claude {
    Claude::new(w.sb.claude_projects())
}

fn codex_source(w: &World) -> Codex {
    Codex::new(w.sb.codex_home.clone())
}

/// A scan root with no repos: sessions are read whatever the root is, and an empty one keeps git workers out of
/// the measurement and the run time.
fn empty_root(w: &World) -> PathBuf {
    let root = w.base.join("empty-root");
    fs::create_dir_all(&root).expect("create empty root");
    root
}

fn root_arg(root: &Path) -> &str {
    root.to_str().expect("temp paths are UTF-8")
}

fn parse(run: &Run, what: &str) -> Value {
    serde_json::from_str(&run.stdout).unwrap_or_else(|e| {
        panic!(
            "{what}: stdout is not JSON ({e}); exit {}, stderr: {}",
            run.code, run.stderr
        )
    })
}

fn valid(schema: &str, doc: Value) -> Value {
    if let Err(errors) = validate(schema, &doc) {
        panic!("{schema} document violates schema/{schema}.json: {errors:#?}");
    }
    doc
}

/// `rollcall doctor <root> --json [extra]`: a CLI scan (unless `--no-scan`) and the diagnostics of that scan.
fn doctor(w: &World, root: &Path, extra: &[&str]) -> Value {
    let mut args = vec!["doctor", root_arg(root), "--json"];
    args.extend_from_slice(extra);
    valid("doctor", parse(&run_rollcall(w, &args), "doctor"))
}

fn source<'a>(doc: &'a Value, name: &str) -> &'a Value {
    doc["sources"]
        .as_array()
        .and_then(|all| all.iter().find(|s| s["source"] == name))
        .unwrap_or_else(|| panic!("doctor has no `{name}` source: {doc}"))
}

fn num(v: &Value, key: &str) -> u64 {
    v[key]
        .as_u64()
        .unwrap_or_else(|| panic!("`{key}` is not a number in {v}"))
}

fn cache_index(w: &World) -> SqliteIndex {
    SqliteIndex::open(&rollcall_store::cache_dir(
        &w.sb.home,
        Some(&w.sb.xdg_cache),
    ))
    .expect("open the scan cache")
}

fn claude_file(w: &World, project_dir: &str, file: &str) -> PathBuf {
    w.sb.claude_projects().join(project_dir).join(file)
}

/// `rel` is under `$CODEX_HOME`, or an absolute token path (which `join` keeps).
fn rollout_file(w: &World, rel: &str) -> PathBuf {
    w.sb.codex_home.join(w.tokens.subst(rel))
}

// ------------------------------------------------------------- step ops

/// The byte offset just past the `lines`-th newline.
fn end_of_lines(bytes: &[u8], lines: u64) -> u64 {
    let mut seen = 0;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            seen += 1;
            if seen == lines {
                return i as u64 + 1;
            }
        }
    }
    panic!("the file has fewer than {lines} complete lines");
}

/// Applies one manifest step to `path`; returns where the log lives afterwards (`move` changes it).
fn apply_step(w: &World, path: &Path, step: &Step) -> PathBuf {
    let open = |o: &mut OpenOptions| {
        o.open(path)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    };
    match step.op {
        Op::Append => {
            let mut f = open(OpenOptions::new().append(true));
            f.write_all(&log_bytes(&w.tokens, &step.lines))
                .expect("append lines");
        }
        Op::Truncate => {
            let Some(StepTo::Count(keep)) = &step.to else {
                panic!("truncate needs a line count in `to`");
            };
            let cut = end_of_lines(&fs::read(path).expect("read log"), *keep);
            open(OpenOptions::new().write(true))
                .set_len(cut)
                .expect("truncate log");
        }
        Op::Replace => {
            // Create the replacement first, then rename over: ext4 reuses a freed inode (WORKER-RULES).
            let staged = path.with_extension("jsonl.new");
            fs::write(&staged, log_bytes(&w.tokens, &step.lines)).expect("write replacement");
            fs::rename(&staged, path).expect("rename replacement over the log");
        }
        Op::SymlinkSwap => {
            let real = path.with_extension("jsonl.real");
            fs::rename(path, &real).expect("move the real log aside");
            std::os::unix::fs::symlink(&real, path).expect("symlink the log path");
        }
        Op::Delete => fs::remove_file(path).expect("delete log"),
        Op::Move => {
            let Some(StepTo::Path(to)) = &step.to else {
                panic!("move needs a path in `to`");
            };
            let to = rollout_file(w, to);
            fs::create_dir_all(to.parent().expect("target has a parent")).expect("mkdir target");
            fs::rename(path, &to).expect("move log");
            return to;
        }
    }
    path.to_path_buf()
}

/// Steps up to and including the last one that scans: the state the oracles describe ("after the LAST scan").
fn scanned_prefix(steps: &[Step]) -> &[Step] {
    let end = steps.iter().rposition(|s| s.then_scan).map_or(0, |i| i + 1);
    &steps[..end]
}

/// Consecutive steps ending in a scan: what one CLI scan sees of an edit. Trailing no-scan steps are dropped.
fn groups(steps: &[Step]) -> Vec<&[Step]> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, step) in steps.iter().enumerate() {
        if step.then_scan {
            out.push(&steps[start..=i]);
            start = i + 1;
        }
    }
    out
}

/// Materialises `corpora`, then edits each log to the state its last scanning step leaves.
fn build_converged(corpora: &[Corpus], mode: CodexMode) -> World {
    let w = fresh(Which::F);
    build_corpora(&w, corpora, mode);
    let m = manifest();
    for r in m.claude.iter().filter(|r| corpora.contains(&r.corpus)) {
        let mut path = claude_file(&w, &r.project_dir, &r.file);
        for step in scanned_prefix(&r.steps) {
            path = apply_step(&w, &path, step);
        }
    }
    if mode == CodexMode::Fallback {
        for r in m
            .codex_rollout
            .iter()
            .filter(|r| corpora.contains(&r.corpus))
        {
            let mut path = rollout_file(&w, &r.file);
            for step in scanned_prefix(&r.steps) {
                path = apply_step(&w, &path, step);
            }
        }
    }
    w
}

// ------------------------------------------------------- shared #4 / #5 worlds

/// A converged world plus what one CLI scan and one in-process scan of it report. Built once per mode and read
/// by several tests.
struct Shared {
    w: World,
    doctor: Value,
    claude: Vec<Value>,
    codex: Vec<Value>,
}

fn rendered(source: &dyn SessionSource) -> Vec<Value> {
    source
        .scan(&SourceState::default())
        .sessions
        .iter()
        .map(|s| serde_json::to_value(render_session(s, source, POLICY)).expect("serialise"))
        .collect()
}

fn shared(corpora: &[Corpus], mode: CodexMode) -> Shared {
    let w = build_converged(corpora, mode);
    let doctor = doctor(&w, &empty_root(&w), &[]);
    let claude = rendered(&claude_source(&w));
    let codex = rendered(&codex_source(&w));
    Shared {
        w,
        doctor,
        claude,
        codex,
    }
}

fn db_world() -> &'static Shared {
    static DB: OnceLock<Shared> = OnceLock::new();
    DB.get_or_init(|| shared(&RULES_AND_MORE, CodexMode::Db))
}

fn fallback_world() -> &'static Shared {
    static FALLBACK: OnceLock<Shared> = OnceLock::new();
    FALLBACK.get_or_init(|| shared(&FALLBACK_CORPORA, CodexMode::Fallback))
}

fn ids(sessions: &[Value]) -> BTreeSet<String> {
    sessions
        .iter()
        .map(|s| s["id"].as_str().expect("session id").to_owned())
        .collect()
}

/// Every oracle row's pinned fields equal the rendered session with that id, and no other session exists.
fn assert_oracle(name: &str, w: &World, actual: &[Value]) {
    let oracle = expected(name, w);
    let rows = oracle.as_array().expect("oracle is an array");
    let by_id: BTreeMap<&str, &Value> = actual
        .iter()
        .map(|s| (s["id"].as_str().expect("session id"), s))
        .collect();
    let want: BTreeSet<String> = rows
        .iter()
        .map(|r| r["id"].as_str().expect("oracle id").to_owned())
        .collect();
    assert_eq!(ids(actual), want, "{name}: session id sets differ");
    for row in rows {
        let id = row["id"].as_str().expect("oracle id");
        for field in SESSION_FIELDS {
            assert!(
                row.get(field).is_some(),
                "{name} {id}: oracle lacks {field}"
            );
            assert_eq!(by_id[id][field], row[field], "{name} {id}: {field}");
        }
    }
}

fn manifest_ids(
    rows: impl Iterator<Item = (&'static str, &'static Option<String>)>,
) -> Vec<&'static str> {
    rows.filter(|(_, excluded)| excluded.is_some())
        .map(|(id, _)| id)
        .collect()
}

/// Fixture rows named `not-a-uuid` / `subagent-a1` are not session ids.
fn is_uuid(id: &str) -> bool {
    id.len() == 36
}

// ------------------------------------------------------------- #4 Claude

#[test]
fn claude_sessions_equal_the_oracle() {
    let s = db_world();
    assert_eq!(
        s.claude.len() as u32,
        manifest().expect_counts.claude_sessions
    );
    assert_oracle("claude", &s.w, &s.claude);
}

#[test]
fn claude_source_counts_equal_the_manifest() {
    let m = manifest();
    let claude = source(&db_world().doctor, "claude");
    let excluded = |prefix: &str| {
        m.claude
            .iter()
            .filter(|r| RULES_AND_MORE.contains(&r.corpus))
            .filter(|r| r.excluded.as_deref().is_some_and(|e| e.starts_with(prefix)))
            .count() as u64
    };
    assert_eq!(claude["mode"], "jsonl");
    assert_eq!(
        num(claude, "sessions"),
        u64::from(m.expect_counts.claude_sessions)
    );
    assert_eq!(num(claude, "excluded_children"), excluded("subagent file"));
    assert_eq!(num(&claude["skipped"], "non_uuid"), excluded("non-UUID"));
    // The spec's per-fixture skip counts (claude.json `_why`): one oversize line, two malformed, two unknown types.
    assert_eq!(num(&claude["skipped"], "oversize"), 1);
    assert_eq!(num(&claude["skipped"], "malformed"), 2);
    assert_eq!(num(&claude["skipped"], "unknown_type"), 2);
    assert_eq!(num(claude, "vanished"), 0);
}

// -------------------------------------------------------- #5 Codex, both modes

#[test]
fn codex_database_sessions_equal_the_oracle() {
    let s = db_world();
    assert_eq!(
        s.codex.len() as u32,
        manifest().expect_counts.codex_db_sessions
    );
    assert_oracle("codex_db", &s.w, &s.codex);
}

#[test]
fn codex_database_counts_children_and_the_outside_rollout() {
    let m = manifest();
    let codex = source(&db_world().doctor, "codex");
    let threads = || {
        m.codex_thread
            .iter()
            .filter(|t| RULES_AND_MORE.contains(&t.corpus))
    };
    assert_eq!(codex["mode"], "db");
    assert_eq!(num(codex, "db_version"), CODEX_DB_VERSION);
    assert_eq!(
        num(codex, "sessions"),
        u64::from(m.expect_counts.codex_db_sessions)
    );
    assert_eq!(
        num(codex, "excluded_children"),
        threads().filter(|t| t.excluded.is_some()).count() as u64
    );
    let outside = threads()
        .filter(|t| {
            t.rollout_path
                .as_deref()
                .is_some_and(|p| p.starts_with("$EXT"))
        })
        .count() as u64;
    assert_eq!(
        outside, 1,
        "the manifest has one thread with an outside rollout"
    );
    assert_eq!(num(&codex["skipped"], "bad_rollout_path"), outside);
}

#[test]
fn codex_rollout_fallback_sessions_equal_the_oracle() {
    let s = fallback_world();
    assert_eq!(
        s.codex.len() as u32,
        manifest().expect_counts.codex_rollout_sessions
    );
    assert_oracle("codex_rollout", &s.w, &s.codex);
}

#[test]
fn codex_rollout_fallback_reports_its_mode_and_children() {
    let m = manifest();
    let doc = &fallback_world().doctor;
    let codex = source(doc, "codex");
    let children = m
        .codex_rollout
        .iter()
        .filter(|r| FALLBACK_CORPORA.contains(&r.corpus))
        .filter(|r| !r.file.starts_with("$EXT"))
        .filter(|r| {
            r.excluded
                .as_deref()
                .is_some_and(|e| e.starts_with("child"))
        })
        .count() as u64;
    assert_eq!(codex["mode"], "rollouts");
    assert_eq!(
        num(codex, "sessions"),
        u64::from(m.expect_counts.codex_rollout_sessions)
    );
    assert_eq!(num(codex, "excluded_children"), children);
    assert!(
        doc["notes"].as_array().is_some_and(|n| n
            .iter()
            .any(|n| n.as_str().is_some_and(|n| n.contains("no thread database")))),
        "doctor must say why the database was not used: {}",
        doc["notes"]
    );
}

#[test]
fn excluded_fixtures_yield_no_session() {
    let m = manifest();
    let db = db_world();
    let claude = manifest_ids(
        m.claude
            .iter()
            .filter(|r| RULES_AND_MORE.contains(&r.corpus))
            .map(|r| (r.id.as_str(), &r.excluded)),
    );
    let children = manifest_ids(m.codex_thread.iter().map(|t| (t.id.as_str(), &t.excluded)));
    assert!(claude.iter().any(|id| is_uuid(id)) && !children.is_empty());
    let (claude_ids, codex_ids) = (ids(&db.claude), ids(&db.codex));
    for id in claude.iter().filter(|id| is_uuid(id)) {
        assert!(
            !claude_ids.contains(*id),
            "excluded Claude fixture {id} became a session"
        );
    }
    for id in &children {
        assert!(
            !codex_ids.contains(*id),
            "excluded Codex thread {id} became a session"
        );
    }
    let rollouts = manifest_ids(m.codex_rollout.iter().map(|r| (r.id.as_str(), &r.excluded)));
    let fallback_ids = ids(&fallback_world().codex);
    for id in &rollouts {
        assert!(
            !fallback_ids.contains(*id),
            "excluded rollout {id} became a session"
        );
    }
}

// ------------------------------------------------------------ #6 matching

struct Matched {
    w: World,
    status: Value,
    doctor: Value,
}

/// The `matching` corpus alone (Codex in database mode, no `--submodules`), scanned over the real root `F`.
fn matched_world() -> &'static Matched {
    static MATCHED: OnceLock<Matched> = OnceLock::new();
    MATCHED.get_or_init(|| {
        let w = fresh(Which::F);
        build_corpora(&w, &[Corpus::Matching], CodexMode::Db);
        let status = valid(
            "status",
            parse(
                &run_rollcall(&w, &["status", root_arg(&w.root), "--json"]),
                "status",
            ),
        );
        // `status` ran the scan; `--no-scan` reads back that scan's diagnostics.
        let doctor = doctor(&w, &w.root, &["--no-scan"]);
        Matched { w, status, doctor }
    })
}

fn worktrees<'a>(repos: &'a Value, out: &mut Vec<&'a Value>) {
    for repo in repos.as_array().expect("repos array") {
        out.extend(repo["worktrees"].as_array().expect("worktrees array"));
        if let Some(subs) = repo.get("submodules") {
            worktrees(subs, out);
        }
    }
}

#[test]
fn every_worktree_has_the_oracle_last_session_and_older_count() {
    let m = matched_world();
    let oracle = expected("matching", &m.w);
    let mut all = Vec::new();
    worktrees(&m.status["repos"], &mut all);
    let by_path: BTreeMap<&str, &Value> = all
        .iter()
        .map(|w| (w["path"].as_str().expect("worktree path"), *w))
        .collect();
    let rows = oracle["worktrees"].as_array().expect("oracle worktrees");
    // Every mismatch is collected, so one run shows the whole diff.
    let mut problems = Vec::new();
    let mut hosts = BTreeSet::new();
    let mut matched = 0;
    for row in rows {
        let path = row["path"].as_str().expect("oracle path");
        hosts.insert(path);
        matched += 1 + num(row, "older_sessions");
        let Some(got) = by_path.get(path) else {
            problems.push(format!("{path}: no such worktree in status"));
            continue;
        };
        let (id, older) = (&got["last_session"]["id"], &got["older_sessions"]);
        if *id != row["last_session"]["id"] || *older != row["older_sessions"] {
            problems.push(format!(
                "{path}: last_session.id/older_sessions are {id}/{older}, oracle {}/{}",
                row["last_session"]["id"], row["older_sessions"]
            ));
        }
    }
    // Every other worktree hosts no matched session (matching.json `_doc`).
    for (path, wt) in &by_path {
        if !hosts.contains(path)
            && (!wt["last_session"].is_null() || num(wt, "older_sessions") != 0)
        {
            problems.push(format!(
                "{path}: hosts {} (+{} older), oracle says none; worktree error {}",
                wt["last_session"]["id"], wt["older_sessions"], wt["error"]
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "matching.json mismatches:\n{}",
        problems.join("\n")
    );
    assert_eq!(matched, u64::from(manifest().matching.matched));
    assert!(
        rows.iter().any(|r| num(r, "older_sessions") >= 1),
        "the oracle must include a worktree with older_sessions >= 1"
    );
}

#[test]
fn doctor_reports_the_unmatched_classes_and_the_matched_total() {
    let m = matched_world();
    let want = &manifest().matching;
    let sum = |field: &dyn Fn(&Value) -> u64| -> u64 {
        m.doctor["sources"]
            .as_array()
            .expect("sources array")
            .iter()
            .map(field)
            .sum()
    };
    let unmatched = |class: &'static str| sum(&|s: &Value| num(&s["unmatched"], class));
    let got = json!({
        "matched": sum(&|s: &Value| num(s, "matched")),
        "unmatched": {
            "non_repo": unmatched("non_repo"),
            "path_gone": unmatched("path_gone"),
            "outside_root": unmatched("outside_root"),
        }
    });
    let oracle = expected("matching", &m.w);
    let manifest_says = json!({
        "non_repo": want.non_repo,
        "path_gone": want.path_gone,
        "outside_root": want.outside_root
    });
    assert_eq!(
        oracle["unmatched"], manifest_says,
        "matching.json vs [matching]"
    );
    assert_eq!(
        got,
        json!({ "matched": want.matched, "unmatched": manifest_says }),
        "doctor matched / unmatched vs the manifest"
    );
}

// ------------------------------------------------------------ #7 incremental

/// One incremental fixture log and the scan groups its steps form.
struct Row {
    id: &'static str,
    kind: SourceKind,
    file: PathBuf,
    groups: Vec<&'static [Step]>,
}

fn incremental_rows(w: &World, kind: SourceKind) -> Vec<Row> {
    let m = manifest();
    let mut rows = Vec::new();
    match kind {
        SourceKind::Claude => {
            for r in m.claude.iter().filter(|r| r.corpus == Corpus::Incremental) {
                rows.push(Row {
                    id: &r.id,
                    kind,
                    file: claude_file(w, &r.project_dir, &r.file),
                    groups: groups(&r.steps),
                });
            }
        }
        SourceKind::Codex => {
            for r in m
                .codex_rollout
                .iter()
                .filter(|r| r.corpus == Corpus::Incremental)
            {
                rows.push(Row {
                    id: &r.id,
                    kind,
                    file: rollout_file(w, &r.file),
                    groups: groups(&r.steps),
                });
            }
        }
        SourceKind::Copilot => unreachable!("no Copilot fixtures before P3"),
    }
    rows.retain(|r| !r.groups.is_empty());
    rows
}

fn kind_name(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Claude => "claude",
        SourceKind::Codex => "codex",
        SourceKind::Copilot => "copilot",
    }
}

fn resolve_text(w: &World, kind: SourceKind, r: &TextRef) -> Option<String> {
    match kind {
        SourceKind::Claude => claude_source(w).resolve(r),
        SourceKind::Codex => codex_source(w).resolve(r),
        SourceKind::Copilot => unreachable!("no Copilot fixtures before P3"),
    }
}

fn file_identity(path: &Path) -> (u64, u64) {
    let meta = fs::metadata(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    (meta.dev(), meta.ino())
}

/// The manifest's offsets are token-free: each incremental log has one `$F` in its first line.
fn offset_with_token(w: &World, offset: u64) -> u64 {
    if offset == 0 {
        0
    } else {
        offset + w.root.as_os_str().len() as u64
    }
}

/// `session` (a rendered `SessionOut`) equals the manifest's `expect` (an absent key is null).
fn assert_expect(what: &str, session: &Value, e: &Expect, tokens: &Tokens) {
    let want = json!({
        "title": e.title,
        "title_kind": e.title_kind,
        "last_prompt": e.last_prompt,
        "branch": e.branch,
        "cwd": e.cwd.as_deref().map(|c| tokens.subst(c)),
        "message_count": e.message_count,
        "started_at": e.started_at,
        "ended_at": e.ended_at,
    });
    for field in SESSION_FIELDS {
        assert_eq!(session[field], want[field], "{what}: {field}");
    }
}

fn step_expect(group: &[Step]) -> &StepExpect {
    group
        .last()
        .and_then(|s| s.expect.as_ref())
        .expect("a scanning step has an expect")
}

/// One initial scan, then one CLI scan per group round (every row's n-th group is applied before the n-th scan).
fn run_rounds(w: &World, rows: &[Row], kind: SourceKind) {
    let root = empty_root(w);
    doctor(w, &root, &[]);
    let mut files: Vec<PathBuf> = rows.iter().map(|r| r.file.clone()).collect();
    let rounds = rows.iter().map(|r| r.groups.len()).max().expect("rows");
    for round in 0..rounds {
        let mut before = BTreeMap::new();
        let (mut records, mut vanished) = (0, 0);
        for (i, row) in rows.iter().enumerate() {
            let Some(group) = row.groups.get(round) else {
                continue;
            };
            before.insert(i, file_identity(&files[i]));
            for step in *group {
                files[i] = apply_step(w, &files[i], step);
            }
            let e = step_expect(group);
            records += e.records.expect("records");
            vanished += e.vanished.expect("vanished");
        }
        let diag = doctor(w, &root, &[]);
        let diag = source(&diag, kind_name(kind));
        assert_eq!(
            num(diag, "records"),
            records,
            "round {round}: records the scan read"
        );
        assert_eq!(
            num(diag, "vanished"),
            vanished,
            "round {round}: vanished sessions"
        );
        let state = cache_index(w).source_state(kind);
        for (i, identity) in before {
            let e = step_expect(rows[i].groups[round]);
            assert_round_state(w, &rows[i], &files[i], identity, &state, e, round);
        }
    }
}

fn assert_round_state(
    w: &World,
    row: &Row,
    file: &Path,
    identity: (u64, u64),
    state: &SourceState,
    e: &StepExpect,
    round: usize,
) {
    let what = format!("{} round {round}", row.id);
    let sessions: Vec<_> = state
        .sessions
        .iter()
        .filter(|s| s.id.as_str() == row.id)
        .collect();
    assert_eq!(
        sessions.len() as u64,
        e.sessions.expect("sessions"),
        "{what}: sessions"
    );
    let file_state = state.files.iter().find(|f| (f.dev, f.inode) == identity);
    let Some(session) = sessions.first() else {
        assert!(
            file_state.is_none(),
            "{what}: a dropped session keeps no file state"
        );
        assert_eq!(
            e.offset,
            Some(0),
            "{what}: manifest expects offset 0 for a dropped file"
        );
        return;
    };
    let Backing::LogFile { dev, inode, path } = &session.backing else {
        panic!("{what}: a log session is backed by a log file");
    };
    assert_eq!(
        (*dev, *inode),
        identity,
        "{what}: the session follows the same inode"
    );
    assert_eq!(path, file, "{what}: the session's path follows the log");
    let f = file_state.unwrap_or_else(|| panic!("{what}: no file state for the log"));
    assert_eq!(f.path, file, "{what}: file state path");
    assert_eq!(
        f.offset,
        offset_with_token(w, e.offset.expect("offset")),
        "{what}: offset"
    );
    if let Some(count) = e.message_count {
        assert_eq!(
            u64::from(session.message_count),
            count,
            "{what}: message_count"
        );
    }
    if let (Some(title), Some((r, kind))) = (&e.title, &session.title) {
        assert_eq!(
            resolve_text(w, row.kind, r).as_ref(),
            Some(title),
            "{what}: title text"
        );
        assert_eq!(
            Some(serde_json::to_value(kind).expect("kind")),
            e.title_kind.clone().map(Value::from),
            "{what}: title kind"
        );
    }
}

/// After the rounds, fixture `only`'s merged session equals its manifest `expect` (absent when it has none).
fn assert_final_session(w: &World, kind: SourceKind, only: &str) {
    let m = manifest();
    let state = cache_index(w).source_state(kind);
    let rows: Vec<(&str, Option<&Expect>)> = match kind {
        SourceKind::Claude => m
            .claude
            .iter()
            .filter(|r| r.id == only)
            .map(|r| (r.id.as_str(), r.expect.as_ref()))
            .collect(),
        SourceKind::Codex => m
            .codex_rollout
            .iter()
            .filter(|r| r.id == only)
            .map(|r| (r.id.as_str(), r.expect.as_ref()))
            .collect(),
        SourceKind::Copilot => unreachable!("no Copilot fixtures before P3"),
    };
    let (claude, codex) = (claude_source(w), codex_source(w));
    let source: &dyn SessionSource = match kind {
        SourceKind::Claude => &claude,
        SourceKind::Codex => &codex,
        SourceKind::Copilot => unreachable!("no Copilot fixtures before P3"),
    };
    assert_eq!(rows.len(), 1, "no fixture {only}");
    for (id, expect) in rows {
        let session = state.sessions.iter().find(|s| s.id.as_str() == id);
        let Some(e) = expect else {
            assert!(
                session.is_none(),
                "{id}: an excluded fixture became a session"
            );
            continue;
        };
        let session = session.unwrap_or_else(|| panic!("{id}: no session in the cache"));
        let out = serde_json::to_value(render_session(session, source, POLICY)).expect("serialise");
        assert_expect(id, &out, e, &w.tokens);
    }
}

/// One incremental fixture in its own world: an initial CLI scan, then one CLI scan per step group with the edit
/// applied before it (every group's expected records, offset, sessions, vanished as in the manifest).
fn assert_incremental(kind: SourceKind, id: &str) {
    let w = fresh(Which::F);
    build_corpora(&w, &[Corpus::Incremental], CodexMode::Fallback);
    let rows: Vec<Row> = incremental_rows(&w, kind)
        .into_iter()
        .filter(|r| r.id == id)
        .collect();
    assert_eq!(rows.len(), 1, "{id} has no scanning steps");
    run_rounds(&w, &rows, kind);
    assert_final_session(&w, kind, id);
}

#[test]
fn appended_lines_and_a_partial_tail_leave_the_offset_before_the_partial() {
    assert_incremental(SourceKind::Claude, "c1a00003-0000-4000-8000-000000000001");
}

#[test]
fn a_truncated_log_is_read_again_from_the_start() {
    assert_incremental(SourceKind::Claude, "c1a00003-0000-4000-8000-000000000002");
}

#[test]
fn a_log_truncated_then_regrown_past_its_offset_is_read_again_by_fingerprint() {
    assert_incremental(SourceKind::Claude, "c1a00003-0000-4000-8000-000000000003");
}

#[test]
fn an_appended_custom_title_replaces_the_prompt_title() {
    assert_incremental(SourceKind::Claude, "c1a00003-0000-4000-8000-000000000004");
}

#[test]
fn a_deleted_log_drops_its_session_and_counts_it_vanished() {
    assert_incremental(SourceKind::Claude, "c1a00003-0000-4000-8000-000000000005");
}

#[test]
fn a_rollout_append_then_archive_move_continues_incrementally_on_the_same_inode() {
    assert_incremental(SourceKind::Codex, "c0d00003-0000-4000-8000-000000000001");
}

// ------------------------------------------------------------ #7 pointers

/// A `then_scan = false` step with `resolves = false`: refs captured by the last scan must resolve to null.
fn assert_pointer_goes_null(id: &str, op: Op) {
    let w = fresh(Which::F);
    build_corpora(&w, &[Corpus::Incremental], CodexMode::Fallback);
    let row = manifest()
        .claude
        .iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("no fixture {id}"));
    let step = row.steps.first().expect("pointer fixture has a step");
    assert!(
        step.op == op
            && !step.then_scan
            && step.expect.as_ref().and_then(|e| e.resolves) == Some(false),
        "{id} is not the expected no-scan pointer step"
    );
    doctor(&w, &empty_root(&w), &[]);
    let state = cache_index(&w).source_state(SourceKind::Claude);
    let session = state
        .sessions
        .iter()
        .find(|s| s.id.as_str() == id)
        .unwrap_or_else(|| panic!("{id}: no session in the cache"));
    let refs: Vec<&TextRef> = session
        .title
        .iter()
        .map(|(r, _)| r)
        .chain(session.last_prompt.iter())
        .collect();
    assert_eq!(
        refs.len(),
        2,
        "{id}: the fixture has a title and a last prompt"
    );
    let text = row.expect.as_ref().and_then(|e| e.title.clone());
    let claude = claude_source(&w);
    // Positive control: before the edit the refs resolve to the fixture text, so the null below is the edit's doing.
    for r in &refs {
        assert_eq!(
            claude.resolve(r),
            text,
            "{id}: ref resolves before the edit"
        );
    }
    apply_step(&w, &claude_file(&w, &row.project_dir, &row.file), step);
    for r in &refs {
        assert_eq!(
            claude.resolve(r),
            None,
            "{id}: ref must resolve to null after {op:?}"
        );
    }
}

#[test]
fn a_log_replaced_after_the_scan_resolves_to_null() {
    assert_pointer_goes_null("c1a00003-0000-4000-8000-000000000006", Op::Replace);
}

#[test]
fn a_symlink_swapped_in_after_the_scan_resolves_to_null() {
    assert_pointer_goes_null("c1a00003-0000-4000-8000-000000000007", Op::SymlinkSwap);
}

// ------------------------------------------------------------ #7 memory

#[test]
fn a_64_mib_single_line_log_scans_under_the_rss_bound() {
    if require_tool(TIME_TOOL).is_none() {
        return;
    }
    let w = fresh(Which::F);
    build_big_log(&w);
    let big = &manifest().big_log[0];
    let log = fs::read_dir(w.sb.claude_projects().join("big-log"))
        .expect("big-log dir")
        .next()
        .expect("big log file")
        .expect("dir entry")
        .path();
    assert_eq!(fs::metadata(&log).expect("stat big log").len(), big.bytes);
    let root = empty_root(&w);
    let (out, rss) = peak_rss(
        w.sb.rollcall(&["scan", root_arg(&root)], &[]),
        SCAN_DEADLINE,
    );
    eprintln!(
        "peak RSS of `rollcall scan` over a {} byte line: {rss} bytes",
        big.bytes
    );
    assert!(
        out.status.success(),
        "scan failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        rss < RSS_LIMIT_BYTES,
        "peak RSS {rss} bytes >= {RSS_LIMIT_BYTES}"
    );
    // Positive control: the scan opened the file and found no session in it, so the bound is not vacuous. (The one
    // unterminated line is a partial tail, spec 6.3: not read past and not counted as oversize, see
    // `jsonl::tests::oversize_partial_tail_is_not_counted_and_offset_stays_before_it`.)
    let diag = doctor(&w, &root, &["--no-scan"]);
    let claude = source(&diag, "claude");
    assert_eq!(num(claude, "files"), 1);
    assert_eq!(num(claude, "sessions"), 0);
    assert_eq!(num(claude, "records"), 0);
}
