//! Git fixture builder (P27b): every `[[repo]]` of the world's root, built with the real git binary.
//!
//! Layout per world (`w.base` holds it all): `root` (F, F_clean or H), `ext` (EXT; `ext` sorts before `root`,
//! which P09's oracle relies on), the sandbox `home` (the `home` root and `[home]` files), `budget/` (the
//! budget root, F only, deliberately outside `root`), plus `scratch/` (submodule children and bare-repo
//! sources), `helpers/` (second-hop alternates) and `sparse-4gib`. H gets no `[home]` files: P35 scans it
//! with a HOME that has no `core.excludesFile`.
//!
//! Per repo: init, commits, branches, head, remotes, worktrees, submodules, state and edit, then the
//! specials that need git, then `config` (written to the config file, never through git), then the `target`
//! replacement. No git command runs in a repo after its hostile shape is in place.
use crate::support::require_tool;
use crate::support::{Commit, Kind, Manifest, Repo, RootId, Special, Which, World, git_path};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{FileExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

const BUDGET_DIR: &str = "budget";
const SPARSE_FILE: &str = "sparse-4gib";
const SPARSE_BIG_LEN: u64 = 4 << 30;
const BIG_TRACKED_LEN: u64 = 1 << 30;
const REPLACE_BASE_LEN: u64 = 256 << 20;
const MIB: usize = 1 << 20;
/// Nesting of the accepted positive control (every node on the path counts, root included).
const DEEP_OK_NODES: usize = 1024;
/// One node more than the validator accepts.
const TOO_DEEP_NODES: usize = 1025;
const SHARED_INDEX_ID: [u8; 20] = [0xaa; 20];
/// An EWAH bitmap with no bits: bit count, word count and run-length-word position, all zero.
const EMPTY_EWAH: [u8; 12] = [0; 12];
const REUC_BYTES: usize = 2 * MIB;
const TREE_WIDE_BYTES: usize = 64 * MIB;
const UNTR_WIDE_BYTES: usize = 8 * MIB;
const UNTR_WIDE_BLOCKS: usize = 8;
const UNTR_FIXED_LEN: usize = 36 + 36 + 4 + 20 + 20;
const TREE_BOMB_FANOUT: usize = 200;
const LONG_NAME_LEN: usize = 500;
const LONG_BOMB_SUBTREE: usize = 1000;
const LONG_BOMB_TOP: usize = 999;
const BIG_TREE_CLAIM: &str = "tree 1073741824";

const SEAL_SCRIPT: &str = "import hashlib,sys\np=sys.argv[1]\nd=open(p,'rb').read()\nopen(p,'ab').write(hashlib.sha1(d).digest())";
const BIG_TREE_SCRIPT: &str = "import os,sys,zlib\np=sys.argv[1]\nraw=zlib.decompress(open(p,'rb').read())\nhead,body=raw.split(b'\\0',1)\nassert head.startswith(b'tree ')\nos.chmod(p,0o644)\nopen(p,'wb').write(zlib.compress(b'tree 1073741824\\0'+body))";

/// Writes `<pack dir>/pack-<sha1 of the sorted ids>.{pack,idx}` (version 2 idx, real SHA-1 trailers); argv:
/// pack dir, kind, link count, hex id of the one listed object. The pack lists only that object; the
/// `ofs` kind's 8,000,000 links are two-byte entries with no deflate stream, as in the P15b probe.
const PACK_SCRIPT: &str = r#"import hashlib,os,struct,sys,zlib
d,kind,n,hexid=sys.argv[1],sys.argv[2],int(sys.argv[3]),sys.argv[4]
x=bytes.fromhex(hexid)
if kind=='ref_head':
    body=b'\x70'+x+zlib.compress(b'');count,last,crc=1,12,zlib.crc32(body)
elif kind=='ref_attr':
    body=b'\x72'+x+zlib.compress(b'\0\0');count,last,crc=1,12,zlib.crc32(body)
else:
    blob=b'\x30'+zlib.compress(b'')
    body=blob+b'\x60'+bytes([len(blob)])+b'\x60\x02'*(n-1)
    count,last,crc=n+1,12+len(blob)+2*(n-1),zlib.crc32(b'\x60\x02')
pack=b'PACK'+struct.pack('>II',2,count)+body
pack+=hashlib.sha1(pack).digest()
idx=b'\xfftOc'+struct.pack('>I',2)
idx+=b''.join(struct.pack('>I',1 if b>=x[0] else 0) for b in range(256))
idx+=x+struct.pack('>II',crc,last)+pack[-20:]
idx+=hashlib.sha1(idx).digest()
name=os.path.join(d,'pack-'+hashlib.sha1(x).hexdigest())
open(name+'.pack','wb').write(pack)
open(name+'.idx','wb').write(idx)"#;

/// Loose tree chain; argv: git dir, blob hex, levels, bytes per tree, blob name length. Prints the top tree id.
/// The first entry of each tree is `a` (the next tree; a blob at the bottom), then blobs `b0000000aaa...`;
/// 27,262,976 is not a multiple of the 188-byte entry, so the last name absorbs the remainder.
const TREE_SCRIPT: &str = r#"import hashlib,os,sys,zlib
g,blob,levels,total,nl=sys.argv[1],bytes.fromhex(sys.argv[2]),int(sys.argv[3]),int(sys.argv[4]),int(sys.argv[5])
def put(body):
    raw=b'tree %d\0'%len(body)+body
    h=hashlib.sha1(raw).hexdigest()
    d=os.path.join(g,'objects',h[:2]);os.makedirs(d,exist_ok=True)
    open(os.path.join(d,h[2:]),'wb').write(zlib.compress(raw,1))
    return bytes.fromhex(h)
child=None
for _ in range(levels):
    first=b'40000 a\0'+child if child else b'100644 a\0'+blob
    k,pad=divmod(total-len(first),7+nl+1+20)
    names=[b'b%07d'%i+b'a'*(nl-8) for i in range(k)]
    names[-1]+=b'a'*pad
    body=first+b''.join(b'100644 '+n+b'\0'+blob for n in names)
    assert len(body)==total
    child=put(body)
print(child.hex())"#;
/// Object id the self-referencing REF_DELTA entries name as their base (their own id).
const SELF_DELTA_ID: &str = "1111111111111111111111111111111111111111";
const GIT_DIR_PREFIX: &str = "git-dir:";

pub fn build(which: Which, w: &mut World, m: &Manifest) {
    let cx = Ctx { w, m };
    if which != Which::H {
        write_home_files(&cx);
    }
    let wanted = |r: &Repo| match which {
        Which::F => matches!(
            r.root,
            RootId::F | RootId::Ext | RootId::Home | RootId::Budget
        ),
        Which::FClean => matches!(r.root, RootId::F | RootId::Ext | RootId::Home) && r.in_clean(),
        Which::H => r.root == RootId::H,
    };
    let mut rows: Vec<&Repo> = m.repo.iter().filter(|r| wanted(r)).collect();
    // EXT holds remote targets and the case B main; everything else may refer to it.
    rows.sort_by_key(|r| r.root != RootId::Ext);
    for r in rows {
        // Submodule children are built by their parent.
        if !matches!(r.kind, Kind::Submodule | Kind::SubmoduleOld) {
            build_repo(&cx, r);
        }
    }
}

struct Ctx<'a> {
    w: &'a World,
    m: &'a Manifest,
}

