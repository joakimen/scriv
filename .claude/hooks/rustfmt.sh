#!/usr/bin/env bash
# Format a Rust file the moment it is written, so `make` never fails on
# whitespace.
#
# `fmt-check` is the first thing `make` runs, so an unformatted edit costs a
# full gate — clippy, tests and a release build never start — to be told about
# indentation. Running `cargo fmt` here removes that failure mode entirely
# rather than making it faster to hit.
#
# Reads the PostToolUse payload on stdin. Never fails the tool call: a missing
# `jq`, a path outside a cargo project, or a `cargo fmt` that cannot parse
# half-written code all exit 0 and leave the edit alone.
set -uo pipefail

command -v jq >/dev/null 2>&1 || exit 0

path=$(jq -r '.tool_input.file_path // .tool_response.filePath // empty' 2>/dev/null) || exit 0
[ -n "${path:-}" ] || exit 0

case "$path" in
*.rs) ;;
*) exit 0 ;;
esac

# Format from the edited file's own directory rather than the project root.
# Feature work here happens in git worktrees under `.claude/worktrees/`, each a
# separate checkout with its own `Cargo.toml`; `cargo fmt` walks up from the
# working directory, so this reaches the right one instead of formatting the
# main checkout on every edit made in a worktree.
dir=$(dirname "$path")
[ -d "$dir" ] || exit 0
cd "$dir" || exit 0

cargo fmt >/dev/null 2>&1 || true
exit 0
