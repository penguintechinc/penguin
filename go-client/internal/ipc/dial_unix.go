//go:build linux || darwin

package ipc

import (
	"context"
	"net"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
)

// Dial connects to the daemon's Unix domain socket at path.
//
// The transport is "insecure" in the TLS sense on purpose: the security
// boundary is the OS (socket permissions plus SO_PEERCRED checks in the
// daemon), not a certificate. ctx is honored by the dialer.
func Dial(ctx context.Context, path string) (*grpc.ClientConn, error) {
	var d net.Dialer
	return grpc.NewClient("passthrough:///"+path,
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithContextDialer(func(dialCtx context.Context, addr string) (net.Conn, error) {
			return d.DialContext(dialCtx, "unix", addr)
		}),
	)
}
