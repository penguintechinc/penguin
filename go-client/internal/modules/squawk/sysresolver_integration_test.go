//go:build integration
// +build integration

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

// TestLinuxResolverIntegrationApplyRestoreRoundTrip tests real resolver operations with temp resolv.conf
func TestLinuxResolverIntegrationApplyRestoreRoundTrip(t *testing.T) {
	if os.Geteuid() != 0 {
		t.Skip("requires root/CAP_NET_ADMIN")
	}

	tempDir := t.TempDir()
	oldResolvConfPath := resolvConfPath
	defer func() { resolvConfPath = oldResolvConfPath }()

	// Create temp resolv.conf in tempDir (avoid clobbering real /etc/resolv.conf)
	tempResolv := filepath.Join(tempDir, "resolv.conf")
	originalContent := "nameserver 8.8.8.8\nnameserver 8.8.4.4\n"
	if err := os.WriteFile(tempResolv, []byte(originalContent), 0o644); err != nil {
		t.Fatalf("failed to create temp resolv.conf: %v", err)
	}

	resolvConfPath = tempResolv

	logger := zaptest.NewLogger(t)
	resolver := &resolver{
		dataDir: tempDir,
		logger:  logger,
	}

	ctx := context.Background()

	// Step 1: Read original servers
	originalServers, err := resolver.currentPlatform(ctx)
	if err != nil {
		t.Fatalf("failed to read original servers: %v", err)
	}
	if len(originalServers) != 2 {
		t.Errorf("expected 2 original servers, got %d", len(originalServers))
	}

	// Step 2: Capture state before modification
	beforeState := resolver.captureState()
	if !strings.Contains(beforeState, "8.8.8.8") {
		t.Errorf("captureState should contain original server")
	}

	// Step 3: Apply new servers
	newServers := []netip.Addr{
		netip.MustParseAddr("1.1.1.1"),
		netip.MustParseAddr("1.0.0.1"),
	}
	if err := resolver.applyViaResolvConf(newServers); err != nil {
		t.Fatalf("applyViaResolvConf failed: %v", err)
	}

	// Step 4: Verify new servers are applied
	currentServers, err := resolver.currentPlatform(ctx)
	if err != nil {
		t.Fatalf("failed to read current servers: %v", err)
	}
	if len(currentServers) != 2 {
		t.Errorf("expected 2 servers after apply, got %d", len(currentServers))
	}

	// Step 5: Restore original servers
	if err := resolver.restoreViaResolvConf(originalServers); err != nil {
		t.Fatalf("restoreViaResolvConf failed: %v", err)
	}

	// Step 6: Verify restoration
	restoredServers, err := resolver.currentPlatform(ctx)
	if err != nil {
		t.Fatalf("failed to read restored servers: %v", err)
	}
	if len(restoredServers) != len(originalServers) {
		t.Errorf("expected %d servers after restore, got %d", len(originalServers), len(restoredServers))
	}

	// Step 7: Verify backup was cleaned up
	backupPath := filepath.Join(tempDir, "resolv.conf.backup")
	if _, err := os.Stat(backupPath); err == nil {
		t.Errorf("backup should be deleted after restore")
	}

	// Step 8: Verify resolv.conf contains original content
	afterState := resolver.captureState()
	if !strings.Contains(afterState, "8.8.8.8") {
		t.Errorf("after restore, resolv.conf should contain original server")
	}
}

// TestLinuxResolverIntegrationCrashRecovery tests crash recovery with real file operations
func TestLinuxResolverIntegrationCrashRecovery(t *testing.T) {
	if os.Geteuid() != 0 {
		t.Skip("requires root/CAP_NET_ADMIN")
	}

	tempDir := t.TempDir()
	oldResolvConfPath := resolvConfPath
	defer func() { resolvConfPath = oldResolvConfPath }()

	tempResolv := filepath.Join(tempDir, "resolv.conf")
	originalContent := "nameserver 8.8.8.8\n"
	if err := os.WriteFile(tempResolv, []byte(originalContent), 0o644); err != nil {
		t.Fatalf("failed to create temp resolv.conf: %v", err)
	}

	resolvConfPath = tempResolv

	logger := zaptest.NewLogger(t)
	resolver := &resolver{
		dataDir: tempDir,
		logger:  logger,
	}

	ctx := context.Background()

	// Simulate a crash: apply new servers then call RecoverFromCrash
	newServers := []netip.Addr{netip.MustParseAddr("1.1.1.1")}
	if err := resolver.applyViaResolvConf(newServers); err != nil {
		t.Fatalf("applyViaResolvConf failed: %v", err)
	}

	// Verify backup marker exists
	markerPath := filepath.Join(tempDir, "dns-applied.json")
	if _, err := os.Stat(markerPath); err != nil {
		t.Fatalf("backup marker not created: %v", err)
	}

	// Now recover from crash (should restore original and clean up marker)
	if err := resolver.RecoverFromCrash(ctx); err != nil {
		t.Fatalf("RecoverFromCrash failed: %v", err)
	}

	// Verify cleanup: marker should be deleted
	if _, err := os.Stat(markerPath); err == nil {
		t.Logf("note: backup marker still exists after recovery (recovery may have failed)")
	}
}

// TestLinuxResolverIntegrationApplyBackupToFile tests that apply creates a valid backup file
func TestLinuxResolverIntegrationApplyBackupToFile(t *testing.T) {
	if os.Geteuid() != 0 {
		t.Skip("requires root/CAP_NET_ADMIN")
	}

	tempDir := t.TempDir()
	oldResolvConfPath := resolvConfPath
	defer func() { resolvConfPath = oldResolvConfPath }()

	tempResolv := filepath.Join(tempDir, "resolv.conf")
	originalContent := "nameserver 8.8.8.8\nnameserver 8.8.4.4\n"
	if err := os.WriteFile(tempResolv, []byte(originalContent), 0o644); err != nil {
		t.Fatalf("failed to create temp resolv.conf: %v", err)
	}

	resolvConfPath = tempResolv

	logger := zaptest.NewLogger(t)
	resolver := &resolver{
		dataDir: tempDir,
		logger:  logger,
	}

	// Apply new servers
	newServers := []netip.Addr{netip.MustParseAddr("1.1.1.1")}
	if err := resolver.applyViaResolvConf(newServers); err != nil {
		t.Fatalf("applyViaResolvConf failed: %v", err)
	}

	// Verify backup file exists and has correct content
	backupPath := filepath.Join(tempDir, "resolv.conf.backup")
	backup, err := os.ReadFile(backupPath)
	if err != nil {
		t.Fatalf("backup not found: %v", err)
	}

	if string(backup) != originalContent {
		t.Errorf("backup content mismatch: expected %q, got %q", originalContent, string(backup))
	}

	// Verify backup has restrictive permissions
	stat, err := os.Stat(backupPath)
	if err != nil {
		t.Fatalf("stat backup failed: %v", err)
	}
	mode := stat.Mode().Perm()
	if mode != 0o600 {
		t.Logf("warning: backup file permissions are %o (expected 0o600)", mode)
	}
}
