#!/usr/bin/env python3
"""Coverage checks for tests/fixtures/MANIFEST.toml (stdlib only).

  check-manifest.py git            every git-fixture bullet is covered (P07)
  check-manifest.py sessions       every session-fixture bullet is covered (P08)
  check-manifest.py expect <key>   print the hand-written [expect_counts].<key> (P09, P10)

The bullet names below are the exact strings a `covers` array must use. They
follow spec section 10.2 (plus a few section 10.1 / 12.1 fixtures that are not
section 10.2 bullets, marked "extra"). Set MANIFEST=<path> to check another file.
"""
import os
import sys
import tomllib
from pathlib import Path

MANIFEST = Path(os.environ.get("MANIFEST") or Path(__file__).resolve().parent.parent / "tests/fixtures/MANIFEST.toml")

ROOTS = {"F", "EXT", "H", "budget", "home", "perf", "demo"}
GIT_ROOTS = {"F", "EXT", "H", "budget", "home"}  # roots P07 owns; perf/demo repos come from P08
KINDS = {"normal", "bare", "nested", "submodule", "submodule_old", "special"}
SPECIALS = {
    "fifo_git", "symlink_fanout", "symlink_out_of_root", "unreadable_dir", "corrupted", "bad_gitdir",
    "untrusted_path", "drivers", "submodule_lfs", "worktree_escape", "commondir_escape",
    "symlinked_files", "worktree_config_symlink", "big_object", "hang_probe",
    "hang_probe_worktree_config", "enclosing_root", "tree_bomb", "tree_bomb_long", "excludes_file",
    "index_bomb", "index_deep_ok", "nested_probe", "exec_type", "big_tracked", "pattern_blob_bomb", "rename_bomb",  # P07b
    "gitoxide_keys", "pack_bomb", "objects_pack_flood", "tree_deep_big",  # P07c
    "dot_ref",  # spec rev 16 (I1)
}
REPO_KEYS = {
    "id", "root", "path", "kind", "special", "in_clean", "target", "covers", "parent", "commits",
    "branches", "head", "remotes", "upstreams", "origin_head", "worktrees", "state", "edit", "config",
}
COMMIT_KEYS = {"branch", "message", "date", "files"}

# Section 10.1: F_clean is F minus corrupted, bad gitdir, untrusted git dir, untrusted worktree path,
# hostile repos, hang probe, unreadable dir, and anything that produces `partial` (the budget root).
# Encoded by root and special so that a mislabelled repo fails in either direction.
NOT_CLEAN_ROOTS = {"H", "budget"}
NOT_CLEAN_SPECIALS = {
    "corrupted", "bad_gitdir", "untrusted_path", "unreadable_dir", "symlinked_files",
    "worktree_config_symlink", "drivers", "submodule_lfs", "worktree_escape", "commondir_escape",
    "big_object", "tree_bomb", "tree_bomb_long", "hang_probe", "hang_probe_worktree_config", "fifo_git", "excludes_file",
}
# `symlinked_files` must cover each of these targets (spec 10.2 "symlinked-files").
SYMLINK_TARGETS = {
    "config", "packed-refs", "shallow", "objects/info/commit-graphs/commit-graph-chain",
    "objects/info/alternates#2", "worktree:.gitmodules", "config.worktree", "HEAD:EVIL",
}

