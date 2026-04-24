package cmd

import (
	"fmt"
	"strings"

	"github.com/joakimen/scriv/internal/config"
	"github.com/joakimen/scriv/internal/logger"
	"github.com/spf13/cobra"
)

func newConfigCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "config",
		Short: "Print current configuration",
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx := cmd.Context()
			cfg, err := config.GetConfig(ctx)
			if err != nil {
				return err
			}
			log := logger.FromContext(ctx)

			cfgPath, err := config.FilePath()
			if err != nil {
				return err
			}
			log.Info("printing configuration", "configFile", cfgPath)

			fmt.Println("paths:")
			for _, p := range cfg.Paths {
				fmt.Printf("  - %s (depth: %d)\n", p.Path, p.Depth)
			}
			fmt.Println()
			fmt.Printf("ignore: %s\n", strings.Join(cfg.Ignore, ", "))
			return nil
		},
	}
}
