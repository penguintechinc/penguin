package daemon

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/santhosh-tekuri/jsonschema/v6"
	"gopkg.in/yaml.v3"
)

// DaemonConfig represents the daemon configuration loaded from config.yaml.
type DaemonConfig struct {
	SocketPath string `yaml:"socketPath"`
	PluginsDir string `yaml:"pluginsDir"`
	LogLevel   string `yaml:"logLevel"`
	Group      string `yaml:"group"`
}

// ConfigStore manages loading and validating daemon and module configurations from disk.
type ConfigStore struct {
	dir string
}

// NewConfigStore returns a new ConfigStore that reads from the given directory.
// Typically dir is /etc/penguin for production or a temp directory for tests.
func NewConfigStore(dir string) *ConfigStore {
	return &ConfigStore{dir: dir}
}

// Daemon reads and returns the daemon configuration from <dir>/config.yaml.
// If the file is missing, it returns a DaemonConfig with sensible defaults.
// If the file exists but is malformed, it returns an error.
func (cs *ConfigStore) Daemon() (DaemonConfig, error) {
	defaults := DaemonConfig{
		SocketPath: "/run/penguin/penguind.sock",
		PluginsDir: "/opt/penguin/plugins",
		LogLevel:   "info",
		Group:      "penguin",
	}

	path := filepath.Join(cs.dir, "config.yaml")
	// #nosec G304 -- fixed config filename under the operator-controlled config dir.
	data, err := os.ReadFile(path)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return defaults, nil
		}
		return DaemonConfig{}, fmt.Errorf("read config.yaml: %w", err)
	}

	var cfg DaemonConfig
	if err := yaml.Unmarshal(data, &cfg); err != nil {
		return DaemonConfig{}, fmt.Errorf("parse config.yaml: %w", err)
	}

	// Fill in defaults for unset fields
	if cfg.SocketPath == "" {
		cfg.SocketPath = defaults.SocketPath
	}
	if cfg.PluginsDir == "" {
		cfg.PluginsDir = defaults.PluginsDir
	}
	if cfg.LogLevel == "" {
		cfg.LogLevel = defaults.LogLevel
	}
	if cfg.Group == "" {
		cfg.Group = defaults.Group
	}

	return cfg, nil
}

// Module reads and returns the module configuration from <dir>/modules.d/<name>.yaml.
// It optionally validates the parsed YAML against a JSON schema if schema is non-nil.
// If the file is missing, it returns an empty map and no error.
// The name parameter is validated to prevent path traversal attacks.
func (cs *ConfigStore) Module(name string, schema []byte) (map[string]any, error) {
	// Guard against path traversal
	if strings.Contains(name, "/") || strings.Contains(name, "\\") || strings.Contains(name, "..") {
		return nil, fmt.Errorf("invalid module name: contains path separators or '..': %q", name)
	}

	path := filepath.Join(cs.dir, "modules.d", name+".yaml")
	// #nosec G304 -- name is a registered module identifier (from the compiled-in
	// registry or a verified plugin manifest), never raw client input.
	data, err := os.ReadFile(path)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return make(map[string]any), nil
		}
		return nil, fmt.Errorf("read module config %q: %w", name, err)
	}

	var cfg map[string]any
	if err := yaml.Unmarshal(data, &cfg); err != nil {
		return nil, fmt.Errorf("parse module config %q: %w", name, err)
	}

	// Validate against schema if provided
	if len(schema) > 0 {
		if err := cs.validateSchema(cfg, schema, name); err != nil {
			return nil, err
		}
	}

	return cfg, nil
}

// ModuleRaw returns the module's configuration file verbatim (YAML bytes)
// after validating it against schema. It is what HostServices.Config() hands
// to the module, so a module never parses an unvalidated file itself.
// A missing file yields nil bytes and no error: modules apply their defaults.
func (cs *ConfigStore) ModuleRaw(name string, schema []byte) ([]byte, error) {
	if strings.Contains(name, "/") || strings.Contains(name, "\\") || strings.Contains(name, "..") {
		return nil, fmt.Errorf("invalid module name: contains path separators or '..': %q", name)
	}

	// Validate first; Module() applies the schema and the traversal guard.
	if _, err := cs.Module(name, schema); err != nil {
		return nil, err
	}

	path := filepath.Join(cs.dir, "modules.d", name+".yaml")
	data, err := os.ReadFile(path) // #nosec G304 -- name is traversal-checked above; dir is operator-owned
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return nil, nil
		}
		return nil, fmt.Errorf("read module config %q: %w", name, err)
	}
	return data, nil
}

// validateSchema validates the parsed config against a JSON schema.
// Returns a formatted error listing all failing paths if validation fails.
func (cs *ConfigStore) validateSchema(data map[string]any, schemaBytes []byte, moduleName string) error {
	// Parse the schema from JSON bytes
	schema, err := jsonschema.UnmarshalJSON(bytes.NewReader(schemaBytes))
	if err != nil {
		return fmt.Errorf("parse schema for module %q: %w", moduleName, err)
	}

	// Compile the schema
	compiler := jsonschema.NewCompiler()
	if err := compiler.AddResource("schema", schema); err != nil {
		return fmt.Errorf("compile schema for module %q: %w", moduleName, err)
	}

	sch, err := compiler.Compile("schema")
	if err != nil {
		return fmt.Errorf("compile schema for module %q: %w", moduleName, err)
	}

	// Convert the YAML-parsed map to JSON for validation
	// (jsonschema works with JSON-like data structures)
	jsonData, err := json.Marshal(data)
	if err != nil {
		return fmt.Errorf("marshal config for schema validation: %w", err)
	}
	var jsonObj interface{}
	if err := json.Unmarshal(jsonData, &jsonObj); err != nil {
		return fmt.Errorf("unmarshal config for schema validation: %w", err)
	}

	// Validate the data
	if err := sch.Validate(jsonObj); err != nil {
		// Extract validation error paths
		var validationErr *jsonschema.ValidationError
		if errors.As(err, &validationErr) {
			paths := cs.extractErrorPaths(validationErr)
			return fmt.Errorf("schema validation failed for module %q: %s", moduleName, strings.Join(paths, "; "))
		}
		return fmt.Errorf("schema validation failed for module %q: %w", moduleName, err)
	}

	return nil
}

// extractErrorPaths recursively extracts all failing paths from a validation error.
func (cs *ConfigStore) extractErrorPaths(ve *jsonschema.ValidationError) []string {
	var paths []string
	if len(ve.InstanceLocation) > 0 {
		paths = append(paths, strings.Join(ve.InstanceLocation, "."))
	}
	for _, e := range ve.Causes {
		paths = append(paths, cs.extractErrorPaths(e)...)
	}
	return paths
}
