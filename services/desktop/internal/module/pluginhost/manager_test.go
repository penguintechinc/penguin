package pluginhost

import (
	"io"
	"testing"

	"github.com/sirupsen/logrus"
)

func TestManagerNewAndAll(t *testing.T) {
	logger := logrus.New()
	logger.SetOutput(io.Discard)

	m := NewManager(logger)
	if m == nil {
		t.Fatal("expected non-nil manager")
	}
	if len(m.All()) != 0 {
		t.Errorf("expected 0 plugins, got %d", len(m.All()))
	}
}

func TestManagerGetNotFound(t *testing.T) {
	logger := logrus.New()
	logger.SetOutput(io.Discard)

	m := NewManager(logger)
	_, ok := m.Get("nonexistent")
	if ok {
		t.Error("expected not found")
	}
}

func TestManagerIsRunningNotFound(t *testing.T) {
	logger := logrus.New()
	logger.SetOutput(io.Discard)

	m := NewManager(logger)
	if m.IsRunning("nonexistent") {
		t.Error("expected not running for nonexistent plugin")
	}
}

func TestManagerStopNonexistent(t *testing.T) {
	logger := logrus.New()
	logger.SetOutput(io.Discard)

	m := NewManager(logger)
	// Should not panic
	m.Stop("nonexistent")
}

func TestManagerStopAll(t *testing.T) {
	logger := logrus.New()
	logger.SetOutput(io.Discard)

	m := NewManager(logger)
	// Should not panic on empty
	m.StopAll()
}
