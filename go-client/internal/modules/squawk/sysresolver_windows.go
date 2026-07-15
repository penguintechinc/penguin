//go:build windows
// +build windows

package squawk

import (
	"context"
	"fmt"
	"net/netip"
	"os/exec"
	"strings"
	"time"

	"go.uber.org/zap"
)

// applyPlatform applies DNS servers on Windows via netsh.
func (r *resolver) applyPlatform(ctx context.Context, servers []netip.Addr) error {
	if len(servers) == 0 {
		return fmt.Errorf("no DNS servers provided")
	}

	// Get list of network interfaces
	interfaces, err := r.getNetworkInterfaces()
	if err != nil {
		return fmt.Errorf("get network interfaces: %w", err)
	}

	if len(interfaces) == 0 {
		return fmt.Errorf("no network interfaces found")
	}

	// Apply DNS to each interface
	for _, iface := range interfaces {
		// Set static DNS
		args := []string{"interface", "ip", "set", "dnsservers", "name=" + iface, "static", servers[0].String()}
		if len(servers) > 1 {
			args = append(args, "index=2")
			args = append(args, servers[1].String())
		}

		cmd := exec.CommandContext(ctx, "netsh", args...)
		if err := cmd.Run(); err != nil {
			r.logger.Warn("failed to set DNS for interface",
				zap.String("interface", iface), zap.Error(err))
		}
	}

	r.logger.Info("DNS applied via netsh")
	return nil
}

// restorePlatform restores DNS servers on Windows via DHCP.
func (r *resolver) restorePlatform(ctx context.Context, servers []netip.Addr) error {
	interfaces, err := r.getNetworkInterfaces()
	if err != nil {
		return fmt.Errorf("get network interfaces: %w", err)
	}

	// Restore to DHCP for each interface
	for _, iface := range interfaces {
		cmd := exec.CommandContext(ctx, "netsh", "interface", "ip", "set", "dnsservers", "name="+iface, "dhcp")
		if err := cmd.Run(); err != nil {
			r.logger.Warn("failed to restore DNS for interface",
				zap.String("interface", iface), zap.Error(err))
		}
	}

	r.logger.Info("DNS restored via netsh")
	return nil
}

// currentPlatform reads the current DNS servers on Windows.
func (r *resolver) currentPlatform(ctx context.Context) ([]netip.Addr, error) {
	interfaces, err := r.getNetworkInterfaces()
	if err != nil {
		return nil, fmt.Errorf("get network interfaces: %w", err)
	}

	if len(interfaces) == 0 {
		return nil, fmt.Errorf("no network interfaces found")
	}

	// Get DNS for the first interface
	cmd := exec.CommandContext(ctx, "netsh", "interface", "ip", "show", "dns", interfaces[0])
	output, err := cmd.Output()
	if err != nil {
		return nil, fmt.Errorf("show DNS: %w", err)
	}

	var servers []netip.Addr
	for _, line := range strings.Split(string(output), "\n") {
		line = strings.TrimSpace(line)
		// Parse "Static IP Address" or "DHCP-configured IP Address"
		if strings.Contains(line, ":") {
			parts := strings.Split(line, ":")
			if len(parts) > 1 {
				addr := strings.TrimSpace(parts[1])
				if addr, err := netip.ParseAddr(addr); err == nil {
					servers = append(servers, addr)
				}
			}
		}
	}

	if len(servers) == 0 {
		return nil, fmt.Errorf("no DNS servers configured")
	}

	return servers, nil
}

// captureState captures the current resolver state on Windows.
func (r *resolver) captureState() string {
	cmd := exec.Command("netsh", "interface", "ip", "show", "dns", "all")
	output, err := cmd.Output()
	if err != nil {
		return ""
	}
	return string(output)
}

// getNetworkInterfaces returns list of network interfaces on Windows.
func (r *resolver) getNetworkInterfaces() ([]string, error) {
	cmd := exec.Command("netsh", "interface", "show", "interface")
	output, err := cmd.Output()
	if err != nil {
		return nil, fmt.Errorf("show interface: %w", err)
	}

	var interfaces []string
	for _, line := range strings.Split(string(output), "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "Admin State") {
			continue
		}
		// Parse interface name from netsh output
		fields := strings.Fields(line)
		if len(fields) >= 4 {
			// Last field is typically the interface name
			interfaces = append(interfaces, fields[len(fields)-1])
		}
	}

	return interfaces, nil
}

func getCurrentUnixTime() int64 {
	return time.Now().Unix()
}
