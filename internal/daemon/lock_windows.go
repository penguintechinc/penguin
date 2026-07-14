//go:build windows
// +build windows

package daemon

import (
	"fmt"
	"unsafe"

	"golang.org/x/sys/windows"
)

// AcquireLock acquires an exclusive named mutex on Windows.
// If the lock cannot be acquired (e.g., already held), it returns an error.
// The returned release function should be called to release the lock.
// On Windows, this uses CreateMutex with a named mutex derived from the path.
func AcquireLock(path string) (func() error, error) {
	// Convert path to UTF-16 for Windows API
	mutexName, err := windows.UTF16PtrFromString(path)
	if err != nil {
		return nil, fmt.Errorf("convert mutex name: %w", err)
	}

	// Try to create/open the mutex with exclusive access
	handle, err := windows.CreateMutex(nil, false, mutexName)
	if err != nil {
		return nil, fmt.Errorf("create mutex: %w", err)
	}

	// Check if we got an existing mutex (ERROR_ALREADY_EXISTS)
	// If the mutex already exists, someone else has the lock
	if windows.GetLastError() == windows.ERROR_ALREADY_EXISTS {
		windows.CloseHandle(handle)
		return nil, fmt.Errorf("penguind already running: mutex %q is already held", path)
	}

	// Wait for the mutex to be available (non-blocking)
	// WAIT_TIMEOUT = 258; if we get timeout, the mutex is held by another process
	waitResult, err := windows.WaitForSingleObject(handle, 0) // 0 = non-blocking
	if err != nil {
		windows.CloseHandle(handle)
		return nil, fmt.Errorf("wait for mutex: %w", err)
	}

	if waitResult == windows.WAIT_TIMEOUT {
		windows.CloseHandle(handle)
		return nil, fmt.Errorf("penguind already running: mutex %q is locked", path)
	}

	// Mutex is acquired; return a release function
	release := func() error {
		defer windows.CloseHandle(handle)
		if !windows.ReleaseMutex(handle) {
			return fmt.Errorf("release mutex: %w", windows.GetLastError())
		}
		return nil
	}

	return release, nil
}
