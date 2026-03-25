package tray

import (
	"context"
	"fmt"

	"github.com/getlantern/systray"
	"github.com/penguintechinc/penguin/services/desktop/internal/app"
	"github.com/penguintechinc/penguin/services/desktop/internal/module"
)

// Manager manages the system tray icon and menu.
type Manager struct {
	app *app.App
}

// NewManager creates a tray manager.
func NewManager(application *app.App) *Manager {
	return &Manager{app: application}
}

// Run starts the system tray (blocking).
func (m *Manager) Run() {
	systray.Run(m.onReady, m.onExit)
}

func (m *Manager) onReady() {
	systray.SetTitle("PenguinTech")
	systray.SetTooltip("PenguinTech Client")

	// Module status items
	for _, mod := range m.app.Registry.EnabledModules() {
		item := systray.AddMenuItem(mod.DisplayName(), mod.Description())
		item.Disable() // Status display only
	}

	systray.AddSeparator()

	// Status
	statusItem := systray.AddMenuItem("Status: Ready", "Overall status")
	statusItem.Disable()

	systray.AddSeparator()

	// Actions
	mQuit := systray.AddMenuItem("Quit", "Quit PenguinTech Client")

	go func() {
		<-mQuit.ClickedCh
		m.app.Stop(context.Background())
		systray.Quit()
	}()

	// Update status periodically
	go m.updateStatus(statusItem)
}

func (m *Manager) updateStatus(statusItem *systray.MenuItem) {
	statuses := m.app.Health.GetAllStatuses()
	healthy := 0
	for _, s := range statuses {
		if s.State == module.HealthHealthy {
			healthy++
		}
	}
	total := len(statuses)
	statusItem.SetTitle(fmt.Sprintf("Status: %d/%d healthy", healthy, total))
}

func (m *Manager) onExit() {
	m.app.Logger.Info("System tray exiting")
}