GIT_BULLETS = [
    # 10.2 Discovery
    "discovery: normal", "discovery: linked worktree", "discovery: case A", "discovery: case B",
    "discovery: missing worktree", "discovery: untrusted path", "discovery: modern submodule",
    "discovery: old-style submodule", "discovery: bare", "discovery: nested",
    "discovery: gitignored folder", "discovery: node_modules", "discovery: target dir",
    "discovery: symlink fan-out", "discovery: symlink out of root", "discovery: unreadable dir",
    "discovery: enclosing root", "discovery: refused-root suggestions",
    # 10.2 Git state
    "git state: staged", "git state: unstaged", "git state: untracked", "git state: excludesFile",
    "git state: ahead/behind", "git state: no upstream with remote", "git state: no remote",
    "git state: detached HEAD", "git state: merged", "git state: inactive",
    "git state: no default branch", "git state: corrupted",
    # 10.2 Deadline (a run on F_clean, not a repo: tagged on ordinary in_clean repos)
    "deadline: F_clean timed-out run",
    # 10.2 Hostile repos (repo-shaped items; the fake binaries, positive controls and assertions are harness work)
    "hostile: drivers", "hostile: submodule", "hostile: worktree-escape", "hostile: commondir-escape",
    "hostile: symlinked-files", "hostile: big-object", "hostile: tree-bomb", "hostile: tree-bomb-long",
    "hostile: hang-probe", "hostile: excludes-file",
    # revision 12 (P07b)
    "hostile: index-bombs", "hostile: index-deep-ok", "hostile: nested-probe", "hostile: exec-type",
    "hostile: big-tracked", "hostile: pattern-blob-bomb", "hostile: rename-bomb",
    # revision 13 (P07c)
    "hostile: gitoxide-keys", "hostile: pack-bombs", "hostile: objects-pack-flood", "hostile: tree-deep-big",
    # revision 16 (I1)
    "hostile: dot-ref",
    # extra: fixtures the spec requires but 10.2 does not list as bullets
    "extra: bad gitdir", "extra: untrusted git dir", "extra: fifo-git", "extra: walk budget",
]

SESSION_BULLETS = [
    # 10.2 Claude fixtures (each 5.1 rule)
    "claude: tool_result last user record", "claude: isMeta", "claude: command-name",
    "claude: injected marker after user text", "claude: prompt block filter", "claude: isSidechain",
    "claude: title custom", "claude: title ai", "claude: title prompt",
    "claude: pre-truncated lastPrompt", "claude: lastPrompt with zero human prompts",
    "claude: multi-cwd", "claude: HEAD branch", "claude: missing gitBranch", "claude: id from filename",
    "claude: subagent file", "claude: non-UUID id", "claude: oversize line", "claude: malformed line",
    "claude: unknown record type",
    # 10.2 Codex fixtures
    "codex: spawn-edge child", "codex: source subagent child", "codex: parent_thread_id child",
    "codex: archived thread", "codex: empty title", "codex: title equals first_user_message",
    "codex: title kind thread", "codex: empty git_branch with rollout branch",
    "codex: rollout_path outside CODEX_HOME", "codex: rollout user_message events",
    "codex: rollout without user_message", "codex: rollout injection then user blocks",
    "codex: database absent fallback", "codex: rollout in archived_sessions", "codex: second session_meta",
    # 10.2 Incremental reads (inode reuse and parser_version are unit tests, not manifest cases)
    "incremental: append 3 lines plus partial", "incremental: truncate", "incremental: truncate then regrow",
    "incremental: title upgrade prompt to custom", "incremental: vanished file",
    # 10.2 Pointer resolution
    "pointer: log replaced after scan", "pointer: symlink swapped in",
    # 10.2 Matching
    "matching: 16 matchable", "matching: older_sessions", "matching: hidden submodule matches parent",
    "matching: non_repo", "matching: path_gone", "matching: outside_root",
    # 10.2 Privacy: one per section 9 scrub pattern, then truncation (16 prompt sessions)
    "privacy: sk-", "privacy: sk_live/rk_test", "privacy: AKIA", "privacy: AIza", "privacy: gh token",
    "privacy: github_pat_", "privacy: npm_", "privacy: xox", "privacy: JWT", "privacy: Bearer",
    "privacy: private key", "privacy: password=", "privacy: url credentials", "privacy: email",
    "privacy: truncation multi-byte grapheme", "privacy: truncation pre-ellipsised",
    # 10.2 Sanitising
    "sanitise: ESC [2J", "sanitise: C1 0x9b", "sanitise: U+202E", "sanitise: U+200B",
    "sanitise: branch name", "sanitise: prompt",
    # 10.2 Shell safety
    "shell: path space", "shell: path dollar", "shell: path backtick", "shell: path semicolon",
    "shell: path single quote", "shell: path newline", "shell: branch command substitution",
]
KNOWN_COVERS = set(GIT_BULLETS) | set(SESSION_BULLETS)

CORPORA = {"matching", "claude_rules", "codex_rules", "incremental", "privacy", "sanitise", "shell"}
UNMATCHED_CLASSES = ("non_repo", "path_gone", "outside_root")
SESSION_TABLES = ("claude", "codex_thread", "codex_rollout")
MIN_PRIVACY_PROMPTS = 16
MATCHING_TOTAL = 20


