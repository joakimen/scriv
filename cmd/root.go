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
	PersistentPreRunE: func(cmd *cobra.Command, args []string) error {
		cfg, err := config.InitConfig()
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
	// This is read into the Config struct via Viper
	rootCmd.PersistentFlags().BoolP("verbose", "v", false, "verbose output")
	viper.BindPFlag("verbose", rootCmd.PersistentFlags().Lookup("verbose"))
}
