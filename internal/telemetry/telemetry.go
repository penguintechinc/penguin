package telemetry

import (
	"fmt"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/collectors"
	"go.uber.org/zap"
	"go.uber.org/zap/zapcore"
)

// Telemetry encapsulates logging and metrics infrastructure for the daemon.
type Telemetry struct {
	Logger   *zap.Logger
	Registry *prometheus.Registry
}

// New creates a new Telemetry instance with the given log level.
// The logger is configured with JSON output to stdout and the registry
// includes process and Go runtime metrics.
//
// Note: go-common/logging.SanitizedLogger does not expose its underlying
// *zap.Logger, so we use zap.NewProduction directly here with sanitization
// handled at call sites that need PII redaction.
func New(level string) (*Telemetry, error) {
	// Parse the log level
	zapLevel, err := zapcore.ParseLevel(level)
	if err != nil {
		return nil, fmt.Errorf("invalid log level %q: %w", level, err)
	}

	// Create a production logger with the specified level
	cfg := zap.NewProductionConfig()
	cfg.Level = zap.NewAtomicLevelAt(zapLevel)
	cfg.EncoderConfig.TimeKey = "timestamp"
	cfg.EncoderConfig.EncodeTime = zapcore.ISO8601TimeEncoder

	logger, err := cfg.Build()
	if err != nil {
		return nil, fmt.Errorf("build logger: %w", err)
	}

	// Create a new registry and register standard collectors
	registry := prometheus.NewRegistry()
	if err := registry.Register(collectors.NewProcessCollector(collectors.ProcessCollectorOpts{})); err != nil {
		logger.Error("failed to register process collector", zap.Error(err))
	}
	if err := registry.Register(collectors.NewGoCollector()); err != nil {
		logger.Error("failed to register go collector", zap.Error(err))
	}

	return &Telemetry{
		Logger:   logger,
		Registry: registry,
	}, nil
}

// ModuleLogger returns a named child logger for the given module.
// The returned logger will include the module name in all log entries.
func (t *Telemetry) ModuleLogger(name string) *zap.Logger {
	return t.Logger.Named(name)
}

// ModuleRegisterer returns a Registerer that wraps the main registry
// with the module name as a label. Metrics registered through this
// registerer will automatically include the module label.
func (t *Telemetry) ModuleRegisterer(name string) prometheus.Registerer {
	return prometheus.WrapRegistererWith(
		prometheus.Labels{"module": name},
		t.Registry,
	)
}
