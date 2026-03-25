//go:build nogui

package gui

import "github.com/penguintechinc/penguin/services/desktop/internal/app"

// Run is a stub for nogui builds.
func Run(_ *app.App) error {
	return nil
}
