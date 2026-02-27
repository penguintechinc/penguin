package module

import (
	"context"
	"fmt"
	"sync"

	"github.com/sirupsen/logrus"
)

// Registry manages all registered modules (both legacy and plugin-based).
type Registry struct {
	mu       sync.RWMutex
	modules  map[string]ModuleBase
	enabled  map[string]bool
	started  map[string]bool
	order    []string // registration order for deterministic start/stop
	logger   *logrus.Logger
}

// NewRegistry creates a new module registry.
func NewRegistry(logger *logrus.Logger) *Registry {
	return &Registry{
		modules: make(map[string]ModuleBase),
		enabled: make(map[string]bool),
		started: make(map[string]bool),
		logger:  logger,
	}
}

// Register adds a module to the registry.
func (r *Registry) Register(m ModuleBase) error {
	r.mu.Lock()
	defer r.mu.Unlock()

	name := m.Name()
	if _, exists := r.modules[name]; exists {
		return fmt.Errorf("module %q already registered", name)
	}

	r.modules[name] = m
	r.order = append(r.order, name)
	r.logger.WithField("module", name).Info("Module registered")
	return nil
}

// Enable marks a module as enabled.
func (r *Registry) Enable(name string) error {
	r.mu.Lock()
	defer r.mu.Unlock()

	if _, exists := r.modules[name]; !exists {
		return fmt.Errorf("module %q not found", name)
	}
	r.enabled[name] = true
	return nil
}

// Disable marks a module as disabled.
func (r *Registry) Disable(name string) error {
	r.mu.Lock()
	defer r.mu.Unlock()

	if _, exists := r.modules[name]; !exists {
		return fmt.Errorf("module %q not found", name)
	}
	r.enabled[name] = false
	return nil
}

// IsEnabled checks if a module is enabled.
func (r *Registry) IsEnabled(name string) bool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return r.enabled[name]
}

// Get returns a module by name.
func (r *Registry) Get(name string) (ModuleBase, bool) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	m, ok := r.modules[name]
	return m, ok
}

// GetPlugin returns a module as a PluginModule if it implements the interface.
func (r *Registry) GetPlugin(name string) (PluginModule, bool) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	m, ok := r.modules[name]
	if !ok {
		return nil, false
	}
	pm, ok := m.(PluginModule)
	return pm, ok
}

// GetLegacy returns a module as a LegacyModule if it implements the interface.
func (r *Registry) GetLegacy(name string) (LegacyModule, bool) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	m, ok := r.modules[name]
	if !ok {
		return nil, false
	}
	lm, ok := m.(LegacyModule)
	return lm, ok
}

// IsPlugin checks if a module implements the PluginModule interface.
func (r *Registry) IsPlugin(name string) bool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	m, ok := r.modules[name]
	if !ok {
		return false
	}
	_, isPlugin := m.(PluginModule)
	return isPlugin
}

// EnabledModules returns all enabled modules in registration order.
func (r *Registry) EnabledModules() []ModuleBase {
	r.mu.RLock()
	defer r.mu.RUnlock()

	var result []ModuleBase
	for _, name := range r.order {
		if r.enabled[name] {
			result = append(result, r.modules[name])
		}
	}
	return result
}

// AllModules returns all modules in registration order.
func (r *Registry) AllModules() []ModuleBase {
	r.mu.RLock()
	defer r.mu.RUnlock()

	result := make([]ModuleBase, 0, len(r.order))
	for _, name := range r.order {
		result = append(result, r.modules[name])
	}
	return result
}

// InitAll initializes all enabled modules.
func (r *Registry) InitAll(ctx context.Context, deps Dependencies) error {
	for _, m := range r.EnabledModules() {
		r.logger.WithField("module", m.Name()).Info("Initializing module")
		if err := m.Init(ctx, deps); err != nil {
			return fmt.Errorf("init module %q: %w", m.Name(), err)
		}
	}
	return nil
}

// StartAll starts all enabled modules.
func (r *Registry) StartAll(ctx context.Context) error {
	r.mu.Lock()
	defer r.mu.Unlock()

	for _, name := range r.order {
		if !r.enabled[name] {
			continue
		}
		m := r.modules[name]
		r.logger.WithField("module", name).Info("Starting module")
		if err := m.Start(ctx); err != nil {
			return fmt.Errorf("start module %q: %w", name, err)
		}
		r.started[name] = true
	}
	return nil
}

// StopAll stops all started modules in reverse order.
func (r *Registry) StopAll(ctx context.Context) {
	r.mu.Lock()
	defer r.mu.Unlock()

	for i := len(r.order) - 1; i >= 0; i-- {
		name := r.order[i]
		if !r.started[name] {
			continue
		}
		m := r.modules[name]
		r.logger.WithField("module", name).Info("Stopping module")
		if err := m.Stop(ctx); err != nil {
			r.logger.WithError(err).WithField("module", name).Error("Failed to stop module")
		}
		r.started[name] = false
	}
}
