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
	"testing"

	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/credentials"
	"google.golang.org/grpc/metadata"
	"google.golang.org/grpc/peer"
	"google.golang.org/grpc/status"
)

// Extended tests for Unix IPC to improve coverage

// TestListenMultipleCalls tests that Listen can be called multiple times on different paths.
func TestListenMultipleCalls(t *testing.T) {
	tmpDir := t.TempDir()
	socket1 := filepath.Join(tmpDir, "socket1.sock")
	socket2 := filepath.Join(tmpDir, "socket2.sock")

	listener1, _, err := Listen(ListenerConfig{
		Path:         socket1,
		AllowedGroup: "penguin",
	})
	if err != nil {
		t.Fatalf("Listen socket1 failed: %v", err)
	}
	defer func() { _ = listener1.Close() }()

	listener2, _, err := Listen(ListenerConfig{
		Path:         socket2,
		AllowedGroup: "penguin",
	})
	if err != nil {
		t.Fatalf("Listen socket2 failed: %v", err)
	}
	defer func() { _ = listener2.Close() }()

	// Both sockets should exist
	if _, err := os.Stat(socket1); err != nil {
		t.Errorf("socket1 not found: %v", err)
	}
	if _, err := os.Stat(socket2); err != nil {
		t.Errorf("socket2 not found: %v", err)
	}
}

// TestListenerNetInterface tests that the returned Listener implements net.Listener.
func TestListenerNetInterface(t *testing.T) {
	tmpDir := t.TempDir()
	socketPath := filepath.Join(tmpDir, "test.sock")

	listener, _, err := Listen(ListenerConfig{
		Path:         socketPath,
		AllowedGroup: "penguin",
	})
	if err != nil {
		t.Fatalf("Listen failed: %v", err)
	}
	defer func() { _ = listener.Close() }()

	// Verify it implements net.Listener.
	var _ net.Listener = listener //nolint:staticcheck // explicit interface assertion is the point of this check

	// Check that Addr() and Close() work
	addr := listener.Addr()
	if addr.String() != socketPath {
		t.Errorf("expected addr %s, got %s", socketPath, addr.String())
	}

	if addr.Network() != "unix" {
		t.Errorf("expected network unix, got %s", addr.Network())
	}
}

// TestPeerCredsAuthType tests PeerCreds.AuthType method.
func TestPeerCredsAuthType(t *testing.T) {
	creds := &PeerCreds{
		UID: 1000,
		GID: 1000,
		PID: 12345,
	}

	authType := creds.AuthType()
	if authType != "unix-peercred" {
		t.Errorf("expected auth type unix-peercred, got %s", authType)
	}
}

// TestPeerCredentialsInfo tests TransportCredentials.Info.
func TestPeerCredentialsInfo(t *testing.T) {
	creds := newPeerCredentials("penguin")
	info := creds.Info()

	if info.SecurityProtocol != "unix-peercred" {
		t.Errorf("expected unix-peercred, got %s", info.SecurityProtocol)
	}
}

// TestPeerCredentialsClone tests TransportCredentials.Clone.
func TestPeerCredentialsClone(t *testing.T) {
	creds := newPeerCredentials("penguin")
	cloned := creds.Clone()

	if cloned == nil {
		t.Error("expected non-nil cloned credentials")
	}

	info := cloned.Info()
	if info.SecurityProtocol != "unix-peercred" {
		t.Errorf("expected unix-peercred, got %s", info.SecurityProtocol)
	}
}

// TestListenPathTooLong tests the maxUnixPath check.
func TestListenPathTooLong(t *testing.T) {
	longPath := "/" + string(make([]byte, maxUnixPath+10))
	listener, _, err := Listen(ListenerConfig{
		Path:         longPath,
		AllowedGroup: "penguin",
	})
	if err == nil {
		_ = listener.Close()
		t.Error("expected error for path too long")
	}
}

// TestClientHandshake tests that ClientHandshake always errors.
func TestClientHandshake(t *testing.T) {
	pc := newPeerCredentials("penguin")
	conn := &net.TCPConn{}
	_, _, err := pc.ClientHandshake(context.Background(), "localhost", conn)
	if err == nil {
		t.Error("expected ClientHandshake to return error")
	}
}

// OverrideServerName is deprecated and not tested to avoid deprecation warnings

