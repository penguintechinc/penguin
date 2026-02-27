package gui

import (
	"context"

	"fyne.io/fyne/v2"
	fyneapp "fyne.io/fyne/v2/app"
	"github.com/penguintechinc/penguin/services/desktop/internal/app"
)

// Run starts the Fyne GUI application.
func Run(application *app.App) error {
	fyneApp := fyneapp.NewWithID("io.penguintech.penguin")
	fyneApp.Settings().SetTheme(NewPenguinTheme())

	win := fyneApp.NewWindow("PenguinTech Client")
	win.Resize(fyne.NewSize(900, 600))

	layout := NewLayout(application, win)
	win.SetContent(layout.Build())

	// Start modules in background
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	go func() {
		if err := application.Start(ctx); err != nil {
			application.Logger.WithError(err).Error("Failed to start modules")
		}
	}()

	win.SetCloseIntercept(func() {
		cancel()
		application.Stop(context.Background())
		fyneApp.Quit()
	})

	win.ShowAndRun()
	return nil
}
