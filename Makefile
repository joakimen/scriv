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
	cargo install --path .

.PHONY: run
run:
	cargo run -- $(ARGS)

.PHONY: clean
clean:
	cargo clean
