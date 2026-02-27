package pluginhost

import (
	"context"
	"fmt"

	"github.com/penguintechinc/penguin/services/desktop/internal/module"
	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
)

// PluginModuleWrapper wraps a remote plugin connection as a module.PluginModule.
// This allows the registry to treat plugin processes the same as in-process modules.
type PluginModuleWrapper struct {
	managed *ManagedPlugin
}

// NewPluginModuleWrapper creates a wrapper around a managed plugin process.
func NewPluginModuleWrapper(mp *ManagedPlugin) *PluginModuleWrapper {
	return &PluginModuleWrapper{managed: mp}
}

// --- ModuleBase interface ---

func (w *PluginModuleWrapper) Name() string {
	if w.managed.Info != nil {
		return w.managed.Info.Name
	}
	return w.managed.Name
}

func (w *PluginModuleWrapper) DisplayName() string {
	if w.managed.Info != nil {
		return w.managed.Info.DisplayName
	}
	return w.managed.Name
}

func (w *PluginModuleWrapper) Description() string {
	if w.managed.Info != nil {
		return w.managed.Info.Description
	}
	return ""
}

func (w *PluginModuleWrapper) Version() string {
	if w.managed.Info != nil {
		return w.managed.Info.Version
	}
	return "unknown"
}

func (w *PluginModuleWrapper) Init(ctx context.Context, deps module.Dependencies) error {
	resp, err := w.managed.Service.Init(ctx, &modulepb.InitRequest{
		ConfigDir:  deps.ConfigDir,
		DataDir:    deps.DataDir,
		AuthToken:  deps.AuthToken,
		LicenseKey: deps.LicenseKey,
	})
	if err != nil {
		return fmt.Errorf("plugin init RPC: %w", err)
	}
	if !resp.OK {
		return fmt.Errorf("plugin init failed: %s", resp.Error)
	}
	return nil
}

func (w *PluginModuleWrapper) Start(ctx context.Context) error {
	return w.managed.Service.Start(ctx)
}

func (w *PluginModuleWrapper) Stop(ctx context.Context) error {
	return w.managed.Service.Stop(ctx)
}

func (w *PluginModuleWrapper) HealthCheck(ctx context.Context) module.HealthStatus {
	resp, err := w.managed.Service.HealthCheck(ctx)
	if err != nil {
		return module.HealthStatus{
			State:   module.HealthUnhealthy,
			Message: fmt.Sprintf("health check RPC failed: %v", err),
		}
	}
	return module.HealthStatus{
		State:   module.HealthState(resp.State),
		Message: resp.Message,
		Details: resp.Details,
	}
}

// --- PluginModule interface ---

func (w *PluginModuleWrapper) GetGUIPanel(ctx context.Context) (*modulepb.GUIPanel, error) {
	return w.managed.Service.GetGUIPanel(ctx)
}

func (w *PluginModuleWrapper) HandleGUIEvent(ctx context.Context, event *modulepb.GUIEvent) (*modulepb.GUIPanel, error) {
	return w.managed.Service.HandleGUIEvent(ctx, event)
}

func (w *PluginModuleWrapper) GetCLICommands(ctx context.Context) (*modulepb.CLICommandList, error) {
	return w.managed.Service.GetCLICommands(ctx)
}

func (w *PluginModuleWrapper) ExecuteCLICommand(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
	return w.managed.Service.ExecuteCLICommand(ctx, req)
}

func (w *PluginModuleWrapper) GetIcon(ctx context.Context) (*modulepb.IconResponse, error) {
	return w.managed.Service.GetIcon(ctx)
}
