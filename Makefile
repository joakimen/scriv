BINARY := scriv
MODULE := github.com/joakimen/scriv
BIN    := ./bin/$(BINARY)
VERSION ?= $(shell git describe --tags --always --dirty 2>/dev/null || echo "dev")
LDFLAGS := -ldflags "-X $(MODULE)/cmd.version=$(VERSION)"

.PHONY: all
all: build

.PHONY: build
build: fmt lint
	go build $(LDFLAGS) -o $(BIN)
ifeq ($(shell uname),Darwin)
	@codesign -s - -f $(BIN) 2>/dev/null || true
endif

.PHONY: fmt
fmt:
	go tool gofumpt -l -w .
	go tool goimports -w .

.PHONY: fmt-check
fmt-check:
	@out=$$(go tool gofumpt -l .); if [ -n "$$out" ]; then echo "unformatted files:"; echo "$$out"; exit 1; fi

.PHONY: lint
lint:
	go vet ./...
	go tool staticcheck ./...

.PHONY: test
test:
	go test ./...

.PHONY: check
check: fmt-check lint test
	go build $(LDFLAGS) -o $(BIN)

.PHONY: install
install: build
	cp $(BIN) ~/.local/bin/$(BINARY)
ifeq ($(shell uname),Darwin)
	@codesign -s - -f ~/.local/bin/$(BINARY) 2>/dev/null || true
endif

.PHONY: clean
clean:
	rm -f $(BIN)

.PHONY: vulncheck
vulncheck:
	@which govulncheck > /dev/null 2>&1 || { echo "install: go install golang.org/x/vuln/cmd/govulncheck@latest"; exit 1; }
	govulncheck ./...
