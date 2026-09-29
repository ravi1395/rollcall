#!/bin/sh
# Re-measures the external facts the v0 plan header relies on.
# Usage: scripts/probe-plan-facts.sh [path-to-gitoxide-checkout]
set -u
rustc --version
for c in gix rusqlite ratatui crossterm jiff clap; do cargo search --limit 1 "$c" | head -1; done
gx=${1:-$HOME/Projects/gitoxide}
if [ -d "$gx" ]; then
  git -C "$gx" log --oneline -1
  grep -m1 '^version' "$gx/gix/Cargo.toml"
  grep -H -E '^(jiff|libc|crossbeam-channel) ' "$gx/gix-date/Cargo.toml" "$gx/gix-features/Cargo.toml" "$gx/gix-fs/Cargo.toml"
fi
git remote -v | wc -l | sed 's/^ */git remotes: /'
echo "runner labels: see https://docs.github.com/en/actions/reference/runners/github-hosted-runners"
