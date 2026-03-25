package pluginhost

import (
	"context"
	"fmt"
	"os/exec"
	"sync"

	"github.com/hashicorp/go-plugin"
	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
	pluginpkg "github.com/penguintechinc/penguin/services/desktop/pkg/plugin"
	"github.com/sirupsen/logrus"
)

// ManagedPlugin represents a running plugin process.
type ManagedPlugin struct {
	Name       string
	Path       string
	Client     *plugin.Client
	Service    modulepb.ModuleService
	Info       *modulepb.ModuleInfo
	RestartNum int
}

// Manager handles the lifecycle of plugin module processes.
type Manager struct {
	mu      sync.RWMutex
	plugins map[string]*ManagedPlugin
	logger  *logrus.Logger
}

// NewManager creates a new plugin Manager.
func NewManager(logger *logrus.Logger) *Manager {
	return &Manager{
		plugins: make(map[string]*ManagedPlugin),
		logger:  logger,
	}
}

// Launch starts a plugin binary and establishes the RPC connection.
func (m *Manager) Launch(name, path string) (*ManagedPlugin, error) {
	m.mu.Lock()
	defer m.mu.Unlock()

	if existing, ok := m.plugins[name]; ok {
		if !existing.Client.Exited() {
			return existing, nil
		}
		// Previous instance exited, clean up
		existing.Client.Kill()
	}

	m.logger.WithFields(logrus.Fields{
		"module": name,
		"path":   path,
	}).Info("Launching plugin")

	client := plugin.NewClient(&plugin.ClientConfig{
		HandshakeConfig: pluginpkg.Handshake,
		Plugins: map[string]plugin.Plugin{
			pluginpkg.PluginName: &pluginpkg.ModulePlugin{},
		},
		Cmd:              exec.Command(path),
		AllowedProtocols: []plugin.Protocol{plugin.ProtocolNetRPC},
		Logger:           newHCLogAdapter(m.logger, name),
	})

	rpcClient, err := client.Client()
	if err != nil {
		client.Kill()
		return nil, fmt.Errorf("connecting to plugin %s: %w", name, err)
	}

	raw, err := rpcClient.Dispense(pluginpkg.PluginName)
	if err != nil {
		client.Kill()
		return nil, fmt.Errorf("dispensing plugin %s: %w", name, err)
	}

	svc, ok := raw.(modulepb.ModuleService)
	if !ok {
		client.Kill()
		return nil, fmt.Errorf("plugin %s does not implement ModuleService", name)
	}

	// Fetch module info
	info, err := svc.GetInfo(context.Background())
	if err != nil {
		client.Kill()
		return nil, fmt.Errorf("getting info from plugin %s: %w", name, err)
	}

	mp := &ManagedPlugin{
		Name:    name,
		Path:    path,
		Client:  client,
		Service: svc,
		Info:    info,
	}
	m.plugins[name] = mp

	m.logger.WithFields(logrus.Fields{
		"module":  info.Name,
		"version": info.Version,
	}).Info("Plugin launched successfully")

	return mp, nil
}

// Get returns a managed plugin by name.
func (m *Manager) Get(name string) (*ManagedPlugin, bool) {
	m.mu.RLock()
	defer m.mu.RUnlock()
	p, ok := m.plugins[name]
	return p, ok
}

// All returns all managed plugins.
func (m *Manager) All() []*ManagedPlugin {
	m.mu.RLock()
	defer m.mu.RUnlock()
	result := make([]*ManagedPlugin, 0, len(m.plugins))
	for _, p := range m.plugins {
		result = append(result, p)
	}
	return result
}

// Stop kills a specific plugin process.
func (m *Manager) Stop(name string) {
	m.mu.Lock()
	defer m.mu.Unlock()

	if p, ok := m.plugins[name]; ok {
		m.logger.WithField("module", name).Info("Stopping plugin")
		p.Client.Kill()
		delete(m.plugins, name)
	}
}

// StopAll kills all plugin processes.
func (m *Manager) StopAll() {
	m.mu.Lock()
	defer m.mu.Unlock()

	for name, p := range m.plugins {
		m.logger.WithField("module", name).Info("Stopping plugin")
		p.Client.Kill()
	}
	m.plugins = make(map[string]*ManagedPlugin)
}

// IsRunning checks if a plugin process is still alive.
func (m *Manager) IsRunning(name string) bool {
	m.mu.RLock()
	defer m.mu.RUnlock()

	p, ok := m.plugins[name]
	if !ok {
		return false
	}
	return !p.Client.Exited()
}
