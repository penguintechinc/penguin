package plugin

import (
	"context"

	"github.com/penguintechinc/penguin/services/desktop/internal/module"
	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
)

// ModuleAdapter wraps a module.PluginModule into the modulepb.ModuleService
// interface expected by the go-plugin RPC layer. This eliminates the need
// for per-module adapter boilerplate in each plugin binary.
type ModuleAdapter struct {
	Mod module.PluginModule
}

func (a *ModuleAdapter) GetInfo(ctx context.Context) (*modulepb.ModuleInfo, error) {
	return &modulepb.ModuleInfo{
		Name:        a.Mod.Name(),
		DisplayName: a.Mod.DisplayName(),
		Description: a.Mod.Description(),
		Version:     a.Mod.Version(),
	}, nil
}

func (a *ModuleAdapter) Init(ctx context.Context, req *modulepb.InitRequest) (*modulepb.InitResponse, error) {
	deps := module.Dependencies{
		ConfigDir:  req.ConfigDir,
		DataDir:    req.DataDir,
		AuthToken:  req.AuthToken,
		LicenseKey: req.LicenseKey,
	}
	// Inject a logger for the plugin process
	deps.Logger = newPluginLogger()
	if err := a.Mod.Init(ctx, deps); err != nil {
		return &modulepb.InitResponse{OK: false, Error: err.Error()}, nil
	}
	return &modulepb.InitResponse{OK: true}, nil
}

func (a *ModuleAdapter) Start(ctx context.Context) error {
	return a.Mod.Start(ctx)
}

func (a *ModuleAdapter) Stop(ctx context.Context) error {
	return a.Mod.Stop(ctx)
}

func (a *ModuleAdapter) HealthCheck(ctx context.Context) (*modulepb.HealthCheckResponse, error) {
	h := a.Mod.HealthCheck(ctx)
	return &modulepb.HealthCheckResponse{
		State:   modulepb.HealthState(h.State),
		Message: h.Message,
		Details: h.Details,
	}, nil
}

func (a *ModuleAdapter) GetGUIPanel(ctx context.Context) (*modulepb.GUIPanel, error) {
	return a.Mod.GetGUIPanel(ctx)
}

func (a *ModuleAdapter) HandleGUIEvent(ctx context.Context, event *modulepb.GUIEvent) (*modulepb.GUIPanel, error) {
	return a.Mod.HandleGUIEvent(ctx, event)
}

func (a *ModuleAdapter) GetCLICommands(ctx context.Context) (*modulepb.CLICommandList, error) {
	return a.Mod.GetCLICommands(ctx)
}

func (a *ModuleAdapter) ExecuteCLICommand(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
	return a.Mod.ExecuteCLICommand(ctx, req)
}

func (a *ModuleAdapter) GetIcon(ctx context.Context) (*modulepb.IconResponse, error) {
	return a.Mod.GetIcon(ctx)
}

// newPluginLogger creates a logrus logger for use within plugin processes.
// Plugin output goes to stderr which go-plugin captures and routes to the host.
func newPluginLogger() interface{} {
	// Import cycle prevention: we return interface{} to match Dependencies.Logger
	// The module casts it to *logrus.Logger
	logger := newLogrusLogger()
	return logger
}
