package config

import (
	"context"
	"encoding/json"
	"fmt"
	"os"

	"github.com/joakimen/scriv/internal/fs"
)

const (
	defaultConfigFile = "~/.config/scriv/config.json"
	configFileEnvVar  = "SCRIV_CONFIG"
)

var defaultIgnoredDirs = []string{"node_modules", "vendor", "dist", "build", "target"}

type configKey struct{}

type Config struct {
	Paths  []PathEntry `json:"paths"`
	Ignore []string    `json:"ignore"`
}

type PathEntry struct {
	Path  string `json:"path"`
	Depth int    `json:"depth"`
}

func WithConfig(ctx context.Context, cfg Config) context.Context {
	return context.WithValue(ctx, configKey{}, cfg)
}

func GetConfig(ctx context.Context) (Config, error) {
	cfg, ok := ctx.Value(configKey{}).(Config)
	if !ok {
		return Config{}, fmt.Errorf("configuration not found in context")
	}
	return cfg, nil
}

func FilePath() (string, error) {
	if override := os.Getenv(configFileEnvVar); override != "" {
		return override, nil
	}
	return fs.ExpandHomeDir(defaultConfigFile)
}

func Load() (Config, error) {
	path, err := FilePath()
	if err != nil {
		return Config{}, fmt.Errorf("resolving config file path: %w", err)
	}

	data, err := os.ReadFile(path)
	if err != nil {
		return Config{}, fmt.Errorf("reading configuration file: %w", err)
	}

	var cfg Config
	if err := json.Unmarshal(data, &cfg); err != nil {
		return Config{}, fmt.Errorf("parsing configuration file: %w", err)
	}

	if cfg.Ignore == nil {
		cfg.Ignore = defaultIgnoredDirs
	}

	return cfg, nil
}
