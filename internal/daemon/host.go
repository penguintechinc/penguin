package daemon

import (
	"os"
	"path/filepath"

	"github.com/penguintechinc/penguin/internal/secrets"
	"github.com/penguintechinc/penguin/internal/telemetry"
	"github.com/penguintechinc/penguin/pkg/sdk"
	"github.com/prometheus/client_golang/prometheus"
	"go.uber.org/zap"
)

// HostImpl implements sdk.HostServices for a single module.
type HostImpl struct {
	logger      *zap.Logger
	secretStore sdk.SecretStore
	license     sdk.LicenseChecker
	metrics     prometheus.Registerer
	dataDir     string
	eventSink   sdk.EventSink
	config      []byte
}

// NewHost creates a new HostServices implementation for a module.
//
// config is the module's schema-validated configuration (YAML bytes, possibly
// nil). Note: secretStore must be a *secrets.Store; if it is, we call
// Namespaced to get a per-module namespaced view. For testing, if it's a
// different type, we use it as-is.
func NewHost(
	moduleName string,
	telemetry *telemetry.Telemetry,
	secretStore sdk.SecretStore,
	license sdk.LicenseChecker,
	stateDir string,
	eventSink sdk.EventSink,
	config []byte,
) *HostImpl {
	// Create module-specific data directory. A failure here is not fatal:
	// modules that never touch DataDir still work, and those that do will
	// surface a clearer error at first use.
	dataDir := filepath.Join(stateDir, moduleName)
	if err := os.MkdirAll(dataDir, 0o700); err != nil {
		telemetry.ModuleLogger(moduleName).Warn("could not create module data dir",
			zap.String("module", moduleName),
			zap.String("dir", dataDir),
			zap.Error(err))
	}

	// Try to get a namespaced store if possible
	namespacedStore := secretStore
	if store, ok := secretStore.(*secrets.Store); ok {
		namespacedStore = store.Namespaced(moduleName)
	}

	return &HostImpl{
		logger:      telemetry.ModuleLogger(moduleName),
		secretStore: namespacedStore,
		license:     license,
		metrics:     telemetry.ModuleRegisterer(moduleName),
		dataDir:     dataDir,
		eventSink:   eventSink,
		config:      config,
	}
}

// Config returns the module's schema-validated configuration (YAML bytes),
// or nil when the operator supplied none.
func (h *HostImpl) Config() []byte {
	return h.config
}

// Logger returns a named, PII-sanitizing logger for this module.
func (h *HostImpl) Logger() *zap.Logger {
	return h.logger
}

// Secrets returns the module's namespaced secure store.
func (h *HostImpl) Secrets() sdk.SecretStore {
	return h.secretStore
}

// License checks feature-flag / entitlement state.
func (h *HostImpl) License() sdk.LicenseChecker {
	return h.license
}

// Metrics returns a registerer namespaced to the module.
func (h *HostImpl) Metrics() prometheus.Registerer {
	return h.metrics
}

// DataDir returns the module's private state directory.
func (h *HostImpl) DataDir() string {
	return h.dataDir
}

// Events publishes module status changes to subscribers.
func (h *HostImpl) Events() sdk.EventSink {
	return h.eventSink
}
