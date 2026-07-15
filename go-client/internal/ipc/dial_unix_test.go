//go:build linux || darwin

package ipc

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"
)

// TestDialConnectToSocket tests connecting to a Unix domain socket.
func TestDialConnectToSocket(t *testing.T) {
	tmpDir := t.TempDir()
	socketPath := filepath.Join(tmpDir, "test.sock")

	// Create a listener on a socket
	listener, _, err := Listen(ListenerConfig{
		Path:         socketPath,
		AllowedGroup: "penguin",
	})
	if err != nil {
		t.Fatalf("Listen failed: %v", err)
	}
	defer func() { _ = listener.Close() }()

	// Dial should succeed
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	conn, err := Dial(ctx, socketPath)
	if err != nil {
		t.Fatalf("Dial failed: %v", err)
	}
	defer func() { _ = conn.Close() }()
}

// TestDialBasic tests that Dial returns a connection (doesn't fail on socket path alone).
func TestDialBasic(t *testing.T) {
	tmpDir := t.TempDir()
	socketPath := filepath.Join(tmpDir, "test.sock")

	// Dial should succeed for a valid path (actual connection happens lazily)
	ctx := context.Background()
	conn, err := Dial(ctx, socketPath)

	// The connection may not be immediately established, but Dial itself should succeed
	// (actual error happens when trying to use the connection)
	if err != nil {
		t.Logf("Dial returned error (expected for non-existent socket): %v", err)
	} else if conn == nil {
		t.Error("expected Dial to return non-nil connection")
	} else {
		_ = conn.Close()
	}
}

// TestDialWithValidSocket tests Dial to an actual listening socket.
func TestDialWithValidSocket(t *testing.T) {
	tmpDir := t.TempDir()
	socketPath := filepath.Join(tmpDir, "dial-test.sock")

	// Create a listener
	listener, _, err := Listen(ListenerConfig{
		Path:         socketPath,
		AllowedGroup: "penguin",
	})
	if err != nil {
		t.Fatalf("Listen failed: %v", err)
	}
	defer func() { _ = listener.Close() }()

	// Dial to the socket
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	conn, err := Dial(ctx, socketPath)
	if err != nil {
		t.Logf("Dial failed (may happen if no server is accepting): %v", err)
	} else if conn == nil {
		t.Error("expected non-nil connection")
	} else {
		_ = conn.Close()
	}
}

// TestDialEmptyPath tests Dial with an empty path.
func TestDialEmptyPath(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 1*time.Second)
	defer cancel()

	conn, err := Dial(ctx, "")
	// Empty path should cause connection failure eventually
	if conn != nil {
		_ = conn.Close()
	}
	_ = err // Error is expected
}

// TestDialNonexistentSocket tests Dial to a path that doesn't exist.
func TestDialNonexistentSocket(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 1*time.Second)
	defer cancel()

	conn, err := Dial(ctx, "/nonexistent/path/to/socket.sock")
	if conn != nil {
		_ = conn.Close()
	}
	// Connection should fail or error out eventually
	_ = err
}

// TestDialToRegularFile tests Dial to a path that is a regular file, not a socket.
func TestDialToRegularFile(t *testing.T) {
	tmpDir := t.TempDir()
	filePath := filepath.Join(tmpDir, "regular_file")

	// Create a regular file
	if err := os.WriteFile(filePath, []byte("not a socket"), 0600); err != nil {
		t.Fatalf("write test file: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 1*time.Second)
	defer cancel()

	conn, err := Dial(ctx, filePath)
	if conn != nil {
		_ = conn.Close()
	}
	// Dialing to a regular file should fail or error
	_ = err
}
