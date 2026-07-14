//go:build linux || darwin

package ipc

import (
	"context"
	"fmt"
	"net"
	"os"
	"os/user"
	"path/filepath"
	"strconv"

	"golang.org/x/sys/unix"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/credentials"
	"google.golang.org/grpc/peer"
	"google.golang.org/grpc/status"
)

// ListenerConfig configures a Unix socket listener.
type ListenerConfig struct {
	// Path is the socket file path.
	Path string
	// AllowedGroup is the OS group name allowed to connect (e.g., "penguin").
	// Users must be members of this group or uid 0 to connect.
	AllowedGroup string
}

// Listen creates and returns a Unix domain socket listener on the given path.
// It removes any stale socket at that path, creates the parent directory with 0755
// permissions, and sets the socket to 0660 permissions.
// It returns a *net.UnixListener and a grpc.ServerOption for peer credential verification.
// maxUnixPath is the portable limit for sun_path (108 on Linux, 104 on
// darwin). Exceeding it fails deep in bind(2) with a bare "invalid argument",
// so check it up front and say what is actually wrong.
const maxUnixPath = 103

func Listen(cfg ListenerConfig) (net.Listener, grpc.ServerOption, error) {
	if len(cfg.Path) > maxUnixPath {
		return nil, nil, fmt.Errorf("socket path %q is %d bytes; the OS limit is %d — use a shorter path",
			cfg.Path, len(cfg.Path), maxUnixPath)
	}

	// Remove stale socket if it exists
	if err := os.Remove(cfg.Path); err != nil && !os.IsNotExist(err) {
		return nil, nil, fmt.Errorf("remove stale socket: %w", err)
	}

	// Create parent directory if needed
	dir := filepath.Dir(cfg.Path)
	if err := os.MkdirAll(dir, 0o750); err != nil {
		return nil, nil, fmt.Errorf("mkdir parent: %w", err)
	}

	// Listen on Unix socket
	addr := net.UnixAddr{Name: cfg.Path, Net: "unix"}
	listener, err := net.ListenUnix("unix", &addr)
	if err != nil {
		return nil, nil, fmt.Errorf("listen unix: %w", err)
	}

	// 0660 is deliberate: the socket is the daemon's authorization boundary
	// (root + members of AllowedGroup). Peer identity is re-checked per RPC
	// via SO_PEERCRED in checkPeerAuth.
	if err := os.Chmod(cfg.Path, 0o660); err != nil { // #nosec G302 -- group access is the documented design
		_ = listener.Close()
		return nil, nil, fmt.Errorf("chmod socket: %w", err)
	}

	// Create peer credential interceptors
	opt := grpc.Creds(newPeerCredentials(cfg.AllowedGroup))

	return listener, opt, nil
}

// PeerCreds holds peer connection credentials extracted from SO_PEERCRED.
type PeerCreds struct {
	UID int
	GID int
	PID int
}

// peerCredentials is a custom credentials.TransportCredentials that captures
// peer credentials from Unix socket connections.
type peerCredentials struct {
	allowedGroup string
}

// newPeerCredentials creates peer credential capturing transport.
func newPeerCredentials(allowedGroup string) credentials.TransportCredentials {
	return &peerCredentials{allowedGroup: allowedGroup}
}

func (pc *peerCredentials) ClientHandshake(context.Context, string, net.Conn) (net.Conn, credentials.AuthInfo, error) {
	return nil, nil, fmt.Errorf("unix socket should not be used as client credentials")
}

func (pc *peerCredentials) ServerHandshake(conn net.Conn) (net.Conn, credentials.AuthInfo, error) {
	unixConn, ok := conn.(*net.UnixConn)
	if !ok {
		return conn, nil, nil
	}

	// Extract peer credentials using SO_PEERCRED
	raw, err := unixConn.SyscallConn()
	if err != nil {
		return unixConn, nil, fmt.Errorf("get syscall conn: %w", err)
	}

	var creds *unix.Ucred
	var ucredErr error
	if err := raw.Control(func(fd uintptr) {
		creds, ucredErr = unix.GetsockoptUcred(int(fd), unix.SOL_SOCKET, unix.SO_PEERCRED)
	}); err != nil {
		return unixConn, nil, fmt.Errorf("control: %w", err)
	}
	if ucredErr != nil {
		return unixConn, nil, fmt.Errorf("getpeercred: %w", ucredErr)
	}

	peerCreds := &PeerCreds{
		UID: int(creds.Uid),
		GID: int(creds.Gid),
		PID: int(creds.Pid),
	}

	return unixConn, peerCreds, nil
}

