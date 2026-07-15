package daemon

import (
	"sync"
	"testing"

	"github.com/penguintechinc/penguin/pkg/sdk"
	"go.uber.org/zap/zaptest"
)

// TestSupervisorModule tests Module accessor.
func TestSupervisorModule(t *testing.T) {
	logger := zaptest.NewLogger(t)
	supervisor := &Supervisor{
		logger: logger,
		loaded: make(map[string]*moduleState),
		mu:     sync.RWMutex{},
	}

	// Test module not found
	mod, ok := supervisor.Module("nonexistent")
	if ok {
		t.Error("expected Module to return false for nonexistent module")
	}
	if mod != nil {
		t.Error("expected Module to return nil for nonexistent module")
	}

	// Add a module and test retrieval
	fakeModule := &fakeModule{
		info: sdk.ModuleInfo{Name: "test"},
	}

	supervisor.loaded["test"] = &moduleState{
		instance: fakeModule,
	}

	mod, ok = supervisor.Module("test")
	if !ok {
		t.Error("expected Module to return true for loaded module")
	}
	if mod == nil {
		t.Error("expected Module to return non-nil module")
	}
	if mod.Info().Name != "test" {
		t.Errorf("expected module name 'test', got %s", mod.Info().Name)
	}
}

// TestSupervisorHosts tests Hosts accessor.
func TestSupervisorHosts(t *testing.T) {
	logger := zaptest.NewLogger(t)
	supervisor := &Supervisor{
		logger: logger,
		loaded: make(map[string]*moduleState),
		mu:     sync.RWMutex{},
	}

	// Test hosts not found
	host := supervisor.Hosts("nonexistent")
	if host != nil {
		t.Error("expected Hosts to return nil for nonexistent module")
	}

	// Add a module with host and test retrieval
	fakeHost := &fakeHostServices{
		loggerVal: logger,
	}

	supervisor.loaded["test"] = &moduleState{
		instance: &fakeModule{
			info: sdk.ModuleInfo{Name: "test"},
		},
		host: fakeHost,
	}

	host = supervisor.Hosts("test")
	if host == nil {
		t.Error("expected Hosts to return non-nil host")
	}
	if host != fakeHost {
		t.Error("expected Hosts to return the same host object")
	}
}

// TestSupervisorModuleAndHostsConcurrency tests concurrent access.
func TestSupervisorModuleAndHostsConcurrency(t *testing.T) {
	logger := zaptest.NewLogger(t)
	supervisor := &Supervisor{
		logger: logger,
		loaded: make(map[string]*moduleState),
		mu:     sync.RWMutex{},
	}

	fakeModule := &fakeModule{
		info: sdk.ModuleInfo{Name: "test"},
	}
	fakeHost := &fakeHostServices{
		loggerVal: logger,
	}

	supervisor.loaded["test"] = &moduleState{
		instance: fakeModule,
		host:     fakeHost,
	}

	// Read from multiple goroutines concurrently
	done := make(chan bool, 10)
	for i := 0; i < 10; i++ {
		go func() {
			mod, ok := supervisor.Module("test")
			if !ok || mod == nil {
				t.Error("failed to get module")
			}

			host := supervisor.Hosts("test")
			if host == nil {
				t.Error("failed to get host")
			}

			done <- true
		}()
	}

	for i := 0; i < 10; i++ {
		<-done
	}
}

// TestSupervisorModuleEmptyName tests Module with empty string.
func TestSupervisorModuleEmptyName(t *testing.T) {
	logger := zaptest.NewLogger(t)
	supervisor := &Supervisor{
		logger: logger,
		loaded: make(map[string]*moduleState),
		mu:     sync.RWMutex{},
	}

	mod, ok := supervisor.Module("")
	if ok {
		t.Error("expected Module to return false for empty name")
	}
	if mod != nil {
		t.Error("expected Module to return nil for empty name")
	}
}

// TestSupervisorHostsEmptyName tests Hosts with empty string.
func TestSupervisorHostsEmptyName(t *testing.T) {
	logger := zaptest.NewLogger(t)
	supervisor := &Supervisor{
		logger: logger,
		loaded: make(map[string]*moduleState),
		mu:     sync.RWMutex{},
	}

	host := supervisor.Hosts("")
	if host != nil {
		t.Error("expected Hosts to return nil for empty name")
	}
}
