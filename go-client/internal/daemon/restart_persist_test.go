package daemon

import (
	"context"
	"path/filepath"
	"testing"

	"github.com/penguintechinc/penguin/pkg/sdk"
	"go.uber.org/zap/zaptest"
)

// newTestSupervisor builds a supervisor over one fake module sharing statePath.
func newTestSupervisor(t *testing.T, statePath string, fm *fakeModule) *Supervisor {
	t.Helper()
	tmp := t.TempDir()
	return New(Config{
		Modules: []sdk.Factory{func() sdk.Module { return fm }},
		Host: func(string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmp,
				featureEnabled: map[string]bool{},
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	})
}

// TestShutdownKeepsModulesEnabled is the regression guard for a bug where
// daemon shutdown called Unload, wiping the persisted enabled-set — so a
// restart silently forgot every module the operator had loaded.
func TestShutdownKeepsModulesEnabled(t *testing.T) {
	statePath := filepath.Join(t.TempDir(), "state.json")
	ctx := context.Background()

	fm := &fakeModule{info: sdk.ModuleInfo{Name: "test-module", Version: "1.0.0"}, statusState: sdk.StateRunning}
	s1 := newTestSupervisor(t, statePath, fm)
	if err := s1.Load(ctx, "test-module"); err != nil {
		t.Fatalf("Load: %v", err)
	}
	if err := s1.Shutdown(ctx); err != nil {
		t.Fatalf("Shutdown: %v", err)
	}

	// The module was stopped...
	if runningCount(s1.List()) != 0 {
		t.Error("module should be stopped after shutdown")
	}
	// ...but must remain enabled on disk.
	ps, err := LoadState(statePath)
	if err != nil {
		t.Fatalf("LoadState: %v", err)
	}
	if len(ps.Enabled) != 1 || ps.Enabled[0] != "test-module" {
		t.Fatalf("shutdown must not disable modules; enabled=%v", ps.Enabled)
	}

	// A fresh daemon restores it.
	fm2 := &fakeModule{info: sdk.ModuleInfo{Name: "test-module", Version: "1.0.0"}, statusState: sdk.StateRunning}
	s2 := newTestSupervisor(t, statePath, fm2)
	if err := s2.StartEnabled(ctx); err != nil {
		t.Fatalf("StartEnabled: %v", err)
	}
	if runningCount(s2.List()) != 1 {
		t.Errorf("module should be restored after restart, got %+v", s2.List())
	}
}

// TestUnloadForgetsAcrossRestart is the counterpart: an explicit unload must
// persist, so the module does not come back.
func TestUnloadForgetsAcrossRestart(t *testing.T) {
	statePath := filepath.Join(t.TempDir(), "state.json")
	ctx := context.Background()

	fm := &fakeModule{info: sdk.ModuleInfo{Name: "test-module", Version: "1.0.0"}, statusState: sdk.StateRunning}
	s1 := newTestSupervisor(t, statePath, fm)
	if err := s1.Load(ctx, "test-module"); err != nil {
		t.Fatalf("Load: %v", err)
	}
	if err := s1.Unload(ctx, "test-module"); err != nil {
		t.Fatalf("Unload: %v", err)
	}

	fm2 := &fakeModule{info: sdk.ModuleInfo{Name: "test-module", Version: "1.0.0"}}
	s2 := newTestSupervisor(t, statePath, fm2)
	if err := s2.StartEnabled(ctx); err != nil {
		t.Fatalf("StartEnabled: %v", err)
	}
	if runningCount(s2.List()) != 0 {
		t.Errorf("unloaded module must not return after restart, got %+v", s2.List())
	}
}
