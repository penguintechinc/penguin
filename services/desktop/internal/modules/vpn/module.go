package vpn

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

type Module struct {
	client  *Client
	logger  *logrus.Logger
	mu      sync.RWMutex
	started bool
}

func New() *Module {
	return &Module{}
}

func (m *Module) Name() string        { return "vpn" }
func (m *Module) DisplayName() string { return "VPN" }
func (m *Module) Description() string { return "WireGuard VPN client for secure network access" }
func (m *Module) Version() string     { return "0.1.0" }

func (m *Module) Init(ctx context.Context, deps module.Dependencies) error {
	m.logger = deps.Logger.(*logrus.Logger)
	m.client = NewClient(deps, m.logger)
	return nil
}

func (m *Module) Start(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.started = true
	m.logger.Info("VPN module started")
	return nil
}

func (m *Module) Stop(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.client != nil && m.client.IsConnected() {
		m.client.Disconnect(ctx)
	}
	m.started = false
	m.logger.Info("VPN module stopped")
	return nil
}

func (m *Module) HealthCheck(ctx context.Context) module.HealthStatus {
	m.mu.RLock()
	defer m.mu.RUnlock()
	if !m.started {
		return module.HealthStatus{State: module.HealthUnknown, Message: "not started"}
	}
	if m.client == nil {
		return module.HealthStatus{State: module.HealthUnhealthy, Message: "client not initialized"}
	}
	if m.client.IsConnected() {
		return module.HealthStatus{
			State:   module.HealthHealthy,
			Message: "connected",
			Details: m.client.StatusDetails(),
		}
	}
	return module.HealthStatus{State: module.HealthDegraded, Message: "disconnected"}
}

// --- PluginModule interface ---

func (m *Module) GetGUIPanel(ctx context.Context) (*modulepb.GUIPanel, error) {
	if m.client == nil {
		return uischema.Panel(
			uischema.Card("vpn-card", "VPN Connection", "WireGuard VPN",
				uischema.Label("vpn-error", "VPN client not initialized"),
			),
		), nil
	}

	var statusText, ipText, uptimeText string
	var connectDisabled, disconnectDisabled bool

	if m.client.IsConnected() {
		statusText = "Status: Connected"
		details := m.client.StatusDetails()
		ipText = "IP: " + details["ip"]
		uptimeText = "Uptime: " + details["uptime"]
		connectDisabled = true
		disconnectDisabled = false
	} else {
		statusText = "Status: Disconnected"
		ipText = "IP: -"
		uptimeText = "Uptime: -"
		connectDisabled = false
		disconnectDisabled = true
	}

	connectBtn := uischema.Button("vpn-connect", "Connect")
	connectBtn.Disabled = connectDisabled
	disconnectBtn := uischema.Button("vpn-disconnect", "Disconnect")
	disconnectBtn.Disabled = disconnectDisabled

	return uischema.Panel(
		uischema.VBox(
			uischema.Card("vpn-status-card", "VPN Connection", "WireGuard VPN",
				uischema.Label("vpn-status", statusText),
				uischema.Label("vpn-ip", ipText),
				uischema.Label("vpn-uptime", uptimeText),
			),
			uischema.HBox(connectBtn, disconnectBtn),
		),
	), nil
}

func (m *Module) HandleGUIEvent(ctx context.Context, event *modulepb.GUIEvent) (*modulepb.GUIPanel, error) {
	switch event.WidgetID {
	case "vpn-connect":
		if m.client != nil {
			go m.client.Connect(ctx)
		}
	case "vpn-disconnect":
		if m.client != nil {
			go m.client.Disconnect(ctx)
		}
	}
	return m.GetGUIPanel(ctx)
}

func (m *Module) GetCLICommands(ctx context.Context) (*modulepb.CLICommandList, error) {
	vpnCmd := clischema.Command("vpn", "VPN management commands")
	clischema.WithSubcommands(vpnCmd,
		*clischema.Command("connect", "Connect to VPN"),
		*clischema.Command("disconnect", "Disconnect from VPN"),
		*clischema.Command("status", "Show VPN status"),
	)
	return clischema.CommandList(*vpnCmd), nil
}

func (m *Module) ExecuteCLICommand(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
	switch req.CommandPath {
	case "vpn connect":
		if err := m.client.Connect(ctx); err != nil {
			return &modulepb.CLICommandResponse{Stderr: err.Error() + "\n", ExitCode: 1}, nil
		}
		return &modulepb.CLICommandResponse{Stdout: "VPN connected\n"}, nil

	case "vpn disconnect":
		if err := m.client.Disconnect(ctx); err != nil {
			return &modulepb.CLICommandResponse{Stderr: err.Error() + "\n", ExitCode: 1}, nil
		}
		return &modulepb.CLICommandResponse{Stdout: "VPN disconnected\n"}, nil

	case "vpn status":
		if m.client.IsConnected() {
			out := "VPN: Connected\n"
			for k, v := range m.client.StatusDetails() {
				out += fmt.Sprintf("  %s: %s\n", k, v)
			}
			return &modulepb.CLICommandResponse{Stdout: out}, nil
		}
		return &modulepb.CLICommandResponse{Stdout: "VPN: Disconnected\n"}, nil

	default:
		return &modulepb.CLICommandResponse{
			Stderr:   fmt.Sprintf("unknown command: %s\n", req.CommandPath),
			ExitCode: 1,
		}, nil
	}
}

func (m *Module) GetIcon(ctx context.Context) (*modulepb.IconResponse, error) {
	return &modulepb.IconResponse{}, nil
}
