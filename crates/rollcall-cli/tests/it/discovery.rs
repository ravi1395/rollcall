//! Gates P32 (spec 12.1 #1, #2, #3, #9): discovery, git state, order, guards, through the real binary.
//!
//! Each world here is private to this test module and every run on it is serialised by `Private::gate`:
//! two scans of one root would race for the scan lock, and the loser answers `stale` from the cache.
use crate::build_sessions::{CodexMode, build_corpora};
use crate::support::{
    Corpus, RUN_DEADLINE, Run, Which, World, assert_error_prefix, expected, fresh, manifest,
    run_rollcall_with, validate,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Spec 12.1 #1: the symlink-loop run finishes within this.
const DISCOVERY_DEADLINE: Duration = Duration::from_secs(10);
/// Spec 10.2 Deadline: a budget no repo can meet.
const IMPOSSIBLE_REPO_BUDGET: &str = "repo_budget_secs = 0.000001\n";
/// Below the budget root's ~100 dirents (manifest note), so the walk stops early.
const SMALL_WALK_BUDGET: &str = "walk_budget_entries = 50\n";
const EXIT_REFUSED_ROOT: i32 = 3;
/// Spec 3.1: a completed scan with some error row.
const EXIT_ERROR_ROWS: i32 = 4;

/// A fixture world whose runs are serialised.
pub(crate) struct Private {
    pub w: World,
    gate: Mutex<()>,
}

impl Private {
    pub fn new(which: Which) -> Private {
        Private {
            w: fresh(which),
            gate: Mutex::new(()),
        }
    }

    pub fn run(&self, args: &[&str], env: &[(&str, &str)], deadline: Duration) -> Run {
        let _held = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        run_rollcall_with(&self.w, args, env, deadline)
    }

    pub fn status(&self, flags: &[&str]) -> (i32, Value) {
        let root = self.w.root.to_str().expect("utf-8 root");
        let mut args = vec!["status", "--json"];
        args.extend_from_slice(flags);
        args.push(root);
        json_of(self.run(&args, &[], RUN_DEADLINE))
    }
}

pub(crate) fn json_of(run: Run) -> (i32, Value) {
    let doc = serde_json::from_str(&run.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {:.300}\nstderr: {}",
            run.stdout, run.stderr
        )
    });
    (run.code, doc)
}

/// A private `F`, for the tests that scan it with default flags and no config.
pub(crate) fn own_f() -> &'static Private {
    static F: OnceLock<Private> = OnceLock::new();
    F.get_or_init(|| Private::new(Which::F))
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).expect("serialise JSON")
}

fn assert_json_eq(what: &str, actual: &Value, expected: &Value) {
    let (a, e) = (pretty(actual), pretty(expected));
    assert!(
        a == e,
        "{what}: actual != oracle\n--- actual\n{a}\n--- oracle\n{e}"
    );
}

/// Errors compare by prefix (spec 12.1 #1): checks each `error` in `actual` against the oracle's prefix,
/// then rewrites it to the oracle's text so the remaining comparison is exact.
fn normalise_errors(what: &str, actual: &mut Value, oracle: &Value) {
    match (actual, oracle) {
        (Value::Object(a), Value::Object(o)) => {
            for (key, value) in a.iter_mut() {
                let Some(want) = o.get(key) else { continue };
                if key == "error" {
                    assert_error_prefix(what, value, want);
                    *value = want.clone();
                } else {
                    normalise_errors(what, value, want);
                }
            }
        }
        (Value::Array(a), Value::Array(o)) => {
            for (value, want) in a.iter_mut().zip(o) {
                normalise_errors(what, value, want);
            }
        }
        _ => {}
    }
}

fn repos(doc: &Value) -> &Vec<Value> {
    doc["repos"].as_array().expect("status has repos")
}

fn worktrees(repo: &Value) -> &Vec<Value> {
    repo["worktrees"].as_array().expect("repo has worktrees")
}

fn pick(v: &Value, keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .map(|k| (k.to_string(), v[*k].clone()))
            .collect(),
    )
}

/// The spec 12.1 #1 `jq` projection.
fn discovery_projection(doc: &Value) -> Value {
    Value::Array(
        repos(doc)
            .iter()
            .map(|r| {
                let mut row = pick(r, &["path", "kind", "contains_root", "error"]);
                row["worktrees"] = Value::Array(
                    worktrees(r)
                        .iter()
                        .map(|w| pick(w, &["path", "external", "stale_reason", "error"]))
                        .collect(),
                );
                row
            })
            .collect(),
    )
}