#[derive(Default)]
struct Opts<'a> {
    date: Option<&'a str>,
    stdin: Option<&'a [u8]>,
    allow_file_protocol: bool,
}

impl Ctx<'_> {
    fn root_dir(&self, root: RootId) -> PathBuf {
        match root {
            RootId::F | RootId::H => self.w.root.clone(),
            RootId::Ext => self.w.ext.clone(),
            RootId::Home => self.w.sb.home.clone(),
            RootId::Budget => self.w.base.join(BUDGET_DIR),
            RootId::Perf | RootId::Demo => panic!("{root:?} is built by P28"),
        }
    }

    fn repo_path(&self, r: &Repo) -> PathBuf {
        self.root_dir(r.root).join(&r.path)
    }

    fn repo(&self, id: &str) -> &Repo {
        self.m
            .repo
            .iter()
            .find(|r| r.id == id)
            .unwrap_or_else(|| panic!("no [[repo]] with id {id}"))
    }

    fn subst(&self, s: &str) -> String {
        self.w.tokens.subst(s)
    }

    fn scratch(&self, id: &str) -> PathBuf {
        self.w.base.join("scratch").join(id)
    }

    /// The 4 GiB sparse file symlinks point at, created on first use.
    fn sparse(&self) -> PathBuf {
        let path = self.w.base.join(SPARSE_FILE);
        if !path.exists() {
            sparse_file(&path, SPARSE_BIG_LEN);
        }
        path
    }

    fn git_command(&self, dir: &Path, opts: &Opts, args: &[&str]) -> Command {
        let meta = &self.m.meta;
        let mut cmd = self.w.sb.command(git_path());
        cmd.current_dir(dir)
            .args(["-c", "gc.auto=0", "-c", "commit.gpgsign=false"])
            .args(["-c", "advice.detachedHead=false"]);
        if opts.allow_file_protocol {
            cmd.args(["-c", "protocol.file.allow=always"]);
        }
        cmd.args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", &meta.author.name)
            .env("GIT_AUTHOR_EMAIL", &meta.author.email)
            .env("GIT_COMMITTER_NAME", &meta.committer.name)
            .env("GIT_COMMITTER_EMAIL", &meta.committer.email);
        if let Some(date) = opts.date {
            cmd.env("GIT_AUTHOR_DATE", date)
                .env("GIT_COMMITTER_DATE", date);
        }
        cmd
    }

    /// Runs git, returns the exit success and trimmed stdout. Plain `output()`: no FIFO exists yet.
    fn try_git(&self, dir: &Path, opts: &Opts, args: &[&str]) -> (bool, String, String) {
        let mut cmd = self.git_command(dir, opts, args);
        cmd.stdin(if opts.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .unwrap_or_else(|e| panic!("spawn git {args:?}: {e}"));
        if let Some(input) = opts.stdin {
            child
                .stdin
                .take()
                .expect("piped stdin")
                .write_all(input)
                .unwrap_or_else(|e| panic!("git {args:?}: write stdin: {e}"));
        }
        let out = child
            .wait_with_output()
            .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn git_with(&self, dir: &Path, opts: &Opts, args: &[&str]) -> String {
        let (ok, stdout, stderr) = self.try_git(dir, opts, args);
        assert!(ok, "git {args:?} in {}: {stderr}", dir.display());
        stdout
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        self.git_with(dir, &Opts::default(), args)
    }
}

// ------------------------------------------------------------- small fs helpers

fn write_file(path: &Path, content: impl AsRef<[u8]>) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("mkdir {}: {e}", parent.display()));
    }
    fs::write(path, content).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn sparse_file(path: &Path, len: u64) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("mkdir {}: {e}", parent.display()));
    }
    fs::File::create(path)
        .and_then(|f| f.set_len(len))
        .unwrap_or_else(|e| panic!("sparse file {}: {e}", path.display()));
}

fn mkfifo(path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("mkdir {}: {e}", parent.display()));
    }
    let status = Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo {} failed", path.display());
}

/// A symlink at `link` (replacing anything there) to `target`.
fn replace_with_symlink(link: &Path, target: &Path) {
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("mkdir {}: {e}", parent.display()));
    }
    match fs::remove_file(link) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => panic!("remove {}: {e}", link.display()),
    }
    symlink(target, link).unwrap_or_else(|e| panic!("symlink {}: {e}", link.display()));
}

