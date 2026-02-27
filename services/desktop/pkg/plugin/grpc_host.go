package plugin

import (
	"context"
	"encoding/json"
	"net/rpc"

	"github.com/hashicorp/go-plugin"
	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
)

// ModulePlugin implements plugin.Plugin for the module interface.
// It provides both net/rpc and gRPC support (using net/rpc under the hood
// for simplicity since we hand-write our types rather than using protoc).
type ModulePlugin struct {
	plugin.NetRPCUnsupportedPlugin
	// Impl is only set on the plugin (server) side.
	Impl modulepb.ModuleService
}

// Server returns the RPC server for the plugin side.
func (p *ModulePlugin) Server(*plugin.MuxBroker) (interface{}, error) {
	return &ModuleRPCServer{Impl: p.Impl}, nil
}

// Client returns the RPC client for the host side.
func (p *ModulePlugin) Client(b *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return &ModuleRPCClient{client: c}, nil
}

// ModuleRPCClient is the host-side RPC client that wraps a plugin
// connection and implements the ModuleService interface.
type ModuleRPCClient struct {
	client *rpc.Client
}

func (m *ModuleRPCClient) GetInfo(ctx context.Context) (*modulepb.ModuleInfo, error) {
	var resp modulepb.ModuleInfo
	err := m.client.Call("Plugin.GetInfo", new(interface{}), &resp)
	return &resp, err
}

func (m *ModuleRPCClient) Init(ctx context.Context, req *modulepb.InitRequest) (*modulepb.InitResponse, error) {
	var resp modulepb.InitResponse
	err := m.client.Call("Plugin.Init", req, &resp)
	return &resp, err
}

func (m *ModuleRPCClient) Start(ctx context.Context) error {
	var resp interface{}
	return m.client.Call("Plugin.Start", new(interface{}), &resp)
}

func (m *ModuleRPCClient) Stop(ctx context.Context) error {
	var resp interface{}
	return m.client.Call("Plugin.Stop", new(interface{}), &resp)
}

func (m *ModuleRPCClient) HealthCheck(ctx context.Context) (*modulepb.HealthCheckResponse, error) {
	var resp modulepb.HealthCheckResponse
	err := m.client.Call("Plugin.HealthCheck", new(interface{}), &resp)
	return &resp, err
}

func (m *ModuleRPCClient) GetGUIPanel(ctx context.Context) (*modulepb.GUIPanel, error) {
	var resp modulepb.GUIPanel
	err := m.client.Call("Plugin.GetGUIPanel", new(interface{}), &resp)
	return &resp, err
}

func (m *ModuleRPCClient) HandleGUIEvent(ctx context.Context, event *modulepb.GUIEvent) (*modulepb.GUIPanel, error) {
	var resp modulepb.GUIPanel
	err := m.client.Call("Plugin.HandleGUIEvent", event, &resp)
	return &resp, err
}

func (m *ModuleRPCClient) GetCLICommands(ctx context.Context) (*modulepb.CLICommandList, error) {
	var resp modulepb.CLICommandList
	err := m.client.Call("Plugin.GetCLICommands", new(interface{}), &resp)
	return &resp, err
}

func (m *ModuleRPCClient) ExecuteCLICommand(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
	var resp modulepb.CLICommandResponse
	err := m.client.Call("Plugin.ExecuteCLICommand", req, &resp)
	return &resp, err
}

func (m *ModuleRPCClient) GetIcon(ctx context.Context) (*modulepb.IconResponse, error) {
	var resp modulepb.IconResponse
	err := m.client.Call("Plugin.GetIcon", new(interface{}), &resp)
	return &resp, err
}

// marshalJSON is a helper that serializes a value to JSON bytes.
// Used internally for RPC transport of complex types.
func marshalJSON(v interface{}) ([]byte, error) {
	return json.Marshal(v)
}

// unmarshalJSON is a helper that deserializes JSON bytes into a value.
func unmarshalJSON(data []byte, v interface{}) error {
	return json.Unmarshal(data, v)
}
