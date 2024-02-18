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

const CONFIG_FILE_DEFAULT = "~/.config/scriv/config.json"
const CONFIG_FILE_ENV_OVERRIDE = "SCRIV_CONFIG"

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
func GetConfig(ctx context.Context) Config {
	return ctx.Value(ConfigKey{}).(Config)
}

// Determine the path to the configuration file, as it may be overridden by an environment variable
func ConfigFilePath() string {

	configFileEnvOverride := os.Getenv(CONFIG_FILE_ENV_OVERRIDE)

	var configFile string
	if configFileEnvOverride != "" {
		configFile = configFileEnvOverride
	} else {
		configFile = fs.ExpandHomeDir(CONFIG_FILE_DEFAULT)
	}
	return configFile
}

// Load the Viper-centric part of the configuration into a Config object
func initViper() Config {
	configFile := ConfigFilePath()
	viper.SetConfigFile(configFile)
	err := viper.ReadInConfig()
	if err != nil {
		panic(fmt.Errorf("error reading config file '%s': %w", configFile, err))
	}

	defaultIgnoredDirs := []string{"node_modules", "vendor", "dist", "build", "target"}
	viper.SetDefault("Settings.Ignore", defaultIgnoredDirs)

	var cfg Config
	if err := viper.Unmarshal(&cfg); err != nil {
		panic(fmt.Errorf("an error occurred while unmarshaling configuration file '%s': %w", configFile, err))
	}

	return cfg
}

// Add logging to the configuration
func addLogging(cfg Config) Config {
	log := logger.ConfigureStructuredLogger()
	cfg.Logger = log
	return cfg
}

// Initialize the entire configuration
func InitConfig() Config {
	return addLogging(initViper())
}
