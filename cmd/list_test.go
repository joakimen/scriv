package cmd

import (
	"context"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"slices"
	"testing"

	"github.com/joakimen/scriv/internal/config"
)

func mkRepo(t *testing.T, root, rel string) string {
	t.Helper()
	full := filepath.Join(root, rel)
	if err := os.MkdirAll(filepath.Join(full, ".git"), 0o755); err != nil {
		t.Fatalf("mkRepo: %v", err)
	}
	return full
}

func mkDir(t *testing.T, root, rel string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Join(root, rel), 0o755); err != nil {
		t.Fatalf("mkDir: %v", err)
	}
}

func discardLog() *slog.Logger {
	return slog.New(slog.NewTextHandler(io.Discard, nil))
}

func TestFindReposFindsGitDirs(t *testing.T) {
	root := t.TempDir()
	a := mkRepo(t, root, "a")
	b := mkRepo(t, root, "nested/b")
	mkDir(t, root, "nested/not-a-repo")

	got, err := findRepos(
		context.Background(),
		config.PathEntry{Path: root, Depth: 5},
		nil,
		discardLog(),
	)
	if err != nil {
		t.Fatalf("findRepos: %v", err)
	}
	slices.Sort(got)
	want := []string{a, b}
	slices.Sort(want)
	if !slices.Equal(got, want) {
		t.Errorf("got %v, want %v", got, want)
	}
}

func TestFindReposRespectsDepth(t *testing.T) {
	root := t.TempDir()
	mkRepo(t, root, "top") // depth 1 relative to root
	deep := mkRepo(t, root, "a/b/c/deep")

	got, err := findRepos(
		context.Background(),
		config.PathEntry{Path: root, Depth: 1},
		nil,
		discardLog(),
	)
	if err != nil {
		t.Fatalf("findRepos: %v", err)
	}
	if slices.Contains(got, deep) {
		t.Errorf("depth limit ignored; found %q", deep)
	}
}

func TestFindReposSkipsIgnored(t *testing.T) {
	root := t.TempDir()
	mkRepo(t, root, "node_modules/hidden")
	visible := mkRepo(t, root, "visible")

	got, err := findRepos(
		context.Background(),
		config.PathEntry{Path: root, Depth: 5},
		[]string{"node_modules"},
		discardLog(),
	)
	if err != nil {
		t.Fatalf("findRepos: %v", err)
	}
	if !slices.Equal(got, []string{visible}) {
		t.Errorf("got %v, want [%s]", got, visible)
	}
}

func TestFindReposMissingRootReturnsError(t *testing.T) {
	missing := filepath.Join(t.TempDir(), "nope")
	_, err := findRepos(
		context.Background(),
		config.PathEntry{Path: missing, Depth: 1},
		nil,
		discardLog(),
	)
	if err == nil {
		t.Fatal("expected error for missing root, got nil")
	}
}
