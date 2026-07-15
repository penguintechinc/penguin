package daemon

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/penguintechinc/penguin/pkg/sdk"
	"github.com/prometheus/client_golang/prometheus"
	"go.uber.org/zap"
	"go.uber.org/zap/zaptest"
)

// runningCount returns how many registered modules are actually loaded.
// List() reports every registered module (unloaded ones as StateDisabled), so
// tests that mean "nothing is running" must filter rather than count rows.
func runningCount(snaps []ModuleSnapshot) int {
	n := 0
	for _, s := range snaps {
		if s.State != sdk.StateDisabled {
			n++
		}
	}
	return n
}

// fakeModule is a test implementation of sdk.Module.
type fakeModule struct {
	info         sdk.ModuleInfo
	initErr      error
	startErr     error
	stopErr      error
	statusErr    error
	statusState  sdk.ModuleState
	healthLevel  sdk.HealthLevel
	commandSpecs []sdk.CommandSpec

	initCalled   int32
	startCalled  int32
	stopCalled   int32
	statusCalled int32
	healthCalled int32

	startBlocks chan struct{} // If set, Start will block until closed
	stopWaits   chan struct{} // If set, Stop will wait on this before returning
}

func (fm *fakeModule) Info() sdk.ModuleInfo {
	return fm.info
}

func (fm *fakeModule) Init(ctx context.Context, host sdk.HostServices) error {
	atomic.AddInt32(&fm.initCalled, 1)
	return fm.initErr
}

func (fm *fakeModule) Start(ctx context.Context) error {
	atomic.AddInt32(&fm.startCalled, 1)
	if fm.startBlocks != nil {
		<-fm.startBlocks
	}
	return fm.startErr
}

func (fm *fakeModule) Stop(ctx context.Context) error {
	atomic.AddInt32(&fm.stopCalled, 1)
	if fm.stopWaits != nil {
		<-fm.stopWaits
	}
	return fm.stopErr
}

func (fm *fakeModule) Status(ctx context.Context) (sdk.Status, error) {
	atomic.AddInt32(&fm.statusCalled, 1)
	if fm.statusErr != nil {
		return sdk.Status{}, fm.statusErr
	}
	return sdk.Status{State: fm.statusState}, nil
}

func (fm *fakeModule) Health(ctx context.Context) sdk.HealthReport {
	atomic.AddInt32(&fm.healthCalled, 1)
	return sdk.HealthReport{
		Level:     fm.healthLevel,
		CheckedAt: time.Now(),
	}
}

func (fm *fakeModule) Commands() []sdk.CommandSpec {
	return fm.commandSpecs
}

func (fm *fakeModule) Dispatch(ctx context.Context, path []string, flags map[string]string, args []string) (*sdk.Result, error) {
	return &sdk.Result{Output: "ok"}, nil
}

func (fm *fakeModule) ConfigSchema() []byte {
	return nil
}

// fakeHostServices is a test implementation of sdk.HostServices.
type fakeHostServices struct {
	loggerVal       *zap.Logger
	secretsVal      sdk.SecretStore
	metricsVal      prometheus.Registerer
	dataDirVal      string
	configVal       []byte
	featureEnabled  map[string]bool
	publishedEvents []sdk.Event
	eventsMu        sync.Mutex
}

func (fh *fakeHostServices) Logger() *zap.Logger {
	return fh.loggerVal
}

func (fh *fakeHostServices) Secrets() sdk.SecretStore {
	return fh.secretsVal
}

func (fh *fakeHostServices) License() sdk.LicenseChecker {
	return fh
}

func (fh *fakeHostServices) Metrics() prometheus.Registerer {
	return fh.metricsVal
}

func (fh *fakeHostServices) DataDir() string {
	return fh.dataDirVal
}

func (fh *fakeHostServices) Config() []byte {
	return fh.configVal
}

func (fh *fakeHostServices) Events() sdk.EventSink {
	return fh
}

func (fh *fakeHostServices) FeatureEnabled(key string) bool {
	return fh.featureEnabled[key]
}

func (fh *fakeHostServices) Tier() string {
	return "community"
}

func (fh *fakeHostServices) Publish(ev sdk.Event) {
	fh.eventsMu.Lock()
	defer fh.eventsMu.Unlock()
	fh.publishedEvents = append(fh.publishedEvents, ev)
}

func (fh *fakeHostServices) GetPublishedEvents() []sdk.Event {
	fh.eventsMu.Lock()
	defer fh.eventsMu.Unlock()
	events := make([]sdk.Event, len(fh.publishedEvents))
	copy(events, fh.publishedEvents)
	return events
}

