package pluginhost

import (
	"io"
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"github.com/sirupsen/logrus"
)

func TestExtractModuleName(t *testing.T) {
	tests := []struct {
		filename string
		want     string
	}{
		{"penguin-mod-vpn", "vpn"},
		{"penguin-mod-dns", "dns"},
		{"penguin-mod-my-module", "my-module"},
		{"penguin-mod-", ""},
		{"other-binary", ""},
		{"penguin-mod-vpn.exe", "vpn"},
	}

	// The .exe test only applies on Windows
	for _, tt := range tests {
		got := extractModuleName(tt.filename)
		if tt.filename == "penguin-mod-vpn.exe" && runtime.GOOS != "windows" {
			// On non-Windows, .exe is not stripped
			if got != "vpn.exe" {
				t.Errorf("extractModuleName(%q) = %q, want %q (non-windows)", tt.filename, got, "vpn.exe")
			}
			continue
		}
		if got != tt.want {
			t.Errorf("extractModuleName(%q) = %q, want %q", tt.filename, got, tt.want)
		}
	}
}

func TestDiscoverEmptyDir(t *testing.T) {
	dir := t.TempDir()
	logger := logrus.New()
	logger.SetOutput(io.Discard)

	d := NewDiscovery([]string{dir}, logger)
	plugins, err := d.Discover()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(plugins) != 0 {
		t.Errorf("expected 0 plugins, got %d", len(plugins))
	}
}

func TestDiscoverNonExistentDir(t *testing.T) {
	logger := logrus.New()
	logger.SetOutput(io.Discard)

	d := NewDiscovery([]string{"/nonexistent/path"}, logger)
	plugins, err := d.Discover()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(plugins) != 0 {
		t.Errorf("expected 0 plugins, got %d", len(plugins))
	}
}

func TestDiscoverFindsPlugins(t *testing.T) {
	dir := t.TempDir()
	logger := logrus.New()
	logger.SetOutput(io.Discard)

	// Create fake plugin binaries
	for _, name := range []string{"penguin-mod-vpn", "penguin-mod-dns", "not-a-plugin"} {
		path := filepath.Join(dir, name)
		if err := os.WriteFile(path, []byte("#!/bin/sh\n"), 0755); err != nil {
			t.Fatal(err)
		}
	}

	d := NewDiscovery([]string{dir}, logger)
	plugins, err := d.Discover()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if len(plugins) != 2 {
		t.Fatalf("expected 2 plugins, got %d", len(plugins))
	}

	names := map[string]bool{}
	for _, p := range plugins {
		names[p.Name] = true
	}
	if !names["vpn"] {
		t.Error("expected vpn plugin")
	}
	if !names["dns"] {
		t.Error("expected dns plugin")
	}
}

func TestDiscoverSkipsDuplicates(t *testing.T) {
	dir1 := t.TempDir()
	dir2 := t.TempDir()
	logger := logrus.New()
	logger.SetOutput(io.Discard)

	// Same plugin in both dirs
	for _, dir := range []string{dir1, dir2} {
		path := filepath.Join(dir, "penguin-mod-vpn")
		if err := os.WriteFile(path, []byte("#!/bin/sh\n"), 0755); err != nil {
			t.Fatal(err)
		}
	}

	d := NewDiscovery([]string{dir1, dir2}, logger)
	plugins, err := d.Discover()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if len(plugins) != 1 {
		t.Errorf("expected 1 plugin (deduped), got %d", len(plugins))
	}
}

func TestDiscoverSkipsNonExecutable(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("skipping on windows — no exec bit check")
	}

	dir := t.TempDir()
	logger := logrus.New()
	logger.SetOutput(io.Discard)

	// Non-executable file
	path := filepath.Join(dir, "penguin-mod-vpn")
	if err := os.WriteFile(path, []byte("not executable"), 0644); err != nil {
		t.Fatal(err)
	}

	d := NewDiscovery([]string{dir}, logger)
	plugins, err := d.Discover()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(plugins) != 0 {
		t.Errorf("expected 0 plugins (non-executable), got %d", len(plugins))
	}
}

func TestDefaultSearchPaths(t *testing.T) {
	paths := DefaultSearchPaths("/tmp/test-config")
	if len(paths) == 0 {
		t.Error("expected at least one search path")
	}
	if paths[0] != "plugins" {
		t.Errorf("expected first path to be 'plugins', got %s", paths[0])
	}
}
