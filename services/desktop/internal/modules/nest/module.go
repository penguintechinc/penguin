package nest

import (
	"context"
	"fmt"
	"sync"

	"github.com/penguintechinc/penguin/services/desktop/internal/module"
	"github.com/penguintechinc/penguin/services/desktop/pkg/clischema"
	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
	"github.com/penguintechinc/penguin/services/desktop/pkg/uischema"
	"github.com/sirupsen/logrus"
)

// Module implements the Nest module for the desktop client.
type Module struct {
	client  *Client
	logger  *logrus.Logger
	mu      sync.RWMutex
	started bool
}

func New() *Module              { return &Module{} }
func (m *Module) Name() string        { return "nest" }
func (m *Module) DisplayName() string { return "Nest" }
func (m *Module) Description() string { return "Resource management via Nest API" }
func (m *Module) Version() string     { return "0.1.0" }

func (m *Module) Init(ctx context.Context, deps module.Dependencies) error {
	m.logger = deps.Logger.(*logrus.Logger)
	m.logger.WithField("module", m.Name()).Debug("Initializing Nest module")
	baseURL := "http://localhost:8080"
	m.client = NewClient(baseURL, deps.AuthToken, m.logger)
	return nil
}

func (m *Module) Start(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.started = true
	m.logger.WithField("module", m.Name()).Info("Nest module started")
	return nil
}

func (m *Module) Stop(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.started = false
	m.logger.WithField("module", m.Name()).Info("Nest module stopped")
	return nil
}

func (m *Module) HealthCheck(ctx context.Context) module.HealthStatus {
	m.mu.RLock()
	defer m.mu.RUnlock()
	if !m.started {
		return module.HealthStatus{State: module.HealthUnknown, Message: "module not started"}
	}
	if m.client != nil {
		return module.HealthStatus{State: module.HealthHealthy, Message: "connected to Nest API"}
	}
	return module.HealthStatus{State: module.HealthDegraded, Message: "Nest client not configured"}
}

// --- PluginModule interface ---

func (m *Module) GetGUIPanel(ctx context.Context) (*modulepb.GUIPanel, error) {
	return uischema.Panel(
		uischema.Scroll(
			uischema.VBox(
				uischema.Card("nest-info", "Nest Resource Manager", "API-based resource management",
					uischema.Label("nest-version", "Nest Module v0.1.0"),
					uischema.Label("nest-desc", "Manages resources, teams, and configurations via REST API"),
				),
				uischema.HBox(uischema.Button("nest-refresh", "Refresh")),
				uischema.Card("nest-resources", "Resources", "Managed resources list",
					uischema.Label("nest-res-list", "Click Refresh to load resources"),
				),
				uischema.Card("nest-teams", "Teams", "Available teams",
					uischema.Label("nest-team-list", "Click Refresh to load teams"),
				),
			),
		),
	), nil
}

func (m *Module) HandleGUIEvent(ctx context.Context, event *modulepb.GUIEvent) (*modulepb.GUIPanel, error) {
	if event.WidgetID != "nest-refresh" || m.client == nil {
		return m.GetGUIPanel(ctx)
	}

	// Fetch resources
	resText := "Resources: Error loading"
	resources, err := m.client.ListResources(ctx, nil)
	if err == nil {
		if len(resources) == 0 {
			resText = "Resources: None available"
		} else {
			resText = fmt.Sprintf("Resources (%d):\n", len(resources))
			for _, r := range resources {
				resText += fmt.Sprintf("  - %s (ID: %d, Status: %s)\n", r.Name, r.ID, r.Status)
			}
		}
	}

	// Fetch teams
	teamsText := "Teams: Error loading"
	teams, err := m.client.ListTeams(ctx)
	if err == nil {
		if len(teams) == 0 {
			teamsText = "Teams: None available"
		} else {
			teamsText = fmt.Sprintf("Teams (%d):\n", len(teams))
			for _, t := range teams {
				teamsText += fmt.Sprintf("  - %s (ID: %d)\n", t.Name, t.ID)
			}
		}
	}

	return uischema.Panel(
		uischema.Scroll(
			uischema.VBox(
				uischema.Card("nest-info", "Nest Resource Manager", "API-based resource management",
					uischema.Label("nest-version", "Nest Module v0.1.0"),
					uischema.Label("nest-desc", "Manages resources, teams, and configurations via REST API"),
				),
				uischema.HBox(uischema.Button("nest-refresh", "Refresh")),
				uischema.Card("nest-resources", "Resources", "Managed resources list",
					uischema.Label("nest-res-list", resText),
				),
				uischema.Card("nest-teams", "Teams", "Available teams",
					uischema.Label("nest-team-list", teamsText),
				),
			),
		),
	), nil
}

func (m *Module) GetCLICommands(ctx context.Context) (*modulepb.CLICommandList, error) {
	nestCmd := clischema.Command("nest", "Nest resource management")
	clischema.WithSubcommands(nestCmd,
		*clischema.Command("list", "List all resources"),
		*clischema.Command("get [id]", "Get resource details"),
		*clischema.Command("stats [id]", "Get resource statistics"),
		*clischema.Command("connection [id]", "Get resource connection info"),
		*clischema.Command("teams", "List all teams"),
		*clischema.Command("team [id]", "Get team details"),
		*clischema.Command("health", "Check Nest module health"),
	)
	return clischema.CommandList(*nestCmd), nil
}

