package squawk

import (
	"context"
	"encoding/json"
	"fmt"
	"net/netip"
	"os"
	"path/filepath"
	"sync"

	"go.uber.org/zap"
)

// SysResolver manages system DNS resolver settings.
// It provides platform-specific DNS resolution management with crash recovery.
type SysResolver interface {
	// Apply sets the system DNS resolver to the given servers.
	Apply(ctx context.Context, servers []netip.Addr) error

	// Restore reverts the system DNS to its previous state.
	Restore(ctx context.Context) error

	// Current returns the current system DNS servers.
	Current(ctx context.Context) ([]netip.Addr, error)

	// RecoverFromCrash checks for a crash recovery marker and restores if present.
	RecoverFromCrash(ctx context.Context) error
}

// resolver implements SysResolver with platform-specific logic.
type resolver struct {
	dataDir  string
	logger   *zap.Logger
	backup   *DNSBackup
	backupMu sync.RWMutex
}

// DNSBackup contains the previous DNS state for recovery.
type DNSBackup struct {
	PreviousServers []string `json:"previous_servers"`
	PreviousState   string   `json:"previous_state"` // Platform-specific state blob
	AppliedAt       string   `json:"applied_at"`
}

// NewSysResolver creates a new system resolver for the current platform.
func NewSysResolver(dataDir string, logger *zap.Logger) SysResolver {
	return &resolver{
		dataDir: dataDir,
		logger:  logger,
	}
}

// Apply sets system DNS to the given servers with automatic backup for recovery.
func (r *resolver) Apply(ctx context.Context, servers []netip.Addr) error {
	if len(servers) == 0 {
		return fmt.Errorf("no DNS servers provided")
	}

	r.backupMu.Lock()
	defer r.backupMu.Unlock()

	// Save current state before applying
	current, err := r.Current(ctx)
	if err != nil {
		r.logger.Warn("could not read current DNS state", zap.Error(err))
	}

	// Call platform-specific apply
	if err := r.applyPlatform(ctx, servers); err != nil {
		return fmt.Errorf("apply platform DNS: %w", err)
	}

	// Write backup marker for crash recovery
	backup := &DNSBackup{
		PreviousServers: make([]string, len(current)),
		AppliedAt:       fmt.Sprintf("%d", getCurrentUnixTime()),
	}
	for i, addr := range current {
		backup.PreviousServers[i] = addr.String()
	}

	// Capture platform-specific state for recovery
	backup.PreviousState = r.captureState()

	if err := r.writeBackup(backup); err != nil {
		r.logger.Warn("failed to write backup marker (recovery may fail)", zap.Error(err))
		// Non-fatal: continue with applied DNS
	}

	r.backup = backup
	r.logger.Info("DNS applied", zap.Strings("servers", backup.PreviousServers))
	return nil
}

// Restore reverts to the previous DNS state.
func (r *resolver) Restore(ctx context.Context) error {
	r.backupMu.Lock()
	defer r.backupMu.Unlock()

	// Try to read backup from disk if not in memory
	if r.backup == nil {
		if backup, err := r.readBackup(); err == nil {
			r.backup = backup
		} else {
			r.logger.Warn("no backup found; cannot restore", zap.Error(err))
			return fmt.Errorf("no backup state available")
		}
	}

	// Convert previous servers back to netip.Addr
	servers := make([]netip.Addr, 0, len(r.backup.PreviousServers))
	for _, s := range r.backup.PreviousServers {
		addr, err := netip.ParseAddr(s)
		if err != nil {
			r.logger.Warn("invalid address in backup", zap.String("addr", s), zap.Error(err))
			continue
		}
		servers = append(servers, addr)
	}

	if len(servers) == 0 {
		r.logger.Warn("no valid servers in backup")
		servers = []netip.Addr{netip.MustParseAddr("1.1.1.1")} // Fallback
	}

	// Call platform-specific restore
	if err := r.restorePlatform(ctx, servers); err != nil {
		return fmt.Errorf("restore platform DNS: %w", err)
	}

	// Clean up backup marker
	_ = r.deleteBackup()

	r.logger.Info("DNS restored", zap.Strings("servers", r.backup.PreviousServers))
	r.backup = nil
	return nil
}

// Current returns the current system DNS servers.
func (r *resolver) Current(ctx context.Context) ([]netip.Addr, error) {
	return r.currentPlatform(ctx)
}

// RecoverFromCrash checks for a backup marker and restores if present.
func (r *resolver) RecoverFromCrash(ctx context.Context) error {
	r.backupMu.Lock()
	defer r.backupMu.Unlock()

	backup, err := r.readBackup()
	if err != nil {
		// No backup present is not an error
		return nil
	}

	r.logger.Info("crash recovery: found backup marker, restoring DNS state",
		zap.Strings("servers", backup.PreviousServers))

	// Convert servers
	servers := make([]netip.Addr, 0, len(backup.PreviousServers))
	for _, s := range backup.PreviousServers {
		addr, err := netip.ParseAddr(s)
		if err != nil {
			continue
		}
		servers = append(servers, addr)
	}

	if len(servers) > 0 {
		if err := r.restorePlatform(ctx, servers); err != nil {
			r.logger.Error("crash recovery restore failed", zap.Error(err))
			return err
		}
	}

	// Clean up backup
	return r.deleteBackup()
}

// Helper methods

func (r *resolver) writeBackup(backup *DNSBackup) error {
	data, err := json.Marshal(backup)
	if err != nil {
		return fmt.Errorf("marshal backup: %w", err)
	}

	markerPath := filepath.Join(r.dataDir, "dns-applied.json")
	if err := os.WriteFile(markerPath, data, 0o600); err != nil {
		return fmt.Errorf("write backup marker: %w", err)
	}

	return nil
}

func (r *resolver) readBackup() (*DNSBackup, error) {
	markerPath := filepath.Join(r.dataDir, "dns-applied.json")
	// #nosec G304 - markerPath is constructed from dataDir which is controlled
	data, err := os.ReadFile(markerPath)
	if err != nil {
		return nil, fmt.Errorf("read backup marker: %w", err)
	}

	var backup DNSBackup
	if err := json.Unmarshal(data, &backup); err != nil {
		return nil, fmt.Errorf("unmarshal backup: %w", err)
	}

	return &backup, nil
}

func (r *resolver) deleteBackup() error {
	markerPath := filepath.Join(r.dataDir, "dns-applied.json")
	if err := os.Remove(markerPath); err != nil && !os.IsNotExist(err) {
		return fmt.Errorf("delete backup marker: %w", err)
	}
	return nil
}

// Platform-specific implementations are in sysresolver_*os*.go files
