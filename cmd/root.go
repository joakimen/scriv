package cmd

import (
	"os"

	"github.com/joakimen/scriv/internal/config"
	"github.com/spf13/cobra"
	"github.com/spf13/viper"
)

// rootCmd represents the base command when called without any subcommands
var rootCmd = &cobra.Command{
	Use:   "scriv",
	Short: "scriv is a tool for discovering Git repositories.",
	PersistentPreRun: func(cmd *cobra.Command, args []string) {
		// Initialize configuration and logging
		cfg := config.InitConfig()
		ctx := config.WithConfig(cmd.Context(), cfg)
		cmd.SetContext(ctx)
	},
}

func Execute() {
	err := rootCmd.Execute()
	if err != nil {
		os.Exit(1)
	}
}

func init() {
	// This is read into the Config struct via Viper
	rootCmd.PersistentFlags().BoolP("verbose", "v", false, "verbose output")
	viper.BindPFlag("verbose", rootCmd.PersistentFlags().Lookup("verbose"))
}
