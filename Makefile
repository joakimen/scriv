BIN=./bin/scriv

.PHONY: all
all: build

.PHONY: build
build: fmt lint
	go build -o $(BIN) $(MAINPRG)

.PHONY: fmt
fmt:
	go tool gofumpt -l -w .
	go tool goimports -w .

.PHONY: install
install: build
	install $(BIN) ~/bin/

.PHONY: lint
lint:
	go vet ./...
	go tool staticcheck ./...
