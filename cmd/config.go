package cmd

import (
	"encoding/json"
	"fmt"

	"github.com/joakimen/scriv/internal/config"
	"github.com/spf13/cobra"
	"github.com/spf13/viper"
)

var configCmd = &cobra.Command{
	Use:   "config",
	Short: "Print current configuration",
	Run: func(cmd *cobra.Command, args []string) {
		cfg := config.GetConfig(cmd.Context())
		log := cfg.Logger
		log.Info("printing current configuration", "configFile", config.ConfigFilePath(), "verbose", viper.GetBool("verbose"))

		allSettingsJson, err := json.MarshalIndent(cfg, "", "  ")
		if err != nil {
			panic(fmt.Errorf("an error occurred while marshalling configuration data to json: %w", err))
		}

		fmt.Println(string(allSettingsJson))
	},
}

func init() {
	rootCmd.AddCommand(configCmd)
}
