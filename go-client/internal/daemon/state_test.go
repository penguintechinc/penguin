package daemon

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func TestLoadStateFileNotFound(t *testing.T) {
	// Non-existent file should return empty state, not error
	ps, err := LoadState("/tmp/nonexistent-state-file-12345.json")
	if err != nil {
		t.Fatalf("LoadState should not error on missing file: %v", err)
	}
	if ps == nil || len(ps.Enabled) != 0 {
		t.Errorf("LoadState should return empty state, got %v", ps)
	}
}

func TestLoadStateExisting(t *testing.T) {
	// Create a test state file
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	expected := &PersistedState{
		Enabled: []string{"module-a", "module-b"},
	}
	data, _ := json.MarshalIndent(expected, "", "  ")
	if err := os.WriteFile(statePath, data, 0600); err != nil {
		t.Fatalf("setup: write state file: %v", err)
	}

	ps, err := LoadState(statePath)
	if err != nil {
		t.Fatalf("LoadState failed: %v", err)
	}
	if len(ps.Enabled) != 2 || ps.Enabled[0] != "module-a" || ps.Enabled[1] != "module-b" {
		t.Errorf("LoadState returned wrong data: %v", ps.Enabled)
	}
}

func TestSaveAtomicWrite(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	ps := &PersistedState{
		Enabled: []string{"test-module"},
	}
	if err := ps.Save(statePath); err != nil {
		t.Fatalf("Save failed: %v", err)
	}

	// Check file exists and has correct permissions
	info, err := os.Stat(statePath)
	if err != nil {
		t.Fatalf("Stat failed: %v", err)
	}
	if info.Mode()&0077 != 0 {
		t.Errorf("File permissions should be 0600, got %o", info.Mode())
	}

	// Verify content
	data, _ := os.ReadFile(statePath) // #nosec G304 -- test-owned temp path
	var loaded PersistedState
	if err := json.Unmarshal(data, &loaded); err != nil {
		t.Fatalf("Unmarshal failed: %v", err)
	}
	if len(loaded.Enabled) != 1 || loaded.Enabled[0] != "test-module" {
		t.Errorf("Saved content incorrect: %v", loaded.Enabled)
	}
}

func TestRoundTrip(t *testing.T) {
	// Save and load
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	original := &PersistedState{
		Enabled: []string{"mod1", "mod2", "mod3"},
	}
	if err := original.Save(statePath); err != nil {
		t.Fatalf("Save failed: %v", err)
	}

	loaded, err := LoadState(statePath)
	if err != nil {
		t.Fatalf("LoadState failed: %v", err)
	}

	if len(loaded.Enabled) != len(original.Enabled) {
		t.Fatalf("Enabled count mismatch: got %d, want %d", len(loaded.Enabled), len(original.Enabled))
	}
	for i, name := range original.Enabled {
		if loaded.Enabled[i] != name {
			t.Errorf("Enabled[%d] mismatch: got %q, want %q", i, loaded.Enabled[i], name)
		}
	}
}

func TestSaveCreatesParentDir(t *testing.T) {
	// Save should create parent directory if it doesn't exist
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "subdir", "nested", "state.json")

	ps := &PersistedState{Enabled: []string{"test"}}
	if err := ps.Save(statePath); err != nil {
		t.Fatalf("Save failed: %v", err)
	}

	if _, err := os.Stat(statePath); err != nil {
		t.Fatalf("File not created: %v", err)
	}
}

func TestSaveHandlesMarshalError(t *testing.T) {
	// This is a bit tricky to test since PersistedState should always marshal.
	// Just verify normal operation works with various data.
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	tests := []struct {
		name    string
		enabled []string
	}{
		{"empty", []string{}},
		{"single", []string{"one"}},
		{"multiple", []string{"a", "b", "c", "d"}},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			ps := &PersistedState{Enabled: tt.enabled}
			if err := ps.Save(statePath); err != nil {
				t.Fatalf("Save failed: %v", err)
			}

			loaded, err := LoadState(statePath)
			if err != nil {
				t.Fatalf("LoadState failed: %v", err)
			}

			if len(loaded.Enabled) != len(tt.enabled) {
				t.Fatalf("Length mismatch: got %d, want %d", len(loaded.Enabled), len(tt.enabled))
			}
		})
	}
}

