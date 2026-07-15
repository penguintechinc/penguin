//go:build linux
// +build linux

package squawk

import (
	"context"
	"fmt"
	"net/netip"
)

// applyViaSystemdResolved applies DNS via systemd-resolved D-Bus (stub).
// This requires go-systemd/dbus; for now, this is a placeholder.
func (r *resolver) applyViaSystemdResolved(ctx context.Context, servers []netip.Addr) error {
	// TODO: Implement via go-systemd or gdbus if needed.
	// For now, return error to fall through to /etc/resolv.conf
	return fmt.Errorf("systemd-resolved not available")
}

// restoreViaSystemdResolved restores DNS via systemd-resolved (stub).
func (r *resolver) restoreViaSystemdResolved(ctx context.Context, servers []netip.Addr) error {
	return fmt.Errorf("systemd-resolved not available")
}
