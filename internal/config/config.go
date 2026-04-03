package config

import (
	"context"
	"fmt"
	"log/slog"
	"os"

	"github.com/joakimen/scriv/internal/fs"
	"github.com/joakimen/scriv/internal/logger"
	"github.com/spf13/viper"
)

const (
	CONFIG_FILE_DEFAULT      = "~/.config/scriv/config.json"
	CONFIG_FILE_ENV_OVERRIDE = "SCRIV_CONFIG"
)

// The key struct for accesing the config object in the context
type ConfigKey struct{}

// The main configuration object
type Config struct {
	Paths    []PathEntry
	Settings Settings
	Logger   *slog.Logger
}

// Defines a single Path under which to search for repositories
type PathEntry struct {
	Path  string
	Depth int
}

// General configuration settings
type Settings struct {
	Ignore []string
}

// Add the configuration to the context
func WithConfig(ctx context.Context, config Config) context.Context {
	return context.WithValue(ctx, ConfigKey{}, config)
}

// Get the configuration from the context
func GetConfig(ctx context.Context) (Config, error) {
	cfg, ok := ctx.Value(ConfigKey{}).(Config)
	if !ok {
		return Config{}, fmt.Errorf("configuration not found in context")
	}
	return cfg, nil
}

// Determine the path to the configuration file, as it may be overridden by an environment variable
func ConfigFilePath() (string, error) {
	if override := os.Getenv(CONFIG_FILE_ENV_OVERRIDE); override != "" {
		return override, nil
	}
	return fs.ExpandHomeDir(CONFIG_FILE_DEFAULT)
}

// Load the Viper-centric part of the configuration into a Config object
func initViper() (Config, error) {
	configFile, err := ConfigFilePath()
	if err != nil {
		return Config{}, fmt.Errorf("resolving config file path: %w", err)
	}

	viper.SetConfigFile(configFile)
	if err := viper.ReadInConfig(); err != nil {
		return Config{}, fmt.Errorf("reading configuration file: %w", err)
	}

	defaultIgnoredDirs := []string{"node_modules", "vendor", "dist", "build", "target"}
	viper.SetDefault("Settings.Ignore", defaultIgnoredDirs)

	var cfg Config
	if err := viper.Unmarshal(&cfg); err != nil {
		return Config{}, fmt.Errorf("parsing configuration file: %w", err)
	}
	return cfg, nil
}

// Initialize the entire configuration
func InitConfig() (Config, error) {
	cfg, err := initViper()
	if err != nil {
		return Config{}, err
	}
	cfg.Logger = logger.ConfigureStructuredLogger()
	return cfg, nil
}
