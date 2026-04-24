package cmd

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"

	"golang.org/x/sync/errgroup"

	"github.com/joakimen/scriv/internal/config"
	"github.com/joakimen/scriv/internal/fs"
	"github.com/joakimen/scriv/internal/logger"
	"github.com/spf13/cobra"
)

func newListCmd() *cobra.Command {
	var printAbsolutePaths bool

	listCmd := &cobra.Command{
		Use:     "list",
		Aliases: []string{"ls"},
		Short:   "List all repositories discovered using the paths in the user configuration",
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx := cmd.Context()
			cfg, err := config.GetConfig(ctx)
			if err != nil {
				return err
			}
			log := logger.FromContext(ctx)

			allRepos, err := findAllRepos(ctx, cfg, log)
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

			formatPath := func(repo string) string {
				if printAbsolutePaths {
					return repo
				}
				if strings.HasPrefix(repo, homeDir) {
					return "~" + repo[len(homeDir):]
				}
				return repo
			}

			log.Info("returning repositories", "count", len(allRepos))
			for _, repo := range allRepos {
				fmt.Println(formatPath(repo))
			}
			return nil
		},
	}
	listCmd.Flags().BoolVarP(&printAbsolutePaths, "absolute-paths", "A", false, "Return absolute file paths")

	return listCmd
}

func findAllRepos(ctx context.Context, cfg config.Config, log *slog.Logger) ([]string, error) {
	log.Info("settings", "ignore", cfg.Ignore)

	if len(cfg.Paths) == 0 {
		cfgPath, _ := config.FilePath()
		return nil, fmt.Errorf("no paths found in config file: %s", cfgPath)
	}

	var (
		mu    sync.Mutex
		repos []string
	)
	g, ctx := errgroup.WithContext(ctx)
	for _, path := range cfg.Paths {
		g.Go(func() error {
			found, err := findRepos(ctx, path, cfg.Ignore, log)
			if err != nil {
				return err
			}
			mu.Lock()
			repos = append(repos, found...)
			mu.Unlock()
			return nil
		})
	}
	if err := g.Wait(); err != nil {
		return nil, err
	}
	return repos, nil
}

func findRepos(ctx context.Context, pathEntry config.PathEntry, ignore []string, log *slog.Logger) ([]string, error) {
	rootPath, err := fs.ExpandHomeDir(pathEntry.Path)
	if err != nil {
		log.Warn("skipping path entry", "path", pathEntry.Path, "error", err)
		return nil, nil
	}
	if _, err := os.Stat(rootPath); err != nil {
		return nil, fmt.Errorf("root path %s: %w", rootPath, err)
	}
	rootDepth := fs.PathDepth(rootPath)
	maxDepth := pathEntry.Depth

	log.Info("path entry", "path", pathEntry.Path, "depth", pathEntry.Depth)

	var repos []string
	walkErr := filepath.WalkDir(rootPath, func(path string, d os.DirEntry, err error) error {
		if ctxErr := ctx.Err(); ctxErr != nil {
			return ctxErr
		}
		if err != nil {
			log.Warn("skipping unreadable path", "path", path, "error", err)
			if d != nil && d.IsDir() {
				return filepath.SkipDir
			}
			return nil
		}

		curDepth := fs.PathDepth(path) - rootDepth
		if curDepth > maxDepth {
			log.Debug("skipping: depth exceeded", "rootPath", rootPath, "curDepth", curDepth, "maxDepth", maxDepth, "path", path)
			return filepath.SkipDir
		}

		if slices.Contains(ignore, filepath.Base(path)) {
			log.Debug("skipping excluded dir", "path", path)
			return filepath.SkipDir
		}

		if !d.IsDir() {
			return nil
		}

		if _, err := os.Stat(filepath.Join(path, ".git")); err == nil {
			repos = append(repos, path)
			return filepath.SkipDir
		}
		return nil
	})
	if walkErr != nil && !errors.Is(walkErr, context.Canceled) {
		return repos, fmt.Errorf("walking %s: %w", rootPath, walkErr)
	}
	return repos, nil
}
