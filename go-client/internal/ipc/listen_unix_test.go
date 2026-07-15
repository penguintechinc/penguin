//go:build linux || darwin

package ipc

import (
	"os"
	"path/filepath"
	"testing"
)

// TestListenCreateSocket verifies socket creation with correct permissions.
func TestListenCreateSocket(t *testing.T) {
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

	// Check socket exists
	stat, err := os.Stat(socketPath)
	if err != nil {
		t.Fatalf("socket stat failed: %v", err)
	}

	// Check permissions are 0660
	perms := stat.Mode().Perm()
	if perms != 0660 {
		t.Errorf("expected socket perms 0660, got %o", perms)
	}
}

// TestListenRemoveStale verifies stale socket removal.
func TestListenRemoveStale(t *testing.T) {
	tmpDir := t.TempDir()
	socketPath := filepath.Join(tmpDir, "test.sock")

	// Create a dummy socket file
	if err := os.WriteFile(socketPath, []byte("stale"), 0600); err != nil {
		t.Fatalf("write stale socket: %v", err)
	}

	listener, _, err := Listen(ListenerConfig{
		Path:         socketPath,
		AllowedGroup: "penguin",
	})
	if err != nil {
		t.Fatalf("Listen failed: %v", err)
	}
	defer func() { _ = listener.Close() }()

	// Verify socket was replaced (is a real socket, not the dummy file)
	stat, err := os.Stat(socketPath)
	if err != nil {
		t.Fatalf("socket stat failed: %v", err)
	}

	// Socket should be a socket (not a regular file)
	if stat.Mode()&os.ModeSocket == 0 {
		t.Error("expected socket, got regular file")
	}
}

// TestListenCreateParent verifies parent directory creation.
func TestListenCreateParent(t *testing.T) {
	tmpDir := t.TempDir()
	socketPath := filepath.Join(tmpDir, "subdir1", "subdir2", "test.sock")

	listener, _, err := Listen(ListenerConfig{
		Path:         socketPath,
		AllowedGroup: "penguin",
	})
	if err != nil {
		t.Fatalf("Listen failed: %v", err)
	}
	defer func() { _ = listener.Close() }()

	// Parent dir is 0750: world must not be able to traverse into the
	// daemon's runtime dir.
	parentStat, err := os.Stat(filepath.Dir(socketPath))
	if err != nil {
		t.Fatalf("parent stat failed: %v", err)
	}

	parentPerms := parentStat.Mode().Perm()
	if parentPerms != 0o750 {
		t.Errorf("expected parent perms 0750, got %o", parentPerms)
	}
}

// TestListenDoubleListenPathReuse tests that listening twice on the same path
// (after closing the first listener) succeeds due to stale socket removal.
func TestListenDoubleListenPathReuse(t *testing.T) {
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

	// Close the first listener
	if err := listener1.Close(); err != nil {
		t.Logf("close first listener: %v", err)
	}

	// Create second listener on the same path - should succeed
	listener2, _, err := Listen(ListenerConfig{
		Path:         socketPath,
		AllowedGroup: "penguin",
	})
	if err != nil {
		t.Fatalf("second Listen failed: %v", err)
	}
	defer func() { _ = listener2.Close() }()

	// Verify socket exists
	stat, err := os.Stat(socketPath)
	if err != nil {
		t.Fatalf("socket stat failed: %v", err)
	}
	if stat.Mode()&os.ModeSocket == 0 {
		t.Error("expected socket, got regular file")
	}
}
