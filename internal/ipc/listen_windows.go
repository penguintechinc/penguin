//go:build windows

package ipc

import (
	"fmt"
	"net"

	"github.com/Microsoft/go-winio"
	"google.golang.org/grpc"
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
	return listener, grpc.WithInsecure(), nil
}
