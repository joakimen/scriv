#!/bin/sh
# Render the README demo with VHS.
#
# Builds scriv, generates the sandbox the tape runs against, then plays the
# tape. Recording is a deliberate act rather than something CI does on every
# push: the rendered GIF depends on the fonts installed on the machine, so a
# CI-generated one would differ from a locally generated one for no good reason.
# CI runs this with --check instead, which renders to a throwaway path purely to
# catch a tape that has stopped working.
#
# Usage: demo/record.sh [--check]
set -eu

CHECK=false
[ "${1:-}" = "--check" ] && CHECK=true

if ! command -v vhs > /dev/null 2>&1; then
    echo "demo: vhs is not installed." >&2
    echo "  macOS:  brew install vhs" >&2
    echo "  other:  https://github.com/charmbracelet/vhs#installation" >&2
    exit 1
fi

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

cargo build --release
SCRIV_BIN_DIR=$ROOT/target/release
export SCRIV_BIN_DIR

sh demo/fixture.sh target/demo-fixture

# VHS drives a real terminal over a local socket, and very occasionally loses it
# mid-recording ("use of closed network connection"). That is a flake in the
# recorder, not a broken tape, so give it a second go before failing.
play() {
    vhs "$@" || {
        echo "demo: recording failed, retrying once..." >&2
        sleep 2
        vhs "$@"
    }
}

if [ "$CHECK" = true ]; then
    # Render somewhere disposable; the committed GIF stays untouched.
    out=$(mktemp -d)/demo-check.gif
    play --output "$out" demo/demo.tape
    [ -s "$out" ] || { echo "demo: tape produced no output" >&2; exit 1; }
    echo "demo: tape renders ($(wc -c < "$out" | tr -d ' ') bytes)"
    rm -f "$out"
else
    mkdir -p docs
    play demo/demo.tape
    echo "demo: wrote docs/demo.gif"
fi