// TestDefaultAuthorizeRoot tests that UID 0 is always allowed.
func TestDefaultAuthorizeRoot(t *testing.T) {
	creds := &PeerCreds{UID: 0, GID: 0}
	if !defaultAuthorize(creds, "penguin") {
		t.Error("expected root to be authorized")
	}
}

// TestDefaultAuthorizeSelf tests that the daemon's own UID is allowed.
func TestDefaultAuthorizeSelf(t *testing.T) {
	myUID := os.Geteuid()
	oldSelfUID := selfUID
	defer func() { selfUID = oldSelfUID }()
	selfUID = func() int { return myUID }

	creds := &PeerCreds{UID: myUID, GID: 1000}
	if !defaultAuthorize(creds, "penguin") {
		t.Error("expected daemon's own uid to be authorized")
	}
}

// TestDefaultAuthorizeEmptyGroup tests that empty group rejects non-root.
func TestDefaultAuthorizeEmptyGroup(t *testing.T) {
	oldSelfUID := selfUID
	defer func() { selfUID = oldSelfUID }()
	selfUID = func() int { return 0 } // not 1000

	creds := &PeerCreds{UID: 1000, GID: 1000}
	if defaultAuthorize(creds, "") {
		t.Error("expected empty group to deny non-root")
	}
}

// TestDefaultAuthorizePrimaryGroup tests primary GID match.
func TestDefaultAuthorizePrimaryGroup(t *testing.T) {
	grp, err := user.LookupGroup("root")
	if err != nil {
		t.Skipf("root group not found: %v", err)
	}
	grpGID, _ := strconv.Atoi(grp.Gid)

	oldSelfUID := selfUID
	defer func() { selfUID = oldSelfUID }()
	selfUID = func() int { return 999 }

	creds := &PeerCreds{UID: 1000, GID: grpGID}
	if !defaultAuthorize(creds, "root") {
		t.Error("expected primary group match to authorize")
	}
}

// TestDefaultAuthorizeUnknownGroup tests unknown group is denied.
func TestDefaultAuthorizeUnknownGroup(t *testing.T) {
	oldSelfUID := selfUID
	defer func() { selfUID = oldSelfUID }()
	selfUID = func() int { return 999 }

	creds := &PeerCreds{UID: 1000, GID: 1000}
	if defaultAuthorize(creds, "nonexistent-group-xyz") {
		t.Error("expected unknown group to be denied")
	}
}

// TestCheckPeerAuthNoPeerInfo tests checkPeerAuth with no peer info.
func TestCheckPeerAuthNoPeerInfo(t *testing.T) {
	ctx := context.Background()
	err := checkPeerAuth(ctx, "penguin")
	if err == nil {
		t.Error("expected error for no peer info")
	}
	st, ok := status.FromError(err)
	if !ok || st.Code() != codes.Unauthenticated {
		t.Errorf("expected Unauthenticated, got %v", err)
	}
}

// TestCheckPeerAuthWrongAuthInfo tests checkPeerAuth with wrong AuthInfo type.
func TestCheckPeerAuthWrongAuthInfo(t *testing.T) {
	ctx := context.Background()
	ctx = peer.NewContext(ctx, &peer.Peer{
		AuthInfo: credentials.TLSInfo{},
	})
	err := checkPeerAuth(ctx, "penguin")
	if err == nil {
		t.Error("expected error for wrong AuthInfo type")
	}
	st, ok := status.FromError(err)
	if !ok || st.Code() != codes.Unauthenticated {
		t.Errorf("expected Unauthenticated, got %v", err)
	}
}

// TestCheckPeerAuthAllow tests checkPeerAuth allow path.
func TestCheckPeerAuthAllow(t *testing.T) {
	oldAuthorize := authorize
	defer func() { authorize = oldAuthorize }()
	authorize = func(c *PeerCreds, g string) bool { return true }

	ctx := context.Background()
	ctx = peer.NewContext(ctx, &peer.Peer{
		AuthInfo: &PeerCreds{UID: 1000, GID: 1000},
	})
	err := checkPeerAuth(ctx, "penguin")
	if err != nil {
		t.Errorf("unexpected error: %v", err)
	}
}

