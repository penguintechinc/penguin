// Package uischema provides helpers for building declarative UI widget trees.
// Modules use these builders to describe their GUI without importing Fyne.
// The host process renders the widget tree into actual Fyne widgets.
package uischema

import (
	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
)

// Label creates a text label widget.
func Label(id, text string) *modulepb.Widget {
	return &modulepb.Widget{
		Type: modulepb.WidgetLabel,
		ID:   id,
		Text: text,
	}
}

// Button creates a clickable button widget.
func Button(id, text string) *modulepb.Widget {
	return &modulepb.Widget{
		Type: modulepb.WidgetButton,
		ID:   id,
		Text: text,
	}
}

// DisabledButton creates a disabled button widget.
func DisabledButton(id, text string) *modulepb.Widget {
	w := Button(id, text)
	w.Disabled = true
	return w
}

// Entry creates a text entry widget.
func Entry(id, placeholder string) *modulepb.Widget {
	return &modulepb.Widget{
		Type: modulepb.WidgetEntry,
		ID:   id,
		Text: placeholder,
	}
}

// EntryWithValue creates a text entry with a pre-filled value.
func EntryWithValue(id, placeholder, value string) *modulepb.Widget {
	return &modulepb.Widget{
		Type:  modulepb.WidgetEntry,
		ID:    id,
		Text:  placeholder,
		Value: value,
	}
}

// Select creates a dropdown select widget.
func Select(id string, options []string, selected string) *modulepb.Widget {
	return &modulepb.Widget{
		Type:    modulepb.WidgetSelect,
		ID:      id,
		Options: options,
		Value:   selected,
	}
}

// Card creates a card container with title and subtitle.
func Card(id, title, subtitle string, children ...*modulepb.Widget) *modulepb.Widget {
	props := map[string]string{}
	if subtitle != "" {
		props["subtitle"] = subtitle
	}
	return &modulepb.Widget{
		Type:       modulepb.WidgetCard,
		ID:         id,
		Text:       title,
		Children:   children,
		Properties: props,
	}
}

// VBox creates a vertical box container.
func VBox(children ...*modulepb.Widget) *modulepb.Widget {
	return &modulepb.Widget{
		Type:     modulepb.WidgetVBox,
		Children: children,
	}
}

// HBox creates a horizontal box container.
func HBox(children ...*modulepb.Widget) *modulepb.Widget {
	return &modulepb.Widget{
		Type:     modulepb.WidgetHBox,
		Children: children,
	}
}

// Separator creates a horizontal separator line.
func Separator() *modulepb.Widget {
	return &modulepb.Widget{
		Type: modulepb.WidgetSeparator,
	}
}

// Scroll wraps content in a scrollable container.
func Scroll(child *modulepb.Widget) *modulepb.Widget {
	return &modulepb.Widget{
		Type:     modulepb.WidgetScroll,
		Children: []*modulepb.Widget{child},
	}
}

// Checkbox creates a checkbox widget.
func Checkbox(id, label string, checked bool) *modulepb.Widget {
	value := "false"
	if checked {
		value = "true"
	}
	return &modulepb.Widget{
		Type:  modulepb.WidgetCheckbox,
		ID:    id,
		Text:  label,
		Value: value,
	}
}

// RichText creates a rich text widget (rendered as markdown).
func RichText(id, markdown string) *modulepb.Widget {
	return &modulepb.Widget{
		Type: modulepb.WidgetRichText,
		ID:   id,
		Text: markdown,
	}
}

// Panel is a convenience for creating a GUIPanel from a root widget.
func Panel(root *modulepb.Widget) *modulepb.GUIPanel {
	return &modulepb.GUIPanel{Root: root}
}
