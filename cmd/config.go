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
	RunE: func(cmd *cobra.Command, args []string) error {
		cfg, err := config.GetConfig(cmd.Context())
		if err != nil {
			return err
		}
		log := cfg.Logger

		cfgPath, err := config.ConfigFilePath()
		if err != nil {
			return err
		}
		log.Info("printing current configuration", "configFile", cfgPath, "verbose", viper.GetBool("verbose"))

		allSettingsJson, err := json.MarshalIndent(cfg, "", "  ")
		if err != nil {
			return fmt.Errorf("marshalling configuration to json: %w", err)
		}

		fmt.Println(string(allSettingsJson))
		return nil
	},
}

func init() {
	rootCmd.AddCommand(configCmd)
}