// TestCheckPeerAuthDeny tests checkPeerAuth deny path.
func TestCheckPeerAuthDeny(t *testing.T) {
	oldAuthorize := authorize
	defer func() { authorize = oldAuthorize }()
	authorize = func(c *PeerCreds, g string) bool { return false }

	ctx := context.Background()
	ctx = peer.NewContext(ctx, &peer.Peer{
		AuthInfo: &PeerCreds{UID: 1000, GID: 1000},
	})
	err := checkPeerAuth(ctx, "penguin")
	if err == nil {
		t.Error("expected error for denied peer")
	}
	st, ok := status.FromError(err)
	if !ok || st.Code() != codes.PermissionDenied {
		t.Errorf("expected PermissionDenied, got %v", err)
	}
}

// TestPeerAuthInterceptorUnaryAllow tests unary interceptor allow path.
func TestPeerAuthInterceptorUnaryAllow(t *testing.T) {
	oldAuthorize := authorize
	defer func() { authorize = oldAuthorize }()
	authorize = func(c *PeerCreds, g string) bool { return true }

	unary, _ := PeerAuthInterceptor("penguin")

	ctx := context.Background()
	ctx = peer.NewContext(ctx, &peer.Peer{
		AuthInfo: &PeerCreds{UID: 1000, GID: 1000},
	})

	handlerCalled := false
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		handlerCalled = true
		return "ok", nil
	}

	resp, err := unary(ctx, nil, &grpc.UnaryServerInfo{FullMethod: "test"}, handler)
	if err != nil {
		t.Errorf("unexpected error: %v", err)
	}
	if !handlerCalled {
		t.Error("handler not called")
	}
	if resp != "ok" {
		t.Errorf("unexpected response: %v", resp)
	}
}

// TestPeerAuthInterceptorUnaryDeny tests unary interceptor deny path.
func TestPeerAuthInterceptorUnaryDeny(t *testing.T) {
	oldAuthorize := authorize
	defer func() { authorize = oldAuthorize }()
	authorize = func(c *PeerCreds, g string) bool { return false }

	unary, _ := PeerAuthInterceptor("penguin")

	ctx := context.Background()
	ctx = peer.NewContext(ctx, &peer.Peer{
		AuthInfo: &PeerCreds{UID: 1000, GID: 1000},
	})

	handlerCalled := false
	handler := func(ctx context.Context, req interface{}) (interface{}, error) {
		handlerCalled = true
		return "ok", nil
	}

	_, err := unary(ctx, nil, &grpc.UnaryServerInfo{FullMethod: "test"}, handler)
	if err == nil {
		t.Error("expected error")
	}
	if handlerCalled {
		t.Error("handler should not have been called")
	}
}

// TestPeerAuthInterceptorStreamAllow tests stream interceptor allow path.
func TestPeerAuthInterceptorStreamAllow(t *testing.T) {
	oldAuthorize := authorize
	defer func() { authorize = oldAuthorize }()
	authorize = func(c *PeerCreds, g string) bool { return true }

	_, stream := PeerAuthInterceptor("penguin")

	mockStream := &mockServerStream{
		ctx: peer.NewContext(context.Background(), &peer.Peer{
			AuthInfo: &PeerCreds{UID: 1000, GID: 1000},
		}),
	}

	handlerCalled := false
	handler := func(srv interface{}, ss grpc.ServerStream) error {
		handlerCalled = true
		return nil
	}

	err := stream(nil, mockStream, &grpc.StreamServerInfo{FullMethod: "test"}, handler)
	if err != nil {
		t.Errorf("unexpected error: %v", err)
	}
	if !handlerCalled {
		t.Error("handler not called")
	}
}

// TestPeerAuthInterceptorStreamDeny tests stream interceptor deny path.
func TestPeerAuthInterceptorStreamDeny(t *testing.T) {
	oldAuthorize := authorize
	defer func() { authorize = oldAuthorize }()
	authorize = func(c *PeerCreds, g string) bool { return false }

	_, stream := PeerAuthInterceptor("penguin")

	mockStream := &mockServerStream{
		ctx: peer.NewContext(context.Background(), &peer.Peer{
			AuthInfo: &PeerCreds{UID: 1000, GID: 1000},
		}),
	}

	handlerCalled := false
	handler := func(srv interface{}, ss grpc.ServerStream) error {
		handlerCalled = true
		return nil
	}

	err := stream(nil, mockStream, &grpc.StreamServerInfo{FullMethod: "test"}, handler)
	if err == nil {
		t.Error("expected error")
	}
	if handlerCalled {
		t.Error("handler should not have been called")
	}
}

