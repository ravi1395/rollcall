//! §4.4 hardened open and the §4.1 worktree trust rule.

use std::fs;
use std::path::{Path, PathBuf};

use rollcall_core::GitOptions;

use crate::classify::{GitFile, resolve_git_file};

const OPEN_FAILED: &str = "open failed: ";
const UNTRUSTED_WORKTREE: &str = "untrusted worktree path";

/// Config sections whose every subsection can name an external program.
const STRIPPED_SECTIONS: [&str; 3] = ["filter", "diff", "merge"];
/// Keys of `[core]` that redirect reads or name a program (§4.4 Config stripping).
const STRIPPED_CORE_KEYS: [&str; 6] = [
    "attributesFile",
    "excludesFile",
    "worktree",
    "sshCommand",
    "hooksPath",
    "fsmonitor",
];
const NO_EXCLUDES: &str = "/dev/null";
/// `gitoxide.objects.allocLimit` = 64 MiB, in bytes.
const ALLOC_LIMIT_BYTES: u64 = 64 * 1024 * 1024;

/// `Options::isolated()` (not just `Config::isolated()`, see the gix audit note N1) plus the
/// open-time overrides. `forced_trust` exists only for tests; production always passes `None`
/// so gix derives trust from ownership.
fn hardened_options(forced_trust: Option<gix::sec::Trust>) -> gix::open::Options {
    let options = gix::open::Options::isolated()
        .open_path_as_is(true)
        .config_overrides([
            "core.commitGraph=false".to_string(),
            format!("gitoxide.objects.allocLimit={ALLOC_LIMIT_BYTES}"),
        ]);
    match forced_trust {
        Some(trust) => options.with(trust),
        None => options,
    }
}

/// Opens `git_dir` and commits the stripped config snapshot (§4.4). Err is a full row error.
fn open_with(
    git_dir: &Path,
    opts: &GitOptions,
    forced_trust: Option<gix::sec::Trust>,
) -> Result<gix::Repository, String> {
    let open_failed =
        |e: &dyn std::fmt::Display| format!("{OPEN_FAILED}{}: {e}", git_dir.display());
    let mut repo =
        gix::open_opts(git_dir, hardened_options(forced_trust)).map_err(|e| open_failed(&e))?;
    let mut snapshot = repo.config_snapshot_mut();
    strip_dangerous_config(&mut snapshot);
    let excludes = opts
        .excludes_file
        .as_deref()
        .unwrap_or(Path::new(NO_EXCLUDES));
    snapshot
        .set_raw_value("core.excludesFile", gix::path::into_bstr(excludes).as_ref())
        .map_err(|e| open_failed(&e))?;
    snapshot.commit().map_err(|e| open_failed(&e))?;
    Ok(repo)
}

/// Removes every `filter` / `diff` / `merge` section (one removal call removes one section, so
/// loop over ids) and the [`STRIPPED_CORE_KEYS`] from every `[core]` section, whatever its source.
fn strip_dangerous_config(snapshot: &mut gix::config::SnapshotMut<'_>) {
    let section_named = |names: &[&str], header_name: String| {
        names.iter().any(|n| header_name.eq_ignore_ascii_case(n))
    };
    let doomed: Vec<_> = snapshot
        .sections_and_ids()
        .filter(|(s, _)| section_named(&STRIPPED_SECTIONS, s.header().name().to_string()))
        .map(|(_, id)| id)
        .collect();
    for id in doomed {
        snapshot.remove_section_by_id(id);
    }
    let core_ids: Vec<_> = snapshot
        .sections_and_ids()
        .filter(|(s, _)| section_named(&["core"], s.header().name().to_string()))
        .map(|(_, id)| id)
        .collect();
    for id in core_ids {
        let Some(mut section) = snapshot.section_mut_by_id(id) else {
            continue;
        };
        for key in STRIPPED_CORE_KEYS {
            while section.remove(key).is_some() {}
        }
    }
}

/// The §4.4 hardened open. Production never forces `git_dir_trust`: gix determines ownership
/// trust, and a repo owned by another uid opens at reduced trust.
pub fn open_hardened(git_dir: &Path, opts: &GitOptions) -> Result<gix::Repository, String> {
    open_with(git_dir, opts, None)
}

