//go:build nogui

package main

import (
	"fmt"

	"github.com/penguintechinc/penguin/services/desktop/internal/app"
)

func runGUI(application *app.App) error {
	fmt.Println("GUI not available in this build. Use penguin-cli instead.")
	return application.Run(nil)
}
