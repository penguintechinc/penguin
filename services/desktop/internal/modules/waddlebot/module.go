package waddlebot

import (
	"context"
	"fmt"

	"github.com/penguintechinc/penguin/services/desktop/internal/module"
	"github.com/penguintechinc/penguin/services/desktop/pkg/clischema"
	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
	"github.com/penguintechinc/penguin/services/desktop/pkg/uischema"
	"github.com/sirupsen/logrus"
)

// Module implements the WaddleBot bridge as a PluginModule.
type Module struct {
	module.BaseModule
	client        *Client
	poller        *Poller
	obs           *OBSClient
	config        BridgeConfig
	recentActions []RecentAction
}

// New returns a new, unconfigured WaddleBot bridge module.
func New() *Module { return &Module{} }

func (m *Module) Name() string        { return "waddlebot" }
func (m *Module) DisplayName() string { return "Waddles" }
func (m *Module) Description() string { return "WaddleBot bridge for OBS control and scripting" }
func (m *Module) Version() string     { return "0.1.0" }

// Init wires up the module with the shared dependencies.
// Client and OBS connection are created lazily when config is available.
func (m *Module) Init(_ context.Context, deps module.Dependencies) error {
	m.Logger = deps.Logger.(*logrus.Logger)
	return nil
}

// Start registers the bridge with the WaddleBot API and begins polling.
func (m *Module) Start(ctx context.Context) error {
	m.BaseModule.MarkStarted()
	if m.client != nil {
		if err := m.client.Register(ctx); err != nil {
			m.Logger.WithError(err).Warn("bridge registration failed, will retry")
		}
		if m.poller != nil {
			m.poller.Start()
		}
	}
	m.Logger.Info("Waddles module started")
	return nil
}

// Stop stops the poller, disconnects OBS, and unregisters the bridge.
func (m *Module) Stop(ctx context.Context) error {
	if m.poller != nil {
		m.poller.Stop()
	}
	if m.obs != nil {
		m.obs.Disconnect()
	}
	if m.client != nil {
		if err := m.client.Unregister(ctx); err != nil {
			m.Logger.WithError(err).Warn("bridge unregister failed")
		}
	}
	m.BaseModule.MarkStopped()
	return nil
}

// HealthCheck reports the current bridge connection health.
func (m *Module) HealthCheck(_ context.Context) module.HealthStatus {
	if !m.BaseModule.IsStarted() {
		return m.BaseModule.NotStartedStatus()
	}
	if m.client == nil {
		return m.BaseModule.ClientNotConfiguredStatus()
	}
	status := m.client.GetStatus()
	details := map[string]string{
		"connected":    fmt.Sprintf("%v", status.Connected),
		"community_id": status.CommunityID,
		"bridge_id":    status.BridgeID,
	}
	if !status.Connected {
		return module.HealthStatus{State: module.HealthDegraded, Message: "disconnected", Details: details}
	}
	return module.HealthStatus{State: module.HealthHealthy, Message: "connected", Details: details}
}

// --- GUI ---

