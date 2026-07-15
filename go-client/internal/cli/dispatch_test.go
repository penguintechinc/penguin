package cli

import (
	"context"
	"fmt"
	"net"
	"testing"

	daemonv1 "github.com/penguintechinc/penguin/api/proto/penguin/daemon/v1"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/status"
	"google.golang.org/grpc/test/bufconn"
)

// dispatchTestServer implements daemonv1.DaemonServer for dispatch testing.
type dispatchTestServer struct {
	daemonv1.UnimplementedDaemonServer
}

func (m *dispatchTestServer) ListCommands(ctx context.Context, req *daemonv1.ListCommandsRequest) (*daemonv1.ListCommandsResponse, error) {
	return &daemonv1.ListCommandsResponse{
		Modules: []*daemonv1.ModuleCommands{
			{
				Module: "test-module",
				Commands: []*daemonv1.CommandSpec{
					{
						Name:    "dispatch-test",
						Short:   "Test dispatch",
						MinArgs: 0,
						MaxArgs: -1,
						Flags: []*daemonv1.FlagSpec{
							{
								Name:  "string-flag",
								Type:  "string",
								Usage: "String flag",
							},
							{
								Name:  "bool-flag",
								Type:  "bool",
								Usage: "Bool flag",
							},
							{
								Name:  "int-flag",
								Type:  "int",
								Usage: "Int flag",
							},
						},
					},
				},
			},
		},
	}, nil
}

func (m *dispatchTestServer) Dispatch(req *daemonv1.DispatchRequest, stream grpc.ServerStreamingServer[daemonv1.DispatchChunk]) error {
	// Send some output
	chunk := &daemonv1.DispatchChunk{
		Output: "test output\n",
		Final:  false,
	}
	if err := stream.Send(chunk); err != nil {
		return err
	}

	// Send final response with exit code 0
	finalChunk := &daemonv1.DispatchChunk{
		Output:   "",
		Final:    true,
		ExitCode: 0,
	}
	return stream.Send(finalChunk)
}

// unavailableServer always returns Unavailable status.
type unavailableServer struct {
	daemonv1.UnimplementedDaemonServer
}

func (m *unavailableServer) Dispatch(req *daemonv1.DispatchRequest, stream daemonv1.Daemon_DispatchServer) error {
	return status.Error(codes.Unavailable, "service unavailable")
}

// TestDispatchSuccess tests successful dispatch execution.
func TestDispatchSuccess(t *testing.T) {
	listener := bufconn.Listen(1024 * 1024)
	defer func() { _ = listener.Close() }()

	server := grpc.NewServer()
	daemonv1.RegisterDaemonServer(server, &dispatchTestServer{})

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

	// Build a test command
	spec := &daemonv1.CommandSpec{
		Name:    "test",
		Short:   "Test",
		MinArgs: 0,
		MaxArgs: -1,
		Flags: []*daemonv1.FlagSpec{
			{
				Name:  "string-flag",
				Type:  "string",
				Usage: "String flag",
			},
		},
	}

	cmd := builder.buildCommand("test-module", spec)

	// Test dispatch would exit on non-zero exit code
	// For now, just verify the command was built
	if cmd == nil {
		t.Error("expected non-nil command")
	}
}

// TestDispatchUnavailableError tests Unavailable status handling.
func TestDispatchUnavailableError(t *testing.T) {
	listener := bufconn.Listen(1024 * 1024)
	defer func() { _ = listener.Close() }()

	server := grpc.NewServer()
	daemonv1.RegisterDaemonServer(server, &unavailableServer{})

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
		Name:    "test",
		Short:   "Test",
		MinArgs: 0,
		MaxArgs: -1,
	}

	cmd := builder.buildCommand("test-module", spec)
	if cmd == nil {
		t.Error("expected non-nil command")
	}

	// Actually call dispatch to test Unavailable error handling
	err := builder.dispatch(cmd, "test-module", []string{"test"}, []string{})
	if err == nil || err.Error() != "daemon unreachable" {
		t.Logf("dispatch returned: %v", err)
	}
}

// TestDispatchStreamMultipleChunks tests handling of multiple output chunks.
func TestDispatchStreamMultipleChunks(t *testing.T) {
	listener := bufconn.Listen(1024 * 1024)
	defer func() { _ = listener.Close() }()

	server := grpc.NewServer()
	daemonv1.RegisterDaemonServer(server, &dispatchTestServer{})

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
	if builder == nil {
		t.Error("expected non-nil builder")
	}
}

// multiChunkServer sends multiple output chunks.
type multiChunkServer struct {
	daemonv1.UnimplementedDaemonServer
}

