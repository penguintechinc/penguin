package daemon

import (
	"context"
	"errors"
	"fmt"
	"math"
	"math/rand"
	"sort"
	"sync"
	"time"

	"github.com/penguintechinc/penguin/pkg/sdk"
	"go.uber.org/zap"
)

// ErrUnknownModule is returned when referencing a module not in the registry.
var ErrUnknownModule = errors.New("unknown module")

// BackoffConfig configures exponential backoff behavior for module restart.
type BackoffConfig struct {
	// Initial is the starting backoff duration.
	Initial time.Duration
	// Max is the maximum backoff duration.
	Max time.Duration
	// Multiplier is the exponential growth factor per attempt.
	Multiplier float64
	// Jitter enables randomization to prevent thundering herd.
	Jitter bool
}

// DefaultBackoff returns sensible backoff defaults: 100ms initial, 30s max, 2x multiplier.
func DefaultBackoff() BackoffConfig {
	return BackoffConfig{
		Initial:    100 * time.Millisecond,
		Max:        30 * time.Second,
		Multiplier: 2.0,
		Jitter:     true,
	}
}

// backoffFor calculates the backoff duration for the given attempt (0-indexed).
func (bc BackoffConfig) backoffFor(attempt int) time.Duration {
	dur := time.Duration(float64(bc.Initial) * math.Pow(bc.Multiplier, float64(attempt)))
	if dur > bc.Max {
		dur = bc.Max
	}
	if bc.Jitter {
		jitterFrac := rand.Float64() * 0.1 // #nosec G404 -- jitter needs no crypto randomness
		dur = time.Duration(float64(dur) * (1.0 + jitterFrac - 0.05))
	}
	return dur
}

// Clock provides testable time operations.
type Clock interface {
	Now() time.Time
	After(time.Duration) <-chan time.Time
}

// realClock wraps the standard time package.
type realClock struct{}

func (rc realClock) Now() time.Time {
	return time.Now()
}

func (rc realClock) After(d time.Duration) <-chan time.Time {
	return time.After(d)
}

// Config is the configuration for a Supervisor.
type Config struct {
	// Modules is the list of available module factories.
	Modules []sdk.Factory
	// Host is a function that provides per-module HostServices.
	// Called during Load for each module.
	Host func(moduleName string) sdk.HostServices
	// StatePath is the path to the persisted enabled-set JSON file.
	StatePath string
	// Logger is used for all logging.
	Logger *zap.Logger
	// Backoff configures restart retry behavior.
	Backoff BackoffConfig
	// Clock provides testable time (defaults to realClock).
	Clock Clock
	// MaxRestarts is the maximum number of restart attempts before parking
	// a module in failed state. Defaults to 5.
	MaxRestarts int
}

// moduleState tracks a loaded module's internal lifecycle.
type moduleState struct {
	// factory is the original Factory function.
	factory sdk.Factory
	// instance is the instantiated Module.
	instance sdk.Module
	// state tracks the current ModuleState.
	state sdk.ModuleState
	// restartAttempt counts consecutive restart attempts.
	restartAttempt int
	// nextRestartAt is when the next restart should occur.
	nextRestartAt time.Time
	// host are the HostServices provided to this module.
	host sdk.HostServices
	// cancelCtx cancels the module's background work.
	cancelCtx context.CancelFunc
}

// Supervisor manages the lifecycle of a set of modules and persists
// the enabled-set to disk.
type Supervisor struct {
	cfg        Config
	modules    map[string]*sdk.Factory // name -> factory
	loaded     map[string]*moduleState // name -> loaded module state
	persisted  *PersistedState         // current enabled-set
	mu         sync.RWMutex            // protects loaded, persisted
	logger     *zap.Logger             // named logger for supervisor
	clock      Clock                   // testable time
	maxRestart int                     // max restart attempts
	lifeCtx    context.Context         // supervisor lifetime; cancels pending restarts
	lifeStop   context.CancelFunc      // called by Shutdown
}

