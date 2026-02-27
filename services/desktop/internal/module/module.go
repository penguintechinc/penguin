package module

import (
	"context"

	"fyne.io/fyne/v2"
	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
	"github.com/spf13/cobra"
)

// HealthState represents module health.
type HealthState int

const (
	HealthUnknown HealthState = iota
	HealthHealthy
	HealthDegraded
	HealthUnhealthy
)

func (h HealthState) String() string {
	switch h {
	case HealthHealthy:
		return "healthy"
	case HealthDegraded:
		return "degraded"
	case HealthUnhealthy:
		return "unhealthy"
	default:
		return "unknown"
	}
}

// HealthStatus contains health check results.
type HealthStatus struct {
	State   HealthState
	Message string
	Details map[string]string
}

// Dependencies provides shared services to modules.
type Dependencies struct {
	ConfigDir  string
	DataDir    string
	AuthToken  string
	LicenseKey string
	Logger     interface{} // *logrus.Logger - avoid import cycle
}

// ModuleBase defines the core interface shared by all module types.
// Both in-process (legacy) and plugin modules implement this.
type ModuleBase interface {
	// Identity
	Name() string
	DisplayName() string
	Description() string
	Version() string

	// Lifecycle
	Init(ctx context.Context, deps Dependencies) error
	Start(ctx context.Context) error
	Stop(ctx context.Context) error

	// Health
	HealthCheck(ctx context.Context) HealthStatus
}

// PluginModule extends ModuleBase with declarative UI and CLI support.
// Plugin modules describe their UI as a widget tree (no Fyne imports)
// and their CLI as a command tree (no Cobra imports). The host renders both.
type PluginModule interface {
	ModuleBase

	// Declarative GUI — returns a widget tree describing the panel
	GetGUIPanel(ctx context.Context) (*modulepb.GUIPanel, error)
	// HandleGUIEvent processes a user interaction and returns updated panel
	HandleGUIEvent(ctx context.Context, event *modulepb.GUIEvent) (*modulepb.GUIPanel, error)

	// Declarative CLI — returns a command tree describing CLI commands
	GetCLICommands(ctx context.Context) (*modulepb.CLICommandList, error)
	// ExecuteCLICommand runs a CLI command and returns output
	ExecuteCLICommand(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error)

	// Icon returns PNG icon data (nil if no custom icon)
	GetIcon(ctx context.Context) (*modulepb.IconResponse, error)
}

// LegacyModule is the original Module interface with direct Fyne/Cobra dependencies.
// Kept for backward compatibility during migration; will be removed once all
// modules are converted to PluginModule.
type LegacyModule interface {
	ModuleBase

	// GUI - returns nil if module has no GUI or running headless
	GUIPanel() fyne.CanvasObject
	Icon() fyne.Resource

	// CLI
	CLICommands() []*cobra.Command
}

// Module is a union type — any module in the registry implements at least ModuleBase,
// and additionally either PluginModule or LegacyModule (or both during migration).
// This type alias preserves backward compatibility with existing code that
// references module.Module.
type Module = ModuleBase