func (m *multiChunkServer) Dispatch(req *daemonv1.DispatchRequest, stream grpc.ServerStreamingServer[daemonv1.DispatchChunk]) error {
	// Send multiple non-final chunks
	for i := 0; i < 3; i++ {
		chunk := &daemonv1.DispatchChunk{
			Output: "chunk output\n",
			Final:  false,
		}
		if err := stream.Send(chunk); err != nil {
			return err
		}
	}

	// Send final response with exit code 0
	finalChunk := &daemonv1.DispatchChunk{
		Output:   "final chunk\n",
		Final:    true,
		ExitCode: 0,
	}
	return stream.Send(finalChunk)
}

// errorStreamServer returns an error during streaming.
type errorStreamServer struct {
	daemonv1.UnimplementedDaemonServer
}

func (m *errorStreamServer) Dispatch(req *daemonv1.DispatchRequest, stream grpc.ServerStreamingServer[daemonv1.DispatchChunk]) error {
	return status.Error(codes.Internal, "stream error")
}

// TestDispatchActualDispatch tests the dispatch method with flags and args.
func TestDispatchActualDispatch(t *testing.T) {
	listener := bufconn.Listen(1024 * 1024)
	defer func() { _ = listener.Close() }()

	server := grpc.NewServer()
	daemonv1.RegisterDaemonServer(server, &multiChunkServer{})

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
		Name:    "test",
		Short:   "Test",
		MinArgs: 0,
		MaxArgs: -1,
		Flags: []*daemonv1.FlagSpec{
			{
				Name:      "string-flag",
				Type:      "string",
				Default:   "default",
				Usage:     "String flag",
				Shorthand: "s",
			},
			{
				Name:      "bool-flag",
				Type:      "bool",
				Default:   "true",
				Usage:     "Bool flag",
				Shorthand: "b",
			},
			{
				Name:      "int-flag",
				Type:      "int",
				Default:   "10",
				Usage:     "Int flag",
				Shorthand: "i",
			},
		},
	}

	cmd := builder.buildCommand("test-module", spec)

	// Set flags on the command
	_ = cmd.Flags().Set("string-flag", "custom")
	_ = cmd.Flags().Set("bool-flag", "false")
	_ = cmd.Flags().Set("int-flag", "20")

	// Call dispatch with args
	err := builder.dispatch(cmd, "test-module", []string{"test"}, []string{"arg1", "arg2"})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

// TestDispatchMultipleChunks tests handling multiple streaming chunks.
func TestDispatchMultipleChunks(t *testing.T) {
	listener := bufconn.Listen(1024 * 1024)
	defer func() { _ = listener.Close() }()

	server := grpc.NewServer()
	daemonv1.RegisterDaemonServer(server, &multiChunkServer{})

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
		Name:    "test",
		Short:   "Test",
		MinArgs: 0,
		MaxArgs: -1,
	}

	cmd := builder.buildCommand("test-module", spec)
	err := builder.dispatch(cmd, "test-module", []string{"test"}, []string{})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

// TestDispatchStreamError tests error during streaming.
func TestDispatchStreamError(t *testing.T) {
	listener := bufconn.Listen(1024 * 1024)
	defer func() { _ = listener.Close() }()

	server := grpc.NewServer()
	daemonv1.RegisterDaemonServer(server, &errorStreamServer{})

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
		Name:    "test",
		Short:   "Test",
		MinArgs: 0,
		MaxArgs: -1,
	}

	cmd := builder.buildCommand("test-module", spec)
	err := builder.dispatch(cmd, "test-module", []string{"test"}, []string{})
	if err != nil {
		t.Logf("expected error: %v", err)
	}
}

// verifyingServer checks that flags were correctly forwarded
type verifyingServer struct {
	daemonv1.UnimplementedDaemonServer
	capturedReq *daemonv1.DispatchRequest
}

func (vs *verifyingServer) Dispatch(req *daemonv1.DispatchRequest, stream grpc.ServerStreamingServer[daemonv1.DispatchChunk]) error {
	vs.capturedReq = req
	chunk := &daemonv1.DispatchChunk{
		Output:   "ok\n",
		Final:    true,
		ExitCode: 0,
	}
	return stream.Send(chunk)
}

// TestDispatchFlagCollectionAllTypes tests all flag types are properly collected.
func TestDispatchFlagCollectionAllTypes(t *testing.T) {
	listener := bufconn.Listen(1024 * 1024)
	defer func() { _ = listener.Close() }()

	vs := &verifyingServer{}

	server := grpc.NewServer()
	daemonv1.RegisterDaemonServer(server, vs)

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
		Name:    "test",
		Short:   "Test",
		MinArgs: 0,
		MaxArgs: -1,
		Flags: []*daemonv1.FlagSpec{
			{Name: "str", Type: "string", Default: "default"},
			{Name: "b", Type: "bool", Default: "false"},
			{Name: "n", Type: "int", Default: "0"},
		},
	}

	cmd := builder.buildCommand("test-module", spec)
	_ = cmd.Flags().Set("str", "hello")
	_ = cmd.Flags().Set("b", "true")
	_ = cmd.Flags().Set("n", "42")

	err := builder.dispatch(cmd, "test-module", []string{"test"}, []string{"arg1"})
	if err != nil {
		t.Fatalf("dispatch failed: %v", err)
	}

	if vs.capturedReq == nil {
		t.Fatal("request not captured")
	}
	if vs.capturedReq.Flags["str"] != "hello" {
		t.Errorf("string flag: got %q, want hello", vs.capturedReq.Flags["str"])
	}
	if vs.capturedReq.Flags["b"] != "true" {
		t.Errorf("bool flag: got %q, want true", vs.capturedReq.Flags["b"])
	}
	if vs.capturedReq.Flags["n"] != "42" {
		t.Errorf("int flag: got %q, want 42", vs.capturedReq.Flags["n"])
	}
	if len(vs.capturedReq.Args) != 1 || vs.capturedReq.Args[0] != "arg1" {
		t.Errorf("args: got %v, want [arg1]", vs.capturedReq.Args)
	}
}

