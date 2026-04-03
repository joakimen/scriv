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
			cfg, err := config.GetConfig(cmd.Context())
			if err != nil {
				return err
			}
			log := cfg.Logger

			allRepos, err := findAllRepos(cfg)
			if err != nil {
				return err
			}
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
				}
				return strings.Replace(repo, homeDir, "~", 1)
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
func findAllRepos(cfg config.Config) ([]string, error) {
	paths := cfg.Paths
	settings := cfg.Settings
	log := cfg.Logger

	log.Info("Settings", "settings", fmt.Sprintf("%+v", settings))

	if len(paths) == 0 {
		cfgPath, _ := config.ConfigFilePath()
		return nil, fmt.Errorf("no paths found in config file: %s", cfgPath)
	}

	repoChan := make(chan []string, len(paths))
	var wg sync.WaitGroup

	for _, path := range paths {
		wg.Add(1)
		go func() {
			defer wg.Done()
			repos := findRepos(path, settings, log)
			repoChan <- repos
		}()
	}

	wg.Wait()
	close(repoChan)

	var totalRepos []string
	for repos := range repoChan {
		totalRepos = append(totalRepos, repos...)
	}

	return totalRepos, nil
}

// Find all Git repos for a single path entry.
// We walk the directory tree, look for .git directories and return their parents.
// Directories are ignored if:
// - they are at a depth greater than the configured max depth
// - they are in the list of ignored paths
// - they are not a directory
func findRepos(pathEntry config.PathEntry, settings config.Settings, log *slog.Logger) []string {
	rootPath, err := fs.ExpandHomeDir(pathEntry.Path)
	if err != nil {
		log.Warn("Skipping path entry", "path", pathEntry.Path, "error", err)
		return nil
	}
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
			return filepath.SkipDir
		}
		return nil
	})
	return repos
}
