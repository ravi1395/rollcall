# rollcall

**Every repo, every worktree, and the Claude Code or Codex session that left it dirty — one screen, one key to resume.**

![rollcall demo](demo.gif)

Point rollcall at a folder. It finds every git repo and linked worktree under it, shows what is dirty, unpushed,
stale or merged, and next to each worktree the last Claude Code or Codex session that worked there, with its title.
Press `R` to resume that session in the right directory.

With no agent logs on the machine, rollcall is a plain multi-repo git dashboard and shows no agent UI.

## How is this different?

Each piece exists separately; the combination does not:

| Tool | Multi-repo discovery | Per-worktree git state | Claude + Codex history | One-key resume in right dir |
|---|---|---|---|---|
| Claude Code `/resume`, `codex resume --all` | — | — | one agent each | yes |
| worktrunk, gita | gita only | yes | — | — |
| agent-deck, ccmanager | yes | partial | live processes only | — |
| agent-sessions, nicknisi/sessions | — | — | yes | copy / print line |
| xlaude | — | — | last prompt per worktree | opens agent |
| **rollcall** | **yes** | **yes** | **yes** | **yes** |

## Install

macOS (arm64, x86_64) and Linux (x86_64, arm64). Windows is not supported.

```sh
# Homebrew
brew install ravi1395/tap/rollcall-cli

# Shell installer (release binaries with SHA-256 checksums and GitHub attestations)
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/ravi1395/rollcall/releases/latest/download/rollcall-cli-installer.sh | sh

# From source
cargo install --locked --git https://github.com/ravi1395/rollcall rollcall-cli
```

## Use

```sh
rollcall ~/Projects                  # TUI
rollcall status ~/Projects           # plain table; --json for the StatusDoc schema in schema/
rollcall scan ~/Projects             # refresh the cache, one summary line
rollcall doctor ~/Projects           # what was read, what did not match, and why
```

In the TUI: `j`/`k` move, `enter` expands, `/` filters, `R` resumes the selected session, `Y` copies the resume
command, `c` copies a `cd` line, `?` shows every key.

rollcall refuses `/` and `$HOME` as roots (it suggests the project folders it finds instead); pass `--yes` or list the
root under `roots` in `~/.config/rollcall/config.toml` to allow one.

### Land in the session's directory: `rc`

`R` runs the agent in the session's directory, but your shell stays where it was once the agent exits. To have the
shell follow, add the `rc` function and use `rc` in place of `rollcall`:

```sh
eval "$(rollcall init zsh)"           # in ~/.zshrc
eval "$(rollcall init bash)"          # in ~/.bashrc
rollcall init fish | source           # in ~/.config/fish/config.fish
```

## MCP: "where was I?"

`rollcall mcp` is a read-only stdio MCP server with two tools, `where_was_i` and `list_stale`. Run one scan first so
the first answer comes from a warm cache:

```sh
rollcall scan ~/Projects
```

Claude Code:

```sh
claude mcp add rollcall -- rollcall mcp --root "$HOME/Projects"
```

Codex (`~/.codex/config.toml`):

```toml
[mcp_servers.rollcall]
command = "rollcall"
args = ["mcp", "--root", "/Users/you/Projects"]
```

`--root` can repeat. Without `--prompts` the server never returns prompts or prompt-derived titles; external
worktrees are left out of its answers.

## Privacy

- Local only: rollcall has no network code (network crates are banned in `deny.toml`). It writes only its cache
  (`~/Library/Caches/rollcall` or `~/.cache/rollcall`, mode `0700`) and config directories.
- The cache stores no log text: titles and prompts are pointers re-read from the logs when shown.
- Titles and prompts are stripped of terminal escape sequences and invisible characters in every output, and
  common secrets (API keys, tokens, JWTs, private keys, URL credentials) are replaced with `[redacted]`. Set
  `redact_emails = true` to scrub email addresses too, or `redact_prompts = true` to hide titles and prompts
  everywhere.
- Reading a repo never runs its hooks, filters, diff or merge drivers, or anything its config names.

## Limitations

- **Squash merges** are not detected: a worktree whose branch was squash-merged is not shown as merged.
- **Bare repos inside other repos** (test fixtures such as `fixtures/foo.git`) are listed as repos.
- **Repos owned by another user** open with their repo-local config ignored, so their upstream, ahead and behind
  fields are empty.
- **Filters and LFS:** rollcall never runs clean/smudge filters, so files using them (Git LFS, custom eol) can
  count as dirty where `git status` says clean.
- **Subdirectory ignore and attributes files:** `.gitignore` and `.gitattributes` files in subdirectories of a
  hostile unpacked tree are read without a size cap; a symlink there to `/dev/zero` can exhaust memory.
- **Without `rc`, your shell stays in its directory** after `R`; only the agent runs in the session's directory.
- **The agent's own workspace-trust prompts** still apply when `R` resumes a session inside a repo the agent has
  not trusted yet.

## License

MIT or Apache-2.0, at your option.