// New creates a fresh Supervisor. It does not load any modules; call
// StartEnabled to load persisted modules on daemon boot.
func New(cfg Config) *Supervisor {
	if cfg.Clock == nil {
		cfg.Clock = realClock{}
	}
	if cfg.MaxRestarts == 0 {
		cfg.MaxRestarts = 5
	}
	if cfg.Logger == nil {
		cfg.Logger = zap.NewNop()
	}

	// Build module registry: name -> factory
	modules := make(map[string]*sdk.Factory)
	for i := range cfg.Modules {
		m := cfg.Modules[i]()
		info := m.Info()
		modules[info.Name] = &cfg.Modules[i]
	}

	lifeCtx, lifeStop := context.WithCancel(context.Background())
	return &Supervisor{
		cfg:        cfg,
		modules:    modules,
		loaded:     make(map[string]*moduleState),
		persisted:  &PersistedState{Enabled: []string{}},
		logger:     cfg.Logger.Named("supervisor"),
		clock:      cfg.Clock,
		maxRestart: cfg.MaxRestarts,
		lifeCtx:    lifeCtx,
		lifeStop:   lifeStop,
	}
}

// Load enables and starts a module by name. If already running, it is a no-op.
// It persists the enabled-set to disk. If the module's LicenseFeature is set
// and not enabled, Load returns an error.
func (s *Supervisor) Load(ctx context.Context, name string) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	// Check if already loaded
	if _, ok := s.loaded[name]; ok {
		return nil // Already loaded, idempotent
	}

	// Check if module exists
	factory, ok := s.modules[name]
	if !ok {
		return fmt.Errorf("%w: %s", ErrUnknownModule, name)
	}

	// Create fresh instance
	m := (*factory)()
	info := m.Info()

	// License check
	host := s.cfg.Host(name)
	if info.LicenseFeature != "" && !host.License().FeatureEnabled(info.LicenseFeature) {
		return fmt.Errorf("license feature %q not enabled for module %q", info.LicenseFeature, name)
	}

	// Publish state transition: disabled -> initializing
	s.publishEvent(name, sdk.EventStateChanged, sdk.StateInitializing, nil)

	// Init
	if err := m.Init(ctx, host); err != nil {
		s.publishEvent(name, sdk.EventError, "", map[string]string{"error": err.Error()})
		return fmt.Errorf("init %q: %w", name, err)
	}

	// Create child context for this module's lifecycle
	modCtx, cancel := context.WithCancel(context.Background())

	// Publish state transition: initializing -> running
	s.publishEvent(name, sdk.EventStateChanged, sdk.StateRunning, nil)

	// Start
	if err := m.Start(modCtx); err != nil {
		cancel()
		s.publishEvent(name, sdk.EventError, "", map[string]string{"error": err.Error()})
		s.publishEvent(name, sdk.EventStateChanged, sdk.StateFailed, nil)
		return fmt.Errorf("start %q: %w", name, err)
	}

	// Record loaded state
	s.loaded[name] = &moduleState{
		factory:        *factory,
		instance:       m,
		state:          sdk.StateRunning,
		restartAttempt: 0,
		host:           host,
		cancelCtx:      cancel,
	}

	// Persist enabled-set
	s.addToEnabled(name)
	if err := s.persisted.Save(s.cfg.StatePath); err != nil {
		s.logger.Warn("failed to persist state", zap.String("module", name), zap.Error(err))
	}

	s.logger.Info("module loaded", zap.String("name", name))
	return nil
}

// Unload stops and disables a module. If not loaded, it is a no-op.
// It persists the enabled-set to disk.
func (s *Supervisor) Unload(ctx context.Context, name string) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	if _, ok := s.loaded[name]; !ok {
		return nil // Not loaded, idempotent
	}
	s.stopLocked(ctx, name)

	// Unload is an operator decision: forget the module across restarts.
	s.removeFromEnabled(name)
	if err := s.persisted.Save(s.cfg.StatePath); err != nil {
		s.logger.Warn("failed to persist state", zap.String("module", name), zap.Error(err))
	}

	s.logger.Info("module unloaded", zap.String("name", name))
	return nil
}

// stopLocked stops a loaded module and drops it from the loaded set without
// touching the persisted enabled-set. Callers must hold s.mu.
func (s *Supervisor) stopLocked(ctx context.Context, name string) {
	ms, ok := s.loaded[name]
	if !ok {
		return
	}

	s.publishEvent(name, sdk.EventStateChanged, sdk.StateStopping, nil)

	if err := ms.instance.Stop(ctx); err != nil {
		s.logger.Warn("error stopping module", zap.String("name", name), zap.Error(err))
	}
	ms.cancelCtx()

	ms.state = sdk.StateStopped
	s.publishEvent(name, sdk.EventStateChanged, sdk.StateStopped, nil)
	delete(s.loaded, name)
}

