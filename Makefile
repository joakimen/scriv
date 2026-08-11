.DEFAULT_GOAL := build

.PHONY: check
check: fmt-check lint test

.PHONY: build
build: check
	cargo build --release

.PHONY: test
test:
	cargo test

.PHONY: lint
lint:
	cargo clippy --all-targets -- -D warnings

.PHONY: fmt
fmt:
	cargo fmt

.PHONY: fmt-check
fmt-check:
	cargo fmt --check

.PHONY: install
install:
	cargo install --path . --force

# Install the git hooks in prek.toml. Opt-in, and once per clone — the hooks
# directory lives in the common `.git`, so every worktree shares it.
.PHONY: hooks
hooks:
	prek install --hook-type pre-commit --hook-type pre-push

# Cut a release, step 1 of 2. Pick a bump level; cargo-release bumps the version
# in Cargo.toml and Cargo.lock, then this opens a PR for the bump and arms squash
# auto-merge. The bump lands through a PR rather than straight onto main because
# the `main` ruleset requires the `build` and `demo` checks on the branch tip and
# grants no bypass. cargo-release prints the resolved version and asks before it
# changes anything. Run `make release-publish` once the PR has merged.
.PHONY: release
release:
	@test "$$(git branch --show-current)" = main || { echo "release: run from main" >&2; exit 1; }
	@git pull --ff-only
	@printf 'Select release level:\n  1) patch\n  2) minor\n  3) major\n#? '; \
	read level; \
	case "$$level" in \
	  1) level=patch ;; \
	  2) level=minor ;; \
	  3) level=major ;; \
	  *) echo "release: invalid selection '$$level'" >&2; exit 1 ;; \
	esac; \
	cargo release version "$$level" --execute; \
	ver=$$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1); \
	git switch -c "release-v$$ver"; \
	git commit -am "Release v$$ver"; \
	git push -u origin "release-v$$ver"; \
	gh pr create --title "Release v$$ver" --body "Bump version to $$ver for release."; \
	gh pr merge --squash --auto

# Cut a release, step 2 of 2. Run once the `make release` PR has merged. Hands
# the version on `main` to dist, which refuses a tag no package carries, builds
# every target, then creates the tag and the release from binaries that already
# exist. Nothing local pushes a tag — a tag nobody remembered to push is what
# left v0.3.1 sitting on `main` unreleased.
.PHONY: release-publish
release-publish:
	@git switch main
	@git pull --ff-only
	@ver=$$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1); \
	gh workflow run release.yml -f tag="v$$ver"; \
	echo "Release v$$ver dispatched. Watch it with: gh run watch"

# Exercise the release pipeline without releasing. dist builds every target and
# throws the artifacts away, leaving no tag and no release behind. The `dry-run`
# string is the workflow's own sentinel, not a flag of ours.
.PHONY: release-dry-run
release-dry-run:
	@gh workflow run release.yml -f tag=dry-run
	@echo "Dry run dispatched. Watch it with: gh run watch"

# Re-record docs/demo.gif. Local only — the render depends on installed fonts.
# Requires vhs (brew install vhs).
.PHONY: demo
demo:
	./demo/record.sh

# Render the tape to a throwaway path, leaving the committed GIF alone.
.PHONY: demo-check
demo-check:
	./demo/record.sh --check

# Build the demo sandbox and print how to enter it.
.PHONY: demo-fixture
demo-fixture:
	cargo build --release
	SCRIV_BIN_DIR=$(CURDIR)/target/release sh demo/fixture.sh target/demo-fixture
	@echo
	@echo "Enter the sandbox with:"
	@echo "  source target/demo-fixture/env.sh"
