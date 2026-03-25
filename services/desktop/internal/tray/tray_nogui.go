//go:build nogui

package tray

import "github.com/penguintechinc/penguin/services/desktop/internal/app"

// Manager is a stub tray manager for nogui builds.
type Manager struct{}

// NewManager creates a stub tray manager.
func NewManager(_ *app.App) *Manager {
	return &Manager{}
}

// Run is a no-op for nogui builds.
func (m *Manager) Run() {}
