package pluginhost

import (
	"context"
	"sync"
	"time"

	"github.com/sirupsen/logrus"
)

// Supervisor monitors plugin processes and restarts them on failure.
type Supervisor struct {
	manager     *Manager
	mu          sync.RWMutex
	tracked     map[string]*trackedPlugin
	checkPeriod time.Duration
	logger      *logrus.Logger
	cancel      context.CancelFunc
}

type trackedPlugin struct {
	path       string
	restarts   int
	maxRestart int
	lastFail   time.Time
	healthy    bool
}

// Backoff durations for restart attempts.
var backoffDurations = []time.Duration{
	1 * time.Second,
	5 * time.Second,
	15 * time.Second,
}

const maxRestarts = 3

// NewSupervisor creates a Supervisor that watches plugin health.
func NewSupervisor(manager *Manager, logger *logrus.Logger) *Supervisor {
	return &Supervisor{
		manager:     manager,
		tracked:     make(map[string]*trackedPlugin),
		checkPeriod: 10 * time.Second,
		logger:      logger,
	}
}

// Track adds a plugin to the supervisor's watch list.
func (s *Supervisor) Track(name, path string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.tracked[name] = &trackedPlugin{
		path:       path,
		maxRestart: maxRestarts,
		healthy:    true,
	}
}

// Untrack removes a plugin from the supervisor's watch list.
func (s *Supervisor) Untrack(name string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.tracked, name)
}

// Start begins the supervision loop.
func (s *Supervisor) Start(ctx context.Context) {
	ctx, s.cancel = context.WithCancel(ctx)
	go s.loop(ctx)
	s.logger.Info("Plugin supervisor started")
}

// Stop halts the supervision loop.
func (s *Supervisor) Stop() {
	if s.cancel != nil {
		s.cancel()
	}
}

// IsHealthy reports whether a tracked plugin is considered healthy.
func (s *Supervisor) IsHealthy(name string) bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if tp, ok := s.tracked[name]; ok {
		return tp.healthy
	}
	return false
}

func (s *Supervisor) loop(ctx context.Context) {
	ticker := time.NewTicker(s.checkPeriod)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			s.check()
		}
	}
}

func (s *Supervisor) check() {
	s.mu.Lock()
	defer s.mu.Unlock()

	for name, tp := range s.tracked {
		if s.manager.IsRunning(name) {
			tp.healthy = true
			continue
		}

		// Plugin is not running
		tp.healthy = false

		if tp.restarts >= tp.maxRestart {
			s.logger.WithFields(logrus.Fields{
				"module":   name,
				"restarts": tp.restarts,
			}).Error("Plugin exceeded max restarts, marking unhealthy")
			continue
		}

		// Apply backoff
		backoffIdx := tp.restarts
		if backoffIdx >= len(backoffDurations) {
			backoffIdx = len(backoffDurations) - 1
		}
		backoff := backoffDurations[backoffIdx]

		if time.Since(tp.lastFail) < backoff {
			continue // Still in backoff period
		}

		s.logger.WithFields(logrus.Fields{
			"module":  name,
			"attempt": tp.restarts + 1,
		}).Warn("Plugin exited, attempting restart")

		tp.lastFail = time.Now()
		tp.restarts++

		// Restart — release lock briefly for the potentially slow Launch
		path := tp.path
		s.mu.Unlock()
		mp, err := s.manager.Launch(name, path)
		s.mu.Lock()

		if err != nil {
			s.logger.WithError(err).WithField("module", name).Error("Failed to restart plugin")
			continue
		}

		mp.RestartNum = tp.restarts
		tp.healthy = true
		s.logger.WithFields(logrus.Fields{
			"module":  name,
			"attempt": tp.restarts,
		}).Info("Plugin restarted successfully")
	}
}
