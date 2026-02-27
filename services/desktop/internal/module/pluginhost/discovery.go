// Package pluginhost manages the lifecycle of external plugin module binaries.
// It discovers, launches, and supervises plugin processes using the
// HashiCorp go-plugin framework.
package pluginhost

import (
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"

	"github.com/sirupsen/logrus"
)

const pluginPrefix = "penguin-mod-"

// DiscoveredPlugin holds metadata about a discovered plugin binary.
type DiscoveredPlugin struct {
	Name string // module name (e.g., "vpn")
	Path string // absolute path to binary
}

// Discovery scans directories for plugin binaries matching the naming convention.
type Discovery struct {
	searchPaths []string
	logger      *logrus.Logger
}

// NewDiscovery creates a Discovery that scans the given paths for plugin binaries.
func NewDiscovery(searchPaths []string, logger *logrus.Logger) *Discovery {
	return &Discovery{
		searchPaths: searchPaths,
		logger:      logger,
	}
}

// Discover scans all search paths for plugin binaries.
// Binaries must be named "penguin-mod-<name>" (with optional .exe on Windows).
func (d *Discovery) Discover() ([]DiscoveredPlugin, error) {
	seen := make(map[string]bool)
	var plugins []DiscoveredPlugin

	for _, dir := range d.searchPaths {
		found, err := d.scanDir(dir)
		if err != nil {
			d.logger.WithError(err).WithField("dir", dir).Warn("Failed to scan plugin directory")
			continue
		}
		for _, p := range found {
			if seen[p.Name] {
				d.logger.WithFields(logrus.Fields{
					"name": p.Name,
					"path": p.Path,
				}).Warn("Duplicate plugin found, skipping")
				continue
			}
			seen[p.Name] = true
			plugins = append(plugins, p)
		}
	}

	d.logger.WithField("count", len(plugins)).Info("Plugin discovery complete")
	return plugins, nil
}

func (d *Discovery) scanDir(dir string) ([]DiscoveredPlugin, error) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, nil
		}
		return nil, fmt.Errorf("reading directory %s: %w", dir, err)
	}

	var plugins []DiscoveredPlugin
	for _, entry := range entries {
		if entry.IsDir() {
			continue
		}

		name := entry.Name()
		modName := extractModuleName(name)
		if modName == "" {
			continue
		}

		fullPath := filepath.Join(dir, name)

		// Check executable permission
		info, err := entry.Info()
		if err != nil {
			continue
		}
		if !isExecutable(info) {
			d.logger.WithField("path", fullPath).Debug("Plugin binary not executable, skipping")
			continue
		}

		plugins = append(plugins, DiscoveredPlugin{
			Name: modName,
			Path: fullPath,
		})

		d.logger.WithFields(logrus.Fields{
			"name": modName,
			"path": fullPath,
		}).Debug("Discovered plugin")
	}

	return plugins, nil
}

// extractModuleName extracts the module name from a plugin binary filename.
// e.g., "penguin-mod-vpn" -> "vpn", "penguin-mod-dns.exe" -> "dns"
func extractModuleName(filename string) string {
	// Strip .exe suffix on Windows
	name := filename
	if runtime.GOOS == "windows" {
		name = strings.TrimSuffix(name, ".exe")
	}

	if !strings.HasPrefix(name, pluginPrefix) {
		return ""
	}

	return strings.TrimPrefix(name, pluginPrefix)
}

func isExecutable(info os.FileInfo) bool {
	if runtime.GOOS == "windows" {
		return true // Windows uses file extension, not permission bits
	}
	return info.Mode()&0111 != 0
}

// DefaultSearchPaths returns the standard plugin search paths for the current platform.
func DefaultSearchPaths(configDir string) []string {
	paths := []string{
		"plugins", // relative to working directory
	}

	// Platform-specific plugin directories
	switch runtime.GOOS {
	case "linux":
		paths = append(paths,
			filepath.Join(configDir, "plugins"),
			"/usr/lib/penguin/plugins",
			"/usr/local/lib/penguin/plugins",
		)
	case "darwin":
		home, _ := os.UserHomeDir()
		paths = append(paths,
			filepath.Join(configDir, "plugins"),
			filepath.Join(home, "Library", "Application Support", "PenguinTech", "Penguin", "plugins"),
			"/usr/local/lib/penguin/plugins",
		)
	case "windows":
		paths = append(paths,
			filepath.Join(configDir, "plugins"),
		)
	}

	return paths
}
