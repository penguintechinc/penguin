package sdk

import (
	"context"
	"fmt"
	"net/rpc"

	"github.com/hashicorp/go-plugin"
	v1 "github.com/penguintechinc/penguin/pkg/sdk/proto/penguin/sdk/v1"
	"google.golang.org/grpc"
)

const (
	// PluginHandshakeConfigMagicCookieKey is the magic cookie key for plugin handshakes.
	PluginHandshakeConfigMagicCookieKey = "PENGUIN_PLUGIN"
	// PluginHandshakeConfigMagicCookieValue is the value of the magic cookie.
	PluginHandshakeConfigMagicCookieValue = "penguin-sdk-v1"
	// PluginProtocolVersion is the protocol version for plugins.
	PluginProtocolVersion = 1
)

// Serve is the entry point for an external plugin. Call this from main() after
// constructing your Module implementation:
//
//	func main() {
//		sdk.Serve(&MyModule{})
//	}
//
// The function sets up hashicorp/go-plugin infrastructure, AutoMTLS,
// and bridges back to the daemon's HostServices via the plugin's gRPC broker.
// It never returns (plugins exit via signal).
func Serve(m Module) {
	plugin.Serve(&plugin.ServeConfig{
		HandshakeConfig: plugin.HandshakeConfig{
			ProtocolVersion:  PluginProtocolVersion,
			MagicCookieKey:   PluginHandshakeConfigMagicCookieKey,
			MagicCookieValue: PluginHandshakeConfigMagicCookieValue,
		},
		Plugins: map[string]plugin.Plugin{
			"module": &ModulePlugin{impl: m},
		},
		GRPCServer: plugin.DefaultGRPCServer,
	})
}

// ModulePlugin adapts a Module to the hashicorp/go-plugin Plugin interface.
// It implements both the legacy Plugin interface (for compatibility) and GRPCPlugin.
type ModulePlugin struct {
	impl Module
}

// GRPCServer implements plugin.GRPCPlugin.GRPCServer.
func (p *ModulePlugin) GRPCServer(broker *plugin.GRPCBroker, s *grpc.Server) error {
	v1.RegisterModuleServiceServer(s, &ModuleServiceImpl{m: p.impl})
	return nil
}

// GRPCClient implements plugin.GRPCPlugin.GRPCClient.
func (p *ModulePlugin) GRPCClient(ctx context.Context, broker *plugin.GRPCBroker, c *grpc.ClientConn) (interface{}, error) {
	// Dial the HostService (served by the daemon on the broker).
	hostConn, err := broker.Dial(HostServiceBrokerID)
	if err != nil {
		return nil, fmt.Errorf("dial host service: %w", err)
	}

	hostClient := v1.NewHostServiceClient(hostConn)
	hostServices := &HostServicesProxy{hostClient: hostClient}

	// Initialize the module with the proxied HostServices.
	if err := p.impl.Init(ctx, hostServices); err != nil {
		return nil, fmt.Errorf("init module: %w", err)
	}

	return p.impl, nil
}

// Client implements plugin.Plugin (required for interface compatibility).
func (p *ModulePlugin) Client(broker *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return nil, fmt.Errorf("unsupported: use GRPCClient")
}

// Server implements plugin.Plugin (required for interface compatibility).
func (p *ModulePlugin) Server(broker *plugin.MuxBroker) (interface{}, error) {
	return nil, fmt.Errorf("unsupported: use GRPCServer")
}

// HostServiceBrokerID is the broker ID for the daemon's HostService.
const HostServiceBrokerID uint32 = 1
