package extplugin

import (
	"os"
	"path/filepath"
	"testing"
)

// TestLoadManifestHappyPath loads a valid plugin.json.
func TestLoadManifestHappyPath(t *testing.T) {
	tmpDir := t.TempDir()

	jsonContent := `{
  "name": "test-plugin",
  "version": "1.0.0",
  "sdk_version": "v1",
  "binary": "test-binary",
  "sha256": "abc123def456",
  "publisher": "test-publisher"
}`

	manifestPath := filepath.Join(tmpDir, "plugin.json")
	if err := os.WriteFile(manifestPath, []byte(jsonContent), 0o600); err != nil {
		t.Fatalf("write plugin.json: %v", err)
	}

	manifest, err := LoadManifest(tmpDir)
	if err != nil {
		t.Fatalf("load manifest: %v", err)
	}

	if manifest.Name != "test-plugin" {
		t.Errorf("name mismatch: got %s, want test-plugin", manifest.Name)
	}
	if manifest.Version != "1.0.0" {
		t.Errorf("version mismatch: got %s, want 1.0.0", manifest.Version)
	}
	if manifest.Binary != "test-binary" {
		t.Errorf("binary mismatch: got %s, want test-binary", manifest.Binary)
	}
	if manifest.SHA256 != "abc123def456" {
		t.Errorf("sha256 mismatch: got %s, want abc123def456", manifest.SHA256)
	}
}

// TestLoadManifestMissingFile rejects missing plugin.json.
func TestLoadManifestMissingFile(t *testing.T) {
	tmpDir := t.TempDir()

	_, err := LoadManifest(tmpDir)
	if err == nil {
		t.Fatalf("load manifest should have failed for missing plugin.json")
	}
}

// TestLoadManifestGarbageJSON rejects invalid JSON.
func TestLoadManifestGarbageJSON(t *testing.T) {
	tmpDir := t.TempDir()

	manifestPath := filepath.Join(tmpDir, "plugin.json")
	if err := os.WriteFile(manifestPath, []byte("not valid json {{{"), 0o600); err != nil {
		t.Fatalf("write garbage json: %v", err)
	}

	_, err := LoadManifest(tmpDir)
	if err == nil {
		t.Fatalf("load manifest should have failed for garbage json")
	}
}

// TestLoadManifestMissingRequiredFields rejects incomplete manifests.
func TestLoadManifestMissingRequiredFields(t *testing.T) {
	tests := []struct {
		name     string
		jsonBody string
	}{
		{
			name:     "missing name",
			jsonBody: `{"version": "1.0.0", "binary": "bin", "sha256": "abc"}`,
		},
		{
			name:     "missing binary",
			jsonBody: `{"name": "test", "version": "1.0.0", "sha256": "abc"}`,
		},
		{
			name:     "missing sha256",
			jsonBody: `{"name": "test", "version": "1.0.0", "binary": "bin"}`,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			tmpDir := t.TempDir()
			manifestPath := filepath.Join(tmpDir, "plugin.json")
			if err := os.WriteFile(manifestPath, []byte(tt.jsonBody), 0o600); err != nil {
				t.Fatalf("write plugin.json: %v", err)
			}

			_, err := LoadManifest(tmpDir)
			if err == nil {
				t.Fatalf("load manifest should have failed for %s", tt.name)
			}
		})
	}
}

// TestBinaryPath returns the correct binary path.
func TestBinaryPath(t *testing.T) {
	manifest := &Manifest{
		Binary: "mybin",
	}

	pluginDir := "/path/to/plugin"
	expected := "/path/to/plugin/mybin"
	actual := manifest.BinaryPath(pluginDir)

	if actual != expected {
		t.Errorf("binary path mismatch: got %s, want %s", actual, expected)
	}
}

// TestSignaturePath returns the correct signature path.
func TestSignaturePath(t *testing.T) {
	manifest := &Manifest{
		Binary: "mybin",
	}

	pluginDir := "/path/to/plugin"
	expected := "/path/to/plugin/mybin.minisig"
	actual := manifest.SignaturePath(pluginDir)

	if actual != expected {
		t.Errorf("signature path mismatch: got %s, want %s", actual, expected)
	}
}
