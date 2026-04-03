package cmd

import (
	"fmt"
	"strings"

	"github.com/joakimen/scriv/internal/config"
	"github.com/joakimen/scriv/internal/logger"
	"github.com/spf13/cobra"
)

var configCmd = &cobra.Command{
	Use:   "config",
	Short: "Print current configuration",
	RunE: func(cmd *cobra.Command, args []string) error {
		cfg, err := config.GetConfig(cmd.Context())
		if err != nil {
			return err
		}
		log := logger.New(verbose)

		cfgPath, err := config.FilePath()
		if err != nil {
			return err
		}
		log.Info("printing current configuration", "configFile", cfgPath, "verbose", verbose)

		fmt.Println("paths:")
		for _, p := range cfg.Paths {
			fmt.Printf("  - %s (depth: %d)\n", p.Path, p.Depth)
		}
		fmt.Println()
		fmt.Printf("ignore: %s\n", strings.Join(cfg.Settings.Ignore, ", "))
		return nil
	},
}

func init() {
	rootCmd.AddCommand(configCmd)
}
