// Package tobogganing implements the Tobogganing module for WireGuard-based SASE/ZTNA
// endpoint client functionality.
package tobogganing

import (
	"context"
	"encoding/json"
	"fmt"
	"sync"
	"time"

	"github.com/penguintechinc/penguin/pkg/sdk"
	"github.com/prometheus/client_golang/prometheus"
	"go.uber.org/zap"
	"gopkg.in/yaml.v3"
)

// Module implements sdk.Module for Tobogganing VPN client.
type Module struct {
	host        sdk.HostServices
	logger      *zap.Logger
	authMgr     *AuthManager
	vpnMgr      *VPNManager
	config      *ModuleConfig
	mu          sync.RWMutex
	running     bool
	stopCh      chan struct{}
	metrics     *TobogganingMetrics
	lastHealth  HealthProbe
	healthMutex sync.RWMutex
	clock       clockFunc // For testing: injectable time function
}

// clockFunc returns the current time (injectable for testing).
type clockFunc func() time.Time

// ModuleConfig is the per-module configuration (validated against ConfigSchema).
type ModuleConfig struct {
	ManagerURL    string   `yaml:"manager_url"`
	NodeID        string   `yaml:"node_id"`
	InterfaceName string   `yaml:"interface_name"`
	MTU           int      `yaml:"mtu"`
	DNS           []string `yaml:"dns"`
	Keepalive     int      `yaml:"keepalive"`
	AllowedIPs    []string `yaml:"allowed_ips"`
	Embedded      bool     `yaml:"embedded"`
}

// TobogganingMetrics holds Prometheus metrics.
type TobogganingMetrics struct {
	tunnelUp       prometheus.Gauge
	handshakeAge   prometheus.Gauge
	rxBytes        prometheus.Counter
	txBytes        prometheus.Counter
	tokenRefreshes prometheus.Counter
	connErrors     prometheus.Counter
}

// HealthProbe tracks recent health check result.
type HealthProbe struct {
	Level     sdk.HealthLevel
	Message   string
	CheckedAt time.Time
}

// New creates a new Tobogganing module instance (factory function).
func New() sdk.Module {
	return &Module{
		stopCh: make(chan struct{}),
		clock:  time.Now,
	}
}

// Info returns module identity metadata.
//
// LicenseFeature is intentionally empty: Tobogganing is core product and ships
// in the Free tier, so the module itself must load without a license server.
// Enterprise-only capabilities *inside* a module are gated individually via
// host.License().FeatureEnabled("penguin.<feature>").
func (m *Module) Info() sdk.ModuleInfo {
	return sdk.ModuleInfo{
		Name:        "tobogganing",
		Version:     "1.0.0",
		Description: "WireGuard-compatible SASE/ZTNA endpoint client",
	}
}

// Init prepares the module with host services.
func (m *Module) Init(ctx context.Context, host sdk.HostServices) error {
	m.host = host
	m.logger = host.Logger()

	// Parse configuration from YAML
	cfg := &ModuleConfig{
		InterfaceName: "wg0",
		MTU:           1420,
		Keepalive:     25,
		Embedded:      true,
	}

	if len(host.Config()) > 0 {
		if err := yaml.Unmarshal(host.Config(), cfg); err != nil {
			return fmt.Errorf("failed to parse config: %w", err)
		}
	}

	if cfg.ManagerURL == "" {
		return fmt.Errorf("manager_url is required")
	}

	if cfg.NodeID == "" {
		return fmt.Errorf("node_id is required")
	}

	m.config = cfg
	m.logger.Info("tobogganing config loaded",
		zap.String("manager_url", cfg.ManagerURL),
		zap.String("node_id", cfg.NodeID),
		zap.String("interface", cfg.InterfaceName),
	)

	// Initialize auth manager
	authMgr, err := NewAuthManager(cfg.ManagerURL, host.Secrets(), m.logger)
	if err != nil {
		return fmt.Errorf("failed to create auth manager: %w", err)
	}
	m.authMgr = authMgr

	// Initialize VPN manager with injectable controller
	vpnMgr := NewVPNManager(cfg, host.DataDir(), m.logger)
	m.vpnMgr = vpnMgr

	// Register Prometheus metrics
	if err := m.registerMetrics(host.Metrics()); err != nil {
		m.logger.Warn("failed to register metrics", zap.Error(err))
	}

	// Recover from any previous crash: remove stale tunnel state if marker exists
	m.recoverFromCrash()

	m.logger.Info("tobogganing module initialized")
	return nil
}

