package plugin

import (
	"context"

	"github.com/hashicorp/go-plugin"
	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
)

// ModuleRPCServer is the plugin-side RPC server that wraps
// a module implementation and exposes it over RPC.
type ModuleRPCServer struct {
	Impl modulepb.ModuleService
}

func (s *ModuleRPCServer) GetInfo(_ interface{}, resp *modulepb.ModuleInfo) error {
	info, err := s.Impl.GetInfo(context.Background())
	if err != nil {
		return err
	}
	*resp = *info
	return nil
}

func (s *ModuleRPCServer) Init(req *modulepb.InitRequest, resp *modulepb.InitResponse) error {
	result, err := s.Impl.Init(context.Background(), req)
	if err != nil {
		return err
	}
	*resp = *result
	return nil
}

func (s *ModuleRPCServer) Start(_ interface{}, _ *interface{}) error {
	return s.Impl.Start(context.Background())
}

func (s *ModuleRPCServer) Stop(_ interface{}, _ *interface{}) error {
	return s.Impl.Stop(context.Background())
}

func (s *ModuleRPCServer) HealthCheck(_ interface{}, resp *modulepb.HealthCheckResponse) error {
	result, err := s.Impl.HealthCheck(context.Background())
	if err != nil {
		return err
	}
	*resp = *result
	return nil
}

func (s *ModuleRPCServer) GetGUIPanel(_ interface{}, resp *modulepb.GUIPanel) error {
	panel, err := s.Impl.GetGUIPanel(context.Background())
	if err != nil {
		return err
	}
	*resp = *panel
	return nil
}

func (s *ModuleRPCServer) HandleGUIEvent(event *modulepb.GUIEvent, resp *modulepb.GUIPanel) error {
	panel, err := s.Impl.HandleGUIEvent(context.Background(), event)
	if err != nil {
		return err
	}
	*resp = *panel
	return nil
}

func (s *ModuleRPCServer) GetCLICommands(_ interface{}, resp *modulepb.CLICommandList) error {
	list, err := s.Impl.GetCLICommands(context.Background())
	if err != nil {
		return err
	}
	*resp = *list
	return nil
}

func (s *ModuleRPCServer) ExecuteCLICommand(req *modulepb.CLICommandRequest, resp *modulepb.CLICommandResponse) error {
	result, err := s.Impl.ExecuteCLICommand(context.Background(), req)
	if err != nil {
		return err
	}
	*resp = *result
	return nil
}

func (s *ModuleRPCServer) GetIcon(_ interface{}, resp *modulepb.IconResponse) error {
	icon, err := s.Impl.GetIcon(context.Background())
	if err != nil {
		return err
	}
	*resp = *icon
	return nil
}

// Serve starts the plugin server. Call this from plugin main() functions.
func Serve(impl modulepb.ModuleService) {
	plugin.Serve(&plugin.ServeConfig{
		HandshakeConfig: Handshake,
		Plugins: map[string]plugin.Plugin{
			PluginName: &ModulePlugin{Impl: impl},
		},
	})
}