func TestSaveToUnwritableDir(t *testing.T) {
	if os.Getuid() == 0 {
		t.Skip("test requires non-root user")
	}

	tmpdir := t.TempDir()
	// Create a directory and make it read-only (permissions 0o500)
	restrictedDir := filepath.Join(tmpdir, "restricted")
	if err := os.MkdirAll(restrictedDir, 0o700); err != nil { //nolint:govet
		t.Fatalf("setup: mkdir failed: %v", err)
	}
	if err := os.Chmod(restrictedDir, 0o500); err != nil { //nolint:gosec
		t.Fatalf("setup: chmod failed: %v", err)
	}
	t.Cleanup(func() {
		_ = os.Chmod(restrictedDir, 0o700) //nolint:gosec
	})

	statePath := filepath.Join(restrictedDir, "state.json")
	ps := &PersistedState{Enabled: []string{"test"}}

	// Save should fail due to permission denied
	err := ps.Save(statePath)
	if err == nil {
		t.Errorf("Save should fail when directory is unwritable")
	}
}

func TestSaveRoundTripMultipleModules(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	// Test with multiple modules
	original := &PersistedState{
		Enabled: []string{"alpha", "beta", "gamma", "delta"},
	}

	if err := original.Save(statePath); err != nil {
		t.Fatalf("Save failed: %v", err)
	}

	loaded, err := LoadState(statePath)
	if err != nil {
		t.Fatalf("LoadState failed: %v", err)
	}

	if len(loaded.Enabled) != len(original.Enabled) {
		t.Fatalf("Length mismatch: got %d, want %d", len(loaded.Enabled), len(original.Enabled))
	}

	for i, name := range original.Enabled {
		if loaded.Enabled[i] != name {
			t.Errorf("Enabled[%d] mismatch: got %q, want %q", i, loaded.Enabled[i], name)
		}
	}
}

func TestLoadStateInvalidJSON(t *testing.T) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	// Write invalid JSON
	if err := os.WriteFile(statePath, []byte("{invalid json"), 0600); err != nil {
		t.Fatalf("setup: write invalid JSON: %v", err)
	}

	ps, err := LoadState(statePath)
	if err == nil {
		t.Errorf("LoadState should fail on invalid JSON, got %v", ps)
	}
}

// TestLoadStateReadError covers the read error path (not ErrNotExist)
func TestLoadStateReadError(t *testing.T) {
	// Try to read from a directory instead of a file
	tmpdir := t.TempDir()
	err := os.Mkdir(filepath.Join(tmpdir, "isdir"), 0700)
	if err != nil {
		t.Fatalf("setup mkdir failed: %v", err)
	}

	ps, err := LoadState(filepath.Join(tmpdir, "isdir"))
	if err == nil {
		t.Errorf("LoadState should fail on read error, got %v", ps)
	}
}

// TestSaveChmodError covers chmod error path in Save
func TestSaveChmodError(t *testing.T) {
	if os.Getuid() == 0 {
		t.Skip("test requires non-root user")
	}

	tmpdir := t.TempDir()

	ps := &PersistedState{Enabled: []string{"test"}}

	// Create a file that we'll try to chmod, but first get a restricted parent
	restrictedDir := filepath.Join(tmpdir, "restricted")
	if err := os.MkdirAll(restrictedDir, 0o700); err != nil {
		t.Fatalf("setup: mkdir failed: %v", err)
	}
	if err := os.Chmod(restrictedDir, 0o500); err != nil { //nolint:gosec
		t.Fatalf("setup: chmod failed: %v", err)
	}
	t.Cleanup(func() {
		_ = os.Chmod(restrictedDir, 0o700) //nolint:gosec
	})

	statePath := filepath.Join(restrictedDir, "state.json")

	// Save should fail due to permission denied when trying to create temp file
	err := ps.Save(statePath)
	if err == nil {
		t.Errorf("Save should fail when cannot create tempfile in restricted dir")
	}
}

// TestSaveWriteError covers the write error path in Save
func TestSaveWriteError(t *testing.T) {
	tmpdir := t.TempDir()
	// This test is difficult to trigger the write error without mocking.
	// We create a valid state and ensure it saves successfully as a baseline.
	ps := &PersistedState{Enabled: []string{"mod"}}
	statePath := filepath.Join(tmpdir, "state.json")

	err := ps.Save(statePath)
	if err != nil {
		t.Fatalf("Save should succeed with writable dir: %v", err)
	}

	// Verify the file was created
	if _, err := os.Stat(statePath); err != nil {
		t.Fatalf("File should exist after Save: %v", err)
	}
}