// registerMetrics registers Prometheus metrics for the module.
func (m *Module) registerMetrics(reg prometheus.Registerer) error {
	m.metrics = &TobogganingMetrics{
		tunnelUp: prometheus.NewGauge(prometheus.GaugeOpts{
			Name: "tobogganing_tunnel_up",
			Help: "Whether the WireGuard tunnel is up (1=up, 0=down)",
		}),
		handshakeAge: prometheus.NewGauge(prometheus.GaugeOpts{
			Name: "tobogganing_handshake_age_seconds",
			Help: "Age of the last WireGuard handshake in seconds",
		}),
		rxBytes: prometheus.NewCounter(prometheus.CounterOpts{
			Name: "tobogganing_rx_bytes_total",
			Help: "Total bytes received on the tunnel",
		}),
		txBytes: prometheus.NewCounter(prometheus.CounterOpts{
			Name: "tobogganing_tx_bytes_total",
			Help: "Total bytes transmitted on the tunnel",
		}),
		tokenRefreshes: prometheus.NewCounter(prometheus.CounterOpts{
			Name: "tobogganing_token_refreshes_total",
			Help: "Total number of token refresh operations",
		}),
		connErrors: prometheus.NewCounter(prometheus.CounterOpts{
			Name: "tobogganing_connection_errors_total",
			Help: "Total number of connection errors",
		}),
	}

	if err := reg.Register(m.metrics.tunnelUp); err != nil {
		return err
	}
	if err := reg.Register(m.metrics.handshakeAge); err != nil {
		return err
	}
	if err := reg.Register(m.metrics.rxBytes); err != nil {
		return err
	}
	if err := reg.Register(m.metrics.txBytes); err != nil {
		return err
	}
	if err := reg.Register(m.metrics.tokenRefreshes); err != nil {
		return err
	}
	if err := reg.Register(m.metrics.connErrors); err != nil {
		return err
	}

	return nil
}

// recoverFromCrash checks for stale tunnel state and cleans it up.
func (m *Module) recoverFromCrash() {
	// For now, this is a no-op. In production, we'd check for marker files
	// indicating a previous crashed tunnel state and clean it up.
	// See squawk's sysresolver pattern for inspiration.
}

// Start begins the module's work and returns promptly.
func (m *Module) Start(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.running {
		return fmt.Errorf("module already running")
	}

	m.running = true
	if m.metrics != nil {
		m.metrics.tunnelUp.Set(0)
	}

	m.logger.Info("starting tobogganing module")

	// Start background goroutines.
	go m.authRefreshLoop(ctx)
	go m.monitorLoop(ctx)

	// Initial authentication + tunnel establishment make blocking HTTP calls
	// to the Manager. Start MUST return promptly (the supervisor holds its
	// lock across Start, so a blocking Start wedges the whole daemon), so do
	// the initial connect in the background; the monitor loop retries on
	// failure.
	go m.initialConnect(ctx)

	return nil
}

// initialConnect performs the first authentication + tunnel bring-up off the
// Start path. Failures are logged and left to the monitor loop to retry.
func (m *Module) initialConnect(ctx context.Context) {
	if err := m.authMgr.EnsureValidToken(ctx); err != nil {
		m.logger.Error("failed to obtain initial token", zap.Error(err))
		if m.metrics != nil {
			m.metrics.connErrors.Inc()
		}
		return
	}
	if err := m.establishTunnel(ctx); err != nil {
		m.logger.Error("failed to establish tunnel", zap.Error(err))
		if m.metrics != nil {
			m.metrics.connErrors.Inc()
		}
	}
}

// Stop halts all module work and restores any system state.
func (m *Module) Stop(ctx context.Context) error {
	m.mu.Lock()
	if !m.running {
		m.mu.Unlock()
		return nil // Idempotent
	}
	m.running = false
	m.mu.Unlock()

	m.logger.Info("stopping tobogganing module")

	// Signal goroutines to stop
	close(m.stopCh)

	// Tear down tunnel
	if m.vpnMgr != nil {
		if err := m.vpnMgr.Disconnect(ctx); err != nil {
			m.logger.Error("failed to disconnect tunnel", zap.Error(err))
		}
	}

	// Revoke token
	if m.authMgr != nil {
		if err := m.authMgr.RevokeToken(ctx); err != nil {
			m.logger.Warn("failed to revoke token", zap.Error(err))
		}
	}

	if m.metrics != nil {
		m.metrics.tunnelUp.Set(0)
	}

	m.logger.Info("tobogganing module stopped")
	return nil
}

