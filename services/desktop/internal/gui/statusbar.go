package gui

import (
	"context"
	"fmt"

	"fyne.io/fyne/v2"
	"fyne.io/fyne/v2/container"
	"fyne.io/fyne/v2/widget"
	"github.com/penguintechinc/penguin/services/desktop/internal/app"
	"github.com/penguintechinc/penguin/services/desktop/internal/module"
)

// StatusBar shows overall status at the bottom of the window.
type StatusBar struct {
	app *app.App
}

// NewStatusBar creates a status bar.
func NewStatusBar(application *app.App) *StatusBar {
	return &StatusBar{app: application}
}

// Build creates the status bar widget.
func (sb *StatusBar) Build() fyne.CanvasObject {
	connStatus := widget.NewLabel("Status: Ready")
	licenseStatus := widget.NewLabel("License: Checking...")
	versionLabel := widget.NewLabel(fmt.Sprintf("v%s", sb.app.Version))

	// Check license in background
	go func() {
		valid := sb.app.License.IsValid(context.Background())
		if valid {
			licenseStatus.SetText("License: Valid")
		} else {
			licenseStatus.SetText("License: Invalid")
		}
	}()

	// Update connection status based on module health
	go func() {
		statuses := sb.app.Health.GetAllStatuses()
		healthy := 0
		total := 0
		for _, s := range statuses {
			total++
			if s.State == module.HealthHealthy {
				healthy++
			}
		}
		if total > 0 {
			connStatus.SetText(fmt.Sprintf("Status: %d/%d modules healthy", healthy, total))
		}
	}()

	return container.NewHBox(connStatus, widget.NewSeparator(), licenseStatus, widget.NewSeparator(), versionLabel)
}
