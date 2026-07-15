package daemon

import (
	"fmt"
	"os"
	"path/filepath"
	"testing"
)

func TestConfigStoreDaemon(t *testing.T) {
	tests := []struct {
		name        string
		setup       func(t *testing.T, dir string)
		expectError bool
		check       func(t *testing.T, cfg DaemonConfig)
	}{
		{
			name: "missing file uses defaults",
			setup: func(t *testing.T, dir string) {
				// No file created
			},
			expectError: false,
			check: func(t *testing.T, cfg DaemonConfig) {
				if cfg.SocketPath != "/run/penguin/penguind.sock" {
					t.Errorf("expected default socket, got %q", cfg.SocketPath)
				}
				if cfg.LogLevel != "info" {
					t.Errorf("expected default log level info, got %q", cfg.LogLevel)
				}
				if cfg.Group != "penguin" {
					t.Errorf("expected default group penguin, got %q", cfg.Group)
				}
			},
		},
		{
			name: "valid config file",
			setup: func(t *testing.T, dir string) {
				content := `socketPath: /tmp/test.sock
pluginsDir: /opt/test/plugins
logLevel: debug
group: testgroup
`
				// nolint: gosec
				if err := os.WriteFile(filepath.Join(dir, "config.yaml"), []byte(content), 0600); err != nil {
					t.Fatal(err)
				}
			},
			expectError: false,
			check: func(t *testing.T, cfg DaemonConfig) {
				if cfg.SocketPath != "/tmp/test.sock" {
					t.Errorf("expected custom socket, got %q", cfg.SocketPath)
				}
				if cfg.LogLevel != "debug" {
					t.Errorf("expected debug log level, got %q", cfg.LogLevel)
				}
				if cfg.Group != "testgroup" {
					t.Errorf("expected testgroup, got %q", cfg.Group)
				}
			},
		},
		{
			name: "partial config with defaults",
			setup: func(t *testing.T, dir string) {
				content := `socketPath: /tmp/custom.sock`
				// nolint: gosec
				if err := os.WriteFile(filepath.Join(dir, "config.yaml"), []byte(content), 0600); err != nil {
					t.Fatal(err)
				}
			},
			expectError: false,
			check: func(t *testing.T, cfg DaemonConfig) {
				if cfg.SocketPath != "/tmp/custom.sock" {
					t.Errorf("expected custom socket, got %q", cfg.SocketPath)
				}
				if cfg.LogLevel != "info" {
					t.Errorf("expected default log level info, got %q", cfg.LogLevel)
				}
			},
		},
		{
			name: "malformed yaml",
			setup: func(t *testing.T, dir string) {
				content := `invalid: yaml: content:`
				// nolint: gosec
				if err := os.WriteFile(filepath.Join(dir, "config.yaml"), []byte(content), 0600); err != nil {
					t.Fatal(err)
				}
			},
			expectError: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			dir := t.TempDir()
			tt.setup(t, dir)

			cs := NewConfigStore(dir)
			cfg, err := cs.Daemon()

			if tt.expectError && err == nil {
				t.Error("expected error, got none")
			}
			if !tt.expectError && err != nil {
				t.Errorf("unexpected error: %v", err)
			}
			if !tt.expectError && tt.check != nil {
				tt.check(t, cfg)
			}
		})
	}
}