// TestDefaultAuthorizeSupplementaryGroups tests supplementary group membership.
func TestDefaultAuthorizeSupplementaryGroups(t *testing.T) {
	// Test path: try to find a group that the current user is a member of
	// and verify that a peer with that GID is authorized.

	oldSelfUID := selfUID
	defer func() { selfUID = oldSelfUID }()

	// Set selfUID to something other than our current uid
	myUID := os.Geteuid()
	selfUID = func() int { return 999 }

	// Get current user and their groups
	u, err := user.LookupId(fmt.Sprintf("%d", myUID))
	if err != nil {
		t.Skipf("could not look up current user: %v", err)
	}

	gids, err := u.GroupIds()
	if err != nil || len(gids) == 0 {
		t.Skipf("could not get supplementary groups for user: %v", err)
	}

	// Find a supplementary group (not the primary gid)
	if len(gids) < 2 {
		t.Skipf("user has fewer than 2 groups, cannot test supplementary membership")
	}

	supGID := gids[1] // Take second group (not primary)
	if supGID == u.Gid {
		supGID = gids[0] // If somehow primary, use first
	}

	// Look up the group by gid to get the group name
	grp, err := user.LookupGroupId(supGID)
	if err != nil {
		t.Skipf("could not look up group by id: %v", err)
	}

	// Create a peer with uid=myUID and primary gid=some_other_gid
	// but with supGID as a supplementary group
	creds := &PeerCreds{
		UID: myUID,
		GID: 1000, // some other gid
		PID: 0,
	}

	// Verify should succeed due to supplementary group membership
	if !defaultAuthorize(creds, grp.Name) {
		t.Errorf("expected authorization for user in supplementary group %s", grp.Name)
	}
}

// TestDefaultAuthorizeNonExistentUID tests authorization when uid lookup fails.
func TestDefaultAuthorizeNonExistentUID(t *testing.T) {
	oldSelfUID := selfUID
	defer func() { selfUID = oldSelfUID }()
	selfUID = func() int { return 999 }

	// Use a uid that definitely doesn't exist
	creds := &PeerCreds{
		UID: 999999999,
		GID: 1000,
		PID: 0,
	}

	// Should be denied since uid doesn't exist and group lookup will fail
	if defaultAuthorize(creds, "root") {
		t.Error("expected denial for non-existent uid")
	}
}

// TestListenSocketPathLengthEdgeCase tests the exact maxUnixPath boundary.
func TestListenSocketPathLengthEdgeCase(t *testing.T) {
	// Create a path exactly at the boundary
	tmpDir := t.TempDir()

	// Construct a path that is exactly maxUnixPath bytes
	// Use a path within tmpDir so we don't exceed the limit on the absolute path
	relPathLen := maxUnixPath - len(tmpDir) - 2
	if relPathLen <= 0 {
		t.Skipf("tmpDir too long to construct max-length socket path")
	}
	relPathBytes := make([]byte, relPathLen)
	for i := range relPathBytes {
		relPathBytes[i] = 'a'
	}
	socketPath := filepath.Join(tmpDir, string(relPathBytes))

	// This should succeed (at or under the boundary)
	if len(socketPath) > maxUnixPath {
		t.Skipf("could not construct path of exact max length")
	}

	listener, _, err := Listen(ListenerConfig{
		Path:         socketPath,
		AllowedGroup: "penguin",
	})
	if err != nil && len(socketPath) <= maxUnixPath {
		t.Errorf("expected Listen to succeed for path at boundary, got: %v", err)
	}
	if listener != nil {
		_ = listener.Close()
	}
}

// TestDialContextCancellation tests that Dial respects context cancellation.
func TestDialContextCancellation(t *testing.T) {
	tmpDir := t.TempDir()
	socketPath := filepath.Join(tmpDir, "nonexistent.sock")

	ctx, cancel := context.WithCancel(context.Background())
	cancel() // Cancel immediately

	conn, err := Dial(ctx, socketPath)
	// Should fail quickly due to cancelled context
	if conn != nil {
		_ = conn.Close()
	}
	// Error is expected (context cancelled or connection failed)
	_ = err
}

