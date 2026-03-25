package plugin

import (
	"os"

	"github.com/sirupsen/logrus"
)

// newLogrusLogger creates a logrus logger configured for plugin process output.
// go-plugin captures stderr, so we log there. JSON format ensures the host
// can parse structured log entries.
func newLogrusLogger() *logrus.Logger {
	logger := logrus.New()
	logger.SetOutput(os.Stderr)
	logger.SetFormatter(&logrus.JSONFormatter{})
	logger.SetLevel(logrus.InfoLevel)
	return logger
}
