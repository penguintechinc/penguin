//go:build linux
// +build linux

package squawk

import (
	"context"
	"net/netip"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"go.uber.org/zap/zaptest"
)

// TestLinuxResolvConfSeam tests that the seam var allows injection of temp resolv.conf path
func TestLinuxResolvConfSeam(t *testing.T) {
	tempDir := t.TempDir()
	oldResolvConfPath := resolvConfPath
	defer func() { resolvConfPath = oldResolvConfPath }()

	// Create temp resolv.conf
	tempResolv := filepath.Join(tempDir, "resolv.conf")
	tempContent := "nameserver 8.8.8.8\nnameserver 8.8.4.4\n"
	if err := os.WriteFile(tempResolv, []byte(tempContent), 0o644); err != nil { // #nosec G306
		t.Fatalf("failed to create temp resolv.conf: %v", err)
	}

	// Inject seam
	resolvConfPath = tempResolv

	logger := zaptest.NewLogger(t)
	resolver := &resolver{
		dataDir: tempDir,
		logger:  logger,
	}

	// Test currentPlatform reads from injected path
	ctx := context.Background()
	servers, err := resolver.currentPlatform(ctx)
	if err != nil {
		t.Fatalf("currentPlatform failed: %v", err)
	}

	if len(servers) != 2 {
		t.Errorf("expected 2 servers, got %d", len(servers))
	}
	if servers[0].String() != "8.8.8.8" {
		t.Errorf("expected first server 8.8.8.8, got %s", servers[0].String())
	}
}

// TestLinuxCaptureStateWithSeam tests captureState with injected path
func TestLinuxCaptureStateWithSeam(t *testing.T) {
	tempDir := t.TempDir()
	oldResolvConfPath := resolvConfPath
	defer func() { resolvConfPath = oldResolvConfPath }()

	// Create temp resolv.conf
	tempResolv := filepath.Join(tempDir, "resolv.conf")
	tempContent := "# comment\nnameserver 1.1.1.1\n"
	if err := os.WriteFile(tempResolv, []byte(tempContent), 0o644); err != nil { // #nosec G306
		t.Fatalf("failed to create temp resolv.conf: %v", err)
	}

	resolvConfPath = tempResolv

	logger := zaptest.NewLogger(t)
	resolver := &resolver{
		dataDir: tempDir,
		logger:  logger,
	}

	state := resolver.captureState()
	if state == "" {
		t.Errorf("expected non-empty state")
	}
	if !strings.Contains(state, "nameserver 1.1.1.1") {
		t.Errorf("state should contain nameserver line")
	}
}

// TestLinuxApplyViaResolvConf tests applyViaResolvConf with injected path
func TestLinuxApplyViaResolvConf(t *testing.T) {
	tempDir := t.TempDir()
	oldResolvConfPath := resolvConfPath
	defer func() { resolvConfPath = oldResolvConfPath }()

	// Create temp resolv.conf
	tempResolv := filepath.Join(tempDir, "resolv.conf")
	if err := os.WriteFile(tempResolv, []byte("nameserver 8.8.8.8\n"), 0o644); err != nil { // #nosec G306
		t.Fatalf("failed to create temp resolv.conf: %v", err)
	}

	resolvConfPath = tempResolv

	logger := zaptest.NewLogger(t)
	resolver := &resolver{
		dataDir: tempDir,
		logger:  logger,
	}

	servers := []netip.Addr{
		netip.MustParseAddr("1.1.1.1"),
		netip.MustParseAddr("1.0.0.1"),
	}

	if err := resolver.applyViaResolvConf(servers); err != nil {
		t.Fatalf("applyViaResolvConf failed: %v", err)
	}

	// Verify backup was created
	backupPath := filepath.Join(tempDir, "resolv.conf.backup")
	backup, err := os.ReadFile(backupPath) // #nosec G304
	if err != nil {
		t.Fatalf("backup not created: %v", err)
	}
	if !strings.Contains(string(backup), "8.8.8.8") {
		t.Errorf("backup should contain original nameserver")
	}

	// Verify new resolv.conf was written
	newContent, err := os.ReadFile(tempResolv) // #nosec G304
	if err != nil {
		t.Fatalf("failed to read updated resolv.conf: %v", err)
	}
	if !strings.Contains(string(newContent), "1.1.1.1") {
		t.Errorf("new resolv.conf should contain new nameserver")
	}
}

// TestLinuxRestoreViaResolvConf tests restoreViaResolvConf with backup
func TestLinuxRestoreViaResolvConf(t *testing.T) {
	tempDir := t.TempDir()
	oldResolvConfPath := resolvConfPath
	defer func() { resolvConfPath = oldResolvConfPath }()

	tempResolv := filepath.Join(tempDir, "resolv.conf")
	if err := os.WriteFile(tempResolv, []byte("nameserver 1.1.1.1\n"), 0o644); err != nil { // #nosec G306
		t.Fatalf("failed to create temp resolv.conf: %v", err)
	}

	resolvConfPath = tempResolv

	logger := zaptest.NewLogger(t)
	resolver := &resolver{
		dataDir: tempDir,
		logger:  logger,
	}

	// Create backup
	backupPath := filepath.Join(tempDir, "resolv.conf.backup")
	originalContent := "nameserver 8.8.8.8\n"
	if err := os.WriteFile(backupPath, []byte(originalContent), 0o644); err != nil { // #nosec G306
		t.Fatalf("failed to create backup: %v", err)
	}

	servers := []netip.Addr{netip.MustParseAddr("1.1.1.1")}
	if err := resolver.restoreViaResolvConf(servers); err != nil {
		t.Fatalf("restoreViaResolvConf failed: %v", err)
	}

	// Verify resolv.conf was restored
	restored, err := os.ReadFile(tempResolv) // #nosec G304
	if err != nil {
		t.Fatalf("failed to read restored resolv.conf: %v", err)
	}
	if !strings.Contains(string(restored), "8.8.8.8") {
		t.Errorf("restored resolv.conf should contain original nameserver")
	}

	// Verify backup was deleted
	if _, err := os.Stat(backupPath); err == nil {
		t.Errorf("backup should be deleted after restore")
	}
}

