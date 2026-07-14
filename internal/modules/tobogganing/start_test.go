package tobogganing

import (
	"context"
	"testing"
	"time"

	"go.uber.org/zap/zaptest"
	"gopkg.in/yaml.v3"
)

// TestStartReturnsPromptly is the regression guard for a bug where Start made
// synchronous auth/tunnel HTTP calls to the Manager. Because the supervisor
// holds its lock across module.Start, a blocking Start wedged the entire
// daemon (no `penguin status`, shutdown hung, lock never released). Start must
// return promptly even when the Manager is unreachable; the monitor loop
// retries the connection in the background.
func TestStartReturnsPromptly(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	// A black-hole manager address: any real HTTP attempt would block for the
	// client timeout (tens of seconds).
	cfg := &ModuleConfig{
		ManagerURL:    "http://10.255.255.1:9", // TEST-NET / discard, unroutable
		NodeID:        "test-node",
		InterfaceName: "wg0",
	}
	y, _ := yaml.Marshal(cfg)
	host.config = y

	m := New()
	if err := m.Init(context.Background(), host); err != nil {
		t.Fatalf("Init: %v", err)
	}
	mod := m.(*Module)

	done := make(chan error, 1)
	go func() { done <- mod.Start(context.Background()) }()

	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("Start returned error: %v", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("Start did not return promptly with an unreachable manager — it must not block")
	}

	// Stop must also return promptly and be safe after a background-connecting
	// Start.
	stopDone := make(chan error, 1)
	go func() { stopDone <- mod.Stop(context.Background()) }()
	select {
	case <-stopDone:
	case <-time.After(2 * time.Second):
		t.Fatal("Stop did not return promptly")
	}
}