func (m *Module) ExecuteCLICommand(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
	if m.client == nil {
		return &modulepb.CLICommandResponse{Stderr: "nest client not configured\n", ExitCode: 1}, nil
	}

	switch req.CommandPath {
	case "nest list":
		resources, err := m.client.ListResources(ctx, nil)
		if err != nil {
			return &modulepb.CLICommandResponse{Stderr: fmt.Sprintf("failed to list resources: %v\n", err), ExitCode: 1}, nil
		}
		if len(resources) == 0 {
			return &modulepb.CLICommandResponse{Stdout: "No resources available\n"}, nil
		}
		out := fmt.Sprintf("%-6s %-20s %-15s %-10s %s\n", "ID", "Name", "Status", "Mode", "Team")
		for _, r := range resources {
			out += fmt.Sprintf("%-6d %-20s %-15s %-10s %d\n", r.ID, r.Name, r.Status, r.LifecycleMode, r.TeamID)
		}
		return &modulepb.CLICommandResponse{Stdout: out}, nil

	case "nest get":
		if len(req.Args) == 0 {
			return &modulepb.CLICommandResponse{Stderr: "resource id required\n", ExitCode: 1}, nil
		}
		r, err := m.client.GetResource(ctx, req.Args[0])
		if err != nil {
			return &modulepb.CLICommandResponse{Stderr: fmt.Sprintf("failed to get resource: %v\n", err), ExitCode: 1}, nil
		}
		out := fmt.Sprintf("ID: %d\nName: %s\nStatus: %s\nLifecycle Mode: %s\nTeam ID: %d\n",
			r.ID, r.Name, r.Status, r.LifecycleMode, r.TeamID)
		return &modulepb.CLICommandResponse{Stdout: out}, nil

	case "nest stats":
		if len(req.Args) == 0 {
			return &modulepb.CLICommandResponse{Stderr: "resource id required\n", ExitCode: 1}, nil
		}
		stats, err := m.client.GetResourceStats(ctx, req.Args[0])
		if err != nil {
			return &modulepb.CLICommandResponse{Stderr: fmt.Sprintf("failed to get stats: %v\n", err), ExitCode: 1}, nil
		}
		out := fmt.Sprintf("Resource ID: %d\nRisk Level: %s\nTimestamp: %s\n",
			stats.ResourceID, stats.RiskLevel, stats.Timestamp.Format("2006-01-02 15:04:05 MST"))
		return &modulepb.CLICommandResponse{Stdout: out}, nil

	case "nest connection":
		if len(req.Args) == 0 {
			return &modulepb.CLICommandResponse{Stderr: "resource id required\n", ExitCode: 1}, nil
		}
		info, err := m.client.GetConnectionInfo(ctx, req.Args[0])
		if err != nil {
			return &modulepb.CLICommandResponse{Stderr: fmt.Sprintf("failed to get connection info: %v\n", err), ExitCode: 1}, nil
		}
		out := fmt.Sprintf("TLS Enabled: %v\nAccess Level: %s\nConnection Info: %s\n",
			info.TLSEnabled, info.AccessLevel, string(info.ConnectionInfo))
		return &modulepb.CLICommandResponse{Stdout: out}, nil

	case "nest teams":
		teams, err := m.client.ListTeams(ctx)
		if err != nil {
			return &modulepb.CLICommandResponse{Stderr: fmt.Sprintf("failed to list teams: %v\n", err), ExitCode: 1}, nil
		}
		if len(teams) == 0 {
			return &modulepb.CLICommandResponse{Stdout: "No teams available\n"}, nil
		}
		out := fmt.Sprintf("%-6s %-20s %-40s %s\n", "ID", "Name", "Description", "Global")
		for _, t := range teams {
			global := "No"
			if t.IsGlobal {
				global = "Yes"
			}
			out += fmt.Sprintf("%-6d %-20s %-40s %s\n", t.ID, t.Name, t.Description, global)
		}
		return &modulepb.CLICommandResponse{Stdout: out}, nil

	case "nest team":
		if len(req.Args) == 0 {
			return &modulepb.CLICommandResponse{Stderr: "team id required\n", ExitCode: 1}, nil
		}
		t, err := m.client.GetTeam(ctx, req.Args[0])
		if err != nil {
			return &modulepb.CLICommandResponse{Stderr: fmt.Sprintf("failed to get team: %v\n", err), ExitCode: 1}, nil
		}
		out := fmt.Sprintf("ID: %d\nName: %s\nDescription: %s\nIs Global: %v\n",
			t.ID, t.Name, t.Description, t.IsGlobal)
		return &modulepb.CLICommandResponse{Stdout: out}, nil

	case "nest health":
		health := m.HealthCheck(ctx)
		out := fmt.Sprintf("Status: %s\nMessage: %s\n", health.State, health.Message)
		for k, v := range health.Details {
			out += fmt.Sprintf("%s: %s\n", k, v)
		}
		return &modulepb.CLICommandResponse{Stdout: out}, nil

	default:
		return &modulepb.CLICommandResponse{Stderr: fmt.Sprintf("unknown command: %s\n", req.CommandPath), ExitCode: 1}, nil
	}
}

func (m *Module) GetIcon(ctx context.Context) (*modulepb.IconResponse, error) {
	return &modulepb.IconResponse{}, nil
}
