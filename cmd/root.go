package cmd

import (
	"os"

	"github.com/joakimen/scriv/internal/config"
	"github.com/spf13/cobra"
)

var verbose bool

var rootCmd = &cobra.Command{
	Use:   "scriv",
	Short: "scriv is a tool for discovering Git repositories.",
	PersistentPreRunE: func(cmd *cobra.Command, args []string) error {
		cfg, err := config.Load()
		if err != nil {
			return err
		}
		ctx := config.WithConfig(cmd.Context(), cfg)
		cmd.SetContext(ctx)
		return nil
	},
}

func Execute() {
	err := rootCmd.Execute()
	if err != nil {
		os.Exit(1)
	}
}

func init() {
	rootCmd.PersistentFlags().BoolVarP(&verbose, "verbose", "v", false, "verbose output")
}