// StartEnabled loads all modules in the persisted enabled-set. Called on
// daemon startup to restore previous state.
func (s *Supervisor) StartEnabled(ctx context.Context) error {
	if err := s.loadPersistedState(); err != nil {
		return fmt.Errorf("load persisted state: %w", err)
	}

	for _, name := range s.persisted.Enabled {
		if err := s.Load(ctx, name); err != nil {
			s.logger.Warn("failed to load persisted module", zap.String("name", name), zap.Error(err))
		}
	}
	return nil
}

// Shutdown stops all loaded modules in reverse load order. It deliberately
// does NOT disable them: a daemon restart must bring back exactly the modules
// the operator had loaded, so the persisted enabled-set is left intact.
// Use Unload to actually forget a module.
func (s *Supervisor) Shutdown(ctx context.Context) error {
	s.lifeStop() // abandon any pending backoff restarts
	s.mu.Lock()
	defer s.mu.Unlock()

	names := make([]string, 0, len(s.loaded))
	for name := range s.loaded {
		names = append(names, name)
	}
	sort.Strings(names) // deterministic order; reversed below (LIFO)

	for i := len(names) - 1; i >= 0; i-- {
		s.stopLocked(ctx, names[i])
	}

	return nil
}

// Status returns the current status of a named module, or ErrUnknownModule if
// not found.
func (s *Supervisor) Status(ctx context.Context, name string) (sdk.Status, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	ms, ok := s.loaded[name]
	if !ok {
		return sdk.Status{}, fmt.Errorf("%w: %s", ErrUnknownModule, name)
	}

	status, err := ms.instance.Status(ctx)
	if err != nil {
		return sdk.Status{State: sdk.StateFailed}, err
	}
	return status, nil
}

// ModuleSnapshot is a point-in-time view of a module's state.
type ModuleSnapshot struct {
	Name           string
	State          sdk.ModuleState
	RestartAttempt int
}

// List returns a snapshot of every *registered* module, loaded or not, sorted
// by name. Modules that are not loaded report StateDisabled — operators must
// be able to discover what they can `penguin load`, not just what is already
// running.
func (s *Supervisor) List() []ModuleSnapshot {
	s.mu.RLock()
	defer s.mu.RUnlock()

	snapshots := make([]ModuleSnapshot, 0, len(s.modules))
	for name := range s.modules {
		snap := ModuleSnapshot{Name: name, State: sdk.StateDisabled}
		if ms, ok := s.loaded[name]; ok {
			snap.State = ms.state
			snap.RestartAttempt = ms.restartAttempt
		}
		snapshots = append(snapshots, snap)
	}
	sort.Slice(snapshots, func(i, j int) bool { return snapshots[i].Name < snapshots[j].Name })
	return snapshots
}

// ReportFailure increments the restart counter for a module and schedules
// a restart with exponential backoff. If max restarts exceeded, the module
// parks in failed state and publishes an EventError.
func (s *Supervisor) ReportFailure(ctx context.Context, name string) {
	s.mu.Lock()
	ms, ok := s.loaded[name]
	if !ok {
		s.mu.Unlock()
		return
	}

	ms.restartAttempt++
	if ms.restartAttempt >= s.maxRestart {
		// Parked in failed state
		ms.state = sdk.StateFailed
		s.publishEvent(name, sdk.EventStateChanged, sdk.StateFailed, nil)
		s.publishEvent(name, sdk.EventError, fmt.Sprintf("max restarts (%d) exceeded", s.maxRestart), nil)
		s.mu.Unlock()
		s.logger.Error("module parked in failed state", zap.String("name", name), zap.Int("attempts", ms.restartAttempt))
		return
	}

	// Schedule restart with backoff
	backoffDur := s.cfg.Backoff.backoffFor(ms.restartAttempt - 1)
	ms.nextRestartAt = s.clock.Now().Add(backoffDur)
	ms.state = sdk.StateDegraded

	s.publishEvent(name, sdk.EventStateChanged, sdk.StateDegraded, map[string]string{
		"restart_at": ms.nextRestartAt.Format(time.RFC3339),
	})

	s.mu.Unlock()

	s.logger.Info("module failure reported, scheduling restart", zap.String("name", name),
		zap.Int("attempt", ms.restartAttempt), zap.Duration("backoff", backoffDur))

	// Schedule the actual restart (non-blocking). Restarts deliberately use
	// the supervisor's lifetime context, not the reporting caller's request
	// context — a CLI request ending must not abandon a pending restart.
	go s.scheduleRestart(s.lifeCtx, name, backoffDur) // #nosec G118 -- see comment above
}