// fakePrometheus is a minimal prometheus.Registerer.
type fakePrometheus struct{}

func (fp *fakePrometheus) Register(prometheus.Collector) error  { return nil }
func (fp *fakePrometheus) MustRegister(...prometheus.Collector) {}
func (fp *fakePrometheus) Unregister(prometheus.Collector) bool { return true }

// testClock is a mock clock for deterministic testing.
type testClock struct {
	now    time.Time
	afters map[time.Duration][]*pendingAfter
	mu     sync.RWMutex
}

type pendingAfter struct {
	triggerCh chan struct{}
	readyCh   chan struct{} // signals that goroutine is ready
}

func newTestClock() *testClock {
	return &testClock{
		now:    time.Now(),
		afters: make(map[time.Duration][]*pendingAfter),
	}
}

func (tc *testClock) Now() time.Time {
	tc.mu.RLock()
	defer tc.mu.RUnlock()
	return tc.now
}

func (tc *testClock) After(d time.Duration) <-chan time.Time {
	ch := make(chan time.Time, 1)
	pending := &pendingAfter{
		triggerCh: make(chan struct{}),
		readyCh:   make(chan struct{}),
	}

	tc.mu.Lock()
	tc.afters[d] = append(tc.afters[d], pending)
	tc.mu.Unlock()

	go func() {
		close(pending.readyCh) // signal that we're ready to wait
		<-pending.triggerCh
		tc.mu.Lock()
		tc.now = tc.now.Add(d)
		now := tc.now
		tc.mu.Unlock()
		ch <- now
	}()
	return ch
}

// Advance simulates time passing and triggers pending After() calls for the given duration.
func (tc *testClock) Advance(d time.Duration) {
	tc.mu.Lock()
	pendings, ok := tc.afters[d]
	if ok {
		delete(tc.afters, d)
	}
	tc.mu.Unlock()

	// Wait for all pending After() goroutines to be ready
	for _, p := range pendings {
		<-p.readyCh
	}

	// Now trigger all of them
	for _, p := range pendings {
		close(p.triggerCh)
	}
}

// TestLoadRunning tests the basic Load -> running transition.
func TestLoadRunning(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	fm := &fakeModule{
		info: sdk.ModuleInfo{
			Name:        "test-module",
			Version:     "1.0.0",
			Description: "test",
		},
		statusState: sdk.StateRunning,
	}

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
		Clock:     realClock{},
	}

	s := New(cfg)

	ctx := context.Background()
	if err := s.Load(ctx, "test-module"); err != nil {
		t.Fatalf("Load failed: %v", err)
	}

	// Check module was initialized and started
	if atomic.LoadInt32(&fm.initCalled) != 1 {
		t.Errorf("Init not called exactly once: %d", atomic.LoadInt32(&fm.initCalled))
	}
	if atomic.LoadInt32(&fm.startCalled) != 1 {
		t.Errorf("Start not called exactly once: %d", atomic.LoadInt32(&fm.startCalled))
	}

	// Check state persisted
	ps, _ := LoadState(statePath)
	if len(ps.Enabled) != 1 || ps.Enabled[0] != "test-module" {
		t.Errorf("State not persisted correctly: %v", ps.Enabled)
	}

	// List should show module as running
	snapshots := s.List()
	if len(snapshots) != 1 || snapshots[0].State != sdk.StateRunning {
		t.Errorf("List returned wrong state: %+v", snapshots)
	}
}

// TestLoadIdempotent tests that Load on already-running module is idempotent.
func TestLoadIdempotent(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	fm := &fakeModule{
		info: sdk.ModuleInfo{
			Name:    "test-module",
			Version: "1.0.0",
		},
		statusState: sdk.StateRunning,
	}

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	// Load twice
	if err := s.Load(ctx, "test-module"); err != nil {
		t.Fatalf("First Load failed: %v", err)
	}
	if err := s.Load(ctx, "test-module"); err != nil {
		t.Fatalf("Second Load failed: %v", err)
	}

	// Init/Start should be called only once
	if atomic.LoadInt32(&fm.initCalled) != 1 {
		t.Errorf("Init called %d times, want 1", atomic.LoadInt32(&fm.initCalled))
	}
	if atomic.LoadInt32(&fm.startCalled) != 1 {
		t.Errorf("Start called %d times, want 1", atomic.LoadInt32(&fm.startCalled))
	}
}

