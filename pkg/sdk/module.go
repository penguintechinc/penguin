// Package sdk defines the public contract between the penguin endpoint-agent
// daemon and product modules. Compiled-in modules and external (go-plugin)
// modules implement the exact same interface; the daemon supervisor cannot
// tell them apart.
//
// Adding a new PenguinTech product client means implementing Module and
// registering its Factory (one line in internal/registry for compiled-in
// modules, or a signed external plugin binary calling sdkplugin.Serve).
package sdk

import (
	"context"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	"go.uber.org/zap"
)

// Module is the lifecycle and command contract every product module
// implements. All methods must be safe for concurrent use; Start must not
// block (modules own their goroutines and stop them in Stop).
type Module interface {
	// Info returns static identity metadata. It must be callable before Init.
	Info() ModuleInfo

	// Init prepares the module with host-provided services. It is called
	// exactly once before Start and must not begin any background work.
	Init(ctx context.Context, host HostServices) error

	// Start begins the module's work and returns promptly.
	Start(ctx context.Context) error

	// Stop halts all module work and restores any system state the module
	// changed (resolver settings, tunnels, ...). It must be idempotent.
	Stop(ctx context.Context) error

	// Status reports the module's current operational state.
	Status(ctx context.Context) (Status, error)

	// Health is a cheap liveness/degradation probe used by the tray and
	// `penguin status`.
	Health(ctx context.Context) HealthReport

	// Commands declares the module's CLI command tree. The penguin CLI builds
	// cobra commands from this data and never links module code.
	Commands() []CommandSpec

	// Dispatch executes the command at path with parsed flags and positional
	// args. It is the single entry point for all module CLI invocations.
	Dispatch(ctx context.Context, path []string, flags map[string]string, args []string) (*Result, error)

	// ConfigSchema returns the JSON Schema the daemon validates
	// /etc/penguin/modules.d/<name>.yaml against. A nil/empty schema means
	// the module takes no configuration.
	ConfigSchema() []byte
}

// Factory constructs a fresh, un-Initialized Module. Registered in
// internal/registry (compiled-in) or served over go-plugin (external).
type Factory func() Module

// ModuleInfo is static module identity metadata.
type ModuleInfo struct {
	// Name is the CLI-visible module name ("tobogganing", "squawk").
	// Lowercase, no spaces; used in `penguin <name> ...` and config paths.
	Name string
	// Version is the module's own semantic version.
	Version string
	// Description is a one-line human summary.
	Description string
	// LicenseFeature is the feature-flag / entitlement key gating this module
	// (e.g. "penguin.tobogganing"). Empty means ungated.
	LicenseFeature string
}

// HostServices are daemon-provided capabilities injected into Init. For
// external plugins these are brokered back to the daemon over gRPC.
type HostServices interface {
	// Logger returns a named, PII-sanitizing logger child for this module.
	Logger() *zap.Logger
	// Secrets returns the module's namespaced secure store. Values live in
	// the OS keychain/keystore — never plaintext files.
	Secrets() SecretStore
	// License checks feature-flag / entitlement state with offline caching.
	License() LicenseChecker
	// Metrics returns a registerer namespaced to the module.
	Metrics() prometheus.Registerer
	// Config returns the module's configuration as raw YAML bytes, already
	// validated by the daemon against the module's ConfigSchema(). Empty when
	// the operator supplied no config — modules must apply their own defaults.
	//
	// Modules must NOT read config files themselves: routing config through
	// the host is what guarantees it was schema-checked, and it keeps modules
	// testable and identical whether compiled in or loaded as an external
	// plugin.
	Config() []byte

	// DataDir returns the module's private state directory
	// (e.g. /var/lib/penguind/<module>).
	DataDir() string
	// Events publishes module status changes to subscribers (tray, CLI).
	Events() EventSink
}

// SecretStore is namespaced secure key/value storage backed by the OS
// keychain/keystore (with an encrypted-file fallback for headless daemons).
type SecretStore interface {
	Get(key string) ([]byte, error)
	Set(key string, value []byte) error
	Delete(key string) error
}

// ErrSecretNotFound is returned by SecretStore.Get for missing keys.
// Implementations must return an error matching this via errors.Is.
var ErrSecretNotFound = errSecretNotFound{}

type errSecretNotFound struct{}

func (errSecretNotFound) Error() string { return "secret not found" }

// LicenseChecker exposes feature-flag and entitlement checks. Implementations
// must degrade gracefully: unreachable servers yield cached results, unknown
// flags are OFF, and no call ever panics.
type LicenseChecker interface {
	// FeatureEnabled reports whether a flag key (e.g. "penguin.squawk") is
	// enabled. Unknown or unfetchable flags return false.
	FeatureEnabled(key string) bool
	// Tier returns the current license tier ("free", "professional",
	// "enterprise"; empty when unknown). Tiers are cumulative.
	Tier() string
}

// EventSink receives module status-change events.
type EventSink interface {
	Publish(ev Event)
}

// Event is a module status-change notification.
type Event struct {
	Module  string
	Type    EventType
	Message string
	At      time.Time
	// Fields carries small, non-sensitive KV context for display.
	Fields map[string]string
}

// EventType classifies events for subscribers.
type EventType string

const (
	EventStateChanged EventType = "state-changed"
	EventHealth       EventType = "health"
	EventInfo         EventType = "info"
	EventError        EventType = "error"
)
