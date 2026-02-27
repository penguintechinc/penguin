package openziti

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
	provider *Provider
	logger   *logrus.Logger
	mu       sync.RWMutex
	started  bool
}

func New() *Module              { return &Module{} }
func (m *Module) Name() string        { return "openziti" }
func (m *Module) DisplayName() string { return "OpenZiti" }
func (m *Module) Description() string { return "Zero-trust overlay network access via OpenZiti" }
func (m *Module) Version() string     { return "0.1.0" }

func (m *Module) Init(ctx context.Context, deps module.Dependencies) error {
	m.logger = deps.Logger.(*logrus.Logger)
	m.provider = NewProvider(deps.DataDir, m.logger)
	return nil
}

func (m *Module) Start(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.started = true
	m.logger.Info("OpenZiti module started")
	return nil
}

func (m *Module) Stop(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.provider != nil {
		m.provider.Disconnect()
	}
	m.started = false
	m.logger.Info("OpenZiti module stopped")
	return nil
}

func (m *Module) HealthCheck(ctx context.Context) module.HealthStatus {
	m.mu.RLock()
	defer m.mu.RUnlock()
	if !m.started {
		return module.HealthStatus{State: module.HealthUnknown, Message: "not started"}
	}
	if m.provider == nil {
		return module.HealthStatus{State: module.HealthUnhealthy, Message: "provider not initialized"}
	}
	if m.provider.IsConnected() {
		return module.HealthStatus{
			State:   module.HealthHealthy,
			Message: "connected",
			Details: map[string]string{"identity": m.provider.GetIdentityName()},
		}
	}
	return module.HealthStatus{State: module.HealthDegraded, Message: "disconnected"}
}

// --- PluginModule interface ---

func (m *Module) GetGUIPanel(ctx context.Context) (*modulepb.GUIPanel, error) {
	statusText := "Status: Disconnected"
	identityText := "Identity: Not enrolled"
	servicesText := "Services: None"

	if m.provider != nil && m.provider.IsConnected() {
		statusText = "Status: Connected"
		identityText = "Identity: " + m.provider.GetIdentityName()
		services := m.provider.Services()
		if len(services) > 0 {
			servicesText = fmt.Sprintf("Services: %d available", len(services))
		}
	}

	return uischema.Panel(
		uischema.VBox(
			uischema.Card("ziti-status-card", "OpenZiti", "Zero-Trust Network Access",
				uischema.Label("ziti-status", statusText),
				uischema.Label("ziti-identity", identityText),
				uischema.Label("ziti-services", servicesText),
			),
			uischema.HBox(
				uischema.Button("ziti-enroll", "Enroll"),
				uischema.Button("ziti-connect", "Connect"),
				uischema.Button("ziti-disconnect", "Disconnect"),
				uischema.Button("ziti-refresh", "Refresh"),
			),
		),
	), nil
}

func (m *Module) HandleGUIEvent(ctx context.Context, event *modulepb.GUIEvent) (*modulepb.GUIPanel, error) {
	if m.provider == nil {
		return m.GetGUIPanel(ctx)
	}
	switch event.WidgetID {
	case "ziti-enroll":
		m.provider.Enroll(ctx, "")
	case "ziti-connect":
		m.provider.Connect(ctx)
	case "ziti-disconnect":
		m.provider.Disconnect()
	case "ziti-refresh":
		m.provider.RefreshServices(ctx)
	}
	return m.GetGUIPanel(ctx)
}

func (m *Module) GetCLICommands(ctx context.Context) (*modulepb.CLICommandList, error) {
	zitiCmd := clischema.Command("ziti", "OpenZiti overlay network management")
	clischema.WithSubcommands(zitiCmd,
		*clischema.Command("enroll [jwt-file]", "Enroll a new OpenZiti identity"),
		*clischema.Command("status", "Show OpenZiti connection status"),
		*clischema.Command("connect", "Connect to the OpenZiti overlay"),
		*clischema.Command("disconnect", "Disconnect from the OpenZiti overlay"),
		*clischema.Command("services", "List available OpenZiti services"),
		*clischema.Command("refresh", "Refresh the list of available services"),
	)
	return clischema.CommandList(*zitiCmd), nil
}

func (m *Module) ExecuteCLICommand(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
	switch req.CommandPath {
	case "ziti enroll":
		jwtFile := ""
		if len(req.Args) > 0 {
			jwtFile = req.Args[0]
		}
		if err := m.provider.Enroll(ctx, jwtFile); err != nil {
			return &modulepb.CLICommandResponse{Stderr: err.Error() + "\n", ExitCode: 1}, nil
		}
		return &modulepb.CLICommandResponse{Stdout: "OpenZiti enrollment initiated\n"}, nil

	case "ziti status":
		if m.provider.IsConnected() {
			out := "OpenZiti: Connected\n"
			out += fmt.Sprintf("Identity: %s\n", m.provider.GetIdentityName())
			services := m.provider.Services()
			if len(services) > 0 {
				out += "Available Services:\n"
				for _, s := range services {
					out += fmt.Sprintf("  - %s\n", s)
				}
			}
			return &modulepb.CLICommandResponse{Stdout: out}, nil
		}
		return &modulepb.CLICommandResponse{Stdout: "OpenZiti: Disconnected\n"}, nil

	case "ziti connect":
		if err := m.provider.Connect(ctx); err != nil {
			return &modulepb.CLICommandResponse{Stderr: fmt.Sprintf("failed to connect: %v\n", err), ExitCode: 1}, nil
		}
		return &modulepb.CLICommandResponse{Stdout: "OpenZiti connected successfully\n"}, nil

	case "ziti disconnect":
		if err := m.provider.Disconnect(); err != nil {
			return &modulepb.CLICommandResponse{Stderr: fmt.Sprintf("failed to disconnect: %v\n", err), ExitCode: 1}, nil
		}
		return &modulepb.CLICommandResponse{Stdout: "OpenZiti disconnected\n"}, nil

	case "ziti services":
		services := m.provider.Services()
		if len(services) == 0 {
			return &modulepb.CLICommandResponse{Stdout: "No services available\n"}, nil
		}
		out := "Available OpenZiti Services:\n"
		for _, s := range services {
			out += fmt.Sprintf("  - %s\n", s)
		}
		return &modulepb.CLICommandResponse{Stdout: out}, nil

	case "ziti refresh":
		if err := m.provider.RefreshServices(ctx); err != nil {
			return &modulepb.CLICommandResponse{Stderr: fmt.Sprintf("failed to refresh services: %v\n", err), ExitCode: 1}, nil
		}
		return &modulepb.CLICommandResponse{Stdout: "Services refreshed\n"}, nil

	default:
		return &modulepb.CLICommandResponse{Stderr: fmt.Sprintf("unknown command: %s\n", req.CommandPath), ExitCode: 1}, nil
	}
}

func (m *Module) GetIcon(ctx context.Context) (*modulepb.IconResponse, error) {
	return &modulepb.IconResponse{}, nil
}
