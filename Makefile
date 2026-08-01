.DEFAULT_GOAL := build

# Everything that can fail on correctness, and nothing that cannot.
#
# The inner loop: a handful of seconds, against `build`'s minute, almost all of
# which is the release build. That build is worth running before a pull request
# — release settings turn on optimisations and `lto`, and a crate can compile in
# debug and fail there — but it is not worth running between two edits to a
# test, and waiting out a minute for that answer is how a fast loop becomes a
# slow one.
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

.PHONY: run
run:
	cargo run -- $(ARGS)

# Re-record docs/demo.gif. Deliberate and local: the render depends on the
# fonts installed on the machine, so CI checks the tape instead of committing
# its own copy. Requires vhs (brew install vhs).
.PHONY: demo
demo:
	./demo/record.sh

# Render the tape to a throwaway path, to catch a demo that has stopped
# working. Leaves the committed GIF alone.
.PHONY: demo-check
demo-check:
	./demo/record.sh --check

# Build the demo sandbox and print how to enter it, for poking at scriv with
# fictional repositories, branches, and pull requests.
.PHONY: demo-fixture
demo-fixture:
	cargo build --release
	SCRIV_BIN_DIR=$(CURDIR)/target/release sh demo/fixture.sh target/demo-fixture
	@echo
	@echo "Enter the sandbox with:"
	@echo "  source target/demo-fixture/env.sh"

.PHONY: clean
clean:
	cargo clean