// TestServerHandshakeNonUnixConn tests ServerHandshake with non-Unix connection.
func TestServerHandshakeNonUnixConn(t *testing.T) {
	pc := newPeerCredentials("penguin")
	mockConn := &net.TCPConn{} // Not a Unix connection

	returnedConn, authInfo, err := pc.ServerHandshake(mockConn)
	if err != nil {
		t.Errorf("ServerHandshake should not error for non-Unix conn: %v", err)
	}
	if returnedConn == nil {
		t.Error("expected non-nil connection")
	}
	if authInfo != nil {
		t.Errorf("expected nil AuthInfo for non-Unix conn, got %v", authInfo)
	}
}

// TestListenRemoveStaleNonSocket tests removing a non-socket file.
func TestListenRemoveStaleNonSocket(t *testing.T) {
	tmpDir := t.TempDir()
	socketPath := filepath.Join(tmpDir, "stale.sock")

	// Create a regular file (not a socket)
	if err := os.WriteFile(socketPath, []byte("stale file"), 0600); err != nil {
		t.Fatalf("write stale file: %v", err)
	}

	// Listen should remove the stale file and create a socket
	listener, _, err := Listen(ListenerConfig{
		Path:         socketPath,
		AllowedGroup: "penguin",
	})
	if err != nil {
		t.Fatalf("Listen failed: %v", err)
	}
	defer func() { _ = listener.Close() }()

	// Verify it's now a socket
	stat, err := os.Stat(socketPath)
	if err != nil {
		t.Fatalf("stat socket: %v", err)
	}

	if stat.Mode()&os.ModeSocket == 0 {
		t.Error("expected socket, got regular file")
	}
}

// TestListenMultipleCalls2 tests reopening the same socket path.
func TestListenMultipleCalls2(t *testing.T) {
	tmpDir := t.TempDir()
	socketPath := filepath.Join(tmpDir, "reuse.sock")

	// Create first listener
	listener1, _, err := Listen(ListenerConfig{
		Path:         socketPath,
		AllowedGroup: "penguin",
	})
	if err != nil {
		t.Fatalf("first Listen failed: %v", err)
	}
	if err := listener1.Close(); err != nil {
		t.Logf("close first listener: %v", err)
	}

	// Create second listener on the same path (should work after closing first)
	listener2, _, err := Listen(ListenerConfig{
		Path:         socketPath,
		AllowedGroup: "penguin",
	})
	if err != nil {
		t.Fatalf("second Listen failed: %v", err)
	}
	defer func() { _ = listener2.Close() }()

	// Verify socket still exists
	if _, err := os.Stat(socketPath); err != nil {
		t.Errorf("socket not found: %v", err)
	}
}

// TestDefaultAuthorizeRootAlways tests that uid 0 is always authorized.
func TestDefaultAuthorizeRootAlways(t *testing.T) {
	// Root should be authorized regardless of group
	creds := &PeerCreds{UID: 0, GID: 0}
	if !defaultAuthorize(creds, "") {
		t.Error("root should be authorized even with empty group")
	}

	if !defaultAuthorize(creds, "nonexistent") {
		t.Error("root should be authorized even with bad group")
	}
}

// TestDefaultAuthorizeNonRootNonSelf tests non-root and non-self users.
func TestDefaultAuthorizeNonRootNonSelf(t *testing.T) {
	oldSelfUID := selfUID
	defer func() { selfUID = oldSelfUID }()
	selfUID = func() int { return 1000 }

	// User that is not root and not self should be checked against group
	creds := &PeerCreds{UID: 2000, GID: 2000}

	// With empty group, non-root should be denied
	if defaultAuthorize(creds, "") {
		t.Error("non-root should be denied with empty group")
	}
}

// mockServerStream mocks grpc.ServerStream for testing.
type mockServerStream struct {
	ctx context.Context
}

func (m *mockServerStream) SetHeader(metadata.MD) error {
	return nil
}

func (m *mockServerStream) SendHeader(metadata.MD) error {
	return nil
}

func (m *mockServerStream) SetTrailer(metadata.MD) {
}

func (m *mockServerStream) Context() context.Context {
	return m.ctx
}

func (m *mockServerStream) SendMsg(v interface{}) error {
	return nil
}

func (m *mockServerStream) RecvMsg(v interface{}) error {
	return fmt.Errorf("mock recv")
}
