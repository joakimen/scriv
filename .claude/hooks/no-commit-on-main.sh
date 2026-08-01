#!/usr/bin/env bash
# Refuse a commit that would land directly on `main`.
#
# CLAUDE.md says never to commit straight to main, and until now nothing
# enforced it. The failure is not expensive to fix but it is expensive to fix
# *well*: undoing it means a reset and a force-push over a branch other people
# may already have pulled, which is exactly the kind of history rewrite the
# same document says to stop and ask about.
#
# Reads the PreToolUse payload on stdin. Exit 2 blocks the command and hands
# this script's stderr back as the reason; every other path exits 0, so a
# missing `jq` or a directory that is not a repository never blocks work.
set -uo pipefail

command -v jq >/dev/null 2>&1 || exit 0

input=$(cat)
command=$(printf '%s' "$input" | jq -r '.tool_input.command // empty' 2>/dev/null) || exit 0
[ -n "${command:-}" ] || exit 0

# Only a command that actually *runs* `git commit`. Matching the bare substring
# would block `rg 'git commit' CLAUDE.md` and any pull request body that quotes
# the command — so the match is anchored to a command position: the start of a
# line, or just after a separator.
#
# `-C <path>` and `-c <name>=<value>` take a value, so they are matched together
# with it; without that, `git -C . commit` reads as `git`, the flag `-C`, and
# then `.` where `commit` was expected, and slips through. Any other global
# option is a single token.
#
# It errs toward blocking. A `gh pr create --body` whose text happens to begin a
# line with `git commit` is refused, which costs a reword; the other direction
# costs a reset and a force-push over a branch other people may have pulled.
printf '%s' "$command" |
	grep -Eq '(^|[;&|(]|&&|\|\|)[[:space:]]*git([[:space:]]+(-[Cc][[:space:]]+[^[:space:]]+|-[^[:space:]]+))*[[:space:]]+commit([[:space:]]|$)' ||
	exit 0

# The branch that matters is the one in the directory the command will run in,
# not the project root: a commit made inside a worktree is on that worktree's
# branch.
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
