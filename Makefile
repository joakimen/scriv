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

# Releasing, step 1 of 2: dispatch the bump. Both steps are `gh workflow run`
# against a workflow rather than work done here, so the machine that cuts a
# release holds no credentials, no toolchain and no opinion — the same release
# comes out whoever runs it. `LEVEL` is patch unless said otherwise.
LEVEL ?= patch
.PHONY: release
release:
	@gh workflow run release-prepare.yml -f level=$(LEVEL)
	@echo "Bump to $(LEVEL) dispatched. It opens a pull request; merge it, then: make release-publish"

# Releasing, step 2 of 2. Run once the bump pull request has merged. Hands the
# version on `main` to dist, which refuses a tag no package carries, builds every
# target, then creates the tag and the release from binaries that already exist.
# Nothing pushes a tag — a tag nobody remembered to push is what left v0.3.1
# sitting on `main` unreleased.
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