fn assert_discovery(w: &World, doc: &Value, what: &str) {
    let oracle = expected("discovery", w);
    let mut actual = discovery_projection(doc);
    normalise_errors(what, &mut actual, &oracle);
    assert_json_eq(what, &actual, &oracle);
}

fn write_config(dir: &Path, toml: &str) {
    let cfg = dir.join("rollcall");
    std::fs::create_dir_all(&cfg).expect("create config dir");
    std::fs::write(cfg.join("config.toml"), toml).expect("write config");
}

// ------------------------------------------------------------------ #1

#[test]
fn discovery_matches_oracle() {
    let p = own_f();
    let (code, doc) = p.status(&[]);
    assert_eq!(
        code, EXIT_ERROR_ROWS,
        "F holds error rows, so the scan completes with exit 4"
    );
    assert_discovery(&p.w, &doc, "status --json $F");
}

#[test]
fn symlink_fanout_finishes_under_10s_in_both_modes() {
    let p = own_f();
    let root = p.w.root.to_str().expect("utf-8 root");
    for flags in [&[][..], &["--follow-symlinks"][..]] {
        let mut args = vec!["status", "--json"];
        args.extend_from_slice(flags);
        args.push(root);
        let start = Instant::now();
        // The watchdog kills and fails the test at the deadline; the elapsed check documents the bound.
        let (code, doc) = json_of(p.run(&args, &[], DISCOVERY_DEADLINE));
        assert!(
            start.elapsed() < DISCOVERY_DEADLINE,
            "{flags:?} took {:?}",
            start.elapsed()
        );
        assert_eq!(code, EXIT_ERROR_ROWS, "{flags:?}");
        // Rows reachable only through a symlink are not repos in either mode (manifest counting rule).
        assert_discovery(&p.w, &doc, &format!("status --json {flags:?} $F"));
    }
}

#[test]
fn submodules_only_with_the_flag() {
    let p = own_f();
    let children = ["sub-parent/libs/child", "sub-old-parent/vendor/lib"];
    let listed = |doc: &Value| -> BTreeSet<String> {
        repos(doc)
            .iter()
            .flat_map(|r| r["submodules"].as_array().into_iter().flatten())
            .map(|s| s["path"].as_str().expect("submodule path").to_string())
            .collect()
    };
    let (_, without) = p.status(&[]);
    assert!(
        listed(&without).is_empty(),
        "submodules without the flag: {:?}",
        listed(&without)
    );

    let (_, with) = p.status(&["--submodules"]);
    let want: BTreeSet<String> = children
        .iter()
        .map(|c| p.w.root.join(c).to_string_lossy().into_owned())
        .collect();
    assert_eq!(listed(&with), want);
    // Submodules are grouped under the parent, never rows of their own.
    assert_eq!(repos(&with).len(), repos(&without).len());
}

// ------------------------------------------------------------------ #2

#[test]
fn git_state_matches_status_oracle() {
    let p = own_f();
    let (_, doc) = p.status(&[]);
    let counts = &manifest().expect_counts;
    assert_eq!(repos(&doc).len(), counts.f_repos as usize, "repo rows");

    const KEYS: [&str; 12] = [
        "path",
        "branch",
        "detached",
        "upstream",
        "ahead",
        "behind",
        "unpushed",
        "dirty_count",
        "untracked_count",
        "stale_reason",
        "attention",
        "error",
    ];
    let by_path =
        |rows: &mut Vec<Value>| rows.sort_by_key(|r| r["path"].as_str().unwrap_or("").to_string());
    let mut actual: Vec<Value> = repos(&doc)
        .iter()
        .flat_map(worktrees)
        .map(|w| pick(w, &KEYS))
        .collect();
    assert_eq!(actual.len(), counts.f_worktrees as usize, "worktree rows");
    let mut oracle: Vec<Value> = expected("status", &p.w)
        .as_array()
        .expect("oracle array")
        .clone();
    by_path(&mut actual);
    by_path(&mut oracle);
    let (mut actual, oracle) = (Value::Array(actual), Value::Array(oracle));
    normalise_errors("status", &mut actual, &oracle);
    assert_json_eq("worktree git state", &actual, &oracle);

    let corrupted = repos(&doc)
        .iter()
        .find(|r| r["path"] == json!(p.w.root.join("corrupted")))
        .expect("corrupted repo row is present");
    assert!(
        corrupted["error"]
            .as_str()
            .is_some_and(|e| e.starts_with("open failed: "))
    );
    assert!(worktrees(corrupted).is_empty());
}

