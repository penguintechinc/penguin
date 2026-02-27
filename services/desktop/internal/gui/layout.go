package gui

import (
	"fyne.io/fyne/v2"
	"fyne.io/fyne/v2/container"
	"fyne.io/fyne/v2/widget"
	"github.com/penguintechinc/penguin/services/desktop/internal/app"
	"github.com/penguintechinc/penguin/services/desktop/internal/module"
)

// Layout manages the main window layout.
type Layout struct {
	app     *app.App
	window  fyne.Window
	content *fyne.Container
	sidebar *Sidebar
}

// NewLayout creates the main layout.
func NewLayout(application *app.App, window fyne.Window) *Layout {
	return &Layout{
		app:    application,
		window: window,
	}
}

// Build constructs the full layout.
func (l *Layout) Build() fyne.CanvasObject {
	l.content = container.NewStack()
	l.sidebar = NewSidebar(l.app, l.onModuleSelected)
	statusBar := NewStatusBar(l.app)

	// Show welcome by default
	l.content.Add(widget.NewLabel("Select a module from the sidebar"))

	split := container.NewHSplit(l.sidebar.Build(), l.content)
	split.SetOffset(0.2)

	return container.NewBorder(nil, statusBar.Build(), nil, nil, split)
}

func (l *Layout) onModuleSelected(name string) {
	m, ok := l.app.Registry.Get(name)
	if !ok {
		return
	}

	var panel fyne.CanvasObject

	// Check if the module is a PluginModule (declarative UI)
	if pm, ok := m.(module.PluginModule); ok {
		bridge := NewEventBridge(pm)
		panel = RenderPluginPanel(pm)
		// Wire up re-rendering by setting the container reference
		bridge.SetContainer(l.content)
	} else if lm, ok := m.(module.LegacyModule); ok {
		// Fall back to legacy direct-Fyne panel
		panel = lm.GUIPanel()
	}

	if panel == nil {
		panel = widget.NewLabel(m.DisplayName() + " - No GUI panel available")
	}

	l.content.Objects = []fyne.CanvasObject{panel}
	l.content.Refresh()
}
