package cmd

import (
	"errors"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"

	"github.com/joakimen/scriv/internal/config"
	"github.com/joakimen/scriv/internal/fs"
	"github.com/spf13/cobra"
)

func newListCmd() *cobra.Command {
	var printAbsolutePaths bool

	listCmd := &cobra.Command{
		Use:   "list",
		Short: "List all repositories discovered using the paths in the user configuration",
		RunE: func(cmd *cobra.Command, args []string) error {
			cfg := config.GetConfig(cmd.Context())
			log := cfg.Logger

			allRepos := findAllRepos(cfg)
			slices.Sort(allRepos)

			if len(allRepos) == 0 {
				return errors.New("no repositories found")
			}

			homeDir, err := os.UserHomeDir()
			if err != nil {
				return err
			}

			fmtFunc := func(repo string) string {
				if printAbsolutePaths {
					return repo
				} else {
					return strings.Replace(repo, homeDir, "~", 1)
				}
			}

			log.Info(fmt.Sprintf("Returning %d repositories", len(allRepos)))
			for _, repo := range allRepos {
				fmt.Println(fmtFunc(repo))
			}
			return nil
		},
	}
	listCmd.Flags().BoolVarP(&printAbsolutePaths, "absolute-paths", "A", false, "Return absolute file paths")

	return listCmd
}

func init() {
	rootCmd.AddCommand(newListCmd())
}

// Find all Git repos for each path in the user config
func findAllRepos(cfg config.Config) []string {
	paths := cfg.Paths
	settings := cfg.Settings
	log := cfg.Logger

	pathCount := len(paths)
	log.Info("Settings", "settings", fmt.Sprintf("%+v", settings))

	if pathCount == 0 {
		panic(fmt.Errorf("no paths found in config file: %s", config.ConfigFilePath()))
	}

	repoChan := make(chan []string, pathCount)
	var wg sync.WaitGroup

	for _, path := range paths {
		wg.Add(1)
		go func(c chan []string, wg *sync.WaitGroup, path config.PathEntry) {
			defer wg.Done()
			repos := findRepos(path, settings, log)
			repoChan <- repos
		}(repoChan, &wg, path)
	}

	wg.Wait()
	close(repoChan)

	var totalRepos []string
	for repos := range repoChan {
		totalRepos = append(totalRepos, repos...)
	}

	return totalRepos
}

// Find all Git repos for a single path entry.
// We walk the directory tree, look for .git directories and return their parents.
// Directories are ignored if:
// - they are at a depth greater than the configured max depth
// - they are in the list of ignored paths
// - they are not a directory
func findRepos(pathEntry config.PathEntry, settings config.Settings, log *slog.Logger) []string {
	rootPath := fs.ExpandHomeDir(pathEntry.Path)
	rootDepth := fs.PathDepth(rootPath)
	maxDepth := pathEntry.Depth
	ignoredPaths := settings.Ignore

	log.Info("PathEntry", "path", pathEntry.Path, "depth", pathEntry.Depth)

	var repos []string
	filepath.WalkDir(rootPath, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}

		pathDepth := fs.PathDepth(path)
		curDepth := pathDepth - rootDepth
		if curDepth > maxDepth {
			log.Debug("Skipping file: path depth exceeds configured max depth", "rootPath", rootPath, "curDepth", curDepth, "maxDepth", maxDepth, "path", path)
			return filepath.SkipDir
		}

		if slices.Contains(ignoredPaths, filepath.Base(path)) {
			log.Debug("Skipping excluded dir: " + path)
			return filepath.SkipDir
		}

		if !d.IsDir() {
			log.Debug("Skipping non-directory path: " + path)
			return nil
		}

		// Check if the current path is a Git repository
		_, err = os.Stat(filepath.Join(path, ".git"))
		if err == nil {
			repos = append(repos, path)
		}
		return nil
	})
	return repos
}