#[test]
fn deadline_run_times_out_every_repo() {
    let p = Private::new(Which::FClean);
    let scratch = p.w.scratch();
    write_config(scratch.path(), IMPOSSIBLE_REPO_BUDGET);
    let env = [("XDG_CONFIG_HOME", scratch.path().to_str().expect("utf-8"))];
    let root = p.w.root.to_str().expect("utf-8 root");

    let (code, doc) = json_of(p.run(&["status", "--json", root], &env, RUN_DEADLINE));
    assert_eq!(code, EXIT_ERROR_ROWS);
    assert!(
        !repos(&doc).is_empty(),
        "no repo rows, so nothing was checked"
    );
    for r in repos(&doc) {
        let error = r["error"].as_str().unwrap_or_default();
        assert!(
            error.starts_with("timed out"),
            "{}: error {:?}",
            r["path"],
            r["error"]
        );
    }

    // The deadline run's own cached result, not a second scan.
    let (_, doctor) = json_of(p.run(&["doctor", "--json", "--no-scan", root], &env, RUN_DEADLINE));
    let timeouts = doctor["repo_timeouts"].as_u64().expect("repo_timeouts");
    assert!(timeouts >= 1, "repo_timeouts = {timeouts}");
}

/// Spec 3.1, exit 4: `status` and `scan` on `F` (error rows), on `F_clean` (none), and the cache answers.
#[test]
fn error_rows_exit_4_and_a_clean_scan_exits_0() {
    let p = own_f();
    let root = p.w.root.to_str().expect("utf-8 root");

    let scan = p.run(&["scan", root], &[], RUN_DEADLINE);
    assert_eq!(scan.code, EXIT_ERROR_ROWS, "stderr: {}", scan.stderr);
    let line = scan.stderr.trim_end();
    assert_eq!(line.lines().count(), 1, "stderr: {}", scan.stderr);
    assert!(line.starts_with("scanned "), "stderr: {line}");
    let errors: u32 = line
        .strip_suffix(" errors)")
        .and_then(|head| head.rsplit_once('('))
        .and_then(|(_, n)| n.parse().ok())
        .unwrap_or_else(|| panic!("no `(<E> errors)` ending: {line}"));
    assert!(errors >= 1, "{line}");

    // A cache answer takes the same code from the stored rows.
    let (code, _) = p.status(&["--no-scan"]);
    assert_eq!(code, EXIT_ERROR_ROWS);

    let clean = Private::new(Which::FClean);
    let clean_root = clean.w.root.to_str().expect("utf-8 root");
    let (code, _) = clean.status(&[]);
    assert_eq!(code, 0);
    let scan = clean.run(&["scan", clean_root], &[], RUN_DEADLINE);
    assert_eq!(scan.code, 0, "stderr: {}", scan.stderr);
}

/// Spec 3.1: `doctor` keeps exiting 1 for error rows (the author named only `status` and `scan`).
#[test]
fn doctor_exits_1_for_error_rows() {
    let p = own_f();
    let root = p.w.root.to_str().expect("utf-8 root");
    for args in [&["doctor", root][..], &["doctor", "--json", root][..]] {
        let run = p.run(args, &[], RUN_DEADLINE);
        assert_eq!(run.code, 1, "{args:?}; stderr: {}", run.stderr);
    }
}

// ------------------------------------------------------------------ #3

#[test]
fn repo_order_and_activity_match_the_oracle() {
    let p = Private::new(Which::F);
    build_corpora(&p.w, &[Corpus::Matching], CodexMode::Db);
    let (_, doc) = p.status(&[]);
    let mut actual = Value::Array(
        repos(&doc)
            .iter()
            .map(|r| pick(r, &["path", "attention", "last_activity_at"]))
            .collect(),
    );
    let oracle = expected("order", &p.w);
    normalise_errors("order", &mut actual, &oracle);
    assert_json_eq("repo order", &actual, &oracle);
}

// ------------------------------------------------------------------ #9

#[test]
fn root_slash_is_refused() {
    let p = own_f();
    let run = p.run(&["status", "/"], &[], RUN_DEADLINE);
    assert_eq!(run.code, EXIT_REFUSED_ROOT, "stderr: {}", run.stderr);
    assert!(
        run.stderr.contains("refusing to scan"),
        "stderr: {}",
        run.stderr
    );
}

