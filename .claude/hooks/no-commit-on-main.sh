#!/usr/bin/env bash
# Refuse a commit that would land directly on `main`.
#
# PreToolUse payload on stdin. Exit 2 blocks and hands stderr back as the
# reason; every other path exits 0, so a missing `jq` never blocks work.
set -uo pipefail

command -v jq >/dev/null 2>&1 || exit 0

input=$(cat)
command=$(printf '%s' "$input" | jq -r '.tool_input.command // empty' 2>/dev/null) || exit 0
[ -n "${command:-}" ] || exit 0

# Anchored to a command position so a quoted `git commit` in prose is not a
# match. `-C <path>` and `-c <k>=<v>` are matched with their value, or
# `git -C . commit` slips through.
printf '%s' "$command" |
	grep -Eq '(^|[;&|(]|&&|\|\|)[[:space:]]*git([[:space:]]+(-[Cc][[:space:]]+[^[:space:]]+|-[^[:space:]]+))*[[:space:]]+commit([[:space:]]|$)' ||
	exit 0

# The command's own directory, not the project root: a commit inside a worktree
# is on that worktree's branch.
cwd=$(printf '%s' "$input" | jq -r '.cwd // empty' 2>/dev/null)
[ -n "${cwd:-}" ] || cwd="${CLAUDE_PROJECT_DIR:-$PWD}"

branch=$(git -C "$cwd" rev-parse --abbrev-ref HEAD 2>/dev/null) || exit 0
[ "$branch" = "main" ] || exit 0

cat >&2 <<'EOF'
Refusing: HEAD is `main`, and this commit would land on it directly.

CLAUDE.md: "Never commit straight to `main`. The pull request is what leaves a
reviewable record of work nobody watched happen."

Put the work on a branch first. Anything larger than a one-line fix gets its
own worktree:

    git worktree add .claude/worktrees/<name> -b <name>

If this really has to go on main, run the commit outside Claude Code.
EOF
exit 2
