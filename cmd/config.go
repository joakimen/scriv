package cmd

import (
	"encoding/json"
	"fmt"

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

		out, err := json.MarshalIndent(cfg, "", "  ")
		if err != nil {
			return fmt.Errorf("marshalling configuration to json: %w", err)
		}

		fmt.Println(string(out))
		return nil
	},
}

func init() {
	rootCmd.AddCommand(configCmd)
}
