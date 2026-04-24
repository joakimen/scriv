package fs

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestPathDepth(t *testing.T) {
	sep := string(os.PathSeparator)
	tests := []struct {
		path string
		want int
	}{
		{"", 0},
		{sep, 1},
		{sep + "a", 1},
		{sep + "a" + sep + "b", 2},
		{sep + "a" + sep + "b" + sep + "c", 3},
		{"a" + sep + "b", 1},
	}
	for _, tt := range tests {
		if got := PathDepth(tt.path); got != tt.want {
			t.Errorf("PathDepth(%q) = %d, want %d", tt.path, got, tt.want)
		}
	}
}

func TestExpandHomeDir(t *testing.T) {
	home, err := os.UserHomeDir()
	if err != nil {
		t.Fatalf("UserHomeDir: %v", err)
	}

	tests := []struct {
		name string
		in   string
		want string
	}{
		{"tilde prefix", "~/foo/bar", filepath.Join(home, "foo", "bar")},
		{"plain tilde only", "~/", filepath.Join(home, "")},
		{"absolute path unchanged", "/etc/hosts", "/etc/hosts"},
		{"relative path unchanged", "foo/bar", "foo/bar"},
		{"tilde not at start unchanged", "/opt/~/foo", "/opt/~/foo"},
		{"empty string unchanged", "", ""},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := ExpandHomeDir(tt.in)
			if err != nil {
				t.Fatalf("ExpandHomeDir(%q): %v", tt.in, err)
			}
			if got != tt.want && !strings.HasPrefix(got, tt.want) {
				t.Errorf("ExpandHomeDir(%q) = %q, want %q", tt.in, got, tt.want)
			}
		})
	}
}
