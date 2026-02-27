package skauswatch

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

// Module implements the SkaUsWatch EDR client as a PluginModule.
type Module struct {
	client  *Client
	logger  *logrus.Logger
	mu      sync.RWMutex
	started bool
}

func New() *Module              { return &Module{} }
func (m *Module) Name() string        { return "skauswatch" }
func (m *Module) DisplayName() string { return "SkaUsWatch" }
func (m *Module) Description() string { return "Endpoint detection and response (EDR) client for SkaUsWatch" }
func (m *Module) Version() string     { return "0.1.0" }

func (m *Module) Init(ctx context.Context, deps module.Dependencies) error {
	m.logger = deps.Logger.(*logrus.Logger)
	return nil
}

func (m *Module) Start(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.started = true
	if m.client != nil {
		if err := m.client.RegisterEndpoint(ctx); err != nil {
			m.logger.WithError(err).Warn("skauswatch registration failed")
		}
		m.client.StartCheckin()
	}
	m.logger.Info("SkaUsWatch module started")
	return nil
}

func (m *Module) Stop(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.client != nil {
		m.client.StopCheckin()
	}
	m.started = false
	return nil
}

func (m *Module) HealthCheck(ctx context.Context) module.HealthStatus {
	m.mu.RLock()
	defer m.mu.RUnlock()
	if !m.started {
		return module.HealthStatus{State: module.HealthUnknown, Message: "not started"}
	}
	if m.client == nil {
		return module.HealthStatus{State: module.HealthDegraded, Message: "client not configured"}
	}
	if !m.client.IsRegistered() {
		return module.HealthStatus{State: module.HealthDegraded, Message: "endpoint not registered"}
	}
	return module.HealthStatus{State: module.HealthHealthy, Message: "registered and monitoring"}
}

// --- GUI ---

func (m *Module) GetGUIPanel(ctx context.Context) (*modulepb.GUIPanel, error) {
	if m.client == nil {
		return uischema.Panel(
			uischema.Card("sw-card", "SkaUsWatch", "Endpoint Security",
				uischema.Label("sw-nocfg", "Not configured. Set skauswatch.api_url in config."),
			),
		), nil
	}

	// Status section.
	statusText := "Not registered"
	if m.client.IsRegistered() {
		statusText = "Registered and monitoring"
	}

	// Alerts section.
	alertText := "Loading alerts..."
	alerts, err := m.client.GetAlerts(ctx, "active")
	if err != nil {
		alertText = "Error loading alerts: " + err.Error()
	} else if len(alerts) == 0 {
		alertText = "No active threats"
	} else {
		alertText = ""
		for _, a := range alerts {
			alertText += fmt.Sprintf("[%s] %s — %s\n", a.Severity, a.Type, a.Description)
		}
	}

	return uischema.Panel(
		uischema.VBox(
			uischema.Card("sw-status-card", "SkaUsWatch", "Endpoint Security",
				uischema.Label("sw-status", fmt.Sprintf("Status: %s", statusText)),
			),
			uischema.Card("sw-threats-card", "Active Threats", "",
				uischema.Label("sw-alerts", alertText),
			),
			uischema.Card("sw-scan-card", "Scan", "",
				uischema.HBox(
					uischema.Button("sw-scan-quick", "Quick Scan"),
					uischema.Button("sw-scan-full", "Full Scan"),
				),
			),
		),
	), nil
}

func (m *Module) HandleGUIEvent(ctx context.Context, event *modulepb.GUIEvent) (*modulepb.GUIPanel, error) {
	if m.client != nil {
		switch event.WidgetID {
		case "sw-scan-quick":
			go func() {
				if _, err := m.client.StartScan(ctx, "quick"); err != nil {
					m.logger.WithError(err).Warn("quick scan failed")
				}
			}()
		case "sw-scan-full":
			go func() {
				if _, err := m.client.StartScan(ctx, "full"); err != nil {
					m.logger.WithError(err).Warn("full scan failed")
				}
			}()
		}
	}
	return m.GetGUIPanel(ctx)
}

// --- CLI ---

func (m *Module) GetCLICommands(ctx context.Context) (*modulepb.CLICommandList, error) {
	swCmd := clischema.Command("skauswatch", "SkaUsWatch endpoint security management")

	alertsCmd := clischema.Command("alerts", "List threat alerts")
	clischema.WithFlags(alertsCmd,
		clischema.Flag("severity", "s", "Filter by severity (critical/high/medium/low/info)", ""),
	)

	scanCmd := clischema.Command("scan", "Start a security scan")
	clischema.WithFlags(scanCmd,
		clischema.Flag("type", "t", "Scan type (quick/full)", "quick"),
	)

	quarantineCmd := clischema.Command("quarantine", "Manage quarantined files")
	clischema.WithSubcommands(quarantineCmd,
		*clischema.Command("list", "List quarantined files"),
		*clischema.Command("restore [id]", "Restore a quarantined file"),
		*clischema.Command("delete [id]", "Permanently delete a quarantined file"),
	)

	clischema.WithSubcommands(swCmd,
		*clischema.Command("status", "Show endpoint security status"),
		*alertsCmd,
		*scanCmd,
		*quarantineCmd,
	)
	return clischema.CommandList(*swCmd), nil
}

