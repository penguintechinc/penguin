//go:build darwin
// +build darwin

package squawk

import (
	"context"
	"fmt"
	"net/netip"
	"os"
	"os/exec"
	"strings"
	"time"

	"go.uber.org/zap"
)

// applyPlatform applies DNS servers on macOS via networksetup.
func (r *resolver) applyPlatform(ctx context.Context, servers []netip.Addr) error {
	if len(servers) == 0 {
		return fmt.Errorf("no DNS servers provided")
	}

	// Get list of network services
	services, err := r.getNetworkServices()
	if err != nil {
		return fmt.Errorf("get network services: %w", err)
	}

	if len(services) == 0 {
		return fmt.Errorf("no network services found")
	}

	// Apply DNS to each service (excluding PPP)
	for _, service := range services {
		if strings.Contains(strings.ToLower(service), "ppp") {
			continue // Skip PPP services
		}

		args := []string{"-setdnsservers", service}
		for _, server := range servers {
			args = append(args, server.String())
		}

		cmd := exec.CommandContext(ctx, "networksetup", args...)
		if err := cmd.Run(); err != nil {
			r.logger.Warn("failed to set DNS for service",
				fmt.Sprintf("service=%s err=%v", service, err))
			// Non-fatal: continue with other services
		}
	}

	r.logger.Info("DNS applied via networksetup")
	return nil
}

// restorePlatform restores DNS servers on macOS by setting to "Empty".
func (r *resolver) restorePlatform(ctx context.Context, servers []netip.Addr) error {
	services, err := r.getNetworkServices()
	if err != nil {
		return fmt.Errorf("get network services: %w", err)
	}

	// Restore to "Empty" (auto-DHCP) for each service
	for _, service := range services {
		if strings.Contains(strings.ToLower(service), "ppp") {
			continue
		}

		cmd := exec.CommandContext(ctx, "networksetup", "-setdnsservers", service, "Empty")
		if err := cmd.Run(); err != nil {
			r.logger.Warn("failed to restore DNS for service",
				fmt.Sprintf("service=%s err=%v", service, err))
		}
	}

	r.logger.Info("DNS restored via networksetup")
	return nil
}

// currentPlatform reads the current DNS servers on macOS.
func (r *resolver) currentPlatform(ctx context.Context) ([]netip.Addr, error) {
	services, err := r.getNetworkServices()
	if err != nil {
		return nil, fmt.Errorf("get network services: %w", err)
	}

	if len(services) == 0 {
		return nil, fmt.Errorf("no network services found")
	}

	// Get DNS for the first active service
	cmd := exec.CommandContext(ctx, "networksetup", "-getdnsservers", services[0])
	output, err := cmd.Output()
	if err != nil {
		return nil, fmt.Errorf("get DNS servers: %w", err)
	}

	var servers []netip.Addr
	for _, line := range strings.Split(string(output), "\n") {
		line = strings.TrimSpace(line)
		if line != "" && !strings.HasPrefix(line, "DNS") && line != "There aren't any DNS Servers set on" {
			if addr, err := netip.ParseAddr(line); err == nil {
				servers = append(servers, addr)
			}
		}
	}

	if len(servers) == 0 {
		return nil, fmt.Errorf("no DNS servers configured")
	}

	return servers, nil
}

// captureState captures the current resolver state on macOS.
func (r *resolver) captureState() string {
	// Capture scutil DNS state for recovery purposes
	cmd := exec.Command("scutil", "-c", "show State:/Network/Global/DNS")
	output, err := cmd.Output()
	if err != nil {
		return ""
	}
	return string(output)
}

// getNetworkServices returns list of network services on macOS.
func (r *resolver) getNetworkServices() ([]string, error) {
	cmd := exec.Command("networksetup", "-listallnetworkservices")
	output, err := cmd.Output()
	if err != nil {
		return nil, fmt.Errorf("networksetup: %w", err)
	}

	var services []string
	for _, line := range strings.Split(string(output), "\n") {
		line = strings.TrimSpace(line)
		if line != "" && !strings.HasPrefix(line, "(") {
			services = append(services, line)
		}
	}

	return services, nil
}

func getCurrentUnixTime() int64 {
	return time.Now().Unix()
}