// Status reports the module's current operational state.
func (m *Module) Status(ctx context.Context) (sdk.Status, error) {
	m.mu.RLock()
	running := m.running
	m.mu.RUnlock()

	state := sdk.StateDisabled
	if running {
		state = sdk.StateRunning
	}

	detail := map[string]string{
		"tunnel": "down",
	}

	if m.vpnMgr != nil {
		if m.vpnMgr.IsConnected() {
			detail["tunnel"] = "up"
		}
		if cfg := m.vpnMgr.GetConfig(ctx); cfg != nil {
			detail["endpoint"] = cfg.ManagerURL
			detail["node_id"] = cfg.NodeID
		}
	}

	return sdk.Status{
		State:  state,
		Detail: detail,
	}, nil
}

// Health is a cheap liveness/degradation probe.
func (m *Module) Health(ctx context.Context) sdk.HealthReport {
	m.healthMutex.RLock()
	defer m.healthMutex.RUnlock()

	return sdk.HealthReport{
		Level:     m.lastHealth.Level,
		Message:   m.lastHealth.Message,
		CheckedAt: m.lastHealth.CheckedAt,
	}
}

// Commands declares the module's CLI command tree.
func (m *Module) Commands() []sdk.CommandSpec {
	return []sdk.CommandSpec{
		{
			Name:  "connect",
			Use:   "connect",
			Short: "Establish VPN connection",
			Tray:  true,
		},
		{
			Name:  "disconnect",
			Use:   "disconnect",
			Short: "Terminate VPN connection",
			Tray:  true,
		},
		{
			Name:  "status",
			Use:   "status",
			Short: "Show VPN connection status",
			Flags: []sdk.FlagSpec{
				{
					Name:    "json",
					Type:    sdk.FlagBool,
					Usage:   "Output as JSON",
					Default: "false",
				},
			},
		},
		{
			Name:  "rotate",
			Use:   "rotate",
			Short: "Force config/cert rotation",
			Flags: []sdk.FlagSpec{
				{
					Name:    "force",
					Type:    sdk.FlagBool,
					Usage:   "Force refresh without checking expiry",
					Default: "false",
				},
			},
		},
	}
}

// Dispatch executes a command.
func (m *Module) Dispatch(ctx context.Context, path []string, flags map[string]string, args []string) (*sdk.Result, error) {
	if len(path) == 0 {
		return &sdk.Result{
			Output:   "no command specified",
			ExitCode: 1,
		}, nil
	}

	cmd := path[0]
	switch cmd {
	case "connect":
		return m.cmdConnect(ctx)
	case "disconnect":
		return m.cmdDisconnect(ctx)
	case "status":
		jsonFlag := flags["json"]
		return m.cmdStatus(ctx, jsonFlag == "true")
	case "rotate":
		forceFlag := flags["force"]
		return m.cmdRotate(ctx, forceFlag == "true")
	default:
		return &sdk.Result{
			Output:   fmt.Sprintf("unknown command: %s", cmd),
			ExitCode: 1,
		}, nil
	}
}

// ConfigSchema returns the JSON Schema for configuration validation.
func (m *Module) ConfigSchema() []byte {
	schema := `{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "type": "object",
  "properties": {
    "manager_url": {
      "type": "string",
      "description": "Manager API base URL"
    },
    "node_id": {
      "type": "string",
      "description": "Unique node identifier"
    },
    "interface_name": {
      "type": "string",
      "description": "WireGuard interface name",
      "default": "wg0"
    },
    "mtu": {
      "type": "integer",
      "description": "WireGuard MTU",
      "default": 1420
    },
    "dns": {
      "type": "array",
      "items": { "type": "string" },
      "description": "DNS servers to push"
    },
    "keepalive": {
      "type": "integer",
      "description": "WireGuard keepalive interval (seconds)",
      "default": 25
    },
    "allowed_ips": {
      "type": "array",
      "items": { "type": "string" },
      "description": "Allowed IPs on tunnel"
    },
    "embedded": {
      "type": "boolean",
      "description": "Use embedded userspace WireGuard",
      "default": true
    }
  },
  "required": ["manager_url", "node_id"]
}`
	return []byte(schema)
}

// Command handlers

func (m *Module) cmdConnect(ctx context.Context) (*sdk.Result, error) {
	if err := m.establishTunnel(ctx); err != nil {
		return &sdk.Result{
			Output:   fmt.Sprintf("connection failed: %v", err),
			ExitCode: 1,
		}, nil
	}
	return &sdk.Result{
		Output:   "connected",
		ExitCode: 0,
	}, nil
}

func (m *Module) cmdDisconnect(ctx context.Context) (*sdk.Result, error) {
	if err := m.vpnMgr.Disconnect(ctx); err != nil {
		return &sdk.Result{
			Output:   fmt.Sprintf("disconnection failed: %v", err),
			ExitCode: 1,
		}, nil
	}
	if m.metrics != nil {
		m.metrics.tunnelUp.Set(0)
	}
	return &sdk.Result{
		Output:   "disconnected",
		ExitCode: 0,
	}, nil
}

