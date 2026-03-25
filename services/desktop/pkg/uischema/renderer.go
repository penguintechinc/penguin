package uischema

import (
	"fyne.io/fyne/v2"
	"fyne.io/fyne/v2/container"
	"fyne.io/fyne/v2/widget"
	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
)

// EventHandler is called when a user interacts with a rendered widget.
type EventHandler func(event *modulepb.GUIEvent)

// Render converts a Widget proto tree into a Fyne CanvasObject tree.
// It installs event callbacks that route interactions through the handler.
func Render(w *modulepb.Widget, handler EventHandler) fyne.CanvasObject {
	if w == nil {
		return widget.NewLabel("")
	}
	return renderWidget(w, handler)
}

func renderWidget(w *modulepb.Widget, handler EventHandler) fyne.CanvasObject {
	switch w.Type {
	case modulepb.WidgetLabel:
		return widget.NewLabel(w.Text)

	case modulepb.WidgetButton:
		btn := widget.NewButton(w.Text, func() {
			if handler != nil {
				handler(&modulepb.GUIEvent{
					WidgetID:  w.ID,
					EventType: "tapped",
				})
			}
		})
		if w.Disabled {
			btn.Disable()
		}
		return btn

	case modulepb.WidgetEntry:
		entry := widget.NewEntry()
		entry.SetPlaceHolder(w.Text)
		if w.Value != "" {
			entry.SetText(w.Value)
		}
		if handler != nil {
			entry.OnChanged = func(val string) {
				handler(&modulepb.GUIEvent{
					WidgetID:  w.ID,
					EventType: "changed",
					Value:     val,
				})
			}
			entry.OnSubmitted = func(val string) {
				handler(&modulepb.GUIEvent{
					WidgetID:  w.ID,
					EventType: "submitted",
					Value:     val,
				})
			}
		}
		if w.Disabled {
			entry.Disable()
		}
		return entry

	case modulepb.WidgetSelect:
		sel := widget.NewSelect(w.Options, func(val string) {
			if handler != nil {
				handler(&modulepb.GUIEvent{
					WidgetID:  w.ID,
					EventType: "changed",
					Value:     val,
				})
			}
		})
		if w.Value != "" {
			sel.SetSelected(w.Value)
		}
		if w.Disabled {
			sel.Disable()
		}
		return sel

	case modulepb.WidgetCard:
		subtitle := w.Properties["subtitle"]
		var content fyne.CanvasObject
		if len(w.Children) > 0 {
			content = renderChildren(w.Children, handler)
		}
		return widget.NewCard(w.Text, subtitle, content)

	case modulepb.WidgetVBox:
		return renderChildren(w.Children, handler)

	case modulepb.WidgetHBox:
		objects := renderChildrenSlice(w.Children, handler)
		return container.NewHBox(objects...)

	case modulepb.WidgetSeparator:
		return widget.NewSeparator()

	case modulepb.WidgetScroll:
		if len(w.Children) > 0 {
			child := renderWidget(w.Children[0], handler)
			return container.NewScroll(child)
		}
		return container.NewScroll(widget.NewLabel(""))

	case modulepb.WidgetCheckbox:
		checked := w.Value == "true"
		cb := widget.NewCheck(w.Text, func(val bool) {
			v := "false"
			if val {
				v = "true"
			}
			if handler != nil {
				handler(&modulepb.GUIEvent{
					WidgetID:  w.ID,
					EventType: "changed",
					Value:     v,
				})
			}
		})
		cb.Checked = checked
		if w.Disabled {
			cb.Disable()
		}
		return cb

	case modulepb.WidgetRichText:
		return widget.NewRichTextFromMarkdown(w.Text)

	default:
		return widget.NewLabel(w.Text)
	}
}

func renderChildren(children []*modulepb.Widget, handler EventHandler) *fyne.Container {
	objects := renderChildrenSlice(children, handler)
	return container.NewVBox(objects...)
}

func renderChildrenSlice(children []*modulepb.Widget, handler EventHandler) []fyne.CanvasObject {
	objects := make([]fyne.CanvasObject, 0, len(children))
	for _, child := range children {
		objects = append(objects, renderWidget(child, handler))
	}
	return objects
}
