package gui

import (
	"fyne.io/fyne/v2"
	"fyne.io/fyne/v2/container"
	"fyne.io/fyne/v2/widget"
	"github.com/penguintechinc/penguin/services/desktop/internal/app"
)

// Sidebar displays the module navigation list.
type Sidebar struct {
	app      *app.App
	onSelect func(string)
}

// NewSidebar creates a sidebar.
func NewSidebar(application *app.App, onSelect func(string)) *Sidebar {
	return &Sidebar{
		app:      application,
		onSelect: onSelect,
	}
}

// Build constructs the sidebar widget.
func (s *Sidebar) Build() fyne.CanvasObject {
	logo := widget.NewLabelWithStyle("PenguinTech", fyne.TextAlignCenter, fyne.TextStyle{Bold: true})

	var items []fyne.CanvasObject
	items = append(items, logo, widget.NewSeparator())

	for _, m := range s.app.Registry.EnabledModules() {
		name := m.Name()
		btn := widget.NewButton(m.DisplayName(), func() {
			s.onSelect(name)
		})
		items = append(items, btn)
	}

	items = append(items, widget.NewSeparator())
	items = append(items, widget.NewButton("Settings", func() {
		s.onSelect("settings")
	}))

	return container.NewVBox(items...)
}