fn python() -> &'static Path {
    static PYTHON: OnceLock<PathBuf> = OnceLock::new();
    PYTHON.get_or_init(|| {
        require_tool("python3").expect("python3 is required to build fixtures (zlib, SHA-1)")
    })
}

fn run_python(script: &str, file: &Path) {
    let out = Command::new(python())
        .args(["-c", script])
        .arg(file)
        .output()
        .expect("run python3");
    assert!(
        out.status.success(),
        "python3 on {}: {}",
        file.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write_home_files(cx: &Ctx) {
    for (rel, content) in &cx.m.home.files {
        write_file(&cx.w.sb.home.join(rel), cx.subst(content));
    }
}

// ------------------------------------------------------------------ one repo

fn build_repo(cx: &Ctx, r: &Repo) {
    let path = cx.repo_path(r);
    if build_directory_fixture(cx, r, &path) {
        return;
    }
    let bare = r.kind == Kind::Bare || r.config.get("core.bare").is_some_and(|v| v == "true");
    let gitdir = if bare {
        path.clone()
    } else {
        path.join(".git")
    };
    if bare {
        build_bare(cx, r, &path);
    } else {
        init_with_commits(cx, r, &path);
        add_remotes(cx, r, &path);
        add_worktrees(cx, r, &path);
        add_submodules(cx, r, &path);
        apply_state(cx, r, &path);
    }
    before_config(cx, r, &path);
    write_config(cx, r, &gitdir);
    apply_target(cx, r, &path, &gitdir);
}

/// Specials that are directories, not repos. True when `r` was one.
fn build_directory_fixture(cx: &Ctx, r: &Repo, path: &Path) -> bool {
    match r.special {
        Some(Special::SymlinkFanout) => {
            for name in ["loop-a", "loop-b"] {
                replace_with_symlink(&path.join("d1/d2").join(name), &cx.w.root);
            }
        }
        Some(Special::SymlinkOutOfRoot) => {
            let target = cx.subst(r.target.as_deref().expect("symlink_out_of_root target"));
            replace_with_symlink(path, Path::new(&target));
        }
        Some(Special::UnreadableDir) => {
            fs::create_dir_all(path).expect("create unreadable dir");
            fs::set_permissions(path, fs::Permissions::from_mode(0o000)).expect("chmod 000");
        }
        Some(Special::BadGitdir) => write_file(&path.join(".git"), "gitdir: ../does-not-exist\n"),
        Some(Special::FifoGit) => mkfifo(&path.join(".git")),
        _ => return false,
    }
    true
}

/// `init` at `dir`, then commits, extra branches and `head`. Returns the commit ids in array order.
fn init_with_commits(cx: &Ctx, r: &Repo, dir: &Path) -> Vec<String> {
    fs::create_dir_all(dir).unwrap_or_else(|e| panic!("mkdir {}: {e}", dir.display()));
    let first = r.commits.first().map_or("main", |c| c.branch.as_str());
    cx.git(dir, &["init", "-q", "-b", first]);
    let ids = make_commits(cx, dir, &r.commits);
    for (name, &i) in &r.branches {
        cx.git(dir, &["branch", name, &ids[i]]);
    }
    match r.head.as_deref() {
        None => {}
        Some(head) => match head.strip_prefix("detached:") {
            Some(i) => {
                let i: usize = i.parse().expect("detached:<index>");
                cx.git(dir, &["checkout", "-q", "--detach", &ids[i]]);
            }
            None => {
                cx.git(dir, &["checkout", "-q", head, "--"]);
            }
        },
    }
    ids
}

/// A commit on a new branch forks from the previous entry's commit; on an existing branch it continues it.
fn make_commits(cx: &Ctx, dir: &Path, commits: &[Commit]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut known: Vec<&str> = Vec::new();
    for (i, c) in commits.iter().enumerate() {
        if i > 0 {
            if known.contains(&c.branch.as_str()) {
                cx.git(dir, &["checkout", "-q", &c.branch, "--"]);
            } else {
                cx.git(dir, &["checkout", "-q", "-b", &c.branch, &ids[i - 1]]);
            }
        }
        for (rel, content) in &c.files {
            let file = dir.join(rel);
            write_file(&file, content);
            if content.starts_with("#!") {
                fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("chmod +x");
            }
        }
        let mut add = vec!["add", "--"];
        add.extend(c.files.keys().map(String::as_str));
        cx.git(dir, &add);
        let opts = Opts {
            date: Some(&c.date),
            ..Opts::default()
        };
        cx.git_with(dir, &opts, &["commit", "-q", "-m", &c.message]);
        ids.push(cx.git(dir, &["rev-parse", "HEAD"]));
        known.push(&c.branch);
    }
    ids
}

/// Bare repos with commits are built by scratch-clone-then-push.
fn build_bare(cx: &Ctx, r: &Repo, path: &Path) {
    let scratch = cx.scratch(&r.id);
    init_with_commits(cx, r, &scratch);
    fs::create_dir_all(path).unwrap_or_else(|e| panic!("mkdir {}: {e}", path.display()));
    let head = r
        .head
        .as_deref()
        .or(r.commits.first().map(|c| c.branch.as_str()))
        .unwrap_or("main");
    cx.git(path, &["init", "-q", "--bare", "-b", head]);
    if !r.commits.is_empty() {
        let url = path.to_string_lossy();
        cx.git(&scratch, &["push", "-q", &url, "refs/heads/*:refs/heads/*"]);
    }
}

fn add_remotes(cx: &Ctx, r: &Repo, dir: &Path) {
    for remote in &r.remotes {
        let url = cx.repo_path(cx.repo(&remote.target_repo_id));
        let url = url.to_string_lossy();
        cx.git(dir, &["remote", "add", &remote.name, &url]);
        cx.git(dir, &["fetch", "-q", &remote.name]);
    }
    for (branch, upstream) in &r.upstreams {
        cx.git(
            dir,
            &[
                "branch",
                "-q",
                &format!("--set-upstream-to={upstream}"),
                branch,
            ],
        );
    }
    if let Some(head) = &r.origin_head {
        cx.git(dir, &["remote", "set-head", "origin", head]);
    }
}

fn add_worktrees(cx: &Ctx, r: &Repo, dir: &Path) {
    for wt in &r.worktrees {
        let path = if wt.path.starts_with('$') {
            PathBuf::from(cx.subst(&wt.path))
        } else {
            cx.root_dir(r.root).join(&wt.path)
        };
        fs::create_dir_all(path.parent().expect("worktree parent"))
            .unwrap_or_else(|e| panic!("mkdir for {}: {e}", path.display()));
        let target = path.to_string_lossy();
        match wt.branch.as_deref() {
            None => cx.git(dir, &["worktree", "add", "-q", "--detach", &target]),
            Some(branch) => {
                let refname = format!("refs/heads/{branch}");
                let (exists, _, _) = cx.try_git(
                    dir,
                    &Opts::default(),
                    &["rev-parse", "--verify", "-q", &refname],
                );
                if exists {
                    cx.git(dir, &["worktree", "add", "-q", &target, branch])
                } else {
                    cx.git(dir, &["worktree", "add", "-q", "-b", branch, &target])
                }
            }
        };
        if wt.missing {
            fs::remove_dir_all(&path).unwrap_or_else(|e| panic!("rm {}: {e}", path.display()));
        }
    }
}

// ---------------------------------------------------------------- submodules

fn add_submodules(cx: &Ctx, r: &Repo, dir: &Path) {
    let date = r.commits.last().map(|c| c.date.as_str());
    let opts = Opts {
        date,
        allow_file_protocol: true,
        ..Opts::default()
    };
    for child in
        cx.m.repo
            .iter()
            .filter(|c| c.parent.as_deref() == Some(&r.id))
    {
        let sub = child
            .path
            .strip_prefix(&format!("{}/", r.path))
            .unwrap_or_else(|| panic!("{} is not under {}", child.path, r.path));
        match child.kind {
            Kind::Submodule => {
                // `git submodule add` of a scratch clone over file://; the child's edits land in the checkout.
                let scratch = cx.scratch(&child.id);
                init_with_commits(cx, child, &scratch);
                let url = format!("file://{}", scratch.display());
                cx.git_with(dir, &opts, &["submodule", "add", "-q", &url, sub]);
                cx.git_with(dir, &opts, &["commit", "-q", "-m", "add submodule"]);
                apply_state(cx, child, &dir.join(sub));
            }
            Kind::SubmoduleOld => {
                // A gitlink and .gitmodules entry in the parent; the child stays an in-place repo.
                init_with_commits(cx, child, &dir.join(sub));
                let modules = format!(
                    "[submodule \"{sub}\"]\n\tpath = {sub}\n\turl = https://fixtures.test/{sub}.git\n"
                );
                write_file(&dir.join(".gitmodules"), modules);
                cx.git(dir, &["add", "--", ".gitmodules", sub]);
                cx.git_with(
                    dir,
                    &opts,
                    &["commit", "-q", "-m", "add old-style submodule"],
                );
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- state, config

fn apply_state(cx: &Ctx, r: &Repo, dir: &Path) {
    for (rel, content) in &r.edit {
        let file = dir.join(rel);
        let old = fs::read(&file).unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
        assert_eq!(
            old.len(),
            content.len(),
            "edit of {rel} must keep the committed byte length"
        );
        write_file(&file, content);
    }
    let Some(state) = &r.state else { return };
    if !state.staged.is_empty() {
        let mut add = vec!["add", "--"];
        add.extend(state.staged.iter().map(String::as_str));
        cx.git(dir, &add);
    }
    for rel in &state.untracked {
        write_file(&dir.join(rel), "untracked\n");
    }
    for rel in &state.ignored {
        write_file(&dir.join(rel), "ignored\n");
    }
}

fn config_value(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `git-dir:<name>` is the absolute path of `<git dir>/<name>`; anything else is substituted like a path token.
fn config_path(cx: &Ctx, gitdir: &Path, value: &str) -> String {
    match value.strip_prefix(GIT_DIR_PREFIX) {
        Some(name) => gitdir.join(name).to_string_lossy().into_owned(),
        None => cx.subst(value),
    }
}

/// Appends `config` entries to the git dir's config file; last assignment wins, and no git command runs.
fn write_config(cx: &Ctx, r: &Repo, gitdir: &Path) {
    if r.config.is_empty() {
        return;
    }
    let mut text = String::new();
    for (key, value) in &r.config {
        let (section, name) = key.rsplit_once('.').expect("config key is section.key");
        let header = match section.split_once('.') {
            Some((section, sub)) => format!("[{section} \"{sub}\"]"),
            None => format!("[{section}]"),
        };
        text.push_str(&format!(
            "{header}\n\t{name} = {}\n",
            config_value(&config_path(cx, gitdir, value))
        ));
    }
    let file = gitdir.join("config");
    let mut existing = fs::read_to_string(&file).unwrap_or_default();
    existing.push_str(&text);
    write_file(&file, existing);
}

// ---------------------------------------------------- specials that need git

fn before_config(cx: &Ctx, r: &Repo, path: &Path) {
    let Some(special) = r.special else { return };
    match special {
        Special::TreeBomb => tree_bomb(cx, r, path),
        Special::TreeBombLong => tree_bomb_long(cx, r, path),
        Special::BigObject => big_object(cx, path),
        Special::PatternBlobBomb => pattern_blob_bomb(cx, r, path),
        Special::RenameBomb => rename_bomb(cx, r, path),
        Special::TreeDeepBig => tree_deep_big(cx, r, path),
        Special::PackBomb => pack_bomb_index(cx, r, path),
        _ => {}
    }
}

/// `(mode, kind, id, name)` of one tree entry.
type TreeEntry = (&'static str, &'static str, String, String);

fn mktree(cx: &Ctx, dir: &Path, entries: &[TreeEntry]) -> String {
    let mut input = String::new();
    for (mode, kind, id, name) in entries {
        input.push_str(&format!("{mode} {kind} {id}\t{name}\n"));
    }
    let opts = Opts {
        stdin: Some(input.as_bytes()),
        ..Opts::default()
    };
    cx.git_with(dir, &opts, &["mktree"])
}

fn hash_object(cx: &Ctx, dir: &Path, body: &[u8]) -> String {
    let opts = Opts {
        stdin: Some(body),
        ..Opts::default()
    };
    cx.git_with(dir, &opts, &["hash-object", "-w", "--stdin"])
}

/// Makes `tree` the HEAD commit's root tree (a new root commit on the current branch).
fn replace_head_tree(cx: &Ctx, r: &Repo, dir: &Path, tree: &str) {
    let last = r.commits.last().expect("tree bomb repos have a commit");
    let opts = Opts {
        date: Some(&last.date),
        ..Opts::default()
    };
    let commit = cx.git_with(dir, &opts, &["commit-tree", tree, "-m", &last.message]);
    cx.git(dir, &["update-ref", "HEAD", &commit]);
}

fn fan_out(
    kind: &'static str,
    mode: &'static str,
    id: &str,
    names: impl Iterator<Item = String>,
) -> Vec<TreeEntry> {
    names.map(|n| (mode, kind, id.to_string(), n)).collect()
}

fn readme_entry(cx: &Ctx, dir: &Path) -> TreeEntry {
    let id = cx.git(dir, &["rev-parse", "HEAD:README.md"]);
    ("100644", "blob", id, "README.md".to_string())
}

/// Three nested levels of 200 entries, each pointing at the same subtree: 8,000,000 expanded paths.
fn tree_bomb(cx: &Ctx, r: &Repo, dir: &Path) {
    let names =
        |prefix: &'static str| (0..TREE_BOMB_FANOUT).map(move |i| format!("{prefix}{i:03}"));
    let blob = hash_object(cx, dir, b"x\n");
    let level1 = mktree(cx, dir, &fan_out("blob", "100644", &blob, names("f")));
    let level2 = mktree(cx, dir, &fan_out("tree", "040000", &level1, names("d")));
    let mut top = fan_out("tree", "040000", &level2, names("d"));
    top.push(readme_entry(cx, dir));
    replace_head_tree(cx, r, dir, &mktree(cx, dir, &top));
}

fn long_name(prefix: char, i: usize) -> String {
    let head = format!("{prefix}{i:04}");
    format!("{head}{}", "a".repeat(LONG_NAME_LEN - head.len()))
}

/// One subtree of 1,000 long-named files, reached from 999 long-named top-level entries.
fn tree_bomb_long(cx: &Ctx, r: &Repo, dir: &Path) {
    let blob = hash_object(cx, dir, b"x\n");
    let files = (0..LONG_BOMB_SUBTREE).map(|i| long_name('f', i));
    let subtree = mktree(cx, dir, &fan_out("blob", "100644", &blob, files));
    let dirs = (0..LONG_BOMB_TOP).map(|i| long_name('d', i));
    let mut top = fan_out("tree", "040000", &subtree, dirs);
    top.push(readme_entry(cx, dir));
    replace_head_tree(cx, r, dir, &mktree(cx, dir, &top));
}

/// The root tree's loose object gets a header that claims 1 GiB.
fn big_object(cx: &Ctx, dir: &Path) {
    let tree = cx.git(dir, &["rev-parse", "HEAD^{tree}"]);
    let object = dir.join(".git/objects").join(&tree[..2]).join(&tree[2..]);
    assert!(object.is_file(), "root tree {tree} must be a loose object");
    debug_assert!(BIG_TREE_SCRIPT.contains(BIG_TREE_CLAIM));
    run_python(BIG_TREE_SCRIPT, &object);
}

/// `target` = `<dir spec>/<file>:<blob bytes>[:skip-worktree]`; the dir spec is one directory or `subA..subB`.
fn pattern_blob_bomb(cx: &Ctx, r: &Repo, dir: &Path) {
    let target = r.target.as_deref().expect("pattern_blob_bomb target");
    let mut parts = target.split(':');
    let spec = parts.next().expect("path spec");
    let size: usize = parts.next().expect("blob size").parse().expect("blob size");
    let skip_worktree = parts.next() == Some("skip-worktree");
    let (dirs, file) = spec.rsplit_once('/').expect("<dirs>/<file>");
    let names: Vec<String> = match dirs.split_once("..") {
        None => vec![dirs.to_string()],
        Some((first, last)) => {
            let digits = |s: &str| {
                s.trim_start_matches(|c: char| !c.is_ascii_digit())
                    .parse::<usize>()
            };
            let prefix = first.trim_end_matches(|c: char| c.is_ascii_digit());
            let (from, to) = (
                digits(first).expect("range start"),
                digits(last).expect("range end"),
            );
            (from..=to).map(|i| format!("{prefix}{i}")).collect()
        }
    };
    let blob = hash_object(cx, dir, "#\n".repeat(size / 2).as_bytes());
    for name in names {
        let path = format!("{name}/{file}");
        let cacheinfo = format!("100644,{blob},{path}");
        cx.git(dir, &["update-index", "--add", "--cacheinfo", &cacheinfo]);
        if skip_worktree {
            cx.git(dir, &["update-index", "--skip-worktree", &path]);
        }
    }
}

/// `target` = `<from>><to>:<bytes>:line1`: commit the first 60 MiB blob, `git mv`, change line 1, stage.
fn rename_bomb(cx: &Ctx, r: &Repo, dir: &Path) {
    let target = r.target.as_deref().expect("rename_bomb target");
    let mut parts = target.split(':');
    let (from, to) = parts
        .next()
        .and_then(|s| s.split_once('>'))
        .expect("<from>><to>");
    let size: usize = parts.next().expect("blob size").parse().expect("blob size");
    write_file(&dir.join(from), "x\n".repeat(size / 2));
    cx.git(dir, &["add", "--", from]);
    let last = r.commits.last().expect("rename bomb repos have a commit");
    let opts = Opts {
        date: Some(&last.date),
        ..Opts::default()
    };
    cx.git_with(dir, &opts, &["commit", "-q", "-m", "big blob"]);
    cx.git(dir, &["mv", from, to]);
    // Same length, so the rename stays a similarity match.
    fs::OpenOptions::new()
        .write(true)
        .open(dir.join(to))
        .and_then(|f| f.write_all_at(b"y", 0))
        .expect("edit line 1");
    cx.git(dir, &["add", "--", to]);
}

fn target_value<'a>(target: &'a str, key: &str) -> &'a str {
    target
        .split(',')
        .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
        .unwrap_or_else(|| panic!("target {target} has no {key}"))
}

/// `target` = `levels=N,tree_bytes=<n>MiB,next_name=1,blob_name=N`: loose trees from python3, one commit on the top.
fn tree_deep_big(cx: &Ctx, r: &Repo, dir: &Path) {
    let target = r.target.as_deref().expect("tree_deep_big target");
    assert_eq!(
        target_value(target, "next_name"),
        "1",
        "the chain entry is `a`"
    );
    let tree_bytes: usize = target_value(target, "tree_bytes")
        .strip_suffix("MiB")
        .and_then(|n| n.parse::<usize>().ok())
        .expect("tree_bytes=<n>MiB")
        * MIB;
    let blob = hash_object(cx, dir, b"x\n");
    let out = Command::new(python())
        .args(["-c", TREE_SCRIPT])
        .arg(dir.join(".git"))
        .args([&blob, target_value(target, "levels")])
        .arg(tree_bytes.to_string())
        .arg(target_value(target, "blob_name"))
        .output()
        .expect("run python3");
    assert!(
        out.status.success(),
        "tree chain: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    replace_head_tree(cx, r, dir, String::from_utf8_lossy(&out.stdout).trim());
}

/// Variant (ii) needs a stage-0 index entry for a blob that exists only as a bad pack entry (git, not the pack, adds it).
fn pack_bomb_index(cx: &Ctx, r: &Repo, dir: &Path) {
    if r.target.as_deref() == Some("ref_delta_self_attr") {
        let cacheinfo = format!("100644,{SELF_DELTA_ID},sub/.gitattributes");
        cx.git(dir, &["update-index", "--add", "--cacheinfo", &cacheinfo]);
    }
}

// ------------------------------------------------------ target replacement

/// The last step of a repo; no git command runs here or afterwards (index specials run `rev-parse` first).
fn apply_target(cx: &Ctx, r: &Repo, path: &Path, gitdir: &Path) {
    let Some(special) = r.special else { return };
    let target = r.target.as_deref().unwrap_or("");
    match special {
        Special::UntrustedPath => {
            let admin = only_admin_dir(gitdir);
            let home_git = format!("{}/.git\n", cx.subst(target));
            write_file(&admin.join("gitdir"), home_git);
        }
        Special::CommondirEscape => {
            let victim = cx.repo_path(cx.repo(target)).join(".git");
            write_file(
                &only_admin_dir(gitdir).join("commondir"),
                format!("{}\n", victim.display()),
            );
        }
        Special::WorktreeConfigSymlink => {
            replace_with_symlink(&only_admin_dir(gitdir).join(target), &cx.sparse());
        }
        Special::SymlinkedFiles => symlinked_file(cx, r, path, gitdir, target),
        Special::ExcludesFile => replace_with_symlink(&path.join(target), &cx.sparse()),
        Special::HangProbe | Special::HangProbeWorktreeConfig => {
            let name = if special == Special::HangProbe {
                "packed-refs"
            } else {
                "config.worktree"
            };
            let fifo = PathBuf::from(cx.subst(target));
            fs::rename(&fifo, gitdir.join(name))
                .unwrap_or_else(|e| panic!("move fifo into {name}: {e}"));
        }
        Special::IndexBomb => index_bomb(cx, path, gitdir, target),
        Special::IndexDeepOk => {
            let version = match target {
                "v2" => 2,
                "v4" => 4,
                other => panic!("index_deep_ok target {other}"),
            };
            let exts = [
                (*b"TREE", tree_chain(DEEP_OK_NODES)),
                (*b"UNTR", untr_chain(DEEP_OK_NODES)),
            ];
            write_index(cx, path, &gitdir.join("index"), version, &exts);
        }
        Special::NestedProbe => nested_probe(cx, path),
        Special::ExecType => {
            let (file, kind) = target.split_once(':').expect("<file>:<symlink|fifo>");
            let file = path.join(file);
            fs::remove_file(&file).unwrap_or_else(|e| panic!("rm {}: {e}", file.display()));
            match kind {
                "symlink" => replace_with_symlink(&file, &cx.sparse()),
                "fifo" => mkfifo(&file),
                other => panic!("exec_type kind {other}"),
            }
        }
        Special::BigTracked => {
            let file = target.split(':').next().expect("<file>:sparse:1GiB");
            let file = path.join(file);
            fs::remove_file(&file).unwrap_or_else(|e| panic!("rm {}: {e}", file.display()));
            sparse_file(&file, BIG_TRACKED_LEN);
        }
        Special::GitoxideKeys => gitoxide_keys(cx, path, gitdir, target),
        Special::PackBomb => pack_bomb(gitdir, target),
        Special::ObjectsPackFlood => {
            let count: usize = target
                .strip_prefix("empty_files:")
                .and_then(|n| n.parse().ok())
                .expect("empty_files:<n>");
            for i in 0..count {
                write_file(&gitdir.join(format!("objects/pack/pack-flood-{i:05}")), "");
            }
        }
        _ => {}
    }
}

fn only_admin_dir(gitdir: &Path) -> PathBuf {
    let mut dirs: Vec<PathBuf> = fs::read_dir(gitdir.join("worktrees"))
        .unwrap_or_else(|e| panic!("read {}/worktrees: {e}", gitdir.display()))
        .map(|e| e.expect("dir entry").path())
        .collect();
    assert_eq!(dirs.len(), 1, "expected one linked worktree: {dirs:?}");
    dirs.remove(0)
}

/// `target` names the git-dir file that becomes a symlink to the 4 GiB file (see the manifest header).
fn symlinked_file(cx: &Ctx, r: &Repo, path: &Path, gitdir: &Path, target: &str) {
    let sparse = cx.sparse();
    if let Some(file) = target.strip_prefix("worktree:") {
        replace_with_symlink(&path.join(file), &sparse);
    } else if let Some(pseudo) = target.strip_prefix("HEAD:") {
        write_file(&gitdir.join("HEAD"), format!("ref: {pseudo}\n"));
        replace_with_symlink(&gitdir.join(pseudo), &sparse);
    } else if let Some(file) = target.strip_suffix("#2") {
        // Hop 1 names a helper objects dir whose own alternates file is the symlink.
        let helper = cx.w.base.join("helpers").join(&r.id).join("objects");
        replace_with_symlink(&helper.join("info/alternates"), &sparse);
        write_file(&gitdir.join(file), format!("{}\n", helper.display()));
    } else {
        replace_with_symlink(&gitdir.join(target), &sparse);
    }
}

/// The config keys are already written; this creates the hostile file each one names.
fn gitoxide_keys(cx: &Ctx, path: &Path, gitdir: &Path, target: &str) {
    match target {
        "shallow_dev_zero" => {}
        "shallow_fifo" => mkfifo(&gitdir.join("shallow-fifo")),
        "index_file_chain" => {
            let chain = [(*b"TREE", tree_chain(TOO_DEEP_NODES))];
            write_index(cx, path, &gitdir.join("evil"), 2, &chain);
        }
        "replace_ref_base" => sparse_file(&gitdir.join("objects/pack/evil"), REPLACE_BASE_LEN),
        other => panic!("gitoxide_keys target {other}"),
    }
}

/// `target` = `ref_delta_self_head`, `ref_delta_self_attr` or `ofs_delta_chain:<links>`.
fn pack_bomb(gitdir: &Path, target: &str) {
    let (kind, links) = match target.split_once(':') {
        Some(("ofs_delta_chain", n)) => ("ofs", n),
        None if target == "ref_delta_self_head" => ("ref_head", "0"),
        None if target == "ref_delta_self_attr" => ("ref_attr", "0"),
        _ => panic!("pack_bomb target {target}"),
    };
    let pack_dir = gitdir.join("objects/pack");
    fs::create_dir_all(&pack_dir).expect("create objects/pack");
    let out = Command::new(python())
        .args(["-c", PACK_SCRIPT])
        .arg(&pack_dir)
        .args([kind, links, SELF_DELTA_ID])
        .output()
        .expect("run python3");
    assert!(
        out.status.success(),
        "pack {target}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Variant (ii) keeps its ordinary HEAD; (i) and (iii) point the branch at the object the pack holds.
    if target != "ref_delta_self_attr" {
        let head = fs::read_to_string(gitdir.join("HEAD")).expect("read HEAD");
        let branch = head.strip_prefix("ref: ").expect("symbolic HEAD").trim();
        write_file(&gitdir.join(branch), format!("{SELF_DELTA_ID}\n"));
    }
}

/// Three untracked directories whose `.git` entries must never be opened by a walk.
fn nested_probe(cx: &Ctx, path: &Path) {
    mkfifo(&path.join("a/.git/HEAD"));
    replace_with_symlink(&path.join("b/.git/HEAD"), &cx.sparse());
    write_file(&path.join("c/.git"), "gitdir: ../d\n");
    mkfifo(&path.join("d/HEAD"));
}

// ------------------------------------------------------------- index writers

/// git's offset varint (`encode_varint`).
fn git_varint(mut n: u64) -> Vec<u8> {
    let mut out = vec![(n & 127) as u8];
    n >>= 7;
    while n > 0 {
        n -= 1;
        out.push(128 | (n & 127) as u8);
        n >>= 7;
    }
    out.reverse();
    out
}

fn hex20(hex: &str) -> [u8; 20] {
    let mut id = [0u8; 20];
    for (i, byte) in id.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex object id");
    }
    id
}

/// A header, one `README.md` entry (mode 100644, zero stat fields) and `exts`, without the SHA-1 trailer.
fn index_bytes(version: u32, blob: &[u8; 20], exts: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    const NAME: &[u8] = b"README.md";
    let mut out = b"DIRC".to_vec();
    out.extend(version.to_be_bytes());
    out.extend(1u32.to_be_bytes());
    let mut entry = vec![0u8; 62];
    entry[24..28].copy_from_slice(&0o100644u32.to_be_bytes());
    entry[40..60].copy_from_slice(blob);
    entry[60..62].copy_from_slice(&(NAME.len() as u16).to_be_bytes());
    if version == 4 {
        entry.extend(git_varint(0));
        entry.extend(NAME);
        entry.push(0);
    } else {
        entry.extend(NAME);
        // One to eight NULs, the first ending the name, pad the entry to a multiple of eight.
        let pad = 8 - entry.len() % 8;
        entry.extend(vec![0u8; pad]);
    }
    out.extend(entry);
    for (signature, data) in exts {
        out.extend(signature);
        out.extend((data.len() as u32).to_be_bytes());
        out.extend(data);
    }
    out
}

/// Writes `index_bytes` to `file` and appends its SHA-1 (python3's hashlib; no SHA-1 crate, R1).
fn write_sealed(file: &Path, bytes: Vec<u8>) {
    write_file(file, bytes);
    run_python(SEAL_SCRIPT, file);
}

fn write_index(cx: &Ctx, worktree: &Path, file: &Path, version: u32, exts: &[([u8; 4], Vec<u8>)]) {
    let blob = hex20(&cx.git(worktree, &["rev-parse", "HEAD:README.md"]));
    write_sealed(file, index_bytes(version, &blob, exts));
}

/// `depth` nested `TREE` nodes (the root is level 1), one invalidated child each.
fn tree_chain(depth: usize) -> Vec<u8> {
    let mut data = Vec::new();
    for level in 0..depth {
        let children = usize::from(level + 1 < depth);
        data.extend(if level == 0 { &b""[..] } else { &b"d"[..] });
        data.extend(format!("\0-1 {children}\n").as_bytes());
    }
    data
}

fn untr_header(blocks: usize) -> Vec<u8> {
    let mut data = git_varint(0); // empty identifier
    data.extend(vec![0u8; UNTR_FIXED_LEN]);
    data.push(0); // empty exclude_per_dir
    data.extend(git_varint(blocks as u64));
    data
}

/// Three empty EWAH bitmaps and the closing NUL every `UNTR` extension ends with.
fn untr_tail() -> Vec<u8> {
    let mut tail = EMPTY_EWAH.repeat(3);
    tail.push(0);
    tail
}

/// `depth` nested `UNTR` directory blocks, one sub-directory each.
fn untr_chain(depth: usize) -> Vec<u8> {
    let mut data = untr_header(depth);
    for level in 0..depth {
        data.extend(git_varint(0)); // untracked files
        data.extend(git_varint(u64::from(level + 1 < depth))); // sub-directories
        data.extend(b"d\0");
    }
    data.extend(untr_tail());
    data
}

/// `REUC` of `total` bytes: entries `a NUL 0 NUL 0 NUL 0 NUL` (no stage, so no object ids).
fn reuc(total: usize) -> Vec<u8> {
    const ENTRY: &[u8] = b"a\x000\x000\x000\x00";
    assert_eq!(total % ENTRY.len(), 0);
    ENTRY.repeat(total / ENTRY.len())
}

/// Depth-3 `TREE` of exactly `total` bytes: a root, 512 mid nodes and unique-named leaves under each.
fn tree_wide(total: usize) -> Vec<u8> {
    const MIDS: usize = 512;
    const LEAF_BYTES: usize = 5 + 1 + 5; // "00000" NUL "-1 0\n"
    let leaves = (total - 4096) / (MIDS * LEAF_BYTES);
    let mut data = format!("\0-1 {MIDS}\n").into_bytes();
    for mid in 0..MIDS {
        data.extend(format!("{mid:03}\0-1 {leaves}\n").as_bytes());
        for leaf in 0..leaves {
            data.extend(format!("{leaf:05}\0-1 0\n").as_bytes());
        }
    }
    // The shortfall lengthens the last leaf's name, which stays unique among its siblings.
    let shortfall = total - data.len();
    let at = data.len() - 5;
    data.splice(at..at, vec![b'z'; shortfall]);
    data
}

/// `UNTR` of exactly `total` bytes: a root block and eight child blocks of one-byte untracked names.
fn untr_wide(total: usize) -> Vec<u8> {
    let names = (total - 1024) / (2 * UNTR_WIDE_BLOCKS);
    let mut data = untr_header(1 + UNTR_WIDE_BLOCKS);
    data.extend(git_varint(0));
    data.extend(git_varint(UNTR_WIDE_BLOCKS as u64));
    data.push(0); // root name
    for block in 0..UNTR_WIDE_BLOCKS {
        data.extend(git_varint(names as u64));
        data.extend(git_varint(0));
        data.extend(format!("d{block}\0").as_bytes());
        data.extend(b"x\0".repeat(names));
    }
    data.extend(untr_tail());
    // The shortfall lengthens the last block's name; its name NUL sits before its names.
    let shortfall = total - data.len();
    let at = data.len() - untr_tail().len() - 2 * names - 1;
    data.splice(at..at, vec![b'z'; shortfall]);
    data
}

fn shared_index_name() -> String {
    format!("sharedindex.{}", "a".repeat(40))
}

/// `target` names the variant (see the manifest header); the overwritten `index` has a valid checksum.
fn index_bomb(cx: &Ctx, path: &Path, gitdir: &Path, target: &str) {
    let index = gitdir.join("index");
    let link = || [(*b"link", SHARED_INDEX_ID.to_vec())];
    let shared = gitdir.join(shared_index_name());
    match target {
        "tree_chain_1025" => write_index(
            cx,
            path,
            &index,
            2,
            &[(*b"TREE", tree_chain(TOO_DEEP_NODES))],
        ),
        "untr_chain_1025" => write_index(
            cx,
            path,
            &index,
            2,
            &[(*b"UNTR", untr_chain(TOO_DEEP_NODES))],
        ),
        "reuc_2mib" => write_index(cx, path, &index, 2, &[(*b"REUC", reuc(REUC_BYTES))]),
        "tree_wide_64mib" => write_index(
            cx,
            path,
            &index,
            2,
            &[(*b"TREE", tree_wide(TREE_WIDE_BYTES))],
        ),
        "untr_wide_8mib" => write_index(
            cx,
            path,
            &index,
            2,
            &[(*b"UNTR", untr_wide(UNTR_WIDE_BYTES))],
        ),
        "link_fifo" => {
            write_index(cx, path, &index, 2, &link());
            mkfifo(&shared);
        }
        "link_symlink_sparse" => {
            write_index(cx, path, &index, 2, &link());
            replace_with_symlink(&shared, &cx.sparse());
        }
        "link_chain" => {
            write_index(cx, path, &index, 2, &link());
            write_index(cx, path, &shared, 2, &link());
        }
        other => panic!("index_bomb target {other}"),
    }
}
