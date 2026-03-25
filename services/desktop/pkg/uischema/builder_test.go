package uischema

import (
	"testing"

	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
)

func TestLabel(t *testing.T) {
	w := Label("lbl-1", "Hello")
	if w.Type != modulepb.WidgetLabel {
		t.Errorf("expected WidgetLabel, got %d", w.Type)
	}
	if w.ID != "lbl-1" {
		t.Errorf("expected id lbl-1, got %s", w.ID)
	}
	if w.Text != "Hello" {
		t.Errorf("expected text Hello, got %s", w.Text)
	}
}

func TestButton(t *testing.T) {
	w := Button("btn-1", "Click Me")
	if w.Type != modulepb.WidgetButton {
		t.Errorf("expected WidgetButton, got %d", w.Type)
	}
	if w.Disabled {
		t.Error("button should not be disabled by default")
	}
}

func TestDisabledButton(t *testing.T) {
	w := DisabledButton("btn-dis", "Disabled")
	if !w.Disabled {
		t.Error("expected button to be disabled")
	}
}

func TestEntry(t *testing.T) {
	w := Entry("entry-1", "placeholder")
	if w.Type != modulepb.WidgetEntry {
		t.Errorf("expected WidgetEntry, got %d", w.Type)
	}
	if w.Text != "placeholder" {
		t.Errorf("expected placeholder text, got %s", w.Text)
	}
}

func TestEntryWithValue(t *testing.T) {
	w := EntryWithValue("entry-v", "ph", "val")
	if w.Value != "val" {
		t.Errorf("expected value val, got %s", w.Value)
	}
}

func TestSelect(t *testing.T) {
	w := Select("sel-1", []string{"a", "b", "c"}, "b")
	if w.Type != modulepb.WidgetSelect {
		t.Errorf("expected WidgetSelect, got %d", w.Type)
	}
	if len(w.Options) != 3 {
		t.Errorf("expected 3 options, got %d", len(w.Options))
	}
	if w.Value != "b" {
		t.Errorf("expected selected b, got %s", w.Value)
	}
}

func TestCard(t *testing.T) {
	w := Card("card-1", "Title", "Sub",
		Label("l1", "child1"),
		Label("l2", "child2"),
	)
	if w.Type != modulepb.WidgetCard {
		t.Errorf("expected WidgetCard, got %d", w.Type)
	}
	if w.Text != "Title" {
		t.Errorf("expected title Title, got %s", w.Text)
	}
	if w.Properties["subtitle"] != "Sub" {
		t.Errorf("expected subtitle Sub, got %s", w.Properties["subtitle"])
	}
	if len(w.Children) != 2 {
		t.Errorf("expected 2 children, got %d", len(w.Children))
	}
}

func TestVBox(t *testing.T) {
	w := VBox(Label("a", "A"), Label("b", "B"))
	if w.Type != modulepb.WidgetVBox {
		t.Errorf("expected WidgetVBox, got %d", w.Type)
	}
	if len(w.Children) != 2 {
		t.Errorf("expected 2 children, got %d", len(w.Children))
	}
}

func TestHBox(t *testing.T) {
	w := HBox(Button("a", "A"), Button("b", "B"))
	if w.Type != modulepb.WidgetHBox {
		t.Errorf("expected WidgetHBox, got %d", w.Type)
	}
}

func TestSeparator(t *testing.T) {
	w := Separator()
	if w.Type != modulepb.WidgetSeparator {
		t.Errorf("expected WidgetSeparator, got %d", w.Type)
	}
}

func TestScroll(t *testing.T) {
	child := Label("l", "content")
	w := Scroll(child)
	if w.Type != modulepb.WidgetScroll {
		t.Errorf("expected WidgetScroll, got %d", w.Type)
	}
	if len(w.Children) != 1 {
		t.Errorf("expected 1 child, got %d", len(w.Children))
	}
}

func TestCheckbox(t *testing.T) {
	w := Checkbox("cb-1", "Accept", true)
	if w.Type != modulepb.WidgetCheckbox {
		t.Errorf("expected WidgetCheckbox, got %d", w.Type)
	}
	if w.Value != "true" {
		t.Errorf("expected value true, got %s", w.Value)
	}

	w2 := Checkbox("cb-2", "Decline", false)
	if w2.Value != "false" {
		t.Errorf("expected value false, got %s", w2.Value)
	}
}

func TestRichText(t *testing.T) {
	w := RichText("rt-1", "**bold**")
	if w.Type != modulepb.WidgetRichText {
		t.Errorf("expected WidgetRichText, got %d", w.Type)
	}
	if w.Text != "**bold**" {
		t.Errorf("expected markdown, got %s", w.Text)
	}
}

func TestPanel(t *testing.T) {
	root := VBox(Label("l", "test"))
	p := Panel(root)
	if p.Root == nil {
		t.Error("expected non-nil root")
	}
	if p.Root.Type != modulepb.WidgetVBox {
		t.Errorf("expected VBox root, got %d", p.Root.Type)
	}
}

func TestNestedWidgetTree(t *testing.T) {
	tree := VBox(
		Card("c1", "Card", "",
			Label("l1", "label"),
			HBox(
				Button("b1", "OK"),
				Button("b2", "Cancel"),
			),
		),
		Separator(),
		Scroll(Label("big", "lots of text")),
	)

	if len(tree.Children) != 3 {
		t.Fatalf("expected 3 top-level children, got %d", len(tree.Children))
	}

	card := tree.Children[0]
	if card.Type != modulepb.WidgetCard {
		t.Errorf("expected card, got %d", card.Type)
	}
	if len(card.Children) != 2 {
		t.Errorf("expected 2 card children, got %d", len(card.Children))
	}

	hbox := card.Children[1]
	if hbox.Type != modulepb.WidgetHBox {
		t.Errorf("expected hbox, got %d", hbox.Type)
	}
	if len(hbox.Children) != 2 {
		t.Errorf("expected 2 buttons in hbox, got %d", len(hbox.Children))
	}
}
