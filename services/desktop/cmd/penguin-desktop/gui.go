//go:build !nogui

package main

import (
	"github.com/penguintechinc/penguin/services/desktop/internal/app"
	"github.com/penguintechinc/penguin/services/desktop/internal/gui"
)

func runGUI(application *app.App) error {
	return gui.Run(application)
}
