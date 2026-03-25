package articdbm

import (
	"context"
	"fmt"

	"github.com/penguintechinc/penguin/services/desktop/internal/module"
	"github.com/penguintechinc/penguin/services/desktop/pkg/clischema"
	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
	"github.com/penguintechinc/penguin/services/desktop/pkg/uischema"
	"github.com/sirupsen/logrus"
)

type Module struct {
	module.BaseModule
	client *Client
}

func New() *Module                    { return &Module{} }
func (m *Module) Name() string        { return "articdbm" }
func (m *Module) DisplayName() string { return "ArticDBM" }
func (m *Module) Description() string { return "Database proxy management via ArticDBM" }
func (m *Module) Version() string     { return "0.1.0" }

func (m *Module) Init(ctx context.Context, deps module.Dependencies) error {
	m.Logger = deps.Logger.(*logrus.Logger)
	return nil
}

func (m *Module) Start(ctx context.Context) error {
	m.MarkStarted()
	m.Logger.Info("ArticDBM module started")
	return nil
}

func (m *Module) Stop(ctx context.Context) error {
	m.MarkStopped()
	return nil
}

func (m *Module) HealthCheck(ctx context.Context) module.HealthStatus {
	if !m.IsStarted() {
		return module.HealthStatus{State: module.HealthUnknown, Message: "not started"}
	}
	if m.client != nil {
		return module.HealthStatus{State: module.HealthHealthy, Message: "connected to ArticDBM"}
	}
	return module.HealthStatus{State: module.HealthDegraded, Message: "client not configured"}
}

// --- PluginModule interface ---

func (m *Module) GetGUIPanel(ctx context.Context) (*modulepb.GUIPanel, error) {
	if m.client == nil {
		return uischema.Panel(
			uischema.Card("adbm-card", "ArticDBM", "Database Proxy Manager",
				uischema.Label("adbm-nocfg", "Not configured. Set articdbm.api_url in config."),
			),
		), nil
	}

	proxyText := "Loading proxies..."
	proxies, err := m.client.ListProxies(ctx)
	if err != nil {
		proxyText = "Error: " + err.Error()
	} else if len(proxies) == 0 {
		proxyText = "No proxies found"
	} else {
		proxyText = ""
		for _, p := range proxies {
			proxyText += fmt.Sprintf("%s (%s) - %s\n", p.Name, p.DatabaseType, p.Status)
		}
	}

	return uischema.Panel(
		uischema.Card("adbm-card", "ArticDBM", "Database Proxy Manager",
			uischema.Label("adbm-status", "Status: Ready"),
			uischema.Separator(),
			uischema.Label("adbm-proxies", proxyText),
		),
	), nil
}

func (m *Module) HandleGUIEvent(ctx context.Context, event *modulepb.GUIEvent) (*modulepb.GUIPanel, error) {
	return m.GetGUIPanel(ctx)
}

func (m *Module) GetCLICommands(ctx context.Context) (*modulepb.CLICommandList, error) {
	adbmCmd := clischema.Command("articdbm", "ArticDBM proxy management")
	clischema.WithSubcommands(adbmCmd,
		*clischema.Command("list", "List proxy instances"),
		*clischema.Command("status", "Show ArticDBM status"),
		*clischema.Command("get [name]", "Get proxy details"),
	)
	return clischema.CommandList(*adbmCmd), nil
}

func (m *Module) ExecuteCLICommand(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
	if m.client == nil {
		return &modulepb.CLICommandResponse{Stderr: "articdbm client not configured\n", ExitCode: 1}, nil
	}

	switch req.CommandPath {
	case "articdbm list":
		proxies, err := m.client.ListProxies(ctx)
		if err != nil {
			return &modulepb.CLICommandResponse{Stderr: err.Error() + "\n", ExitCode: 1}, nil
		}
		out := ""
		for _, p := range proxies {
			out += fmt.Sprintf("%-20s %-12s %-10s %s:%d\n", p.Name, p.DatabaseType, p.Status, p.Host, p.Port)
		}
		return &modulepb.CLICommandResponse{Stdout: out}, nil

	case "articdbm status":
		health, err := m.client.HealthCheck(ctx)
		if err != nil {
			return &modulepb.CLICommandResponse{Stderr: err.Error() + "\n", ExitCode: 1}, nil
		}
		return &modulepb.CLICommandResponse{Stdout: fmt.Sprintf("ArticDBM: %s\n", health)}, nil

	case "articdbm get":
		if len(req.Args) == 0 {
			return &modulepb.CLICommandResponse{Stderr: "proxy name required\n", ExitCode: 1}, nil
		}
		proxy, err := m.client.GetProxy(ctx, req.Args[0])
		if err != nil {
			return &modulepb.CLICommandResponse{Stderr: err.Error() + "\n", ExitCode: 1}, nil
		}
		out := fmt.Sprintf("Name: %s\nType: %s\nHost: %s:%d\nStatus: %s\nConnections: %d\n",
			proxy.Name, proxy.DatabaseType, proxy.Host, proxy.Port, proxy.Status, proxy.ActiveConnections)
		return &modulepb.CLICommandResponse{Stdout: out}, nil

	default:
		return modulepb.UnknownCommandResponse(req.CommandPath), nil
	}
}

func (m *Module) GetIcon(ctx context.Context) (*modulepb.IconResponse, error) {
	return &modulepb.IconResponse{}, nil
}
