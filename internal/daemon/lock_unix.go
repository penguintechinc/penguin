//go:build unix
// +build unix

package daemon

import (
	"fmt"
	"os"
	"syscall"
)

// AcquireLock acquires an exclusive file lock on the given path.
// If the lock cannot be acquired (e.g., already held), it returns an error.
// The returned release function should be called to release the lock.
// On Unix, this uses syscall.Flock with LOCK_EX|LOCK_NB.
func AcquireLock(path string) (func() error, error) {
	// Create or open the lock file with restricted permissions (0600).
	// #nosec G304 -- lock path is the daemon's own runtime dir (operator-controlled), not user input.
	file, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY, 0600)
	if err != nil {
		return nil, fmt.Errorf("open lock file %q: %w", path, err)
	}

	// Try to acquire exclusive non-blocking lock
	if err := syscall.Flock(int(file.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		_ = file.Close()
		if err == syscall.EAGAIN || err == syscall.EWOULDBLOCK {
			return nil, fmt.Errorf("penguind already running: lock file %q is locked", path)
		}
		return nil, fmt.Errorf("acquire lock on %q: %w", path, err)
	}

	// Return a release function
	release := func() error {
		defer func() {
			_ = file.Close()
		}()
		if err := syscall.Flock(int(file.Fd()), syscall.LOCK_UN); err != nil {
			return fmt.Errorf("release lock on %q: %w", path, err)
		}
		return nil
	}

	return release, nil
}
