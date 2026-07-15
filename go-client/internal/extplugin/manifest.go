package extplugin

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
)

// Manifest is the plugin manifest describing a plugin binary, its signature,
// and metadata.
type Manifest struct {
	Name      string `json:"name"`      // Plugin name (e.g., "hello")
	Version   string `json:"version"`   // Semantic version (e.g., "1.0.0")
	SDKVersion string `json:"sdk_version"` // SDK version the plugin targets (e.g., "v1")
	Binary    string `json:"binary"`    // Relative filename of the binary (e.g., "hello")
	SHA256    string `json:"sha256"`    // Hex-encoded SHA256 of the binary
	Publisher string `json:"publisher"` // Publisher name for audit (e.g., "penguintech")
}

// LoadManifest loads and parses plugin.json from a plugin directory.
func LoadManifest(pluginDir string) (*Manifest, error) {
	manifestPath := filepath.Join(pluginDir, "plugin.json")
	data, err := os.ReadFile(manifestPath) // #nosec G304 -- plugin manifest path constructed from trusted pluginDir input; verification follows
	if err != nil {
		return nil, fmt.Errorf("read plugin.json: %w", err)
	}

	var m Manifest
	if err := json.Unmarshal(data, &m); err != nil {
		return nil, fmt.Errorf("parse plugin.json: %w", err)
	}

	if m.Name == "" {
		return nil, fmt.Errorf("plugin.json: missing 'name'")
	}
	if m.Binary == "" {
		return nil, fmt.Errorf("plugin.json: missing 'binary'")
	}
	if m.SHA256 == "" {
		return nil, fmt.Errorf("plugin.json: missing 'sha256'")
	}

	return &m, nil
}

// BinaryPath returns the full path to the plugin binary.
func (m *Manifest) BinaryPath(pluginDir string) string {
	return filepath.Join(pluginDir, m.Binary)
}

// SignaturePath returns the full path to the .minisig signature file.
func (m *Manifest) SignaturePath(pluginDir string) string {
	return filepath.Join(pluginDir, m.Binary+".minisig")
}