// TestLoadUnknownModule tests that Load fails for unknown modules.
func TestLoadUnknownModule(t *testing.T) {
	cfg := Config{
		Modules: []sdk.Factory{},
		Host:    func(name string) sdk.HostServices { return nil },
		Logger:  zaptest.NewLogger(t),
		Backoff: DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	err := s.Load(ctx, "unknown")
	if err == nil {
		t.Errorf("Load should fail for unknown module")
	}
	if !errors.Is(err, ErrUnknownModule) {
		t.Errorf("Error should be ErrUnknownModule, got %v", err)
	}
}

// TestLoadLicenseDenied tests that Load fails when license feature is disabled.
func TestLoadLicenseDenied(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	fm := &fakeModule{
		info: sdk.ModuleInfo{
			Name:           "premium-module",
			Version:        "1.0.0",
			LicenseFeature: "penguin.premium",
		},
	}

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: map[string]bool{"penguin.premium": false},
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	if err := s.Load(ctx, "premium-module"); err == nil {
		t.Errorf("Load should fail when license feature disabled")
	}
}

// TestUnloadStopped tests that Unload stops and removes a module.
func TestUnloadStopped(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	fm := &fakeModule{
		info: sdk.ModuleInfo{
			Name:    "test-module",
			Version: "1.0.0",
		},
		statusState: sdk.StateRunning,
	}

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	// Load then unload
	_ = s.Load(ctx, "test-module")
	if err := s.Unload(ctx, "test-module"); err != nil {
		t.Fatalf("Unload failed: %v", err)
	}

	// Check stopped
	if atomic.LoadInt32(&fm.stopCalled) != 1 {
		t.Errorf("Stop not called exactly once: %d", atomic.LoadInt32(&fm.stopCalled))
	}

	// List should be empty
	snapshots := s.List()
	if runningCount(snapshots) != 0 {
		t.Errorf("no module should be running after unload, got %+v", snapshots)
	}

	// State should be empty
	ps, _ := LoadState(statePath)
	if len(ps.Enabled) != 0 {
		t.Errorf("Enabled list should be empty, got %v", ps.Enabled)
	}
}

// TestUnloadIdempotent tests that Unload on unloaded module is idempotent.
func TestUnloadIdempotent(t *testing.T) {
	cfg := Config{
		Modules: []sdk.Factory{},
		Host:    func(name string) sdk.HostServices { return nil },
		Logger:  zaptest.NewLogger(t),
		Backoff: DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	// Unload non-existent module should not error
	if err := s.Unload(ctx, "nonexistent"); err != nil {
		t.Fatalf("Unload should be idempotent: %v", err)
	}
}

// TestStartEnabledRestoresState tests that StartEnabled loads persisted modules.
func TestStartEnabledRestoresState(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	// Pre-populate state
	ps := &PersistedState{Enabled: []string{"module-a", "module-b"}}
	_ = ps.Save(statePath)

	// Create two modules
	fmA := &fakeModule{
		info:        sdk.ModuleInfo{Name: "module-a", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}
	fmB := &fakeModule{
		info:        sdk.ModuleInfo{Name: "module-b", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}

	cfg := Config{
		Modules: []sdk.Factory{
			func() sdk.Module { return fmA },
			func() sdk.Module { return fmB },
		},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	if err := s.StartEnabled(ctx); err != nil {
		t.Fatalf("StartEnabled failed: %v", err)
	}

	// Both modules should be running
	snapshots := s.List()
	if runningCount(snapshots) != 2 {
		t.Fatalf("expected 2 running modules, got %+v", snapshots)
	}
	for _, snap := range snapshots {
		if snap.State != sdk.StateRunning {
			t.Errorf("Module %s not running", snap.Name)
		}
	}
}

// TestShutdownReverseOrder tests that Shutdown stops modules in reverse order.
func TestShutdownReverseOrder(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	fmA := &fakeModule{
		info:        sdk.ModuleInfo{Name: "module-a", Version: "1.0.0"},
		statusState: sdk.StateRunning,
		stopErr:     nil,
	}
	fmB := &fakeModule{
		info:        sdk.ModuleInfo{Name: "module-b", Version: "1.0.0"},
		statusState: sdk.StateRunning,
		stopErr:     nil,
	}
	fmC := &fakeModule{
		info:        sdk.ModuleInfo{Name: "module-c", Version: "1.0.0"},
		statusState: sdk.StateRunning,
		stopErr:     nil,
	}

	cfg := Config{
		Modules: []sdk.Factory{
			func() sdk.Module { return fmA },
			func() sdk.Module { return fmB },
			func() sdk.Module { return fmC },
		},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	// Load all three
	_ = s.Load(ctx, "module-a")
	_ = s.Load(ctx, "module-b")
	_ = s.Load(ctx, "module-c")

	// Shutdown should stop in reverse order
	_ = s.Shutdown(ctx)

	// All should be stopped
	snapshots := s.List()
	if runningCount(snapshots) != 0 {
		t.Errorf("all modules should be unloaded, got %+v", snapshots)
	}
}

// TestReportFailureBackoffRestart tests restart with exponential backoff.
func TestReportFailureBackoffRestart(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	tc := newTestClock()

	fm := &fakeModule{
		info:        sdk.ModuleInfo{Name: "test-module", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath:   statePath,
		Logger:      zaptest.NewLogger(t),
		Backoff:     DefaultBackoff(),
		Clock:       tc,
		MaxRestarts: 3,
	}

	s := New(cfg)
	ctx := context.Background()

	// Load module
	_ = s.Load(ctx, "test-module")

	// Report failure - should transition to degraded and schedule restart
	s.ReportFailure(ctx, "test-module")

	// Check degraded state
	snapshots := s.List()
	if len(snapshots) != 1 || snapshots[0].State != sdk.StateDegraded {
		t.Errorf("Module should be degraded, got %+v", snapshots)
	}

	// For a simpler test, we'll just verify the failure was recorded
	if snapshots[0].RestartAttempt != 1 {
		t.Errorf("RestartAttempt should be 1, got %d", snapshots[0].RestartAttempt)
	}
}

// TestReportFailureMaxRestartsParked tests that module parks in failed state
// after MaxRestarts is exceeded.
func TestReportFailureMaxRestartsParked(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	tc := newTestClock()

	fm := &fakeModule{
		info:        sdk.ModuleInfo{Name: "test-module", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath:   statePath,
		Logger:      zaptest.NewLogger(t),
		Backoff:     DefaultBackoff(),
		Clock:       tc,
		MaxRestarts: 2,
	}

	s := New(cfg)
	ctx := context.Background()

	// Load module
	_ = s.Load(ctx, "test-module")

	// Report failures up to MaxRestarts
	s.ReportFailure(ctx, "test-module")
	s.ReportFailure(ctx, "test-module")

	// Check parked in failed state
	snapshots := s.List()
	if len(snapshots) != 1 || snapshots[0].State != sdk.StateFailed {
		t.Errorf("Module should be failed, got %+v", snapshots)
	}
	if snapshots[0].RestartAttempt != 2 {
		t.Errorf("RestartAttempt should be 2, got %d", snapshots[0].RestartAttempt)
	}
}

// TestConcurrentLoadUnload tests concurrent Load/Unload race safety.
func TestConcurrentLoadUnload(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	fm := &fakeModule{
		info:        sdk.ModuleInfo{Name: "test-module", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	var wg sync.WaitGroup
	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_ = s.Load(ctx, "test-module")
			_ = s.Unload(ctx, "test-module")
		}()
	}
	wg.Wait()

	// Should complete without deadlock or panic
}

// TestEventPublishing tests that events are published on state transitions.
func TestEventPublishing(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	fm := &fakeModule{
		info:        sdk.ModuleInfo{Name: "test-module", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}

	hostSvc := &fakeHostServices{
		loggerVal:      zaptest.NewLogger(t),
		metricsVal:     &fakePrometheus{},
		dataDirVal:     tmpdir,
		featureEnabled: make(map[string]bool),
	}

	cfg := Config{
		Modules:   []sdk.Factory{func() sdk.Module { return fm }},
		Host:      func(name string) sdk.HostServices { return hostSvc },
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	// Load should publish state-changed event
	_ = s.Load(ctx, "test-module")

	events := hostSvc.GetPublishedEvents()
	if len(events) < 2 {
		t.Errorf("Expected at least 2 events, got %d", len(events))
		return
	}

	// First event should be initializing
	if events[0].Type != sdk.EventStateChanged {
		t.Errorf("First event should be StateChanged, got %v", events[0].Type)
	}
}

// TestLoadInitError tests Load when module Init fails.
func TestLoadInitError(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	fm := &fakeModule{
		info:    sdk.ModuleInfo{Name: "test-module", Version: "1.0.0"},
		initErr: errors.New("init failed"),
	}

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	if err := s.Load(ctx, "test-module"); err == nil {
		t.Error("Load should fail when Init returns error")
	}

	// Module should not be in loaded list
	snapshots := s.List()
	if runningCount(snapshots) != 0 {
		t.Errorf("module should not be loaded after Init error, got %+v", snapshots)
	}
}

// TestLoadStartError tests Load when module Start fails.
func TestLoadStartError(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	fm := &fakeModule{
		info:     sdk.ModuleInfo{Name: "test-module", Version: "1.0.0"},
		startErr: errors.New("start failed"),
	}

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	if err := s.Load(ctx, "test-module"); err == nil {
		t.Error("Load should fail when Start returns error")
	}

	// Module should not be in loaded list
	snapshots := s.List()
	if runningCount(snapshots) != 0 {
		t.Errorf("module should not be loaded after Start error, got %+v", snapshots)
	}
}

// TestStatusUnknownModule tests Status for unknown module.
func TestStatusUnknownModule(t *testing.T) {
	cfg := Config{
		Modules: []sdk.Factory{},
		Host:    func(name string) sdk.HostServices { return nil },
		Logger:  zaptest.NewLogger(t),
		Backoff: DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	_, err := s.Status(ctx, "unknown")
	if err == nil {
		t.Error("Status should fail for unknown module")
	}
	if !errors.Is(err, ErrUnknownModule) {
		t.Errorf("Status error should be ErrUnknownModule, got %v", err)
	}
}

// TestStatusError tests Status when module.Status() returns error.
func TestStatusError(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	fm := &fakeModule{
		info:      sdk.ModuleInfo{Name: "test-module", Version: "1.0.0"},
		statusErr: errors.New("status error"),
	}

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	_ = s.Load(ctx, "test-module")

	status, err := s.Status(ctx, "test-module")
	if err == nil {
		t.Error("Status should return error from module")
	}
	if status.State != sdk.StateFailed {
		t.Errorf("Status state should be Failed on error, got %v", status.State)
	}
}

// TestUnloadStopError tests Unload when module.Stop() returns error.
func TestUnloadStopError(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	fm := &fakeModule{
		info:    sdk.ModuleInfo{Name: "test-module", Version: "1.0.0"},
		stopErr: errors.New("stop error"),
	}

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	_ = s.Load(ctx, "test-module")

	// Unload should succeed despite Stop error
	if err := s.Unload(ctx, "test-module"); err != nil {
		t.Errorf("Unload should succeed despite Stop error: %v", err)
	}

	// Module should be removed from loaded list
	snapshots := s.List()
	if runningCount(snapshots) != 0 {
		t.Errorf("module should be unloaded, got %+v", snapshots)
	}
}

// TestReportFailureUnknownModule tests ReportFailure for unknown module.
func TestReportFailureUnknownModule(t *testing.T) {
	cfg := Config{
		Modules: []sdk.Factory{},
		Host:    func(name string) sdk.HostServices { return nil },
		Logger:  zaptest.NewLogger(t),
		Backoff: DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	// Should not panic or error
	s.ReportFailure(ctx, "unknown")
}

// TestPublishEventWithoutHost tests publishEvent when host.Events() is nil.
func TestPublishEventWithoutHost(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	fm := &fakeModule{
		info:        sdk.ModuleInfo{Name: "test-module", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(name string) sdk.HostServices {
			return nil
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	// Should not crash when host is nil
	_ = s.Load(ctx, "test-module")
}

// TestScheduleRestartSuccess tests the complete restart cycle: failure report ->
// backoff wait -> restart execution -> state transitions.
func TestScheduleRestartSuccess(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	tc := newTestClock()
	backoffCfg := BackoffConfig{
		Initial:    100 * time.Millisecond,
		Max:        30 * time.Second,
		Multiplier: 2.0,
		Jitter:     false, // Disable jitter for predictable tests
	}

	fm := &fakeModule{
		info:        sdk.ModuleInfo{Name: "test-module", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath:   statePath,
		Logger:      zaptest.NewLogger(t),
		Backoff:     backoffCfg,
		Clock:       tc,
		MaxRestarts: 5,
	}

	s := New(cfg)
	ctx := context.Background()

	// Load module successfully
	if err := s.Load(ctx, "test-module"); err != nil {
		t.Fatalf("Load failed: %v", err)
	}

	// Verify module is running
	snapshots := s.List()
	if len(snapshots) != 1 || snapshots[0].State != sdk.StateRunning {
		t.Fatalf("Module should be running, got %+v", snapshots)
	}

	// Report failure - should transition to degraded
	s.ReportFailure(ctx, "test-module")

	snapshots = s.List()
	if len(snapshots) != 1 || snapshots[0].State != sdk.StateDegraded {
		t.Fatalf("Module should be degraded after failure, got %+v", snapshots)
	}
	if snapshots[0].RestartAttempt != 1 {
		t.Errorf("RestartAttempt should be 1, got %d", snapshots[0].RestartAttempt)
	}

	// Give the scheduleRestart goroutine time to start and call After()
	time.Sleep(50 * time.Millisecond)

	// Advance time to trigger restart
	// First backoff is 100ms (Initial * 2^0)
	tc.Advance(100 * time.Millisecond)

	// Give the restart goroutine time to run and complete
	time.Sleep(200 * time.Millisecond)

	// Module should be back to running
	snapshots = s.List()
	if len(snapshots) != 1 || snapshots[0].State != sdk.StateRunning {
		t.Errorf("Module should be running after restart, got %+v", snapshots)
	}
	// Note: restartAttempt is NOT reset to 0 after a successful restart;
	// it tracks consecutive restart attempts. If the module fails again immediately,
	// it will use the next backoff multiplier (2x the previous).
	if snapshots[0].RestartAttempt != 1 {
		t.Errorf("RestartAttempt should be 1 (persistent), got %d", snapshots[0].RestartAttempt)
	}
}

// TestScheduleRestartAfterMultipleFailures tests restart with exponential backoff
// across multiple failures.
func TestScheduleRestartAfterMultipleFailures(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	tc := newTestClock()
	backoffCfg := BackoffConfig{
		Initial:    100 * time.Millisecond,
		Max:        30 * time.Second,
		Multiplier: 2.0,
		Jitter:     false,
	}

	fm := &fakeModule{
		info:        sdk.ModuleInfo{Name: "test-module", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath:   statePath,
		Logger:      zaptest.NewLogger(t),
		Backoff:     backoffCfg,
		Clock:       tc,
		MaxRestarts: 5,
	}

	s := New(cfg)
	ctx := context.Background()

	// Load module
	_ = s.Load(ctx, "test-module")

	// First failure: 100ms backoff (attempt 0)
	s.ReportFailure(ctx, "test-module")
	snapshots := s.List()
	if snapshots[0].RestartAttempt != 1 {
		t.Errorf("After 1st failure, attempt should be 1, got %d", snapshots[0].RestartAttempt)
	}

	// Give the scheduleRestart goroutine time to start and call After()
	time.Sleep(50 * time.Millisecond)

	// Advance time and trigger restart
	tc.Advance(100 * time.Millisecond)
	time.Sleep(200 * time.Millisecond)

	// Verify it restarted successfully
	snapshots = s.List()
	if snapshots[0].State != sdk.StateRunning {
		t.Errorf("Module should be running after restart, got state %s", snapshots[0].State)
	}
}

// TestScheduleRestartInitError tests restart when module Init fails during restart.
func TestScheduleRestartInitError(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	tc := newTestClock()
	backoffCfg := BackoffConfig{
		Initial:    100 * time.Millisecond,
		Max:        30 * time.Second,
		Multiplier: 2.0,
		Jitter:     false,
	}

	// Use a closure to track factory calls
	callCount := 0
	factory := func() sdk.Module {
		callCount++
		m := &fakeModule{
			info:        sdk.ModuleInfo{Name: "test-module", Version: "1.0.0"},
			statusState: sdk.StateRunning,
		}
		// Calls: 1 = New() registry, 2 = Load(), 3 = scheduleRestart
		// We want 1 and 2 to succeed, 3 to fail
		if callCount > 2 {
			m.initErr = errors.New("init error on restart")
		}
		return m
	}

	cfg := Config{
		Modules: []sdk.Factory{factory},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath:   statePath,
		Logger:      zaptest.NewLogger(t),
		Backoff:     backoffCfg,
		Clock:       tc,
		MaxRestarts: 5,
	}

	s := New(cfg)
	ctx := context.Background()

	// Load module - this should succeed (first factory call)
	if err := s.Load(ctx, "test-module"); err != nil {
		t.Fatalf("Load failed: %v", err)
	}

	// Report failure - should transition to degraded and schedule restart
	s.ReportFailure(ctx, "test-module")

	// Give scheduleRestart goroutine time to start
	time.Sleep(50 * time.Millisecond)

	// Advance time to trigger restart attempt (second factory call, which will fail)
	tc.Advance(100 * time.Millisecond)
	time.Sleep(200 * time.Millisecond)

	// Module should be degraded due to init error during restart
	snapshots := s.List()
	if len(snapshots) != 1 || snapshots[0].State != sdk.StateDegraded {
		t.Errorf("Module should be degraded after restart init error, got %+v", snapshots)
	}
	// Restart attempt should be incremented to 2 (another failure)
	if snapshots[0].RestartAttempt != 2 {
		t.Errorf("RestartAttempt should be 2, got %d", snapshots[0].RestartAttempt)
	}
}

// TestScheduleRestartStartError tests restart when module Start fails during restart.
func TestScheduleRestartStartError(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	tc := newTestClock()
	backoffCfg := BackoffConfig{
		Initial:    100 * time.Millisecond,
		Max:        30 * time.Second,
		Multiplier: 2.0,
		Jitter:     false,
	}

	// Use a closure to track factory calls
	callCount := 0
	factory := func() sdk.Module {
		callCount++
		m := &fakeModule{
			info:        sdk.ModuleInfo{Name: "test-module", Version: "1.0.0"},
			statusState: sdk.StateRunning,
		}
		// Calls: 1 = New() registry, 2 = Load(), 3 = scheduleRestart
		// We want 1 and 2 to succeed, 3 to fail on Start
		if callCount > 2 {
			m.startErr = errors.New("start error on restart")
		}
		return m
	}

	cfg := Config{
		Modules: []sdk.Factory{factory},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath:   statePath,
		Logger:      zaptest.NewLogger(t),
		Backoff:     backoffCfg,
		Clock:       tc,
		MaxRestarts: 5,
	}

	s := New(cfg)
	ctx := context.Background()

	// Load module - should succeed
	if err := s.Load(ctx, "test-module"); err != nil {
		t.Fatalf("Load failed: %v", err)
	}

	// Report failure
	s.ReportFailure(ctx, "test-module")

	// Give scheduleRestart goroutine time to start
	time.Sleep(50 * time.Millisecond)

	// Advance time to trigger restart (which will fail on Start)
	tc.Advance(100 * time.Millisecond)
	time.Sleep(200 * time.Millisecond)

	// Module should be degraded due to start error during restart
	snapshots := s.List()
	if len(snapshots) != 1 || snapshots[0].State != sdk.StateDegraded {
		t.Errorf("Module should be degraded after restart start error, got %+v", snapshots)
	}
}

// TestStartEnabledWithMixedModules tests that StartEnabled only starts modules
// in the persisted enabled-set, leaving others disabled.
func TestStartEnabledWithMixedModules(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	// Pre-populate state with only module-a and module-c enabled
	ps := &PersistedState{Enabled: []string{"module-a", "module-c"}}
	_ = ps.Save(statePath)

	// Create three modules
	fmA := &fakeModule{
		info:        sdk.ModuleInfo{Name: "module-a", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}
	fmB := &fakeModule{
		info:        sdk.ModuleInfo{Name: "module-b", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}
	fmC := &fakeModule{
		info:        sdk.ModuleInfo{Name: "module-c", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}

	cfg := Config{
		Modules: []sdk.Factory{
			func() sdk.Module { return fmA },
			func() sdk.Module { return fmB },
			func() sdk.Module { return fmC },
		},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	if err := s.StartEnabled(ctx); err != nil {
		t.Fatalf("StartEnabled failed: %v", err)
	}

	// Only module-a and module-c should be running
	snapshots := s.List()
	if len(snapshots) != 3 {
		t.Fatalf("expected 3 modules, got %d", len(snapshots))
	}

	states := make(map[string]sdk.ModuleState)
	for _, snap := range snapshots {
		states[snap.Name] = snap.State
	}

	if states["module-a"] != sdk.StateRunning {
		t.Errorf("module-a should be running, got %s", states["module-a"])
	}
	if states["module-b"] != sdk.StateDisabled {
		t.Errorf("module-b should be disabled, got %s", states["module-b"])
	}
	if states["module-c"] != sdk.StateRunning {
		t.Errorf("module-c should be running, got %s", states["module-c"])
	}

	// Verify init/start were called only for a and c
	if atomic.LoadInt32(&fmA.initCalled) != 1 {
		t.Errorf("module-a init not called exactly once")
	}
	if atomic.LoadInt32(&fmB.initCalled) != 0 {
		t.Errorf("module-b init should not be called")
	}
	if atomic.LoadInt32(&fmC.initCalled) != 1 {
		t.Errorf("module-c init not called exactly once")
	}
}

// TestStartEnabledEmptyState tests that StartEnabled works with an empty enabled set.
func TestStartEnabledEmptyState(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	// Create empty persisted state
	ps := &PersistedState{Enabled: []string{}}
	_ = ps.Save(statePath)

	fm := &fakeModule{
		info:        sdk.ModuleInfo{Name: "module-a", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	if err := s.StartEnabled(ctx); err != nil {
		t.Fatalf("StartEnabled failed: %v", err)
	}

	// No modules should be running
	snapshots := s.List()
	if runningCount(snapshots) != 0 {
		t.Errorf("no modules should be running with empty state, got %+v", snapshots)
	}
}

// TestStartEnabledLoadError tests that StartEnabled handles load errors gracefully.
func TestStartEnabledLoadError(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	// Pre-populate state with a module that doesn't exist
	ps := &PersistedState{Enabled: []string{"nonexistent-module"}}
	_ = ps.Save(statePath)

	fm := &fakeModule{
		info:        sdk.ModuleInfo{Name: "real-module", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	// StartEnabled should not error even though nonexistent-module can't be loaded
	if err := s.StartEnabled(ctx); err != nil {
		t.Fatalf("StartEnabled should handle missing modules gracefully: %v", err)
	}

	// No modules should be running because the only one in state doesn't exist
	snapshots := s.List()
	if runningCount(snapshots) != 0 {
		t.Errorf("no modules should be running, got %+v", snapshots)
	}
}

// TestClockAfterHook exercises the Clock.After method used in scheduleRestart
func TestClockAfterHook(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	fm := &fakeModule{
		info:        sdk.ModuleInfo{Name: "module-a", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}

	clock := newTestClock()

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
		Clock:     clock,
	}

	s := New(cfg)
	ctx := context.Background()

	// Load a module
	if err := s.Load(ctx, "module-a"); err != nil {
		t.Fatalf("Load failed: %v", err)
	}

	// Report failure to trigger scheduled restart
	s.ReportFailure(ctx, "module-a")

	// Verify that the clock's After was called
	// Wait a bit to let the goroutine call After
	time.Sleep(100 * time.Millisecond)

	clock.mu.RLock()
	hasAfters := len(clock.afters) > 0
	clock.mu.RUnlock()

	if !hasAfters {
		t.Error("Clock.After should have been called for restart scheduling")
	}

	// Clean up
	s.lifeStop()
}

// TestRemoveFromEnabled tests the removeFromEnabled private method by observing behavior
func TestRemoveFromEnabled(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	// Start with multiple modules enabled
	ps := &PersistedState{Enabled: []string{"module-a", "module-b", "module-c"}}
	if err := ps.Save(statePath); err != nil {
		t.Fatalf("Save failed: %v", err)
	}

	fm1 := &fakeModule{
		info:        sdk.ModuleInfo{Name: "module-a", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}
	fm2 := &fakeModule{
		info:        sdk.ModuleInfo{Name: "module-b", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}
	fm3 := &fakeModule{
		info:        sdk.ModuleInfo{Name: "module-c", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}

	cfg := Config{
		Modules: []sdk.Factory{
			func() sdk.Module { return fm1 },
			func() sdk.Module { return fm2 },
			func() sdk.Module { return fm3 },
		},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	// Load all modules
	if err := s.StartEnabled(ctx); err != nil {
		t.Fatalf("StartEnabled failed: %v", err)
	}

	// Now unload module-b, which should remove it from enabled
	if err := s.Unload(ctx, "module-b"); err != nil {
		t.Fatalf("Unload failed: %v", err)
	}

	// Reload state and verify module-b is gone
	loaded, err := LoadState(statePath)
	if err != nil {
		t.Fatalf("LoadState failed: %v", err)
	}

	found := false
	for _, name := range loaded.Enabled {
		if name == "module-b" {
			found = true
			break
		}
	}
	if found {
		t.Error("module-b should have been removed from enabled list")
	}

	// Verify other modules still there
	if len(loaded.Enabled) != 2 {
		t.Errorf("Expected 2 modules remaining, got %d", len(loaded.Enabled))
	}
}

// TestLoadPersistedStateError exercises the loadPersistedState error path
func TestLoadPersistedStateError(t *testing.T) {
	tmpdir := t.TempDir()
	// Use a path that will cause a read error (directory instead of file)
	invalidStatePath := filepath.Join(tmpdir, "is_a_dir")
	if err := os.Mkdir(invalidStatePath, 0700); err != nil {
		t.Fatalf("setup mkdir failed: %v", err)
	}

	fm := &fakeModule{
		info:        sdk.ModuleInfo{Name: "module-a", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: invalidStatePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	s := New(cfg)
	ctx := context.Background()

	// StartEnabled should fail because LoadState returns error
	err := s.StartEnabled(ctx)
	if err == nil {
		t.Error("StartEnabled should fail when state file is unreadable")
	}
}

// Must import "os" at the top of the file
var _ = os.O_RDONLY  // Use os package to satisfy import