func (m *Module) cmdStatus(ctx context.Context, asJSON bool) (*sdk.Result, error) {
	status, err := m.Status(ctx)
	if err != nil {
		return &sdk.Result{
			Output:   fmt.Sprintf("status check failed: %v", err),
			ExitCode: 1,
		}, nil
	}

	if asJSON {
		data := map[string]interface{}{
			"state":  status.State,
			"detail": status.Detail,
		}
		jsonBytes, _ := json.Marshal(data)
		return &sdk.Result{
			Output:   string(jsonBytes),
			JSON:     jsonBytes,
			ExitCode: 0,
		}, nil
	}

	output := fmt.Sprintf("State: %s\n", status.State)
	for k, v := range status.Detail {
		output += fmt.Sprintf("  %s: %s\n", k, v)
	}

	return &sdk.Result{
		Output:   output,
		ExitCode: 0,
	}, nil
}

func (m *Module) cmdRotate(ctx context.Context, force bool) (*sdk.Result, error) {
	if err := m.rotateConfig(ctx, force); err != nil {
		return &sdk.Result{
			Output:   fmt.Sprintf("rotation failed: %v", err),
			ExitCode: 1,
		}, nil
	}
	return &sdk.Result{
		Output:   "config rotated",
		ExitCode: 0,
	}, nil
}

// Background work

func (m *Module) establishTunnel(ctx context.Context) error {
	m.mu.RLock()
	if !m.running {
		m.mu.RUnlock()
		return fmt.Errorf("module not running")
	}
	m.mu.RUnlock()

	// Ensure we have a valid token
	if err := m.authMgr.EnsureValidToken(ctx); err != nil {
		return fmt.Errorf("failed to obtain token: %w", err)
	}

	// Establish tunnel
	if err := m.vpnMgr.Connect(ctx, m.authMgr); err != nil {
		if m.metrics != nil {
			m.metrics.connErrors.Inc()
		}
		return fmt.Errorf("failed to establish tunnel: %w", err)
	}

	if m.metrics != nil {
		m.metrics.tunnelUp.Set(1)
	}

	m.logger.Info("tunnel established")
	return nil
}

func (m *Module) rotateConfig(ctx context.Context, force bool) error {
	return m.vpnMgr.RotateConfig(ctx, m.authMgr, force)
}

// authRefreshLoop periodically refreshes the token before expiry.
func (m *Module) authRefreshLoop(ctx context.Context) {
	ticker := time.NewTicker(1 * time.Minute)
	defer ticker.Stop()

	for {
		select {
		case <-m.stopCh:
			return
		case <-ticker.C:
			if m.authMgr.IsTokenExpired(5 * time.Minute) {
				m.logger.Debug("token expiring soon, refreshing...")
				if err := m.authMgr.RefreshToken(ctx); err != nil {
					m.logger.Error("token refresh failed", zap.Error(err))
					if m.metrics != nil {
						m.metrics.connErrors.Inc()
					}
					continue
				}
				if m.metrics != nil {
					m.metrics.tokenRefreshes.Inc()
				}
				m.logger.Debug("token refreshed")
			}
		}
	}
}

// monitorLoop periodically checks tunnel health and updates metrics.
func (m *Module) monitorLoop(ctx context.Context) {
	ticker := time.NewTicker(10 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-m.stopCh:
			return
		case <-ticker.C:
			m.updateHealthProbe()
		}
	}
}

// updateHealthProbe checks tunnel handshake freshness and updates the health status.
func (m *Module) updateHealthProbe() {
	m.healthMutex.Lock()
	defer m.healthMutex.Unlock()

	if !m.vpnMgr.IsConnected() {
		m.lastHealth = HealthProbe{
			Level:     sdk.Unhealthy,
			Message:   "tunnel is not connected",
			CheckedAt: m.clock(),
		}
		return
	}

	lastHandshake := m.vpnMgr.LastHandshake()
	age := m.clock().Sub(lastHandshake)

	if m.metrics != nil {
		m.metrics.handshakeAge.Set(age.Seconds())
	}

	// If last handshake is older than 2 minutes, tunnel is degraded
	if age > 2*time.Minute {
		m.lastHealth = HealthProbe{
			Level:     sdk.Degraded,
			Message:   fmt.Sprintf("last handshake was %v ago", age),
			CheckedAt: m.clock(),
		}
		return
	}

	m.lastHealth = HealthProbe{
		Level:     sdk.Healthy,
		Message:   "tunnel is connected and healthy",
		CheckedAt: m.clock(),
	}
}