// GetGUIPanel returns the declarative widget tree for the Waddles panel.
func (m *Module) GetGUIPanel(_ context.Context) (*modulepb.GUIPanel, error) {
	if m.client == nil {
		return uischema.Panel(
			uischema.Card("wb-card", "Waddles", "WaddleBot Desktop Bridge",
				uischema.Label("wb-nocfg", "Not configured. Set waddlebot.api_url in config."),
			),
		), nil
	}

	status := m.client.GetStatus()
	connStatus := "Disconnected"
	if status.Connected {
		connStatus = "Connected"
	}

	obsStatus := "Disconnected"
	if m.obs != nil {
		info := m.obs.GetConnectionInfo()
		if info.State == "connected" {
			obsStatus = fmt.Sprintf("Connected - Scene: %s", info.CurrentScene)
		}
	}

	actionsText := "No recent actions"
	if len(m.recentActions) > 0 {
		actionsText = ""
		for _, a := range m.recentActions {
			actionsText += fmt.Sprintf("- %s: %s (%s)\n",
				a.Name, a.Status, a.Timestamp.Format("15:04:05"))
		}
	}

	var connectBtn *modulepb.Widget
	if status.Connected {
		connectBtn = uischema.Button("wb-disconnect-btn", "Disconnect")
	} else {
		connectBtn = uischema.Button("wb-connect-btn", "Connect")
	}

	return uischema.Panel(
		uischema.VBox(
			uischema.Card("wb-status-card", "Waddles Bridge", "WaddleBot Desktop Bridge",
				uischema.Label("wb-conn", fmt.Sprintf("Bridge: %s", connStatus)),
				uischema.Label("wb-community", fmt.Sprintf("Community: %s", status.CommunityID)),
				uischema.Label("wb-user", fmt.Sprintf("User: %s", status.UserID)),
			),
			uischema.Card("wb-obs-card", "OBS Studio", "Streaming & Recording",
				uischema.Label("wb-obs-status", fmt.Sprintf("OBS: %s", obsStatus)),
			),
			uischema.Card("wb-actions-card", "Recent Actions", "Last executed",
				uischema.RichText("wb-actions-list", actionsText),
			),
			uischema.HBox(connectBtn),
		),
	), nil
}

// HandleGUIEvent processes button taps and other widget interactions.
func (m *Module) HandleGUIEvent(ctx context.Context, event *modulepb.GUIEvent) (*modulepb.GUIPanel, error) {
	switch event.WidgetID {
	case "wb-connect-btn":
		if m.client != nil {
			go func() {
				if err := m.client.Register(ctx); err != nil {
					m.Logger.WithError(err).Warn("bridge connect failed")
				}
			}()
		}
	case "wb-disconnect-btn":
		if m.client != nil {
			go func() {
				if err := m.client.Unregister(ctx); err != nil {
					m.Logger.WithError(err).Warn("bridge disconnect failed")
				}
			}()
		}
	}
	return m.GetGUIPanel(ctx)
}

// --- CLI ---

// GetCLICommands returns the declarative command tree for the Waddles CLI.
func (m *Module) GetCLICommands(_ context.Context) (*modulepb.CLICommandList, error) {
	waddlesCmd := clischema.Command("waddles", "WaddleBot bridge management")

	obsCmd := clischema.Command("obs", "OBS Studio control")
	clischema.WithSubcommands(obsCmd,
		*clischema.Command("status", "Show OBS connection status"),
		*clischema.Command("scenes", "List available OBS scenes"),
	)

	runCmd := clischema.CommandWithLong("run", "Execute a script", "Run a Lua, Python, or Bash script via the bridge")
	clischema.WithFlags(runCmd,
		clischema.Flag("lang", "l", "Script language (lua|python|bash)", "lua"),
		clischema.Flag("file", "f", "Script file path", ""),
	)
	scriptCmd := clischema.Command("script", "Script execution")
	clischema.WithSubcommands(scriptCmd, *runCmd)

	actionsCmd := clischema.Command("actions", "Action management")
	clischema.WithSubcommands(actionsCmd,
		*clischema.Command("list", "List available bridge actions"),
	)

	clischema.WithSubcommands(waddlesCmd,
		*clischema.Command("status", "Show bridge connection status"),
		*clischema.Command("connect", "Register bridge with community"),
		*clischema.Command("disconnect", "Unregister bridge from community"),
		*obsCmd,
		*scriptCmd,
		*actionsCmd,
	)

	return clischema.CommandList(*waddlesCmd), nil
}

// ExecuteCLICommand dispatches CLI commands to the appropriate handler.
func (m *Module) ExecuteCLICommand(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
	switch req.CommandPath {
	case "waddles status":
		return m.cmdStatus()
	case "waddles connect":
		return m.cmdConnect(ctx)
	case "waddles disconnect":
		return m.cmdDisconnect(ctx)
	case "waddles obs status":
		return m.cmdOBSStatus()
	case "waddles obs scenes":
		return m.cmdOBSScenes(ctx)
	case "waddles actions list":
		return m.cmdActionsList()
	case "waddles script run":
		return m.cmdScriptRun(ctx, req)
	default:
		return modulepb.UnknownCommandResponse(req.CommandPath), nil
	}
}