def load():
    try:
        with open(MANIFEST, "rb") as f:
            return tomllib.load(f)
    except (OSError, tomllib.TOMLDecodeError) as e:
        sys.exit(f"error: cannot load {MANIFEST}: {e}")


def covered(tables):
    return {c for t in tables for c in t.get("covers", [])}


def report(errors, ok_line):
    if errors:
        print("\n".join(f"error: {e}" for e in errors), file=sys.stderr)
        sys.exit(1)
    print(ok_line)


def committed_content(repo, path):
    """The last committed content of `path` in `repo`, or None."""
    found = None
    for c in repo.get("commits", []):
        found = c.get("files", {}).get(path, found)
    return found


def check_repo(r, ids, errors):
    rid = r.get("id", "<missing id>")
    for k in set(r) - REPO_KEYS:
        errors.append(f"repo {rid}: unknown key {k!r}")
    if r.get("root") not in ROOTS:
        errors.append(f"repo {rid}: root {r.get('root')!r} not in {sorted(ROOTS)}")
    if r.get("kind") not in KINDS:
        errors.append(f"repo {rid}: kind {r.get('kind')!r} not in {sorted(KINDS)}")
    special = r.get("special")
    if r.get("kind") == "special":
        if special not in SPECIALS:
            errors.append(f"repo {rid}: special {special!r} is not in the schema enum")
    elif special is not None:
        errors.append(f"repo {rid}: special {special!r} set on kind {r.get('kind')!r}")
    for i, c in enumerate(r.get("commits", [])):
        if set(c) - COMMIT_KEYS or {"branch", "message", "date"} - set(c):
            errors.append(f"repo {rid}: commits[{i}] keys must be within {sorted(COMMIT_KEYS)} and include branch, message, date")
    head = r.get("head", "")
    if head.startswith("detached:"):
        idx = head.split(":", 1)[1]
        if not idx.isdigit() or int(idx) >= len(r.get("commits", [])):
            errors.append(f"repo {rid}: head {head!r} indexes past commits")
    for path, new in r.get("edit", {}).items():
        old = committed_content(r, path)
        if old is None:
            errors.append(f"repo {rid}: edit of {path!r} has no committed content")
        elif len(old.encode()) != len(new.encode()):
            errors.append(f"repo {rid}: edit of {path!r} changes byte length ({len(old.encode())} -> {len(new.encode())})")
    for w in r.get("worktrees", []):
        if "path" not in w or "branch" not in w:
            errors.append(f"repo {rid}: worktree {w!r} needs path and branch")
    for cov in r.get("covers", []):
        if cov not in KNOWN_COVERS:
            errors.append(f"repo {rid}: unknown covers entry {cov!r}")
    if r.get("root") in GIT_ROOTS and not r.get("covers"):
        errors.append(f"repo {rid}: covers must be a non-empty array")
    if r.get("root") in GIT_ROOTS:
        expect_dirty = r.get("root") in NOT_CLEAN_ROOTS or special in NOT_CLEAN_SPECIALS
        in_clean = r.get("in_clean", True)
        if in_clean == expect_dirty:
            errors.append(f"repo {rid}: in_clean = {in_clean} but the section 10.1 exclusion list says in_clean = {not expect_dirty}")
    if rid in ids:
        errors.append(f"duplicate repo id {rid!r}")
    ids.add(rid)