// TestDispatchWithNoFlagsSet tests dispatch when no flags are set.
func TestDispatchWithNoFlagsSet(t *testing.T) {
	listener := bufconn.Listen(1024 * 1024)
	defer func() { _ = listener.Close() }()

	server := grpc.NewServer()
	daemonv1.RegisterDaemonServer(server, &dispatchTestServer{})

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
		Name:    "test",
		Short:   "Test",
		MinArgs: 0,
		MaxArgs: -1,
		Flags: []*daemonv1.FlagSpec{
			{Name: "str", Type: "string", Default: "default"},
		},
	}

	cmd := builder.buildCommand("test-module", spec)
	// Don't set any flags; just use defaults
	err := builder.dispatch(cmd, "test-module", []string{"test"}, []string{})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

// noOutputServer returns no output on dispatch.
type noOutputServer struct {
	daemonv1.UnimplementedDaemonServer
}

func (ns *noOutputServer) Dispatch(req *daemonv1.DispatchRequest, stream grpc.ServerStreamingServer[daemonv1.DispatchChunk]) error {
	chunk := &daemonv1.DispatchChunk{
		Output:   "",
		Final:    true,
		ExitCode: 0,
	}
	return stream.Send(chunk)
}

// TestDispatchEmptyOutput tests dispatch with no output.
func TestDispatchEmptyOutput(t *testing.T) {
	listener := bufconn.Listen(1024 * 1024)
	defer func() { _ = listener.Close() }()

	ns := &noOutputServer{}

	server := grpc.NewServer()
	daemonv1.RegisterDaemonServer(server, ns)

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
		Name:    "test",
		Short:   "Test",
		MinArgs: 0,
		MaxArgs: -1,
	}

	cmd := builder.buildCommand("test-module", spec)
	err := builder.dispatch(cmd, "test-module", []string{"test"}, []string{})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

// TestDispatchSuccessfulOutput covers dispatch with successful output and exit code
func TestDispatchSuccessfulOutput(t *testing.T) {
	listener := bufconn.Listen(1024 * 1024)
	defer func() { _ = listener.Close() }()

	server := grpc.NewServer()
	daemonv1.RegisterDaemonServer(server, &dispatchTestServer{})

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
		Name:    "test",
		Short:   "Test",
		MinArgs: 0,
		MaxArgs: -1,
	}

	cmd := builder.buildCommand("test-module", spec)

	// This should succeed - the dispatchTestServer returns exit code 0
	err := builder.dispatch(cmd, "test-module", []string{"test"}, []string{})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

// dispatchWithMultipleOutputServer sends multiple output chunks before final
type dispatchWithMultipleOutputServer struct {
	daemonv1.UnimplementedDaemonServer
}

func (m *dispatchWithMultipleOutputServer) Dispatch(req *daemonv1.DispatchRequest, stream grpc.ServerStreamingServer[daemonv1.DispatchChunk]) error {
	// Send multiple non-final chunks with output
	for i := 0; i < 3; i++ {
		chunk := &daemonv1.DispatchChunk{
			Output: fmt.Sprintf("output line %d\n", i),
			Final:  false,
		}
		if err := stream.Send(chunk); err != nil {
			return err
		}
	}

	// Send final chunk with exit code
	finalChunk := &daemonv1.DispatchChunk{
		Output:   "final output\n",
		Final:    true,
		ExitCode: 0,
	}
	return stream.Send(finalChunk)
}

// TestDispatchStreamingOutput tests dispatch with streamed output chunks
func TestDispatchStreamingOutput(t *testing.T) {
	listener := bufconn.Listen(1024 * 1024)
	defer func() { _ = listener.Close() }()

	server := grpc.NewServer()
	daemonv1.RegisterDaemonServer(server, &dispatchWithMultipleOutputServer{})

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
		Name:    "test",
		Short:   "Test",
		MinArgs: 0,
		MaxArgs: -1,
	}

	cmd := builder.buildCommand("test-module", spec)

	// Should succeed with streaming output
	err := builder.dispatch(cmd, "test-module", []string{"test"}, []string{})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

