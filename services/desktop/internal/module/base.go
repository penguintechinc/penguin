package module

import (
	"sync"

	"github.com/sirupsen/logrus"
)

// BaseModule provides shared lifecycle state for desktop modules.
// Embed this in module structs to replace inline mutex + started flag patterns.
type BaseModule struct {
	mu      sync.RWMutex
	started bool
	Logger  *logrus.Logger
}

// MarkStarted sets the started flag. Call from Start().
func (b *BaseModule) MarkStarted() {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.started = true
}

// MarkStopped clears the started flag. Call from Stop().
func (b *BaseModule) MarkStopped() {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.started = false
}

// IsStarted returns whether the module has been started.
func (b *BaseModule) IsStarted() bool {
	b.mu.RLock()
	defer b.mu.RUnlock()
	return b.started
}

// RLock acquires a read lock on the module's mutex.
func (b *BaseModule) RLock() {
	b.mu.RLock()
}

// RUnlock releases the read lock.
func (b *BaseModule) RUnlock() {
	b.mu.RUnlock()
}

// Lock acquires a write lock on the module's mutex.
func (b *BaseModule) Lock() {
	b.mu.Lock()
}

// Unlock releases the write lock.
func (b *BaseModule) Unlock() {
	b.mu.Unlock()
}

// NotStartedStatus returns a HealthStatus indicating the module is not started.
func (b *BaseModule) NotStartedStatus() HealthStatus {
	return HealthStatus{State: HealthUnknown, Message: "not started"}
}

// ClientNotConfiguredStatus returns a HealthStatus indicating no client is configured.
func (b *BaseModule) ClientNotConfiguredStatus() HealthStatus {
	return HealthStatus{State: HealthUnknown, Message: "client not configured"}
}
