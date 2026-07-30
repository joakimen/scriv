#!/bin/sh
# Build a self-contained sandbox for the README demo.
#
# The demo must never show real repositories, branches, or pull requests, and it
# must look the same every time it is recorded. Both fall out of the seams scriv
# already has, so nothing here needs a "demo mode" in the binary:
#
#   HOME, XDG_CONFIG_HOME  point config discovery at the sandbox; paths then
#                          render as ~/dev/... with the sandbox as the home
#   PATH                   a stub `gh` earlier on PATH than the real one, so
#                          pull requests come from canned JSON, never the network
#
# Commit dates are written as offsets from *now*, so relative dates ("2 days
# ago") read identically whenever the demo is re-recorded.
#
# Usage: demo/fixture.sh <dir>    (the directory is wiped and rebuilt)
set -eu

FIX=${1:?usage: demo/fixture.sh <dir>}
case $FIX in
    /*) ;;
    *) FIX=$PWD/$FIX ;;
esac

# Where the recording finds `scriv`; the release build by default.
SCRIV_BIN_DIR=${SCRIV_BIN_DIR:-$PWD/target/release}

rm -rf "$FIX"
# `remotes` lives outside dev/ so the fixture's bare repositories are never
# walked by repository discovery.
mkdir -p "$FIX/bin" "$FIX/.config/scriv" "$FIX/remotes" \
    "$FIX/dev/github.com/acme" "$FIX/dev/github.com/personal" "$FIX/notes"

# Keep the user's real git identity, aliases, hooks, and init.defaultBranch out
# of the fixture: every repository below is built from nothing.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
export GIT_AUTHOR_NAME='Ada Lovelace' GIT_AUTHOR_EMAIL='ada@example.com'
export GIT_COMMITTER_NAME='Ada Lovelace' GIT_COMMITTER_EMAIL='ada@example.com'

NOW=$(date +%s)

# A timestamp `seconds` in the past, in git's internal format.
ago() { echo "@$((NOW - $1)) +0000"; }

# commit <repo> <seconds-ago> <message>
commit() {
    GIT_AUTHOR_DATE=$(ago "$2") GIT_COMMITTER_DATE=$(ago "$2") \
        git -C "$1" commit -q --allow-empty -m "$3"
}

# new_repo <path> — an empty repository on `main` with its own bare remote.
new_repo() {
    remote=$FIX/remotes/$(basename "$1").git
    git init -q -b main "$1"
    git init -q --bare "$remote"
    git -C "$1" remote add origin "$remote"
}

HOUR=3600
DAY=86400

# --- acme/billing-api: the repository the demo drives ------------------------
# Carries one branch of each kind, so the picker shows all three colours.
API=$FIX/dev/github.com/acme/billing-api
new_repo "$API"
commit "$API" $((3 * DAY)) 'feat: meter requests'
commit "$API" $((2 * DAY)) 'fix: round usage up'
git -C "$API" push -q -u origin main

# Pushed, then deleted locally: shows up as remote-only (cyan).
git -C "$API" checkout -q -b feat/token-bucket
commit "$API" $((26 * HOUR)) 'feat: token bucket'
commit "$API" $DAY 'test: cover refill'
git -C "$API" push -q -u origin feat/token-bucket
git -C "$API" checkout -q main
git -C "$API" branch -q -D feat/token-bucket

# Never pushed: local-only (yellow).
git -C "$API" checkout -q -b spike/redis-cache
commit "$API" $((2 * HOUR)) 'spike: try redis cache'

# Pushed and kept: local and remote (green).
git -C "$API" checkout -q -b fix/off-by-one
commit "$API" $HOUR 'fix: off-by-one window'
git -C "$API" push -q -u origin fix/off-by-one
git -C "$API" checkout -q main

# --- a handful of other repositories, so `repo pick` has something to filter --
for repo in checkout-web invoice-worker; do
    R=$FIX/dev/github.com/acme/$repo
    new_repo "$R"
    commit "$R" $((5 * DAY)) 'chore: bump dependencies'
    git -C "$R" push -q -u origin main
done

for repo in dotfiles kingkiller-notes; do
    R=$FIX/dev/github.com/personal/$repo
    new_repo "$R"
    commit "$R" $((9 * DAY)) 'docs: write it down before forgetting'
    git -C "$R" push -q -u origin main
done

# --- configuration -----------------------------------------------------------
cat > "$FIX/.config/scriv/config.toml" <<EOF
[repo]
root = "~/dev/github.com"
ignore = ["node_modules", "target"]
display = "relative"

# Labels name owners, so the picker colours acme's repos as work.
labels = { work = ["acme"], personal = ["personal"] }

[picker]
height = "100%"
preview = true
preview_window = "right:38%"
EOF

# The known-files list, with files that exist so previews have something to show.
cat > "$FIX/notes/standup.md" <<'EOF'
# Standup

- rate limiting: token bucket landed behind a flag
- next: decide redis vs in-process for the quota cache
EOF
cat > "$FIX/.config/scriv/files" <<'EOF'
~/.config/scriv/config.toml
~/notes/standup.md
EOF

# --- stub `gh` ---------------------------------------------------------------
# scriv runs `gh` through PATH, so a script named `gh` earlier on PATH replaces
# it wholesale. The demo therefore needs no network and no GitHub account.
cat > "$FIX/bin/gh" <<'EOF'
#!/bin/sh
# Stand-in for the GitHub CLI, used only by the demo fixture.
if [ "$1" = "pr" ] && [ "$2" = "list" ]; then
    cat <<'JSON'
[
  {"number":128,"title":"Add a token bucket per API key","author":{"login":"ada"},
   "headRefName":"feat/token-bucket","isDraft":false,"state":"OPEN",
   "updatedAt":"2026-07-27T09:12:33Z","mergeable":"MERGEABLE",
   "statusCheckRollup":[
     {"__typename":"CheckRun","name":"build","workflowName":"ci","status":"COMPLETED","conclusion":"SUCCESS"},
     {"__typename":"CheckRun","name":"test","workflowName":"ci","status":"COMPLETED","conclusion":"SUCCESS"},
     {"__typename":"CheckRun","name":"lint","workflowName":"ci","status":"COMPLETED","conclusion":"SUCCESS"}],
   "body":"Replaces the fixed window with a token bucket, so a client that is quiet for a minute can burst afterwards.\n\n- refill is lazy, computed on read\n- burst size is one minute of quota\n- the old limiter stays behind a flag for one release"},
  {"number":127,"title":"Round partial usage up","author":{"login":"grace"},
   "headRefName":"fix/round-usage","isDraft":false,"state":"OPEN",
   "updatedAt":"2026-07-26T16:40:00Z","mergeable":"MERGEABLE",
   "statusCheckRollup":[
     {"__typename":"CheckRun","name":"build","workflowName":"ci","status":"COMPLETED","conclusion":"SUCCESS"},
     {"__typename":"CheckRun","name":"test","workflowName":"ci","status":"COMPLETED","conclusion":"FAILURE"},
     {"__typename":"CheckRun","name":"lint","workflowName":"ci","status":"COMPLETED","conclusion":"SUCCESS"}],
   "body":"A request costing 0.4 units was billed as 0. Round up at the meter instead of at the invoice."},
  {"number":126,"title":"Cache quotas in redis","author":{"login":"ada"},
   "headRefName":"spike/redis-cache","isDraft":true,"state":"OPEN",
   "updatedAt":"2026-07-25T11:02:00Z","mergeable":"CONFLICTING",
   "statusCheckRollup":[
     {"__typename":"CheckRun","name":"build","workflowName":"ci","status":"COMPLETED","conclusion":"SUCCESS"},
     {"__typename":"CheckRun","name":"test","workflowName":"ci","status":"IN_PROGRESS","conclusion":""}],
   "body":"Spike, not for review yet. Measuring whether the round trip is cheaper than recomputing."},
  {"number":124,"title":"Meter requests per API key","author":{"login":"ada"},
   "headRefName":"feat/metering","isDraft":false,"state":"MERGED",
   "updatedAt":"2026-07-22T08:15:00Z","mergeable":"UNKNOWN",
   "statusCheckRollup":[
     {"__typename":"CheckRun","name":"build","workflowName":"ci","status":"COMPLETED","conclusion":"SUCCESS"},
     {"__typename":"CheckRun","name":"test","workflowName":"ci","status":"COMPLETED","conclusion":"SUCCESS"}],
   "body":"The metering groundwork everything else builds on."}
]
JSON
    exit 0
fi

# `gh pr checkout` creates the local branch and sets its upstream. The stub does
# the same thing locally, so the demo can show the flow end to end.
if [ "$1" = "pr" ] && [ "$2" = "checkout" ]; then
    case $3 in
        128) branch=feat/token-bucket ;;
        *) echo "demo stub: no branch for pull request $3" >&2; exit 1 ;;
    esac
    git fetch -q origin "$branch" 2>/dev/null || true
    git checkout -q -b "$branch" --track "origin/$branch" 2>/dev/null ||
        git checkout -q "$branch"
    echo "Switched to branch '$branch'"
    echo "branch '$branch' set up to track 'origin/$branch'."
    exit 0
fi

# `open` and `merge` reach the network for real, so the stub only says what it
# would have done — enough to try the pickers by hand without a GitHub account.
if [ "$1" = "pr" ] && [ "$2" = "view" ] && [ "$3" = "--web" ]; then
    echo "demo stub: would open https://github.com/acme/billing-api/pull/$4"
    exit 0
fi

if [ "$1" = "pr" ] && [ "$2" = "merge" ]; then
    echo "demo stub: would merge pull request $3"
    exit 0
fi

echo "demo stub: unsupported gh invocation: $*" >&2
exit 1
EOF
chmod +x "$FIX/bin/gh"

# --- environment for the tape ------------------------------------------------
# Sourced by the recording so the sandbox applies to the whole session.
cat > "$FIX/env.sh" <<EOF
export HOME='$FIX'
export XDG_CONFIG_HOME='$FIX/.config'
export PATH='$FIX/bin':'$SCRIV_BIN_DIR':"\$PATH"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
export PS1='❯ '
cd '$API'
clear
EOF

echo "fixture ready: $FIX"
