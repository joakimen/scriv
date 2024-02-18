package logger

import (
	"log/slog"
	"os"

	"github.com/spf13/viper"
)

const LOG_LEVEL_DEFAULT = slog.LevelWarn
const LOG_LEVEL_VERBOSE = slog.LevelDebug

// Create a structured logger. Only show warn and worse unless verbose is set
func ConfigureStructuredLogger() *slog.Logger {

	verbose := viper.GetBool("verbose")

	var logLevel slog.Level
	if verbose {
		logLevel = LOG_LEVEL_VERBOSE
	} else {
		logLevel = LOG_LEVEL_DEFAULT
	}

	logOpts := &slog.HandlerOptions{
		Level: logLevel,
	}

	logWriter := os.Stderr
	return slog.New(slog.NewTextHandler(logWriter, logOpts))
}