func (pc *peerCredentials) Info() credentials.ProtocolInfo {
	return credentials.ProtocolInfo{SecurityProtocol: "unix-peercred"}
}

func (pc *peerCredentials) Clone() credentials.TransportCredentials {
	return &peerCredentials{allowedGroup: pc.allowedGroup}
}

func (pc *peerCredentials) OverrideServerName(string) error {
	return nil
}

// AuthInfo implementation for PeerCreds
func (pc *PeerCreds) AuthType() string {
	return "unix-peercred"
}

// PeerAuthInterceptor returns a grpc.UnaryServerInterceptor and StreamServerInterceptor
// that verify peer credentials against the allowed group.
// For testing, you can pass a custom groupCheckFn; if nil, uses default os/user.LookupGroup.
func PeerAuthInterceptor(allowedGroup string) (grpc.UnaryServerInterceptor, grpc.StreamServerInterceptor) {
	return unaryInterceptor(allowedGroup), streamInterceptor(allowedGroup)
}

func unaryInterceptor(allowedGroup string) grpc.UnaryServerInterceptor {
	return func(ctx context.Context, req interface{}, info *grpc.UnaryServerInfo, handler grpc.UnaryHandler) (interface{}, error) {
		if err := checkPeerAuth(ctx, allowedGroup); err != nil {
			return nil, err
		}
		return handler(ctx, req)
	}
}

func streamInterceptor(allowedGroup string) grpc.StreamServerInterceptor {
	return func(srv interface{}, ss grpc.ServerStream, info *grpc.StreamServerInfo, handler grpc.StreamHandler) error {
		if err := checkPeerAuth(ss.Context(), allowedGroup); err != nil {
			return err
		}
		return handler(srv, ss)
	}
}

// selfUID is the daemon's own effective uid. Overridable in tests.
var selfUID = os.Geteuid

// authorize reports whether a peer is allowed to drive the daemon. It is a
// package var so tests can substitute a deterministic policy instead of
// depending on the host's group database.
var authorize = defaultAuthorize

// defaultAuthorize permits root, the uid running the daemon (so an
// unprivileged developer daemon is usable by its owner), and any member of
// allowedGroup — primary GID or supplementary membership.
func defaultAuthorize(creds *PeerCreds, allowedGroup string) bool {
	if creds.UID == 0 || creds.UID == selfUID() {
		return true
	}
	if allowedGroup == "" {
		return false
	}

	grp, err := user.LookupGroup(allowedGroup)
	if err != nil {
		return false // group absent: deny rather than fail open
	}
	if strconv.Itoa(creds.GID) == grp.Gid {
		return true
	}

	// Supplementary group membership.
	usr, err := user.LookupId(strconv.Itoa(creds.UID))
	if err != nil {
		return false
	}
	gids, err := usr.GroupIds()
	if err != nil {
		return false
	}
	for _, gid := range gids {
		if gid == grp.Gid {
			return true
		}
	}
	return false
}

func checkPeerAuth(ctx context.Context, allowedGroup string) error {
	p, ok := peer.FromContext(ctx)
	if !ok {
		return status.Error(codes.Unauthenticated, "no peer info")
	}

	creds, ok := p.AuthInfo.(*PeerCreds)
	if !ok {
		return status.Error(codes.Unauthenticated, "no peer credentials")
	}

	if authorize(creds, allowedGroup) {
		return nil
	}
	return status.Errorf(codes.PermissionDenied, "peer uid %d (gid %d) not authorized", creds.UID, creds.GID)
}