/// Test seam: the same open at a forced trust level (P20 forces `Reduced`).
#[cfg(test)]
pub fn open_hardened_with_trust(
    git_dir: &Path,
    opts: &GitOptions,
    trust: gix::sec::Trust,
) -> Result<gix::Repository, String> {
    open_with(git_dir, opts, Some(trust))
}

/// Realpath where the path exists, else the path itself (§4.3: lexical fallback).
fn real(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// §4.1 worktree trust rule (a)-(d); rule (b) is also checked before open, so this is the
/// post-open defence in depth. `Err` is the fixed `untrusted worktree path` row error.
pub fn check_trust(
    repo: &gix::Repository,
    found_workdir: Option<&Path>,
    enumerating_git_dir: &Path,
    linked_admin: Option<&Path>,
    opts: &GitOptions,
) -> Result<(), String> {
    let workdir = repo.workdir().map(real);
    let rules_hold = workdir == found_workdir.map(real)
        && real(repo.common_dir()) == real(enumerating_git_dir)
        && linked_admin.is_none_or(|admin| {
            workdir
                .as_deref()
                .is_some_and(|wd| dot_git_points_to(wd, admin))
        })
        && workdir
            .as_deref()
            .is_none_or(|wd| !forbidden_workdir(wd, opts));
    if rules_hold {
        Ok(())
    } else {
        Err(UNTRUSTED_WORKTREE.to_string())
    }
}

/// Rule (c): `<workdir>/.git` is a file whose `gitdir:` resolves back to `admin`.
fn dot_git_points_to(workdir: &Path, admin: &Path) -> bool {
    let target = match resolve_git_file(&workdir.join(".git")) {
        Ok(GitFile::Linked { admin_dir, .. }) => admin_dir,
        Ok(GitFile::Submodule { git_dir, .. } | GitFile::Other { git_dir }) => git_dir,
        Err(_) => return false,
    };
    target == real(admin)
}

/// Rule (d): `/`, `$HOME`, a strict ancestor of the root, or under a skipped path.
fn forbidden_workdir(workdir: &Path, opts: &GitOptions) -> bool {
    let root = real(&opts.root);
    workdir == Path::new("/")
        || workdir == real(&opts.home)
        || (workdir != root && root.starts_with(workdir))
        || opts
            .skip_paths
            .iter()
            .any(|skip| workdir.starts_with(real(skip)))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    use gix::sec::Trust;
    use gix::status::plumbing::index_as_worktree::EntryStatus;
    use gix::status::plumbing::index_as_worktree::traits::{FastEq, SubmoduleStatus};
    use gix::status::plumbing::index_as_worktree_with_renames::{Entry, VisitEntry};

    use super::*;
    use crate::testgit::{add_worktree, git, init_bare, init_repo, tempdir_real, within};

    const UNTRUSTED: &str = "untrusted worktree path";
    const MARKER: &str = "marker-filter-x";

    fn git_opts(root: &Path, home: &Path) -> GitOptions {
        GitOptions {
            root: root.to_path_buf(),
            home: home.to_path_buf(),
            skip_paths: Vec::new(),
            repo_budget: Duration::from_secs(5),
            excludes_file: None,
        }
    }

    /// Options under which no rule (d) condition can fire for repos under `base`.
    fn benign(base: &Path) -> GitOptions {
        git_opts(base, &base.join("nowhere-home"))
    }

    fn open(git_dir: &Path, opts: &GitOptions) -> gix::Repository {
        open_hardened(git_dir, opts).expect("hardened open")
    }

    fn section_count(repo: &gix::Repository, name: &str) -> usize {
        repo.config_snapshot()
            .plumbing()
            .sections()
            .filter(|s| s.header().name().to_string().eq_ignore_ascii_case(name))
            .count()
    }

    fn raw(repo: &gix::Repository, key: &str) -> Option<String> {
        repo.config_snapshot()
            .plumbing()
            .raw_value(key)
            .ok()
            .map(|v| v.to_string())
    }

    fn seed_hostile_config(dir: &Path) {
        for (key, value) in [
            ("filter.a.clean", "evil-a"),
            ("filter.b.smudge", "evil-b"),
            ("diff.a.command", "evil-a"),
            ("diff.b.textconv", "evil-b"),
            ("merge.a.driver", "evil-a"),
            ("merge.b.name", "evil-b"),
        ] {
            git(dir, &["config", key, value]);
        }
    }

    // ---- stripping ----------------------------------------------------------------------

    #[test]
    fn strips_every_filter_diff_and_merge_section() {
        let (_t, base) = tempdir_real();
        let repo_dir = base.join("r");
        init_repo(&repo_dir);
        seed_hostile_config(&repo_dir);
        // Sanity: real git sees the sections, so the zero below is stripping, not absence.
        assert!(git(&repo_dir, &["config", "--list"]).contains("filter.b.smudge=evil-b"));

        let repo = open(&repo_dir.join(".git"), &benign(&base));

        for name in ["filter", "diff", "merge"] {
            assert_eq!(section_count(&repo, name), 0, "[{name}] survived stripping");
        }
        // Stripping only what it should: other repo-local sections stay.
        assert!(section_count(&repo, "core") > 0);
    }

    #[test]
    fn strips_sections_that_arrive_via_config_worktree() {
        let (_t, base) = tempdir_real();
        let repo_dir = base.join("r");
        init_repo(&repo_dir);
        git(&repo_dir, &["config", "extensions.worktreeConfig", "true"]);
        std::fs::write(
            repo_dir.join(".git/config.worktree"),
            "[filter \"w\"]\n\tclean = evil-w\n[Diff \"w\"]\n\tcommand = evil-w\n",
        )
        .expect("write config.worktree");

        let repo = open(&repo_dir.join(".git"), &benign(&base));

        assert_eq!(section_count(&repo, "filter"), 0);
        assert_eq!(section_count(&repo, "diff"), 0);
    }

    #[test]
    fn strips_the_six_dangerous_core_keys() {
        let (_t, base) = tempdir_real();
        let repo_dir = base.join("r");
        init_repo(&repo_dir);
        let own_workdir = repo_dir.to_str().expect("utf-8");
        for (key, value) in [
            ("core.attributesFile", "/evil/attributes"),
            ("core.excludesFile", "/evil/excludes"),
            ("core.worktree", own_workdir),
            ("core.sshCommand", "evil-ssh"),
            ("core.hooksPath", "/evil/hooks"),
            ("core.fsmonitor", "evil-monitor"),
        ] {
            git(&repo_dir, &["config", key, value]);
        }
        // A second occurrence of one key: stripping must not stop after the first.
        git(
            &repo_dir,
            &["config", "--add", "core.hooksPath", "/evil/hooks2"],
        );

        let repo = open(&repo_dir.join(".git"), &benign(&base));

        for key in [
            "core.attributesFile",
            "core.sshCommand",
            "core.hooksPath",
            "core.fsmonitor",
            "core.worktree",
        ] {
            assert_eq!(raw(&repo, key), None, "{key} survived stripping");
        }
        // core.excludesFile is stripped and then re-set to the override, never the seeded value.
        assert_eq!(
            raw(&repo, "core.excludesFile").as_deref(),
            Some("/dev/null")
        );
    }

    #[test]
    fn strips_core_keys_and_sections_whatever_their_case() {
        let (_t, base) = tempdir_real();
        let repo_dir = base.join("r");
        init_repo(&repo_dir);
        let config_path = repo_dir.join(".git/config");
        let mut config = std::fs::read_to_string(&config_path).expect("read config");
        config.push_str(
            "[CORE]\n\tAttributesFile = /evil/a\n\tHOOKSPATH = /evil/h\n\tfsMonitor = evil\n\
             \tSSHcommand = evil\n\tExcludesFile = /evil/e\n[FILTER \"z\"]\n\tCLEAN = evil\n",
        );
        std::fs::write(&config_path, config).expect("write config");

        let repo = open(&repo_dir.join(".git"), &benign(&base));

        for key in [
            "core.attributesFile",
            "core.hooksPath",
            "core.fsmonitor",
            "core.sshCommand",
        ] {
            assert_eq!(raw(&repo, key), None, "{key} survived stripping");
        }
        assert_eq!(
            raw(&repo, "core.excludesFile").as_deref(),
            Some("/dev/null")
        );
        assert_eq!(section_count(&repo, "filter"), 0);
    }

    #[test]
    fn excludes_file_is_the_given_override() {
        let (_t, base) = tempdir_real();
        let repo_dir = base.join("r");
        init_repo(&repo_dir);
        git(
            &repo_dir,
            &["config", "core.excludesFile", "/evil/excludes"],
        );
        let mut opts = benign(&base);
        opts.excludes_file = Some(base.join("user-ignore"));

        let repo = open(&repo_dir.join(".git"), &opts);

        assert_eq!(
            raw(&repo, "core.excludesFile"),
            Some(base.join("user-ignore").display().to_string())
        );
    }

    #[test]
    fn excludes_file_is_dev_null_without_an_override() {
        let (_t, base) = tempdir_real();
        let repo_dir = base.join("r");
        init_repo(&repo_dir);
        let repo = open(&repo_dir.join(".git"), &benign(&base));
        assert_eq!(
            raw(&repo, "core.excludesFile").as_deref(),
            Some("/dev/null")
        );
    }

    #[test]
    fn commit_graph_is_off_and_alloc_limit_is_64_mib() {
        let (_t, base) = tempdir_real();
        let repo_dir = base.join("r");
        init_repo(&repo_dir);
        git(&repo_dir, &["config", "core.commitGraph", "true"]);
        git(&repo_dir, &["config", "gitoxide.objects.allocLimit", "1g"]);

        let repo = open(&repo_dir.join(".git"), &benign(&base));
        let snapshot = repo.config_snapshot();

        assert_eq!(snapshot.boolean("core.commitGraph"), Some(false));
        assert_eq!(
            snapshot.integer("gitoxide.objects.allocLimit"),
            Some(64 * 1024 * 1024)
        );
    }

    #[test]
    fn open_options_are_fully_isolated() {
        // Audit note N1: only `Options::isolated()` denies environment access.
        assert!(hardened_options(None).permissions.is_isolated());
    }

    // ---- gix-level positive control -----------------------------------------------------

    #[derive(Clone)]
    struct NoSubmodules;

    impl SubmoduleStatus for NoSubmodules {
        type Output = ();

        fn status(
            &mut self,
            _entry: &gix::index::Entry,
            _rela_path: &gix::bstr::BStr,
        ) -> gix::ExnResult<Option<()>> {
            Ok(None)
        }
    }

    #[derive(Default)]
    struct CountModified(usize);

    impl<'index> VisitEntry<'index> for CountModified {
        type ContentChange = ();
        type SubmoduleStatus = ();

        fn visit_entry(&mut self, entry: Entry<'index, (), ()>) {
            if let Entry::Modification {
                status: EntryStatus::Change(_),
                ..
            } = entry
            {
                self.0 += 1;
            }
        }
    }

    /// The call pattern P20 uses for `dirty_count`.
    fn modified_tracked_files(repo: &gix::Repository) -> usize {
        let index = repo.index_or_empty().expect("index");
        let mut sink = CountModified::default();
        repo.index_worktree_status(
            &index,
            Vec::<gix::bstr::BString>::new(),
            &mut sink,
            FastEq,
            NoSubmodules,
            &mut gix::progress::Discard,
            &AtomicBool::new(false),
            gix::status::index_worktree::Options::default(),
        )
        .expect("index_worktree_status");
        sink.0
    }

    /// A repo whose tracked `a.txt` was edited after checkout to same-size content (no size
    /// short-circuit), with repo-local filter and diff drivers that touch marker files.
    fn drivers_repo(base: &Path) -> (PathBuf, PathBuf) {
        let repo_dir = base.join("drivers");
        let markers = base.join("markers");
        std::fs::create_dir_all(&markers).expect("markers dir");
        init_repo(&repo_dir);
        std::fs::write(repo_dir.join("a.txt"), "aaaa\n").expect("write a.txt");
        git(&repo_dir, &["add", "a.txt"]);
        git(&repo_dir, &["commit", "-q", "-m", "a"]);

        let script = |name: &str, body: &str| {
            let path = base.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
            std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .expect("chmod script");
            path.display().to_string()
        };
        let filter = script(
            "filter.sh",
            &format!("touch {}/{MARKER}\ncat", markers.display()),
        );
        let diff = script(
            "diff.sh",
            &format!("touch {}/marker-diff-x", markers.display()),
        );
        git(&repo_dir, &["config", "filter.x.clean", &filter]);
        git(&repo_dir, &["config", "filter.x.smudge", &filter]);
        git(&repo_dir, &["config", "diff.x.command", &diff]);
        std::fs::write(repo_dir.join(".gitattributes"), "* filter=x diff=x\n")
            .expect("write .gitattributes");
        std::fs::write(repo_dir.join("a.txt"), "bbbb\n").expect("edit a.txt");
        (repo_dir, markers)
    }

    #[test]
    fn positive_control_default_gix_runs_the_filter_driver() {
        let (_t, base) = tempdir_real();
        let (repo_dir, markers) = drivers_repo(&base);
        within({
            let repo_dir = repo_dir.clone();
            move || modified_tracked_files(&gix::open(&repo_dir).expect("default open"))
        });

        assert!(
            markers.join(MARKER).exists(),
            "default gix did not fire the filter driver: the control fixture proves nothing"
        );
    }

    #[test]
    fn hardened_repository_never_runs_the_filter_driver() {
        let (_t, base) = tempdir_real();
        let (repo_dir, markers) = drivers_repo(&base);
        let opts = benign(&base);

        // `gix::Repository` is not `Send`, so the watchdog thread does the open itself.
        let modified = within(move || modified_tracked_files(&open(&repo_dir.join(".git"), &opts)));

        // The compare really ran (a.txt is reported), yet nothing was spawned.
        assert_eq!(modified, 1);
        assert!(!markers.join(MARKER).exists(), "filter driver ran");
        assert!(!markers.join("marker-diff-x").exists(), "diff driver ran");
    }

    // ---- trust rule ---------------------------------------------------------------------

    #[test]
    fn trust_accepts_a_plain_repo_and_a_bare_repo() {
        let (_t, base) = tempdir_real();
        let main = base.join("main");
        init_repo(&main);
        let bare = base.join("bare.git");
        init_bare(&bare);
        let opts = benign(&base);

        let repo = open(&main.join(".git"), &opts);
        assert_eq!(
            check_trust(&repo, Some(&main), &main.join(".git"), None, &opts),
            Ok(())
        );
        let repo = open(&bare, &opts);
        assert_eq!(check_trust(&repo, None, &bare, None, &opts), Ok(()));
    }

    #[test]
    fn trust_accepts_a_linked_worktree() {
        let (_t, base) = tempdir_real();
        let main = base.join("main");
        let linked = base.join("wt");
        init_repo(&main);
        add_worktree(&main, &linked, "feature");
        let admin = main.join(".git/worktrees/wt");
        let opts = benign(&base);

        let repo = open(&admin, &opts);

        assert_eq!(
            repo.workdir().map(|p| p.canonicalize().unwrap()),
            Some(linked.clone())
        );
        assert_eq!(
            check_trust(
                &repo,
                Some(&linked),
                &main.join(".git"),
                Some(&admin),
                &opts
            ),
            Ok(())
        );
    }

    #[test]
    fn trust_a_core_worktree_redirect_is_untrusted() {
        let (_t, base) = tempdir_real();
        let main = base.join("main");
        let elsewhere = base.join("elsewhere");
        init_repo(&main);
        std::fs::create_dir_all(&elsewhere).expect("elsewhere");
        git(
            &main,
            &["config", "core.worktree", elsewhere.to_str().unwrap()],
        );
        let opts = benign(&base);

        let repo = open(&main.join(".git"), &opts);

        // gix resolved the redirect at open time, before rollcall stripped `core.worktree`;
        // stripping does not make rule (a) moot because the check reads the resolved workdir.
        assert_eq!(
            repo.workdir().map(|p| p.canonicalize().unwrap()),
            Some(elsewhere)
        );
        assert_eq!(
            check_trust(&repo, Some(&main), &main.join(".git"), None, &opts),
            Err(UNTRUSTED.to_string())
        );
    }

    #[test]
    fn trust_a_core_worktree_pointing_at_home_is_untrusted() {
        let (_t, base) = tempdir_real();
        let main = base.join("main");
        let home = base.join("home");
        init_repo(&main);
        std::fs::create_dir_all(&home).expect("home dir");
        git(&main, &["config", "core.worktree", home.to_str().unwrap()]);
        let opts = git_opts(&base.join("scan-root"), &home);

        let repo = open(&main.join(".git"), &opts);

        assert_eq!(
            check_trust(&repo, Some(&main), &main.join(".git"), None, &opts),
            Err(UNTRUSTED.to_string())
        );
    }

    #[test]
    fn trust_b_commondir_redirect_is_untrusted() {
        let (_t, base) = tempdir_real();
        let main = base.join("main");
        let other = base.join("other");
        let linked = base.join("wt");
        init_repo(&main);
        init_repo(&other);
        add_worktree(&main, &linked, "feature");
        let admin = main.join(".git/worktrees/wt");
        std::fs::write(
            admin.join("commondir"),
            format!("{}\n", other.join(".git").display()),
        )
        .expect("redirect commondir");
        let opts = benign(&base);

        let repo = open(&admin, &opts);

        assert_eq!(
            repo.common_dir().canonicalize().unwrap(),
            other.join(".git").canonicalize().unwrap()
        );
        assert_eq!(
            check_trust(
                &repo,
                Some(&linked),
                &main.join(".git"),
                Some(&admin),
                &opts
            ),
            Err(UNTRUSTED.to_string())
        );
    }

    #[test]
    fn trust_c_gitdir_mismatch_is_untrusted() {
        let (_t, base) = tempdir_real();
        let main = base.join("main");
        let linked = base.join("wt");
        let decoy = base.join("decoy-wt");
        init_repo(&main);
        add_worktree(&main, &linked, "feature");
        add_worktree(&main, &decoy, "decoy");
        let admin = main.join(".git/worktrees/wt");
        // The worktree's `.git` file points at a different admin dir than the one enumerated.
        std::fs::write(
            linked.join(".git"),
            format!(
                "gitdir: {}\n",
                main.join(".git/worktrees/decoy-wt").display()
            ),
        )
        .expect("rewrite .git file");
        let opts = benign(&base);

        let repo = open(&admin, &opts);

        assert_eq!(
            check_trust(
                &repo,
                Some(&linked),
                &main.join(".git"),
                Some(&admin),
                &opts
            ),
            Err(UNTRUSTED.to_string())
        );
    }

    #[test]
    fn trust_c_missing_dot_git_file_is_untrusted() {
        let (_t, base) = tempdir_real();
        let main = base.join("main");
        let linked = base.join("wt");
        init_repo(&main);
        add_worktree(&main, &linked, "feature");
        let admin = main.join(".git/worktrees/wt");
        std::fs::remove_file(linked.join(".git")).expect("remove .git file");
        let opts = benign(&base);

        let repo = open(&admin, &opts);

        assert_eq!(
            check_trust(
                &repo,
                Some(&linked),
                &main.join(".git"),
                Some(&admin),
                &opts
            ),
            Err(UNTRUSTED.to_string())
        );
    }

    #[test]
    fn trust_d_workdir_equal_to_home_is_untrusted() {
        let (_t, base) = tempdir_real();
        let main = base.join("main");
        init_repo(&main);
        let opts = git_opts(&base.join("scan-root"), &main);

        let repo = open(&main.join(".git"), &opts);

        assert_eq!(
            check_trust(&repo, Some(&main), &main.join(".git"), None, &opts),
            Err(UNTRUSTED.to_string())
        );
    }

    #[test]
    fn trust_d_workdir_that_is_an_ancestor_of_the_root_is_untrusted() {
        let (_t, base) = tempdir_real();
        let main = base.join("main");
        let root = main.join("sub/dir");
        init_repo(&main);
        std::fs::create_dir_all(&root).expect("root dir");
        let opts = git_opts(&root, &base.join("nowhere-home"));

        let repo = open(&main.join(".git"), &opts);

        assert_eq!(
            check_trust(&repo, Some(&main), &main.join(".git"), None, &opts),
            Err(UNTRUSTED.to_string())
        );
    }

    #[test]
    fn trust_d_workdir_equal_to_the_root_is_trusted() {
        let (_t, base) = tempdir_real();
        let main = base.join("main");
        init_repo(&main);
        let opts = git_opts(&main, &base.join("nowhere-home"));

        let repo = open(&main.join(".git"), &opts);

        assert_eq!(
            check_trust(&repo, Some(&main), &main.join(".git"), None, &opts),
            Ok(())
        );
    }

    #[test]
    fn trust_d_workdir_under_a_skip_path_is_untrusted() {
        let (_t, base) = tempdir_real();
        let skipped = base.join("Library");
        let main = skipped.join("main");
        init_repo(&main);
        let mut opts = benign(&base);
        opts.skip_paths = vec![skipped];

        let repo = open(&main.join(".git"), &opts);

        assert_eq!(
            check_trust(&repo, Some(&main), &main.join(".git"), None, &opts),
            Err(UNTRUSTED.to_string())
        );
    }

    #[test]
    fn trust_d_workdir_beside_a_skip_path_is_trusted() {
        let (_t, base) = tempdir_real();
        let main = base.join("Library-not/main");
        init_repo(&main);
        let mut opts = benign(&base);
        opts.skip_paths = vec![base.join("Library")];

        let repo = open(&main.join(".git"), &opts);

        assert_eq!(
            check_trust(&repo, Some(&main), &main.join(".git"), None, &opts),
            Ok(())
        );
    }

    // ---- ownership / trust level --------------------------------------------------------

    /// The text of `fn <name>` up to its closing brace at column 0.
    fn function_body(source: &str, signature: &str) -> String {
        let start = source.find(signature).expect("function present");
        let rest = &source[start..];
        rest[..rest.find("\n}\n").expect("function end")].to_string()
    }

    #[test]
    fn production_open_never_forces_a_trust_level() {
        // `git_dir_trust` is crate-private in gix and cannot be read back, so this pins the
        // production entry point's source: it hands `None` to the shared opener and names no
        // `Trust`, while the only forcing path is the `#[cfg(test)]` function.
        let source = include_str!("trust.rs");
        let production = function_body(source, "pub fn open_hardened(");
        assert!(!production.contains("Trust"), "{production}");
        assert!(production.contains("None"), "{production}");
        let forced = source
            .split("#[cfg(test)]\npub fn open_hardened_with_trust")
            .count();
        assert_eq!(forced, 2, "the trust-forcing seam must be cfg(test)-gated");

        // And behaviourally: a repo owned by this user opens at ownership-derived trust.
        let (_t, base) = tempdir_real();
        let main = base.join("main");
        init_repo(&main);
        let repo = open(&main.join(".git"), &benign(&base));
        assert_eq!(repo.git_dir_trust(), Trust::Full);
    }

    #[test]
    fn forced_reduced_trust_drops_repo_local_config_values() {
        let (_t, base) = tempdir_real();
        let main = base.join("main");
        init_repo(&main);
        git(&main, &["config", "user.name", "repo-local-value"]);
        let opts = benign(&base);

        let full = open_hardened(&main.join(".git"), &opts).expect("open");
        let reduced =
            open_hardened_with_trust(&main.join(".git"), &opts, Trust::Reduced).expect("open");

        // gix keeps repo-local sections in the resolved config at reduced trust and filters
        // them per lookup, so the trust-aware accessor is the one that shows what gix uses.
        let trusted = |repo: &gix::Repository, key: &str| {
            repo.config_snapshot()
                .plumbing()
                .string_filter(key, &mut gix::config::section::is_trusted)
                .map(|v| v.to_string())
        };
        // Control: the value is usable at full trust, so its absence below is the trust level.
        assert_eq!(
            trusted(&full, "user.name").as_deref(),
            Some("repo-local-value")
        );
        assert_eq!(reduced.git_dir_trust(), Trust::Reduced);
        assert_eq!(trusted(&reduced, "user.name"), None);
        // The rollcall overrides are not repo-local, so they survive reduced trust.
        assert_eq!(
            trusted(&reduced, "core.commitGraph").as_deref(),
            Some("false")
        );
        assert_eq!(
            trusted(&reduced, "core.excludesFile").as_deref(),
            Some("/dev/null")
        );
    }

    #[test]
    fn open_failure_is_a_full_row_error() {
        let (_t, base) = tempdir_real();
        let main = base.join("main");
        init_repo(&main);
        git(&main, &["config", "core.repositoryformatversion", "7"]);

        let err = within({
            let git_dir = main.join(".git");
            let opts = benign(&base);
            move || open_hardened(&git_dir, &opts).map(|_| ())
        })
        .expect_err("format version 7 must not open");

        assert!(err.starts_with("open failed: "), "{err}");
    }
}
