package cli

import (
	"context"
	"net"
	"testing"

	daemonv1 "github.com/penguintechinc/penguin/api/proto/penguin/daemon/v1"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/test/bufconn"
)

// Extended test to improve coverage of cli/builder.go

// multiModuleServer simulates a daemon with multiple modules.
type multiModuleServer struct {
	daemonv1.UnimplementedDaemonServer
}

func (m *multiModuleServer) ListCommands(ctx context.Context, req *daemonv1.ListCommandsRequest) (*daemonv1.ListCommandsResponse, error) {
	return &daemonv1.ListCommandsResponse{
		Modules: []*daemonv1.ModuleCommands{
			{
				Module: "module1",
				Commands: []*daemonv1.CommandSpec{
					{Name: "cmd1", Short: "Command 1"},
				},
			},
			{
				Module: "module2",
				Commands: []*daemonv1.CommandSpec{
					{Name: "cmd2", Short: "Command 2"},
				},
			},
		},
	}, nil
}

// TestBuilderBuildRootWithMultipleModules tests handling of multiple modules.
func TestBuilderBuildRootWithMultipleModules(t *testing.T) {
	listener := bufconn.Listen(1024 * 1024)
	defer func() { _ = listener.Close() }()

	server := grpc.NewServer()
	daemonv1.RegisterDaemonServer(server, &multiModuleServer{})

	go func() { _ = server.Serve(listener) }()
	defer server.Stop()

	dialOpts := []grpc.DialOption{
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithContextDialer(func(ctx context.Context, s string) (net.Conn, error) {
			return listener.Dial()
		}),
	}
	conn, _ := grpc.NewClient("passthrough:///bufnet", dialOpts...)
	defer func() { _ = conn.Close() }()

	builder := NewBuilder(conn)
	root, err := builder.BuildRoot(context.Background())
	if err != nil {
		t.Fatalf("BuildRoot failed: %v", err)
	}

	if root == nil {
		t.Error("expected non-nil root command")
	}
}

// emptyModuleServer simulates a daemon with no modules.
type emptyModuleServer struct {
	daemonv1.UnimplementedDaemonServer
}

func (m *emptyModuleServer) ListCommands(ctx context.Context, req *daemonv1.ListCommandsRequest) (*daemonv1.ListCommandsResponse, error) {
	return &daemonv1.ListCommandsResponse{Modules: []*daemonv1.ModuleCommands{}}, nil
}

// TestBuilderBuildRootEmptyModules tests handling of no modules.
func TestBuilderBuildRootEmptyModules(t *testing.T) {
	listener := bufconn.Listen(1024 * 1024)
	defer func() { _ = listener.Close() }()

	server := grpc.NewServer()
	daemonv1.RegisterDaemonServer(server, &emptyModuleServer{})

	go func() { _ = server.Serve(listener) }()
	defer server.Stop()

	dialOpts := []grpc.DialOption{
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithContextDialer(func(ctx context.Context, s string) (net.Conn, error) {
			return listener.Dial()
		}),
	}
	conn, _ := grpc.NewClient("passthrough:///bufnet", dialOpts...)
	defer func() { _ = conn.Close() }()

	builder := NewBuilder(conn)
	root, err := builder.BuildRoot(context.Background())
	if err != nil {
		t.Fatalf("BuildRoot failed: %v", err)
	}

	if root == nil {
		t.Error("expected non-nil root command")
	}
}

// flagServer simulates a daemon serving different flag types.
type flagServer struct {
	daemonv1.UnimplementedDaemonServer
}

func (m *flagServer) ListCommands(ctx context.Context, req *daemonv1.ListCommandsRequest) (*daemonv1.ListCommandsResponse, error) {
	return &daemonv1.ListCommandsResponse{}, nil
}

// TestBuilderFlagTypes tests different flag type handling.
func TestBuilderFlagTypes(t *testing.T) {
	listener := bufconn.Listen(1024 * 1024)
	defer func() { _ = listener.Close() }()

	server := grpc.NewServer()
	daemonv1.RegisterDaemonServer(server, &flagServer{})

	go func() { _ = server.Serve(listener) }()
	defer server.Stop()

	dialOpts := []grpc.DialOption{
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithContextDialer(func(ctx context.Context, s string) (net.Conn, error) {
			return listener.Dial()
		}),
	}
	conn, _ := grpc.NewClient("passthrough:///bufnet", dialOpts...)
	defer func() { _ = conn.Close() }()

	builder := NewBuilder(conn)
	spec := &daemonv1.CommandSpec{
		Name:  "test",
		Short: "Test",
		Flags: []*daemonv1.FlagSpec{
			{Name: "str", Type: "string", Default: "default"},
			{Name: "num", Type: "int", Default: "42"},
			{Name: "flag", Type: "bool", Default: "false"},
			{Name: "unknown", Type: "unknown-type", Default: ""},
		},
	}

	cmd := builder.buildCommand("test-module", spec)

	if cmd.Flags().Lookup("str") == nil {
		t.Error("expected --str flag")
	}
	if cmd.Flags().Lookup("num") == nil {
		t.Error("expected --num flag")
	}
	if cmd.Flags().Lookup("flag") == nil {
		t.Error("expected --flag flag")
	}
}

// slowServer simulates a slow daemon.
type slowServer struct {
	daemonv1.UnimplementedDaemonServer
}

func (m *slowServer) ListCommands(ctx context.Context, req *daemonv1.ListCommandsRequest) (*daemonv1.ListCommandsResponse, error) {
	// Simulate slow response
	<-ctx.Done()
	return nil, context.Canceled
}

// TestBuilderTimeout tests that BuildRoot handles context timeout gracefully.
func TestBuilderTimeout(t *testing.T) {
	listener := bufconn.Listen(1024 * 1024)
	defer func() { _ = listener.Close() }()

	server := grpc.NewServer()
	daemonv1.RegisterDaemonServer(server, &slowServer{})

	go func() { _ = server.Serve(listener) }()
	defer server.Stop()

	dialOpts := []grpc.DialOption{
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithContextDialer(func(ctx context.Context, s string) (net.Conn, error) {
			return listener.Dial()
		}),
	}
	conn, _ := grpc.NewClient("passthrough:///bufnet", dialOpts...)
	defer func() { _ = conn.Close() }()

	builder := NewBuilder(conn)

	// Create a context that times out immediately
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	_, err := builder.BuildRoot(ctx)
	if err == nil {
		t.Error("expected error on cancelled context")
	}
}
