package extplugin

import (
	"context"
	"fmt"
	"net/rpc"

	"github.com/hashicorp/go-plugin"
	sdkv1 "github.com/penguintechinc/penguin/pkg/sdk/proto/penguin/sdk/v1"
	"github.com/penguintechinc/penguin/pkg/sdk"
	"google.golang.org/grpc"
)

// clientModulePlugin adapts go-plugin to the Module interface.
type clientModulePlugin struct {
	hostServices sdk.HostServices
}

// Implement plugin.Plugin.Client (stub).
func (p *clientModulePlugin) Client(broker *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return nil, fmt.Errorf("unsupported: use GRPCClient")
}

// Implement plugin.Plugin.Server (stub).
func (p *clientModulePlugin) Server(broker *plugin.MuxBroker) (interface{}, error) {
	return nil, fmt.Errorf("unsupported: use GRPCServer")
}

// GRPCClient returns a wrapper that adapts the gRPC client to sdk.Module.
func (p *clientModulePlugin) GRPCClient(ctx context.Context, broker *plugin.GRPCBroker, c *grpc.ClientConn) (interface{}, error) {
	// Serve the HostService to the plugin.
	hostServiceImpl := NewHostServiceImpl(p.hostServices)

	// Dial the HostService from the plugin's broker.
	// The plugin will call us on broker ID sdk.HostServiceBrokerID (1).
	const hostServiceBrokerID uint32 = 1
	listener, err := broker.Accept(hostServiceBrokerID)
	if err != nil {
		return nil, fmt.Errorf("failed to accept host service connection: %w", err)
	}

	// Start a gRPC server for the HostService on the listener.
	s := grpc.NewServer()
	sdkv1.RegisterHostServiceServer(s, hostServiceImpl)
	go func() {
		_ = s.Serve(listener)
	}()

	// Return a wrapper that adapts the gRPC client to sdk.Module.
	moduleClient := sdkv1.NewModuleServiceClient(c)
	return &moduleClientAdapter{grpcClient: moduleClient, broker: broker}, nil
}

// GRPCServer is not used on the client side.
func (p *clientModulePlugin) GRPCServer(broker *plugin.GRPCBroker, s *grpc.Server) error {
	return fmt.Errorf("server not used in client plugin")
}
