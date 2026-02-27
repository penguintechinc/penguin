package uischema

import (
	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
)

// EventBridge manages the mapping between widget IDs and their
// event callbacks, bridging Fyne interactions to GUIEvent protos.
type EventBridge struct {
	handler EventHandler
}

// NewEventBridge creates an EventBridge that forwards all events to the handler.
func NewEventBridge(handler EventHandler) *EventBridge {
	return &EventBridge{handler: handler}
}

// Dispatch sends an event through the bridge.
func (b *EventBridge) Dispatch(widgetID, eventType, value string) {
	if b.handler == nil {
		return
	}
	b.handler(&modulepb.GUIEvent{
		WidgetID:  widgetID,
		EventType: eventType,
		Value:     value,
	})
}

// TappedEvent creates a tapped event for convenience.
func TappedEvent(widgetID string) *modulepb.GUIEvent {
	return &modulepb.GUIEvent{
		WidgetID:  widgetID,
		EventType: "tapped",
	}
}

// ChangedEvent creates a changed event for convenience.
func ChangedEvent(widgetID, value string) *modulepb.GUIEvent {
	return &modulepb.GUIEvent{
		WidgetID:  widgetID,
		EventType: "changed",
		Value:     value,
	}
}

// SubmittedEvent creates a submitted event for convenience.
func SubmittedEvent(widgetID, value string) *modulepb.GUIEvent {
	return &modulepb.GUIEvent{
		WidgetID:  widgetID,
		EventType: "submitted",
		Value:     value,
	}
}
