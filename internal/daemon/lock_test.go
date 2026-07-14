//go:build unix
// +build unix

package daemon

import (
	"os"
	"path/filepath"
	"testing"
)

func TestAcquireLock(t *testing.T) {
	tests := []struct {
		name        string
		setup       func(t *testing.T) string
		expectError bool
	}{
		{
			name: "acquire lock on new file",
			setup: func(t *testing.T) string {
				return filepath.Join(t.TempDir(), "test.lock")
			},
			expectError: false,
		},
		{
			name: "acquire lock on existing file",
			setup: func(t *testing.T) string {
				dir := t.TempDir()
				lockPath := filepath.Join(dir, "existing.lock")
				// nolint: gosec
				if err := os.WriteFile(lockPath, []byte(""), 0600); err != nil {
					t.Fatal(err)
				}
				return lockPath
			},
			expectError: false,
		},
		{
			name: "second acquire fails",
			setup: func(t *testing.T) string {
				return filepath.Join(t.TempDir(), "dual.lock")
			},
			expectError: false, // First acquire succeeds in setup, but we test second acquire in the test body
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			lockPath := tt.setup(t)

			release, err := AcquireLock(lockPath)
			if tt.expectError && err == nil {
				t.Error("expected error, got none")
				return
			}
			if !tt.expectError && err != nil {
				t.Errorf("unexpected error: %v", err)
				return
			}

			if !tt.expectError && release == nil {
				t.Error("expected release function, got nil")
				return
			}

			// Test special case: second acquire should fail
			if tt.name == "second acquire fails" {
				_, err := AcquireLock(lockPath)
				if err == nil {
					t.Error("expected second acquire to fail, got none")
				}
			}

			// Clean up by releasing the lock
			if release != nil {
				if err := release(); err != nil {
					t.Errorf("release error: %v", err)
				}
			}
		})
	}
}

func TestAcquireLockRelease(t *testing.T) {
	lockPath := filepath.Join(t.TempDir(), "reuse.lock")

	// First acquire
	release1, err := AcquireLock(lockPath)
	if err != nil {
		t.Fatalf("first acquire: %v", err)
	}
	defer func() {
		_ = release1()
	}()

	// Second acquire should fail while first is held
	_, err = AcquireLock(lockPath)
	if err == nil {
		t.Error("second acquire should fail while first is held")
	}

	// Release the first lock
	if err := release1(); err != nil {
		t.Fatalf("release: %v", err)
	}

	// Now second acquire should succeed
	release2, err := AcquireLock(lockPath)
	if err != nil {
		t.Fatalf("second acquire after release: %v", err)
	}
	defer func() {
		_ = release2()
	}()

	if err := release2(); err != nil {
		t.Fatalf("second release: %v", err)
	}
}

func TestAcquireLockErrorMessage(t *testing.T) {
	lockPath := filepath.Join(t.TempDir(), "msg.lock")

	release, err := AcquireLock(lockPath)
	if err != nil {
		t.Fatalf("first acquire: %v", err)
	}
	defer func() {
		_ = release()
	}()

	// Second acquire should fail with specific error message
	_, err = AcquireLock(lockPath)
	if err == nil {
		t.Error("expected lock error")
		return
	}
	// Error should mention "penguind already running"
	if !contains(err.Error(), "penguind already running") {
		t.Errorf("expected 'penguind already running' in error, got: %v", err)
	}
}

func TestAcquireLockInvalidPath(t *testing.T) {
	// Try to acquire a lock in a non-existent directory without creating parent dirs
	lockPath := "/nonexistent/deeply/nested/path/lock.lock"

	_, err := AcquireLock(lockPath)
	if err == nil {
		t.Error("expected error for invalid path")
	}
}

// Helper function to check if a string contains a substring
func contains(s, substr string) bool {
	for i := 0; i+len(substr) <= len(s); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}
