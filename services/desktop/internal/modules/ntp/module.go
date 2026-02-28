package ntp

import (
	"context"
	"fmt"

	"github.com/penguintechinc/penguin/services/desktop/internal/module"
	"github.com/penguintechinc/penguin/services/desktop/pkg/clischema"
	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
	"github.com/penguintechinc/penguin/services/desktop/pkg/uischema"
	"github.com/sirupsen/logrus"
)

// Module implements the NTP module for the desktop client.
type Module struct {
	module.BaseModule
	client     *Client
	lastSync   string
	lastOffset string
	lastServer string
	lastStrat  string
}

func New() *Module              { return &Module{} }
func (m *Module) Name() string        { return "ntp" }
func (m *Module) DisplayName() string { return "NTP" }
func (m *Module) Description() string { return "Network Time Protocol client for time synchronization" }
func (m *Module) Version() string     { return "0.1.0" }

func (m *Module) Init(ctx context.Context, deps module.Dependencies) error {
	m.Logger = deps.Logger.(*logrus.Logger)
	m.Logger.WithField("module", m.Name()).Debug("Initializing NTP module")
	m.client = NewClient(nil, m.Logger)
	m.lastSync = "Never"
	m.lastOffset = "-"
	m.lastServer = "-"
	m.lastStrat = "-"
	return nil
}

func (m *Module) Start(ctx context.Context) error {
	m.MarkStarted()
	m.Logger.WithField("module", m.Name()).Info("NTP module started")
	return nil
}

func (m *Module) Stop(ctx context.Context) error {
	if m.client != nil {
		m.client.Close()
	}
	m.MarkStopped()
	m.Logger.WithField("module", m.Name()).Info("NTP module stopped")
	return nil
}

func (m *Module) HealthCheck(ctx context.Context) module.HealthStatus {
	if !m.IsStarted() {
		return m.NotStartedStatus()
	}
	if m.client != nil {
		return module.HealthStatus{
			State:   module.HealthHealthy,
			Message: "NTP client ready",
			Details: map[string]string{"servers": fmt.Sprintf("%d", len(m.client.GetServerURLs()))},
		}
	}
	return module.HealthStatus{State: module.HealthDegraded, Message: "NTP client not configured"}
}

// --- PluginModule interface ---

func (m *Module) GetGUIPanel(ctx context.Context) (*modulepb.GUIPanel, error) {
	m.RLock()
	defer m.RUnlock()

	return uischema.Panel(
		uischema.VBox(
			uischema.Card("ntp-card", "Time Sync", "NTP Status",
				uischema.Label("ntp-sync", "Last sync: "+m.lastSync),
				uischema.Label("ntp-offset", "Offset: "+m.lastOffset),
				uischema.Label("ntp-server", "Server: "+m.lastServer),
				uischema.Label("ntp-stratum", "Stratum: "+m.lastStrat),
			),
			uischema.Button("ntp-sync-btn", "Sync Now"),
		),
	), nil
}

func (m *Module) HandleGUIEvent(ctx context.Context, event *modulepb.GUIEvent) (*modulepb.GUIPanel, error) {
	if event.WidgetID == "ntp-sync-btn" && m.client != nil {
		resp, err := m.client.Query(ctx)
		if err != nil {
			m.Lock()
			m.lastSync = "Error: " + err.Error()
			m.Unlock()
		} else {
			m.Lock()
			m.lastSync = resp.Time.Format("15:04:05 MST")
			m.lastOffset = fmt.Sprintf("%v", resp.Offset)
			m.lastServer = resp.Server
			m.lastStrat = fmt.Sprintf("%d", resp.Stratum)
			m.Unlock()
		}
	}
	return m.GetGUIPanel(ctx)
}

func (m *Module) GetCLICommands(ctx context.Context) (*modulepb.CLICommandList, error) {
	ntpCmd := clischema.Command("ntp", "NTP time synchronization management")
	clischema.WithSubcommands(ntpCmd,
		*clischema.Command("sync", "Sync time with NTP server"),
		*clischema.Command("status", "Show NTP module status"),
		*clischema.Command("health", "Check NTP module health"),
	)
	return clischema.CommandList(*ntpCmd), nil
}

func (m *Module) ExecuteCLICommand(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
	switch req.CommandPath {
	case "ntp sync":
		if m.client == nil {
			return modulepb.ErrorResponse(fmt.Errorf("NTP client not initialized")), nil
		}
		resp, err := m.client.Query(ctx)
		if err != nil {
			return modulepb.ErrorResponse(err), nil
		}
		out := fmt.Sprintf("Time:    %s\nOffset:  %v\nDelay:   %v\nServer:  %s\nStratum: %d\n",
			resp.Time.Format("2006-01-02 15:04:05 MST"), resp.Offset, resp.Delay, resp.Server, resp.Stratum)
		return modulepb.OKResponse(out), nil

	case "ntp status":
		health := m.HealthCheck(ctx)
		out := fmt.Sprintf("Status: %s\nMessage: %s\n", health.State, health.Message)
		if m.client != nil {
			out += "\nConfigured servers:\n"
			for _, s := range m.client.GetServerURLs() {
				out += fmt.Sprintf("  - %s\n", s)
			}
		}
		return modulepb.OKResponse(out), nil

	case "ntp health":
		health := m.HealthCheck(ctx)
		out := fmt.Sprintf("Status: %s\nMessage: %s\n", health.State, health.Message)
		for k, v := range health.Details {
			out += fmt.Sprintf("%s: %s\n", k, v)
		}
		return modulepb.OKResponse(out), nil

	default:
		return modulepb.UnknownCommandResponse(req.CommandPath), nil
	}
}

func (m *Module) GetIcon(ctx context.Context) (*modulepb.IconResponse, error) {
	return &modulepb.IconResponse{}, nil
}
