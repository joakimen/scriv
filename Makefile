.DEFAULT_GOAL := build

.PHONY: build
build: fmt-check lint test
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
