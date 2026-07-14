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

// Dial connects to the penguind named pipe on Windows.
// Returns a gRPC ClientConn using insecure transport (no TLS).
func Dial(ctx context.Context, path string) (*grpc.ClientConn, error) {
	pipePath := `\\.\pipe\penguind`

	return grpc.DialContext(ctx, "pipe:"+pipePath,
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithContextDialer(func(ctx context.Context, addr string) (net.Conn, error) {
			return winio.DialPipeContext(ctx, pipePath)
		}),
	)
}