def check_git(m):
    repos = m.get("repo", [])
    errors, ids = [], set()
    for r in repos:
        check_repo(r, ids, errors)
    for r in repos:
        for ref in [r.get("parent")] + [x.get("target_repo_id") for x in r.get("remotes", [])]:
            if ref is not None and ref not in ids:
                errors.append(f"repo {r.get('id')}: references unknown repo id {ref!r}")
        if r.get("kind") in ("nested", "submodule", "submodule_old") and not r.get("parent"):
            errors.append(f"repo {r.get('id')}: kind {r.get('kind')} needs parent")
    git_repos = [r for r in repos if r.get("root") in GIT_ROOTS]
    for root in GIT_ROOTS:
        if not any(r["root"] == root for r in git_repos):
            errors.append(f"no repo for root {root!r}")
    for bullet in GIT_BULLETS:
        if bullet not in covered(git_repos):
            errors.append(f"bullet not covered: {bullet!r}")
    for special in SPECIALS - {r.get("special") for r in git_repos}:
        errors.append(f"special {special!r} is never used")
    targets = {r.get("target") for r in git_repos if r.get("special") == "symlinked_files"}
    for missing in SYMLINK_TARGETS - targets:
        errors.append(f"no symlinked_files repo with target {missing!r}")
    for k in ("F_repos", "F_worktrees"):
        if not isinstance(m.get("expect_counts", {}).get(k), int):
            errors.append(f"[expect_counts].{k} must be a hand-written integer")
    if not isinstance(m.get("home", {}).get("expected_suggestions"), list):
        errors.append("[home].expected_suggestions must be a list")
    if not isinstance(m.get("meta", {}).get("now"), str):
        errors.append("[meta].now is required")
    report(errors, f"ok: {len(repos)} repos, all bullets covered")


def sessions_of(m):
    return {t: m.get(t, []) for t in SESSION_TABLES}


def matching_counts(tables):
    """Matched / unmatched tallies over corpus 'matching'. A rollout that a codex_thread names via
    rollout_id is that thread's body, not a second session."""
    linked = {t.get("rollout_id") for t in tables["codex_thread"]}
    rows = [
        s
        for name, ts in tables.items()
        for s in ts
        if s.get("corpus") == "matching" and not (name == "codex_rollout" and s.get("id") in linked)
    ]
    counts = {"matched": 0, **{c: 0 for c in UNMATCHED_CLASSES}}
    for s in rows:
        if "worktree" in s:
            counts["matched"] += 1
        elif s.get("unmatched") in UNMATCHED_CLASSES:
            counts[s["unmatched"]] += 1
    return counts, len(rows)


def carries_prompt(s):
    e = s.get("expect", {})
    return bool(e.get("last_prompt")) or (bool(e.get("title")) and e.get("title_kind") == "prompt")


def check_sessions(m):
    tables = sessions_of(m)
    rows = [s for ts in tables.values() for s in ts]
    if not rows:
        sys.exit("error: no [[claude]] / [[codex_thread]] / [[codex_rollout]] tables in the manifest (P08 writes these)")
    errors = []
    for s in rows:
        if s.get("corpus") not in CORPORA:
            errors.append(f"session {s.get('id')!r}: corpus {s.get('corpus')!r} not in {sorted(CORPORA)}")
        for cov in s.get("covers", []):
            if cov not in KNOWN_COVERS:
                errors.append(f"session {s.get('id')!r}: unknown covers entry {cov!r}")
    for bullet in SESSION_BULLETS:
        if bullet not in covered(rows):
            errors.append(f"bullet not covered: {bullet!r}")
    expected = m.get("matching", {})
    counts, total = matching_counts(tables)
    for key, got in counts.items():
        if expected.get(key) != got:
            errors.append(f"[matching].{key} = {expected.get(key)!r} but the manifest has {got}")
    if total != MATCHING_TOTAL or sum(counts.values()) != MATCHING_TOTAL:
        errors.append(f"corpus 'matching' has {total} sessions ({sum(counts.values())} classified), need {MATCHING_TOTAL}")
    if sum(expected.get(k, 0) for k in counts) != MATCHING_TOTAL:
        errors.append(f"[matching] values must sum to {MATCHING_TOTAL}")
    prompts = sum(1 for s in rows if s.get("corpus") == "privacy" and carries_prompt(s))
    if prompts < MIN_PRIVACY_PROMPTS:
        errors.append(f"corpus 'privacy' has {prompts} sessions with prompt text, need >= {MIN_PRIVACY_PROMPTS}")
    report(errors, "ok")


def expect(m, key):
    value = m.get("expect_counts", {}).get(key)
    if not isinstance(value, int) or isinstance(value, bool):
        sys.exit(f"error: [expect_counts].{key} is missing or not an integer")
    print(value)


def main(argv):
    if len(argv) == 2 and argv[1] == "git":
        check_git(load())
    elif len(argv) == 2 and argv[1] == "sessions":
        check_sessions(load())
    elif len(argv) == 3 and argv[1] == "expect":
        expect(load(), argv[2])
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main(sys.argv)
