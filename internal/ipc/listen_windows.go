//go:build windows

package ipc

import (
	"context"
	"fmt"
	"net"

	"github.com/Microsoft/go-winio"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
)

// ListenerConfig configures a named pipe listener (Windows).
type ListenerConfig struct {
	// Path is unused on Windows; the named pipe is always \\.\pipe\penguind
	Path string
	// AllowedGroup is unused on Windows
	AllowedGroup string
}

// Listen creates and returns a Windows named pipe listener.
// The pipe is created with SDDL allowing Builtin Administrators and SYSTEM.
// It returns a net.Listener and a grpc.ServerOption (insecure creds).
func Listen(cfg ListenerConfig) (net.Listener, grpc.ServerOption, error) {
	pipePath := `\\.\pipe\penguind`

	// SDDL: allow BA (Builtin Administrators) and SY (SYSTEM) full access
	sddl := "D:P(A;;GA;;;BA)(A;;GA;;;SY)"

	listener, err := winio.ListenPipe(pipePath, &winio.PipeConfig{
		SecurityDescriptor: sddl,
	})
	if err != nil {
		return nil, nil, fmt.Errorf("listen pipe: %w", err)
	}

	// Return insecure credentials for Windows (transport is OS boundary)
	return listener, grpc.Creds(insecure.NewCredentials()), nil
}

// PeerAuthInterceptor returns no-op interceptors for Windows.
// On Windows, access control is handled by the named pipe SDDL (Discretionary Access Control List),
// so additional peer authentication is not needed (unlike Unix socket SO_PEERCRED checks).
func PeerAuthInterceptor(allowedGroup string) (grpc.UnaryServerInterceptor, grpc.StreamServerInterceptor) {
	unary := func(ctx context.Context, req interface{}, info *grpc.UnaryServerInfo, handler grpc.UnaryHandler) (interface{}, error) {
		return handler(ctx, req)
	}
	stream := func(srv interface{}, ss grpc.ServerStream, info *grpc.StreamServerInfo, handler grpc.StreamHandler) error {
		return handler(srv, ss)
	}
	return unary, stream
}