#[test]
fn a_home_root_is_refused_with_the_expected_suggestions() {
    let p = own_f();
    let home = p.w.sb.home.to_str().expect("utf-8 home");
    let run = p.run(&["status", home], &[], RUN_DEADLINE);
    assert_eq!(run.code, EXIT_REFUSED_ROOT, "stderr: {}", run.stderr);

    let listed: BTreeSet<String> = run
        .stderr
        .lines()
        .filter_map(|l| l.strip_prefix("  rollcall "))
        .map(str::to_string)
        .collect();
    let want: BTreeSet<String> = manifest()
        .home
        .expected_suggestions
        .iter()
        .map(|s| p.w.tokens.subst(s))
        .collect();
    assert_eq!(listed, want, "stderr: {}", run.stderr);

    // --yes is the only thing standing between this root and a scan.
    let confirmed = p.run(&["status", "--yes", home], &[], RUN_DEADLINE);
    assert_ne!(
        confirmed.code, EXIT_REFUSED_ROOT,
        "stderr: {}",
        confirmed.stderr
    );
}

#[test]
fn walk_budget_makes_the_scan_partial() {
    let p = own_f();
    let budget = p.w.base.join("budget");
    let root = budget.to_str().expect("utf-8 budget root");
    let scratch = p.w.scratch();
    write_config(scratch.path(), SMALL_WALK_BUDGET);
    let env = [("XDG_CONFIG_HOME", scratch.path().to_str().expect("utf-8"))];

    let (code, doc) = json_of(p.run(&["status", "--json", root], &env, RUN_DEADLINE));
    assert_eq!(doc["partial"], json!(true));
    assert_eq!(code, 1);

    // The same root under the default budget completes, so the flag above came from the budget.
    let (_, whole) = json_of(p.run(&["status", "--json", root], &[], RUN_DEADLINE));
    assert_eq!(whole["partial"], json!(false));
}

/// Spec 3.1: a partial walk wins over error rows (exit 1, not 4).
#[test]
fn a_partial_walk_with_an_error_row_exits_1() {
    let p = own_f();
    let scratch = p.w.scratch();
    write_config(scratch.path(), "walk_budget_entries = 5\n");
    let env = [("XDG_CONFIG_HOME", scratch.path().to_str().expect("utf-8"))];
    // A chain, so the walk order is fixed: root (1 entry), `bad` (`.git` and `sub`: the error row,
    // a repo whose gitdir does not exist, is found), then `sub` runs the budget out.
    let root = p.w.base.join("budget-error");
    let bad = root.join("bad");
    std::fs::create_dir_all(bad.join("sub")).expect("create dirs");
    std::fs::write(bad.join(".git"), "gitdir: ../does-not-exist\n").expect("write .git");
    for i in 0..10 {
        std::fs::write(bad.join(format!("sub/f{i}")), "x").expect("write filler");
    }
    let root = root.to_str().expect("utf-8 budget root");

    let (code, doc) = json_of(p.run(&["status", "--json", root], &env, RUN_DEADLINE));
    assert_eq!(doc["partial"], json!(true));
    assert!(
        repos(&doc).iter().any(|r| r["error"].is_string()),
        "no error row: {}",
        pretty(&doc)
    );
    assert_eq!(code, 1);
}

#[test]
fn a_root_inside_a_repo_lists_the_enclosing_repo() {
    let p = own_f();
    let enclosing = p.w.ext.join("enclosing");
    let root = enclosing.join("sub/dir");
    let run = p.run(
        &["status", "--json", root.to_str().expect("utf-8")],
        &[],
        RUN_DEADLINE,
    );
    let (_, doc) = json_of(run);
    let row = repos(&doc)
        .iter()
        .find(|r| r["path"] == json!(enclosing))
        .unwrap_or_else(|| panic!("no enclosing row in {}", pretty(&doc)));
    assert_eq!(row["contains_root"], json!(true));
}

// -------------------------------------------------------------- schemas

#[test]
fn status_and_doctor_json_validate_against_the_schemas() {
    let p = own_f();
    let root = p.w.root.to_str().expect("utf-8 root");
    let (_, status) = p.status(&[]);
    let (_, doctor) = json_of(p.run(&["doctor", "--json", root], &[], RUN_DEADLINE));
    for (schema, doc) in [("status", status), ("doctor", doctor)] {
        if let Err(errors) = validate(schema, &doc) {
            panic!(
                "{schema} --json violates schema/{schema}.json:\n{}",
                errors.join("\n")
            );
        }
    }
}
