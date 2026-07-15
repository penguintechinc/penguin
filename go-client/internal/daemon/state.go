package daemon

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
)

// PersistedState is the set of modules enabled by the user.
// It is saved to disk and restored on daemon restart.
type PersistedState struct {
	// Enabled is the list of module names the user has enabled.
	Enabled []string `json:"enabled"`
}

// LoadState loads the persisted state from path.
// If the file does not exist, it returns an empty state with no error.
// If reading or unmarshaling fails, an error is returned.
func LoadState(path string) (*PersistedState, error) {
	data, err := os.ReadFile(filepath.Clean(path)) // #nosec G304 -- daemon-owned state path from config, not user input
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return &PersistedState{Enabled: []string{}}, nil
		}
		return nil, err
	}

	var ps PersistedState
	if err := json.Unmarshal(data, &ps); err != nil {
		return nil, err
	}
	return &ps, nil
}

// Save persists the state to path atomically using a temp file + rename pattern.
// File permissions are set to 0600 (read/write by owner only).
func (ps *PersistedState) Save(path string) error {
	data, err := json.MarshalIndent(ps, "", "  ")
	if err != nil {
		return err
	}

	// Ensure parent directory exists
	if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		return err
	}

	// Write to temp file in the same directory for atomic rename
	tmpFile, err := os.CreateTemp(filepath.Dir(path), ".state-tmp-*")
	if err != nil {
		return err
	}
	tmpPath := tmpFile.Name()

	cleanup := func() {
		_ = tmpFile.Close()
		_ = os.Remove(tmpPath)
	}

	// Set permissions before writing data
	if err := os.Chmod(tmpPath, 0600); err != nil {
		cleanup()
		return err
	}

	// Write data
	if _, err = tmpFile.Write(data); err != nil {
		cleanup()
		return err
	}
	if err := tmpFile.Close(); err != nil {
		_ = os.Remove(tmpPath)
		return err
	}

	// Atomic rename
	return os.Rename(tmpPath, path)
}
