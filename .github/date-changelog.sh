#!/bin/sh
# Give the `## Unreleased` section a dated version heading below it, leaving the
# heading itself in place for the next change to be written under.
#
# dist finds a release's notes by matching the version against these headings,
# so the heading has to exist by the time the tag is pushed. release-plz bumps
# the version but never touches this file — it would replace hand-written prose
# with commit subjects — which leaves this, run against the release pull request
# in .github/workflows/release-plz.yml.
#
# Running it twice for the same version changes nothing, which is what the
# release pull request being rebuilt on every push to main asks of it.
set -eu

version=${1:?usage: date-changelog.sh <version>}
cd "$(dirname "$0")/.."

if grep -q "^## $version - " CHANGELOG.md; then
	echo "CHANGELOG.md already has a heading for $version"
	exit 0
fi

awk -v heading="## $version - $(date -u +%Y-%m-%d)" '
	{ print }
	/^## Unreleased$/ && !done { print ""; print heading; done = 1 }
	END { if (!done) exit 1 }
' CHANGELOG.md > CHANGELOG.md.tmp || {
	rm -f CHANGELOG.md.tmp
	echo "CHANGELOG.md has no '## Unreleased' heading to date" >&2
	exit 1
}

mv CHANGELOG.md.tmp CHANGELOG.md
echo "CHANGELOG.md: dated $version"