func (m *Module) ExecuteCLICommand(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
	if m.client == nil {
		return &modulepb.CLICommandResponse{Stderr: "skauswatch client not configured\n", ExitCode: 1}, nil
	}

	switch req.CommandPath {
	case "skauswatch status":
		status, err := m.client.GetEndpointStatus(ctx)
		if err != nil {
			return &modulepb.CLICommandResponse{Stderr: err.Error() + "\n", ExitCode: 1}, nil
		}
		regStr := "no"
		if status.Registered {
			regStr = "yes"
		}
		out := fmt.Sprintf("Registered:    %s\nEndpoint ID:   %s\nLast checkin:  %s\nAgent version: %s\nOS:            %s\nActive threats: %d\n",
			regStr, status.EndpointID, status.LastCheckin, status.AgentVersion, status.OSInfo, status.ThreatCount)
		return &modulepb.CLICommandResponse{Stdout: out}, nil

	case "skauswatch alerts":
		severity := req.Flags["severity"]
		status := "active"
		alerts, err := m.client.GetAlerts(ctx, status)
		if err != nil {
			return &modulepb.CLICommandResponse{Stderr: err.Error() + "\n", ExitCode: 1}, nil
		}
		out := ""
		for _, a := range alerts {
			if severity != "" && a.Severity != severity {
				continue
			}
			out += fmt.Sprintf("%-8s %-10s %-12s %s\n", a.Severity, a.Type, a.Status, a.Description)
		}
		if out == "" {
			out = "No alerts found\n"
		}
		return &modulepb.CLICommandResponse{Stdout: out}, nil

	case "skauswatch scan":
		scanType := req.Flags["type"]
		if scanType == "" {
			scanType = "quick"
		}
		result, err := m.client.StartScan(ctx, scanType)
		if err != nil {
			return &modulepb.CLICommandResponse{Stderr: err.Error() + "\n", ExitCode: 1}, nil
		}
		out := fmt.Sprintf("Scan started: %s (type: %s, status: %s)\n", result.ID, result.Type, result.Status)
		return &modulepb.CLICommandResponse{Stdout: out}, nil

	case "skauswatch quarantine list":
		entries, err := m.client.GetQuarantine(ctx)
		if err != nil {
			return &modulepb.CLICommandResponse{Stderr: err.Error() + "\n", ExitCode: 1}, nil
		}
		if len(entries) == 0 {
			return &modulepb.CLICommandResponse{Stdout: "No quarantined files\n"}, nil
		}
		out := ""
		for _, e := range entries {
			out += fmt.Sprintf("%-12s %-12s %-10s %s\n", e.ID, e.ThreatType, e.Status, e.FilePath)
		}
		return &modulepb.CLICommandResponse{Stdout: out}, nil

	case "skauswatch quarantine restore":
		if len(req.Args) == 0 {
			return &modulepb.CLICommandResponse{Stderr: "quarantine entry ID required\n", ExitCode: 1}, nil
		}
		if err := m.client.RestoreFile(ctx, req.Args[0]); err != nil {
			return &modulepb.CLICommandResponse{Stderr: err.Error() + "\n", ExitCode: 1}, nil
		}
		return &modulepb.CLICommandResponse{Stdout: fmt.Sprintf("File %s restored\n", req.Args[0])}, nil

	case "skauswatch quarantine delete":
		if len(req.Args) == 0 {
			return &modulepb.CLICommandResponse{Stderr: "quarantine entry ID required\n", ExitCode: 1}, nil
		}
		if err := m.client.DeleteFile(ctx, req.Args[0]); err != nil {
			return &modulepb.CLICommandResponse{Stderr: err.Error() + "\n", ExitCode: 1}, nil
		}
		return &modulepb.CLICommandResponse{Stdout: fmt.Sprintf("File %s deleted\n", req.Args[0])}, nil

	default:
		return &modulepb.CLICommandResponse{Stderr: fmt.Sprintf("unknown command: %s\n", req.CommandPath), ExitCode: 1}, nil
	}
}

func (m *Module) GetIcon(ctx context.Context) (*modulepb.IconResponse, error) {
	return &modulepb.IconResponse{}, nil
}