// TestLinuxApplyAndRestoreRoundTrip tests full apply→capture→restore cycle
func TestLinuxApplyAndRestoreRoundTrip(t *testing.T) {
	tempDir := t.TempDir()
	oldResolvConfPath := resolvConfPath
	defer func() { resolvConfPath = oldResolvConfPath }()

	tempResolv := filepath.Join(tempDir, "resolv.conf")
	originalContent := "nameserver 8.8.8.8\nnameserver 8.8.4.4\n"
	if err := os.WriteFile(tempResolv, []byte(originalContent), 0o644); err != nil { // #nosec G306
		t.Fatalf("failed to create temp resolv.conf: %v", err)
	}

	resolvConfPath = tempResolv

	logger := zaptest.NewLogger(t)
	resolver := &resolver{
		dataDir: tempDir,
		logger:  logger,
	}

	ctx := context.Background()

	// Read original
	originalServers, err := resolver.currentPlatform(ctx)
	if err != nil {
		t.Fatalf("failed to read original servers: %v", err)
	}

	// Apply new servers
	newServers := []netip.Addr{netip.MustParseAddr("1.1.1.1")}
	if err := resolver.applyViaResolvConf(newServers); err != nil {
		t.Fatalf("applyViaResolvConf failed: %v", err)
	}

	// Verify new servers are in place
	currentServers, err := resolver.currentPlatform(ctx)
	if err != nil {
		t.Fatalf("failed to read current servers: %v", err)
	}
	if len(currentServers) != 1 || currentServers[0].String() != "1.1.1.1" {
		t.Errorf("new servers not applied correctly")
	}

	// Restore
	if err := resolver.restoreViaResolvConf(originalServers); err != nil {
		t.Fatalf("restoreViaResolvConf failed: %v", err)
	}

	// Verify original is restored
	restoredServers, err := resolver.currentPlatform(ctx)
	if err != nil {
		t.Fatalf("failed to read restored servers: %v", err)
	}
	if len(restoredServers) != len(originalServers) {
		t.Errorf("expected %d servers after restore, got %d", len(originalServers), len(restoredServers))
	}
}

// TestLinuxCurrentPlatformParsingMultipleServers tests parsing multiple servers
func TestLinuxCurrentPlatformParsingMultipleServers(t *testing.T) {
	tempDir := t.TempDir()
	oldResolvConfPath := resolvConfPath
	defer func() { resolvConfPath = oldResolvConfPath }()

	tempResolv := filepath.Join(tempDir, "resolv.conf")
	content := "# Comment line\nnameserver 8.8.8.8\n# Another comment\nnameserver 8.8.4.4\nnameserver 1.1.1.1\n"
	if err := os.WriteFile(tempResolv, []byte(content), 0o644); err != nil { // #nosec G306
		t.Fatalf("failed to create temp resolv.conf: %v", err)
	}

	resolvConfPath = tempResolv

	logger := zaptest.NewLogger(t)
	resolver := &resolver{
		dataDir: tempDir,
		logger:  logger,
	}

	ctx := context.Background()
	servers, err := resolver.currentPlatform(ctx)
	if err != nil {
		t.Fatalf("currentPlatform failed: %v", err)
	}

	if len(servers) != 3 {
		t.Errorf("expected 3 servers, got %d", len(servers))
	}

	expectedServers := []string{"8.8.8.8", "8.8.4.4", "1.1.1.1"}
	for i, expected := range expectedServers {
		if i < len(servers) && servers[i].String() != expected {
			t.Errorf("server %d: expected %s, got %s", i, expected, servers[i].String())
		}
	}
}

// TestLinuxApplyViaResolvConfCreatesBackup verifies backup creation
func TestLinuxApplyViaResolvConfCreatesBackup(t *testing.T) {
	tempDir := t.TempDir()
	oldResolvConfPath := resolvConfPath
	defer func() { resolvConfPath = oldResolvConfPath }()

	tempResolv := filepath.Join(tempDir, "resolv.conf")
	originalContent := "nameserver 8.8.8.8\n"
	if err := os.WriteFile(tempResolv, []byte(originalContent), 0o644); err != nil { // #nosec G306
		t.Fatalf("failed to create temp resolv.conf: %v", err)
	}

	resolvConfPath = tempResolv

	logger := zaptest.NewLogger(t)
	resolver := &resolver{
		dataDir: tempDir,
		logger:  logger,
	}

	servers := []netip.Addr{netip.MustParseAddr("1.1.1.1")}
	if err := resolver.applyViaResolvConf(servers); err != nil {
		t.Fatalf("applyViaResolvConf failed: %v", err)
	}

	// Verify backup exists and has correct content
	backupPath := filepath.Join(tempDir, "resolv.conf.backup")
	backup, err := os.ReadFile(backupPath) // #nosec G304
	if err != nil {
		t.Fatalf("backup not found: %v", err)
	}

	if string(backup) != originalContent {
		t.Errorf("backup content mismatch: expected %q, got %q", originalContent, string(backup))
	}
}
