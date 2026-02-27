package modulepb

import "context"

// ModuleService defines the interface that plugin modules implement.
// This mirrors the gRPC ModuleService defined in the proto file.
// The go-plugin framework routes calls over gRPC between host and plugin processes.
type ModuleService interface {
	GetInfo(ctx context.Context) (*ModuleInfo, error)
	Init(ctx context.Context, req *InitRequest) (*InitResponse, error)
	Start(ctx context.Context) error
	Stop(ctx context.Context) error
	HealthCheck(ctx context.Context) (*HealthCheckResponse, error)
	GetGUIPanel(ctx context.Context) (*GUIPanel, error)
	HandleGUIEvent(ctx context.Context, event *GUIEvent) (*GUIPanel, error)
	GetCLICommands(ctx context.Context) (*CLICommandList, error)
	ExecuteCLICommand(ctx context.Context, req *CLICommandRequest) (*CLICommandResponse, error)
	GetIcon(ctx context.Context) (*IconResponse, error)
}
