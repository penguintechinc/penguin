package nest

import (
	"context"
	"fmt"
	"strings"

	"fyne.io/fyne/v2"
	"fyne.io/fyne/v2/container"
	"fyne.io/fyne/v2/widget"
)

// NewGUIPanel creates the Nest module GUI.
func NewGUIPanel(client *Client) fyne.CanvasObject {
	if client == nil {
		return widget.NewCard("Nest", "Resource Management",
			widget.NewLabel("Not configured. Set nest.api_url in config."))
	}

	resourceList := widget.NewLabel("Loading resources...")
	teamSelect := widget.NewSelect([]string{}, nil)

	refreshBtn := widget.NewButton("Refresh", func() {
		go func() {
			resources, err := client.ListResources(context.Background(), nil)
			if err != nil {
				resourceList.SetText("Error: " + err.Error())
				return
			}
			var lines []string
			for _, r := range resources {
				lines = append(lines, fmt.Sprintf("[%d] %s — %s (%s)", r.ID, r.Name, r.Status, r.LifecycleMode))
			}
			if len(lines) == 0 {
				resourceList.SetText("No resources found")
			} else {
				resourceList.SetText(strings.Join(lines, "\n"))
			}
		}()
	})

	// Load teams
	go func() {
		teams, err := client.ListTeams(context.Background())
		if err != nil {
			return
		}
		var names []string
		for _, t := range teams {
			names = append(names, fmt.Sprintf("%d: %s", t.ID, t.Name))
		}
		teamSelect.Options = names
		teamSelect.Refresh()
	}()

	teamCard := widget.NewCard("Teams", "Select team scope",
		teamSelect)

	resourceCard := widget.NewCard("Resources", "Managed resources",
		container.NewVBox(resourceList, refreshBtn))

	return container.NewVBox(teamCard, resourceCard)
}