// scheduleRestart waits for the backoff duration and restarts the module.
func (s *Supervisor) scheduleRestart(ctx context.Context, name string, backoffDur time.Duration) {
	select {
	case <-s.clock.After(backoffDur):
		s.mu.Lock()
		ms, ok := s.loaded[name]
		if !ok {
			s.mu.Unlock()
			return
		}

		// Stop the failed instance
		if err := ms.instance.Stop(ctx); err != nil {
			s.logger.Warn("error stopping failed module", zap.String("name", name), zap.Error(err))
		}
		ms.cancelCtx()

		// Create fresh instance
		m := ms.factory()
		modCtx, cancel := context.WithCancel(context.Background())

		s.publishEvent(name, sdk.EventStateChanged, sdk.StateInitializing, nil)

		if err := m.Init(ctx, ms.host); err != nil {
			cancel()
			s.publishEvent(name, sdk.EventError, fmt.Sprintf("restart init failed: %v", err), nil)
			s.mu.Unlock()
			// Recursive: report failure to trigger another restart
			s.ReportFailure(ctx, name)
			return
		}

		s.publishEvent(name, sdk.EventStateChanged, sdk.StateRunning, nil)

		if err := m.Start(modCtx); err != nil {
			cancel()
			s.publishEvent(name, sdk.EventError, fmt.Sprintf("restart start failed: %v", err), nil)
			s.mu.Unlock()
			// Recursive: report failure to trigger another restart
			s.ReportFailure(ctx, name)
			return
		}

		// Update module state
		ms.instance = m
		ms.state = sdk.StateRunning
		ms.cancelCtx = cancel
		s.mu.Unlock()

		s.logger.Info("module restarted successfully", zap.String("name", name))
	case <-ctx.Done():
		// Context cancelled, abandon restart
	}
}

// Private helpers

// addToEnabled adds a name to the enabled-set if not already present.
func (s *Supervisor) addToEnabled(name string) {
	for _, n := range s.persisted.Enabled {
		if n == name {
			return // Already present
		}
	}
	s.persisted.Enabled = append(s.persisted.Enabled, name)
}

// removeFromEnabled removes a name from the enabled-set.
func (s *Supervisor) removeFromEnabled(name string) {
	i := 0
	for _, n := range s.persisted.Enabled {
		if n != name {
			s.persisted.Enabled[i] = n
			i++
		}
	}
	s.persisted.Enabled = s.persisted.Enabled[:i]
}

// loadPersistedState loads the enabled-set from disk.
func (s *Supervisor) loadPersistedState() error {
	ps, err := LoadState(s.cfg.StatePath)
	if err != nil {
		return err
	}
	s.persisted = ps
	return nil
}

// publishEvent publishes an Event to the host's EventSink.
// msg is the event message; if it's a ModuleState string, it will be used directly.
// fields are optional key-value context.
//
// Callers must hold s.mu (read or write); publishEvent does not lock.
func (s *Supervisor) publishEvent(name string, evType sdk.EventType, msg interface{}, fields map[string]string) {
	// During Load the module is not yet in s.loaded; resolve the host the
	// same way Load does so those transition events are not dropped.
	var host sdk.HostServices
	if ms, ok := s.loaded[name]; ok {
		host = ms.host
	} else {
		host = s.cfg.Host(name)
	}
	if host == nil {
		return
	}

	if fields == nil {
		fields = make(map[string]string)
	}

	msgStr := ""
	if state, ok := msg.(sdk.ModuleState); ok {
		msgStr = string(state)
	} else if str, ok := msg.(string); ok {
		msgStr = str
	}

	ev := sdk.Event{
		Module:  name,
		Type:    evType,
		Message: msgStr,
		At:      s.clock.Now(),
		Fields:  fields,
	}
	host.Events().Publish(ev)
}
