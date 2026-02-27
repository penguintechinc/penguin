package module

import (
	"context"
	"sync"
	"time"

	"github.com/sirupsen/logrus"
)

// HealthRunner periodically checks module health.
type HealthRunner struct {
	registry *Registry
	interval time.Duration
	logger   *logrus.Logger
	results  map[string]HealthStatus
	mu       sync.RWMutex
}

// NewHealthRunner creates a health check runner.
func NewHealthRunner(registry *Registry, interval time.Duration, logger *logrus.Logger) *HealthRunner {
	return &HealthRunner{
		registry: registry,
		interval: interval,
		logger:   logger,
		results:  make(map[string]HealthStatus),
	}
}

// Start begins periodic health checks.
func (h *HealthRunner) Start(ctx context.Context) {
	go h.run(ctx)
}

func (h *HealthRunner) run(ctx context.Context) {
	ticker := time.NewTicker(h.interval)
	defer ticker.Stop()

	// Initial check
	h.checkAll(ctx)

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			h.checkAll(ctx)
		}
	}
}

func (h *HealthRunner) checkAll(ctx context.Context) {
	for _, m := range h.registry.EnabledModules() {
		status := m.HealthCheck(ctx)
		h.mu.Lock()
		h.results[m.Name()] = status
		h.mu.Unlock()

		if status.State != HealthHealthy {
			h.logger.WithFields(logrus.Fields{
				"module":  m.Name(),
				"state":   status.State.String(),
				"message": status.Message,
			}).Warn("Module health check failed")
		}
	}
}

// GetStatus returns the health status for a module.
func (h *HealthRunner) GetStatus(name string) (HealthStatus, bool) {
	h.mu.RLock()
	defer h.mu.RUnlock()
	s, ok := h.results[name]
	return s, ok
}

// GetAllStatuses returns all health statuses.
func (h *HealthRunner) GetAllStatuses() map[string]HealthStatus {
	h.mu.RLock()
	defer h.mu.RUnlock()

	result := make(map[string]HealthStatus, len(h.results))
	for k, v := range h.results {
		result[k] = v
	}
	return result
}
