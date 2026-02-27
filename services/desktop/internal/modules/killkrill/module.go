package killkrill

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

// Module implements the KillKrill logging/metrics agent as a PluginModule.
type Module struct {
	client  *Client
	logger  *logrus.Logger
	mu      sync.RWMutex
	started bool
}

func New() *Module              { return &Module{} }
func (m *Module) Name() string        { return "killkrill" }
func (m *Module) DisplayName() string { return "KillKrill" }
func (m *Module) Description() string { return "Centralized logging and metrics agent for KillKrill" }
func (m *Module) Version() string     { return "0.1.0" }

func (m *Module) Init(ctx context.Context, deps module.Dependencies) error {
	m.logger = deps.Logger.(*logrus.Logger)
	// Client is created lazily when config is available.
	return nil
}

func (m *Module) Start(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.started = true
	if m.client != nil {
		if err := m.client.Connect(ctx); err != nil {
			m.logger.WithError(err).Warn("killkrill connection failed, will retry on flush")
		}
	}
	m.logger.Info("KillKrill module started")
	return nil
}

func (m *Module) Stop(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.client != nil {
		m.client.Disconnect(ctx)
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
	qs := m.client.GetQueueStatus()
	details := map[string]string{
		"logs_pending":    fmt.Sprintf("%d", qs.LogsPending),
		"metrics_pending": fmt.Sprintf("%d", qs.MetricsPending),
		"last_flush":      qs.LastFlush,
	}
	if !qs.Connected {
		return module.HealthStatus{State: module.HealthDegraded, Message: "disconnected", Details: details}
	}
	return module.HealthStatus{State: module.HealthHealthy, Message: "connected", Details: details}
}

// --- GUI ---

func (m *Module) GetGUIPanel(ctx context.Context) (*modulepb.GUIPanel, error) {
	if m.client == nil {
		return uischema.Panel(
			uischema.Card("kk-card", "KillKrill", "Logging & Metrics Agent",
				uischema.Label("kk-nocfg", "Not configured. Set killkrill.base_url in config."),
			),
		), nil
	}

	qs := m.client.GetQueueStatus()
	connStatus := "Disconnected"
	if qs.Connected {
		connStatus = "Connected"
	}
	lastFlush := qs.LastFlush
	if lastFlush == "" {
		lastFlush = "never"
	}

	return uischema.Panel(
		uischema.VBox(
			uischema.Card("kk-status-card", "KillKrill", "Logging & Metrics Agent",
				uischema.Label("kk-conn", fmt.Sprintf("Connection: %s", connStatus)),
				uischema.Label("kk-logs", fmt.Sprintf("Logs pending: %d", qs.LogsPending)),
				uischema.Label("kk-metrics", fmt.Sprintf("Metrics pending: %d", qs.MetricsPending)),
				uischema.Label("kk-flush", fmt.Sprintf("Last flush: %s", lastFlush)),
			),
			uischema.HBox(
				uischema.Button("kk-flush-btn", "Flush Now"),
			),
		),
	), nil
}

func (m *Module) HandleGUIEvent(ctx context.Context, event *modulepb.GUIEvent) (*modulepb.GUIPanel, error) {
	if event.WidgetID == "kk-flush-btn" && m.client != nil {
		if err := m.client.Flush(ctx); err != nil {
			m.logger.WithError(err).Warn("manual flush failed")
		}
	}
	return m.GetGUIPanel(ctx)
}

// --- CLI ---

func (m *Module) GetCLICommands(ctx context.Context) (*modulepb.CLICommandList, error) {
	kkCmd := clischema.Command("killkrill", "KillKrill logging & metrics management")
	clischema.WithSubcommands(kkCmd,
		*clischema.Command("status", "Show connection and queue status"),
		*clischema.Command("flush", "Force immediate flush of queued data"),
	)
	return clischema.CommandList(*kkCmd), nil
}

func (m *Module) ExecuteCLICommand(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
	if m.client == nil {
		return &modulepb.CLICommandResponse{Stderr: "killkrill client not configured\n", ExitCode: 1}, nil
	}

	switch req.CommandPath {
	case "killkrill status":
		qs := m.client.GetQueueStatus()
		connStr := "disconnected"
		if qs.Connected {
			connStr = "connected"
		}
		lastFlush := qs.LastFlush
		if lastFlush == "" {
			lastFlush = "never"
		}
		out := fmt.Sprintf("Connection:      %s\nLogs pending:    %d\nMetrics pending: %d\nLast flush:      %s\n",
			connStr, qs.LogsPending, qs.MetricsPending, lastFlush)
		return &modulepb.CLICommandResponse{Stdout: out}, nil

	case "killkrill flush":
		if err := m.client.Flush(ctx); err != nil {
			return &modulepb.CLICommandResponse{Stderr: fmt.Sprintf("flush failed: %v\n", err), ExitCode: 1}, nil
		}
		return &modulepb.CLICommandResponse{Stdout: "Flush completed successfully\n"}, nil

	default:
		return &modulepb.CLICommandResponse{Stderr: fmt.Sprintf("unknown command: %s\n", req.CommandPath), ExitCode: 1}, nil
	}
}

func (m *Module) GetIcon(ctx context.Context) (*modulepb.IconResponse, error) {
	return &modulepb.IconResponse{}, nil
}
