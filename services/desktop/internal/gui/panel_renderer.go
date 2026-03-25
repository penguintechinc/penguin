package gui

import (
	"context"

	"fyne.io/fyne/v2"
	"fyne.io/fyne/v2/widget"
	"github.com/penguintechinc/penguin/services/desktop/internal/module"
	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
	"github.com/penguintechinc/penguin/services/desktop/pkg/uischema"
)

// RenderPluginPanel renders a PluginModule's declarative GUI panel
// into a Fyne CanvasObject by fetching the widget tree via GetGUIPanel
// and converting it using the uischema renderer.
func RenderPluginPanel(pm module.PluginModule) fyne.CanvasObject {
	panel, err := pm.GetGUIPanel(context.Background())
	if err != nil {
		return widget.NewLabel("Error loading panel: " + err.Error())
	}
	if panel == nil || panel.Root == nil {
		return widget.NewLabel(pm.DisplayName() + " - No GUI panel available")
	}

	bridge := NewEventBridge(pm)
	return uischema.Render(panel.Root, bridge.Handle)
}

// EventBridge routes Fyne widget events to a PluginModule's HandleGUIEvent RPC.
// When the module returns an updated panel, the bridge triggers a re-render.
type EventBridge struct {
	module    module.PluginModule
	rerender  func()
	container *fyne.Container
}

// NewEventBridge creates an EventBridge for a plugin module.
func NewEventBridge(pm module.PluginModule) *EventBridge {
	return &EventBridge{module: pm}
}

// SetContainer sets the container that will be refreshed on re-render.
func (b *EventBridge) SetContainer(c *fyne.Container) {
	b.container = c
}

// Handle processes a GUI event by forwarding it to the plugin module
// and re-rendering the panel if the module returns an updated widget tree.
func (b *EventBridge) Handle(event *modulepb.GUIEvent) {
	go func() {
		updatedPanel, err := b.module.HandleGUIEvent(context.Background(), event)
		if err != nil {
			return
		}
		if updatedPanel == nil || updatedPanel.Root == nil {
			return
		}
		if b.container != nil {
			newContent := uischema.Render(updatedPanel.Root, b.Handle)
			b.container.Objects = []fyne.CanvasObject{newContent}
			b.container.Refresh()
		}
	}()
}