// GetIcon returns PNG icon data for the module (empty for now).
func (m *Module) GetIcon(_ context.Context) (*modulepb.IconResponse, error) {
	return &modulepb.IconResponse{}, nil
}

// --- command handlers ---

func (m *Module) cmdStatus() (*modulepb.CLICommandResponse, error) {
	if m.client == nil {
		return modulepb.OKResponse("Bridge: not configured\n"), nil
	}
	s := m.client.GetStatus()
	obsState := "not configured"
	if m.obs != nil {
		obsState = m.obs.GetConnectionInfo().State
	}
	out := fmt.Sprintf(
		"Bridge:    %s\nCommunity: %s\nUser:      %s\nBridge ID: %s\nOBS:       %s\n",
		boolStatus(s.Connected), s.CommunityID, s.UserID, s.BridgeID, obsState,
	)
	return modulepb.OKResponse(out), nil
}

func (m *Module) cmdConnect(ctx context.Context) (*modulepb.CLICommandResponse, error) {
	if m.client == nil {
		return modulepb.ErrorResponse(fmt.Errorf("bridge not configured")), nil
	}
	if err := m.client.Register(ctx); err != nil {
		return modulepb.ErrorResponse(fmt.Errorf("connect: %w", err)), nil
	}
	return modulepb.OKResponse("Bridge connected successfully\n"), nil
}

func (m *Module) cmdDisconnect(ctx context.Context) (*modulepb.CLICommandResponse, error) {
	if m.client == nil {
		return modulepb.ErrorResponse(fmt.Errorf("bridge not configured")), nil
	}
	if err := m.client.Unregister(ctx); err != nil {
		return modulepb.ErrorResponse(fmt.Errorf("disconnect: %w", err)), nil
	}
	return modulepb.OKResponse("Bridge disconnected\n"), nil
}

func (m *Module) cmdOBSStatus() (*modulepb.CLICommandResponse, error) {
	if m.obs == nil {
		return modulepb.OKResponse("OBS: not configured\n"), nil
	}
	info := m.obs.GetConnectionInfo()
	out := fmt.Sprintf("OBS:       %s\nVersion:   %s\nScene:     %s\nStreaming: %v\nRecording: %v\n",
		info.State, info.OBSVersion, info.CurrentScene, info.Streaming, info.Recording)
	return modulepb.OKResponse(out), nil
}

func (m *Module) cmdOBSScenes(ctx context.Context) (*modulepb.CLICommandResponse, error) {
	if m.obs == nil {
		return modulepb.ErrorResponse(fmt.Errorf("OBS not configured")), nil
	}
	scenes, err := m.obs.GetScenes(ctx)
	if err != nil {
		return modulepb.ErrorResponse(fmt.Errorf("get scenes: %w", err)), nil
	}
	out := "Scenes:\n"
	for _, s := range scenes {
		marker := "  "
		if s.IsCurrent {
			marker = "* "
		}
		out += fmt.Sprintf("%s%s\n", marker, s.Name)
	}
	return modulepb.OKResponse(out), nil
}

func (m *Module) cmdActionsList() (*modulepb.CLICommandResponse, error) {
	out := "Available Actions:\n"
	out += "  obs.switch_scene    - Switch OBS scene\n"
	out += "  obs.toggle_source   - Toggle source visibility\n"
	out += "  obs.start_stream    - Start streaming\n"
	out += "  obs.stop_stream     - Stop streaming\n"
	out += "  obs.start_recording - Start recording\n"
	out += "  obs.stop_recording  - Stop recording\n"
	out += "  script.run          - Execute a Lua, Python, or Bash script\n"
	return modulepb.OKResponse(out), nil
}

func (m *Module) cmdScriptRun(_ context.Context, _ *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
	return modulepb.OKResponse("Script execution not yet configured\n"), nil
}

// boolStatus converts a bool to a human-readable connection string.
func boolStatus(b bool) string {
	if b {
		return "connected"
	}
	return "disconnected"
}
