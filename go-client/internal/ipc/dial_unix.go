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
		// WithAuthority is required, not cosmetic. Without it grpc-go derives the
		// HTTP/2 :authority pseudo-header from the passthrough target, i.e. the
		// URL-escaped socket path ("%2Frun%2Fpenguin%2Fpenguind.sock"). That is
		// not a valid RFC 3986 authority, and a spec-strict HTTP/2 server rejects
		// the stream with PROTOCOL_ERROR before the request reaches any handler.
		// It went unnoticed because grpc-go's own server accepts it — the bug is
		// only visible against a different implementation.
		grpc.WithAuthority("localhost"),
		grpc.WithContextDialer(func(dialCtx context.Context, addr string) (net.Conn, error) {
			return d.DialContext(dialCtx, "unix", addr)
		}),
	)
}
