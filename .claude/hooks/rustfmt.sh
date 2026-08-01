#!/usr/bin/env bash
# Format a Rust file the moment it is written, so `make` never fails on
# whitespace.
#
# PostToolUse payload on stdin. Never fails the tool call: every path exits 0
# and leaves the edit alone.
set -uo pipefail

command -v jq >/dev/null 2>&1 || exit 0

path=$(jq -r '.tool_input.file_path // .tool_response.filePath // empty' 2>/dev/null) || exit 0
[ -n "${path:-}" ] || exit 0

case "$path" in
*.rs) ;;
*) exit 0 ;;
esac

# From the edited file's own directory: `cargo fmt` walks up from there, which
# is how an edit in a worktree formats that checkout and not the main one.
dir=$(dirname "$path")
[ -d "$dir" ] || exit 0
cd "$dir" || exit 0

cargo fmt >/dev/null 2>&1 || true
exit 0
