package cmd

import (
	"os"

	"github.com/joakimen/scriv/internal/config"
	"github.com/joakimen/scriv/internal/logger"
	"github.com/spf13/cobra"
)

var version = "dev"

func NewRootCmd() *cobra.Command {
	var verbose bool

	root := &cobra.Command{
		Use:     "scriv",
		Short:   "scriv is a tool for discovering Git repositories.",
		Version: version,
		PersistentPreRunE: func(cmd *cobra.Command, args []string) error {
			cfg, err := config.Load()
			if err != nil {
				return err
			}
			log := logger.New(verbose)
			ctx := config.WithConfig(cmd.Context(), cfg)
			ctx = logger.WithLogger(ctx, log)
			cmd.SetContext(ctx)
			return nil
		},
	}
	root.PersistentFlags().BoolVarP(&verbose, "verbose", "v", false, "verbose output")

	root.AddCommand(newListCmd())
	root.AddCommand(newConfigCmd())
	return root
}

func Execute() {
	if err := NewRootCmd().Execute(); err != nil {
		os.Exit(1)
	}
}