func TestConfigStoreModule(t *testing.T) {
	tests := []struct {
		name        string
		moduleName  string
		setup       func(t *testing.T, dir string)
		schema      []byte
		expectError bool
		check       func(t *testing.T, cfg map[string]any)
	}{
		{
			name:       "missing module file returns empty map",
			moduleName: "nonexistent",
			setup:      func(t *testing.T, dir string) {},
			check: func(t *testing.T, cfg map[string]any) {
				if len(cfg) != 0 {
					t.Errorf("expected empty map, got %v", cfg)
				}
			},
		},
		{
			name:       "valid module config",
			moduleName: "test",
			setup: func(t *testing.T, dir string) {
				modulesDir := filepath.Join(dir, "modules.d")
				// nolint: gosec
				if err := os.MkdirAll(modulesDir, 0700); err != nil {
					t.Fatal(err)
				}
				content := `key1: value1
key2: 42
`
				// nolint: gosec
				if err := os.WriteFile(filepath.Join(modulesDir, "test.yaml"), []byte(content), 0600); err != nil {
					t.Fatal(err)
				}
			},
			check: func(t *testing.T, cfg map[string]any) {
				if v, ok := cfg["key1"]; !ok || v != "value1" {
					t.Errorf("expected key1: value1, got %v", cfg)
				}
				if v, ok := cfg["key2"]; !ok || v != 42 {
					t.Errorf("expected key2: 42, got %v", cfg)
				}
			},
		},
		{
			name:        "path traversal defense",
			moduleName:  "../../../etc/passwd",
			setup:       func(t *testing.T, dir string) {},
			expectError: true,
		},
		{
			name:        "backslash path traversal defense",
			moduleName:  "..\\evil",
			setup:       func(t *testing.T, dir string) {},
			expectError: true,
		},
		{
			name:        "dot-dot defense",
			moduleName:  "test..evil",
			setup:       func(t *testing.T, dir string) {},
			expectError: true,
		},
		{
			name:       "schema validation success",
			moduleName: "validated",
			setup: func(t *testing.T, dir string) {
				modulesDir := filepath.Join(dir, "modules.d")
				if err := os.MkdirAll(modulesDir, 0700); err != nil { // nolint: gosec
					t.Fatal(err)
				}
				content := `name: test
port: 8080
`
				if err := os.WriteFile(filepath.Join(modulesDir, "validated.yaml"), []byte(content), 0600); err != nil { // nolint: gosec
					t.Fatal(err)
				}
			},
			schema: []byte(`{
				"type": "object",
				"properties": {
					"name": {"type": "string"},
					"port": {"type": "integer"}
				},
				"required": ["name"]
			}`),
			check: func(t *testing.T, cfg map[string]any) {
				if v, ok := cfg["name"]; !ok || v != "test" {
					t.Errorf("expected name: test, got %v", cfg)
				}
			},
		},
		{
			name:       "schema validation failure",
			moduleName: "invalid",
			setup: func(t *testing.T, dir string) {
				modulesDir := filepath.Join(dir, "modules.d")
				if err := os.MkdirAll(modulesDir, 0700); err != nil { // nolint: gosec
					t.Fatal(err)
				}
				content := `port: "not_a_number"`
				// nolint: gosec
				if err := os.WriteFile(filepath.Join(modulesDir, "invalid.yaml"), []byte(content), 0600); err != nil {
					t.Fatal(err)
				}
			},
			schema: []byte(`{
				"type": "object",
				"properties": {
					"port": {"type": "integer"}
				}
			}`),
			expectError: true,
		},
		{
			name:       "malformed yaml",
			moduleName: "broken",
			setup: func(t *testing.T, dir string) {
				modulesDir := filepath.Join(dir, "modules.d")
				if err := os.MkdirAll(modulesDir, 0700); err != nil { // nolint: gosec
					t.Fatal(err)
				}
				content := `invalid: yaml: syntax:`
				// nolint: gosec
				if err := os.WriteFile(filepath.Join(modulesDir, "broken.yaml"), []byte(content), 0600); err != nil {
					t.Fatal(err)
				}
			},
			expectError: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			dir := t.TempDir()
			tt.setup(t, dir)

			cs := NewConfigStore(dir)
			cfg, err := cs.Module(tt.moduleName, tt.schema)

			if tt.expectError && err == nil {
				t.Error("expected error, got none")
			}
			if !tt.expectError && err != nil {
				t.Errorf("unexpected error: %v", err)
			}
			if !tt.expectError && tt.check != nil {
				tt.check(t, cfg)
			}
		})
	}
}

func TestConfigStoreModulePathTraversal(t *testing.T) {
	tests := []struct {
		name       string
		moduleName string
	}{
		{"slash", "a/b"},
		{"double slash", "a//b"},
		{"dot dot", ".."},
		{"dot dot slash", "../a"},
		{"dot dot in name", "test..name"},
		{"windows backslash", "a\\b"},
	}

	for _, tt := range tests {
		t.Run(fmt.Sprintf("rejects_%s", tt.name), func(t *testing.T) {
			cs := NewConfigStore(t.TempDir())
			_, err := cs.Module(tt.moduleName, nil)
			if err == nil {
				t.Errorf("expected path traversal error for %q", tt.moduleName)
			}
		})
	}
}
