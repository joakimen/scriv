package fs

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// Helper function to determine the depth of a fs path so we know when to short circuit a search
func PathDepth(path string) int {
	return len(strings.Split(path, string(os.PathSeparator))) - 1
}

// Why isn't this in the stdlib?
func ExpandHomeDir(dir string) (string, error) {
	if strings.HasPrefix(dir, "~/") {
		homeDir, err := os.UserHomeDir()
		if err != nil {
			return "", fmt.Errorf("expanding home directory: %w", err)
		}
		return filepath.Join(homeDir, dir[2:]), nil
	}
	return dir, nil
}
