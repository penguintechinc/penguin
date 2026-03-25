package logging

import (
	"io"
	"os"

	"github.com/sirupsen/logrus"
)

// NewLogger creates a configured logrus logger.
func NewLogger(level, format, file string) *logrus.Logger {
	logger := logrus.New()

	// Set level
	lvl, err := logrus.ParseLevel(level)
	if err != nil {
		lvl = logrus.InfoLevel
	}
	logger.SetLevel(lvl)

	// Set format
	switch format {
	case "json":
		logger.SetFormatter(&logrus.JSONFormatter{
			TimestampFormat: "2006-01-02T15:04:05.000Z07:00",
		})
	default:
		logger.SetFormatter(&logrus.TextFormatter{
			FullTimestamp:   true,
			TimestampFormat: "2006-01-02 15:04:05",
		})
	}

	// Set output
	if file != "" {
		f, err := os.OpenFile(file, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0600)
		if err != nil {
			logger.WithError(err).Warn("Failed to open log file, using stderr")
			logger.SetOutput(os.Stderr)
		} else {
			logger.SetOutput(io.MultiWriter(os.Stderr, f))
		}
	} else {
		logger.SetOutput(os.Stderr)
	}

	return logger
}
