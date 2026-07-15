package cli

import (
	"context"
	"github.com/spf13/cobra"
	"net"
	"testing"

	daemonv1 "github.com/penguintechinc/penguin/api/proto/penguin/daemon/v1"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/test/bufconn"
)

// mockDaemonServer implements daemonv1.DaemonServer for testing.
type mockDaemonServer struct {
	daemonv1.UnimplementedDaemonServer
}

func (m *mockDaemonServer) ListCommands(ctx context.Context, req *daemonv1.ListCommandsRequest) (*daemonv1.ListCommandsResponse, error) {
	return &daemonv1.ListCommandsResponse{
		Modules: []*daemonv1.ModuleCommands{
			{
				Module: "test-module",
				Commands: []*daemonv1.CommandSpec{
					{
						Name:  "test-cmd",
						Short: "Test command",
						Flags: []*daemonv1.FlagSpec{
							{
								Name:  "verbose",
								Type:  "bool",
								Usage: "Verbose output",
							},
						},
						MinArgs: 0,
						MaxArgs: -1,
					},
				},
			},
		},
	}, nil
}

func (m *mockDaemonServer) Version(ctx context.Context, req *daemonv1.VersionRequest) (*daemonv1.VersionResponse, error) {
	return &daemonv1.VersionResponse{
		DaemonVersion: "1.0.0",
		ApiVersion:    "v1",
	}, nil
}

// TestBuilderBuildRoot verifies root command construction.
func TestBuilderBuildRoot(t *testing.T) {
	// Create mock gRPC server
	listener := bufconn.Listen(1024 * 1024)
	defer func() { _ = listener.Close() }()

	server := grpc.NewServer()
	daemonv1.RegisterDaemonServer(server, &mockDaemonServer{})

	go func() { _ = server.Serve(listener) }()
	defer server.Stop()

	// Create client connection to mock server
	dialOpts := []grpc.DialOption{
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithContextDialer(func(ctx context.Context, s string) (net.Conn, error) {
			return listener.Dial()
		}),
	}
	conn, err := grpc.NewClient("passthrough:///bufnet", dialOpts...)
	if err != nil {
		t.Fatalf("dial mock: %v", err)
	}
	defer func() { _ = conn.Close() }()

	builder := NewBuilder(conn)
	root, err := builder.BuildRoot(context.Background())
	if err != nil {
		t.Fatalf("BuildRoot failed: %v", err)
	}

	if root.Use != "penguin" {
		t.Errorf("expected use 'penguin', got %s", root.Use)
	}

	// The grammar is `penguin <module> <command>`: the module is the top-level
	// command and its commands hang off it.
	var moduleCmd *cobra.Command
	for _, cmd := range root.Commands() {
		if cmd.Name() == "test-module" {
			moduleCmd = cmd
			break
		}
	}
	if moduleCmd == nil {
		t.Fatal("expected module 'test-module' to be a top-level command")
	}

	foundTestCmd := false
	for _, cmd := range moduleCmd.Commands() {
		if cmd.Name() == "test-cmd" {
			foundTestCmd = true
			break
		}
	}
	if !foundTestCmd {
		t.Error("expected test-cmd under the test-module command")
	}
}

// TestBuilderCommandConstruction verifies flag parsing.
func TestBuilderCommandConstruction(t *testing.T) {
	listener := bufconn.Listen(1024 * 1024)
	defer func() { _ = listener.Close() }()

	server := grpc.NewServer()
	daemonv1.RegisterDaemonServer(server, &mockDaemonServer{})

	go func() { _ = server.Serve(listener) }()
	defer server.Stop()

	dialOpts := []grpc.DialOption{
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithContextDialer(func(ctx context.Context, s string) (net.Conn, error) {
			return listener.Dial()
		}),
	}
	conn, err := grpc.NewClient("passthrough:///bufnet", dialOpts...)
	if err != nil {
		t.Fatalf("dial mock: %v", err)
	}
	defer func() { _ = conn.Close() }()

	builder := NewBuilder(conn)

	// Build command from spec
	spec := &daemonv1.CommandSpec{
		Name:    "test",
		Short:   "Test command",
		Use:     "test [args]",
		MinArgs: 0,
		MaxArgs: 5,
		Flags: []*daemonv1.FlagSpec{
			{
				Name:  "name",
				Type:  "string",
				Usage: "Your name",
			},
			{
				Name:  "count",
				Type:  "int",
				Usage: "Count",
			},
			{
				Name:  "verbose",
				Type:  "bool",
				Usage: "Verbose",
			},
		},
	}

	cmd := builder.buildCommand("test-module", spec)

	if cmd.Use != "test" {
		t.Errorf("expected use 'test', got %s", cmd.Use)
	}

	// Check flags
	if cmd.Flags().Lookup("name") == nil {
		t.Error("expected --name flag")
	}
	if cmd.Flags().Lookup("count") == nil {
		t.Error("expected --count flag")
	}
	if cmd.Flags().Lookup("verbose") == nil {
		t.Error("expected --verbose flag")
	}
}

// TestBuilderArgValidation verifies argument validation.
func TestBuilderArgValidation(t *testing.T) {
	tests := []struct {
		name    string
		minArgs int32
		maxArgs int32
		args    []string
		wantErr bool
	}{
		{"no constraints", 0, -1, []string{"a", "b", "c"}, false},
		{"min constraint met", 2, -1, []string{"a", "b"}, false},
		{"min constraint not met", 2, -1, []string{"a"}, true},
		{"max constraint met", 0, 3, []string{"a", "b"}, false},
		{"max constraint exceeded", 0, 2, []string{"a", "b", "c"}, true},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			spec := &daemonv1.CommandSpec{
				Name:    "test",
				MinArgs: tt.minArgs,
				MaxArgs: tt.maxArgs,
			}

			listener := bufconn.Listen(1024 * 1024)
			defer func() { _ = listener.Close() }()

			server := grpc.NewServer()
			daemonv1.RegisterDaemonServer(server, &mockDaemonServer{})

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
			cmd := builder.buildCommand("test-module", spec)

			err := cmd.Args(cmd, tt.args)
			if (err != nil) != tt.wantErr {
				t.Errorf("Args validation: got error %v, wantErr %v", err, tt.wantErr)
			}
		})
	}
}

// TestBuilderRecursiveSubcommands verifies nested command construction.
func TestBuilderRecursiveSubcommands(t *testing.T) {
	listener := bufconn.Listen(1024 * 1024)
	defer func() { _ = listener.Close() }()

	server := grpc.NewServer()
	daemonv1.RegisterDaemonServer(server, &mockDaemonServer{})

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
		Name: "parent",
		Subcommands: []*daemonv1.CommandSpec{
			{
				Name:  "child1",
				Short: "First child",
			},
			{
				Name:  "child2",
				Short: "Second child",
			},
		},
	}

	cmd := builder.buildCommand("test-module", spec)

	// Check that subcommands were added
	subcommands := cmd.Commands()
	if len(subcommands) != 2 {
		t.Errorf("expected 2 subcommands, got %d", len(subcommands))
	}

	foundChild1 := false
	foundChild2 := false
	for _, sub := range subcommands {
		if sub.Name() == "child1" {
			foundChild1 = true
		}
		if sub.Name() == "child2" {
			foundChild2 = true
		}
	}

	if !foundChild1 {
		t.Error("expected child1 subcommand")
	}
	if !foundChild2 {
		t.Error("expected child2 subcommand")
	}
}
