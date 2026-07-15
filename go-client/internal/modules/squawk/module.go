package squawk

import (
	"context"
	"encoding/json"
	"fmt"
	"net/netip"
	"strings"
	"sync"
	"time"

	"github.com/penguintechinc/penguin/pkg/sdk"
	"github.com/penguintechinc/squawk/squawk-client-go/pkg/client"
	"github.com/penguintechinc/squawk/squawk-client-go/pkg/config"
	"github.com/penguintechinc/squawk/squawk-client-go/pkg/forwarder"
	"github.com/penguintechinc/squawk/squawk-client-go/pkg/license"
	"github.com/prometheus/client_golang/prometheus"
	"go.uber.org/zap"
	"gopkg.in/yaml.v3"
)

// Module implements sdk.Module for Squawk DNS-over-HTTPS client.
type Module struct {
	host        sdk.HostServices
	logger      *zap.Logger
	dohClient   *client.DoHClient
	forwarder   *forwarder.Forwarder
	resolver    SysResolver
	config      *ModuleConfig
	mu          sync.RWMutex
	running     bool
	stopCh      chan struct{}
	metrics     *SquawkMetrics
	lastHealth  HealthProbe
	healthMutex sync.RWMutex
}

// ModuleConfig is the per-module configuration (validated against ConfigSchema).
type ModuleConfig struct {
	DOH struct {
		ServerURL  string `yaml:"server_url"`
		VerifyTLS  bool   `yaml:"verify_tls"`
		AuthToken  string `yaml:"auth_token"`
		ClientCert string `yaml:"client_cert"`
		ClientKey  string `yaml:"client_key"`
		CACert     string `yaml:"ca_cert"`
	} `yaml:"doh"`
	Forwarder struct {
		Enabled bool   `yaml:"enabled"`
		UDPAddr string `yaml:"udp_addr"`
		TCPAddr string `yaml:"tcp_addr"`
	} `yaml:"forwarder"`
	SystemDNS struct {
		Manage bool `yaml:"manage"`
	} `yaml:"system_dns"`
	Cache struct {
		Enabled bool `yaml:"enabled"`
	} `yaml:"cache"`
}

// SquawkMetrics holds Prometheus metrics.
type SquawkMetrics struct {
	queriesTotal prometheus.Counter
	forwarderUp  prometheus.Gauge
	cacheEntries prometheus.Gauge
	dnsApplied   prometheus.Gauge
	healthStatus prometheus.Gauge
}

// HealthProbe tracks recent health check result.
type HealthProbe struct {
	Level     sdk.HealthLevel
	Message   string
	CheckedAt time.Time
}

// New creates a new Squawk module instance (factory function).
func New() sdk.Module {
	return &Module{
		stopCh: make(chan struct{}),
	}
}

// Info returns module identity metadata.
//
// LicenseFeature is intentionally empty: Squawk is core product and ships in
// the Free tier, so the module itself must load without a license server.
// Enterprise-only capabilities *inside* a module are gated individually via
// host.License().FeatureEnabled("penguin.<feature>").
func (m *Module) Info() sdk.ModuleInfo {
	return sdk.ModuleInfo{
		Name:        "squawk",
		Version:     "1.0.0",
		Description: "DNS-over-HTTPS endpoint client with system DNS management",
	}
}

// Init prepares the module with host services.
func (m *Module) Init(ctx context.Context, host sdk.HostServices) error {
	m.host = host
	m.logger = host.Logger()

	// Attempt crash recovery if resolver backup exists
	dataDir := host.DataDir()
	m.resolver = NewSysResolver(dataDir, m.logger)
	if err := m.resolver.RecoverFromCrash(ctx); err != nil {
		m.logger.Warn("crash recovery failed", zap.Error(err))
		// Non-fatal: continue with fresh resolver state
	}

	// Load module configuration from /etc/penguin/modules.d/squawk.yaml
	// The daemon validates it against ConfigSchema()
	// Note: Use IP address (127.0.0.1) instead of hostname for server URL to prevent DNS loops
	cfg := &ModuleConfig{
		DOH: struct {
			ServerURL  string `yaml:"server_url"`
			VerifyTLS  bool   `yaml:"verify_tls"`
			AuthToken  string `yaml:"auth_token"`
			ClientCert string `yaml:"client_cert"`
			ClientKey  string `yaml:"client_key"`
			CACert     string `yaml:"ca_cert"`
		}{
			ServerURL: "https://127.0.0.1:443/dns-query", // Use IP to prevent DNS loops
			VerifyTLS: true,
		},
		SystemDNS: struct {
			Manage bool `yaml:"manage"`
		}{Manage: false},
		Cache: struct {
			Enabled bool `yaml:"enabled"`
		}{Enabled: true},
	}

	// Configuration arrives from the host already validated against
	// ConfigSchema(); empty means the operator supplied none, so defaults win.
	if raw := host.Config(); len(raw) > 0 {
		if err := yaml.Unmarshal(raw, cfg); err != nil {
			return fmt.Errorf("parse squawk config: %w", err)
		}
	}

	// Fetch auth token from host secrets if not in config
	if cfg.DOH.AuthToken == "" {
		if authToken, err := host.Secrets().Get("auth_token"); err == nil {
			cfg.DOH.AuthToken = string(authToken)
		}
	}

	m.config = cfg

	// Create DoH client
	dohConfig := &client.Config{
		ServerURL:  cfg.DOH.ServerURL,
		AuthToken:  cfg.DOH.AuthToken,
		VerifySSL:  cfg.DOH.VerifyTLS,
		ClientCert: cfg.DOH.ClientCert,
		ClientKey:  cfg.DOH.ClientKey,
		CaCert:     cfg.DOH.CACert,
	}

	var err error
	m.dohClient, err = client.NewDoHClient(dohConfig)
	if err != nil {
		m.logger.Error("failed to create DoH client", zap.Error(err))
		return fmt.Errorf("create DoH client: %w", err)
	}

	// Create forwarder if enabled
	if cfg.Forwarder.Enabled {
		fwdConfig := &forwarder.Config{
			UDPAddress: cfg.Forwarder.UDPAddr,
			TCPAddress: cfg.Forwarder.TCPAddr,
			ListenUDP:  true,
			ListenTCP:  true,
		}
		m.forwarder = forwarder.NewForwarder(m.dohClient, fwdConfig)
	}

	// Register Prometheus metrics
	m.registerMetrics(host.Metrics())

	m.logger.Info("squawk module initialized",
		zap.String("server", cfg.DOH.ServerURL),
		zap.Bool("forwarder_enabled", cfg.Forwarder.Enabled))

	return nil
}

// Start begins the module's work (non-blocking).
func (m *Module) Start(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.running {
		return nil // Already running
	}

	// Start forwarder if configured
	if m.forwarder != nil {
		if err := m.forwarder.Start(ctx); err != nil {
			m.logger.Error("failed to start forwarder", zap.Error(err))
			return fmt.Errorf("start forwarder: %w", err)
		}
		m.metrics.forwarderUp.Set(1)
	}

	// Apply system DNS if configured
	if m.config.SystemDNS.Manage {
		if err := m.resolver.Apply(ctx, []netip.Addr{netip.MustParseAddr("8.8.8.8")}); err != nil {
			m.logger.Error("failed to apply system DNS", zap.Error(err))
			// Non-fatal: continue with forwarder active
		} else {
			m.metrics.dnsApplied.Set(1)
		}
	}

	m.running = true
	m.logger.Info("squawk module started")

	return nil
}

// Stop halts module work and restores system state (idempotent).
func (m *Module) Stop(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if !m.running {
		return nil // Already stopped
	}

	var errs []string

	// Stop forwarder
	if m.forwarder != nil {
		if err := m.forwarder.Stop(); err != nil {
			m.logger.Error("failed to stop forwarder", zap.Error(err))
			errs = append(errs, fmt.Sprintf("forwarder: %v", err))
		}
		m.metrics.forwarderUp.Set(0)
	}

	// Restore system DNS
	if m.config.SystemDNS.Manage {
		if err := m.resolver.Restore(ctx); err != nil {
			m.logger.Error("failed to restore system DNS", zap.Error(err))
			errs = append(errs, fmt.Sprintf("resolver: %v", err))
		}
		m.metrics.dnsApplied.Set(0)
	}

	// Close DoH client
	if m.dohClient != nil {
		if err := m.dohClient.Close(); err != nil {
			m.logger.Warn("error closing DoH client", zap.Error(err))
		}
	}

	m.running = false
	m.logger.Info("squawk module stopped")

	if len(errs) > 0 {
		return fmt.Errorf("stop errors: %s", strings.Join(errs, "; "))
	}

	return nil
}

// Status reports the module's operational state.
func (m *Module) Status(ctx context.Context) (sdk.Status, error) {
	m.mu.RLock()
	defer m.mu.RUnlock()

	state := sdk.StateStopped
	if m.running {
		state = sdk.StateRunning
	}

	detail := map[string]string{
		"server": m.config.DOH.ServerURL,
	}

	if m.forwarder != nil {
		detail["forwarder"] = "listening :53"
	}

	// Get resolver status
	if current, err := m.resolver.Current(ctx); err == nil && len(current) > 0 {
		detail["dns_servers"] = strings.Join([]string{current[0].String()}, ", ")
	}

	return sdk.Status{
		State:  state,
		Detail: detail,
	}, nil
}

// Health performs a cheap liveness probe.
func (m *Module) Health(ctx context.Context) sdk.HealthReport {
	m.healthMutex.Lock()
	defer m.healthMutex.Unlock()

	// Use cached result if recent (< 5s old)
	if time.Since(m.lastHealth.CheckedAt) < 5*time.Second {
		return sdk.HealthReport{
			Level:     m.lastHealth.Level,
			Message:   m.lastHealth.Message,
			CheckedAt: m.lastHealth.CheckedAt,
		}
	}

	// Perform health check: test a simple DNS query
	level := sdk.Healthy
	message := "OK"

	ctx, cancel := context.WithTimeout(ctx, 2*time.Second)
	defer cancel()

	if m.dohClient != nil {
		// Try a simple query
		_, err := m.dohClient.Query(ctx, "google.com", "A")
		if err != nil {
			level = sdk.Degraded
			message = fmt.Sprintf("query error: %v", err)
			m.logger.Debug("health check query failed", zap.Error(err))
		}
	} else {
		level = sdk.Unhealthy
		message = "DoH client not initialized"
	}

	m.lastHealth = HealthProbe{
		Level:     level,
		Message:   message,
		CheckedAt: time.Now(),
	}

	return sdk.HealthReport{
		Level:     level,
		Message:   message,
		CheckedAt: time.Now(),
	}
}

// Commands declares the module's CLI command tree.
func (m *Module) Commands() []sdk.CommandSpec {
	return []sdk.CommandSpec{
		{
			Name:    "query",
			Use:     "query <domain> [--type TYPE]",
			Short:   "Query a DNS record",
			MinArgs: 1,
			MaxArgs: 1,
			Flags: []sdk.FlagSpec{
				{
					Name:      "type",
					Shorthand: "t",
					Usage:     "DNS record type (A, AAAA, MX, TXT, etc.)",
					Default:   "A",
					Type:      sdk.FlagString,
				},
			},
		},
		{
			Name:  "forward",
			Use:   "forward",
			Short: "Manage DNS forwarding",
			Subcommands: []sdk.CommandSpec{
				{
					Name:    "status",
					Use:     "status",
					Short:   "Show forwarder status",
					MinArgs: 0,
					MaxArgs: 0,
				},
				{
					Name:    "start",
					Use:     "start",
					Short:   "Start DNS forwarding",
					MinArgs: 0,
					MaxArgs: 0,
					Tray:    true,
				},
				{
					Name:    "stop",
					Use:     "stop",
					Short:   "Stop DNS forwarding",
					MinArgs: 0,
					MaxArgs: 0,
					Tray:    true,
				},
			},
		},
		{
			Name:    "config",
			Use:     "config show",
			Short:   "Show current configuration",
			MinArgs: 0,
			MaxArgs: 1,
		},
		{
			Name:  "cache",
			Use:   "cache",
			Short: "Manage DNS cache",
			Subcommands: []sdk.CommandSpec{
				{
					Name:    "stats",
					Use:     "stats",
					Short:   "Show cache statistics",
					MinArgs: 0,
					MaxArgs: 0,
				},
				{
					Name:    "flush",
					Use:     "flush",
					Short:   "Flush the DNS cache",
					MinArgs: 0,
					MaxArgs: 0,
					Tray:    true,
				},
			},
		},
		{
			Name:    "license",
			Use:     "license status",
			Short:   "Check license status",
			MinArgs: 0,
			MaxArgs: 1,
		},
		{
			Name:    "time",
			Use:     "time",
			Short:   "Check NTP/NTS status",
			MinArgs: 0,
			MaxArgs: 0,
		},
	}
}

// Dispatch executes commands.
func (m *Module) Dispatch(ctx context.Context, path []string, flags map[string]string, args []string) (*sdk.Result, error) {
	if len(path) == 0 {
		return &sdk.Result{
			Output:   "squawk: no command specified",
			ExitCode: 1,
		}, nil
	}

	cmd := path[0]

	switch cmd {
	case "query":
		return m.handleQuery(ctx, flags, args)
	case "forward":
		return m.handleForward(ctx, path, args)
	case "config":
		return m.handleConfig(ctx, args)
	case "cache":
		return m.handleCache(ctx, path, args)
	case "license":
		return m.handleLicense(ctx, args)
	case "time":
		return m.handleTime(ctx, args)
	default:
		return &sdk.Result{
			Output:   fmt.Sprintf("squawk: unknown command '%s'", cmd),
			ExitCode: 1,
		}, nil
	}
}

// ConfigSchema returns JSON Schema for module configuration.
func (m *Module) ConfigSchema() []byte {
	schema := map[string]interface{}{
		"$schema": "http://json-schema.org/draft-07/schema#",
		"type":    "object",
		"properties": map[string]interface{}{
			"doh": map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"server_url": map[string]interface{}{
						"type":        "string",
						"description": "DoH server URL",
						"default":     "https://dns.penguintech.io/dns-query",
					},
					"verify_tls": map[string]interface{}{
						"type":        "boolean",
						"description": "Verify TLS certificate",
						"default":     true,
					},
					"auth_token": map[string]interface{}{
						"type":        "string",
						"description": "Authentication token (prefer secrets)",
					},
					"client_cert": map[string]interface{}{
						"type":        "string",
						"description": "mTLS client certificate path",
					},
					"client_key": map[string]interface{}{
						"type":        "string",
						"description": "mTLS client key path",
					},
					"ca_cert": map[string]interface{}{
						"type":        "string",
						"description": "CA certificate path for server verification",
					},
				},
			},
			"forwarder": map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"enabled": map[string]interface{}{
						"type":        "boolean",
						"description": "Enable local DNS forwarding on :53",
						"default":     false,
					},
					"udp_addr": map[string]interface{}{
						"type":        "string",
						"description": "UDP listen address",
						"default":     "127.0.0.1:53",
					},
					"tcp_addr": map[string]interface{}{
						"type":        "string",
						"description": "TCP listen address",
						"default":     "127.0.0.1:53",
					},
				},
			},
			"system_dns": map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"manage": map[string]interface{}{
						"type":        "boolean",
						"description": "Manage system DNS resolver",
						"default":     false,
					},
				},
			},
			"cache": map[string]interface{}{
				"type": "object",
				"properties": map[string]interface{}{
					"enabled": map[string]interface{}{
						"type":        "boolean",
						"description": "Enable DNS result caching",
						"default":     true,
					},
				},
			},
		},
	}

	data, _ := json.Marshal(schema)
	return data
}

// Helper methods

func (m *Module) handleQuery(ctx context.Context, flags map[string]string, args []string) (*sdk.Result, error) {
	if len(args) == 0 {
		return &sdk.Result{
			Output:   "Usage: squawk query <domain> [--type TYPE]",
			ExitCode: 1,
		}, nil
	}

	domain := args[0]
	recordType := flags["type"]
	if recordType == "" {
		recordType = "A"
	}

	ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()

	result, err := m.dohClient.Query(ctx, domain, recordType)
	if err != nil {
		return &sdk.Result{
			Output:   fmt.Sprintf("Query failed: %v", err),
			ExitCode: 1,
		}, nil
	}

	output := fmt.Sprintf("%s %s: %v", domain, recordType, result)
	jsonData, _ := json.Marshal(map[string]interface{}{
		"domain":      domain,
		"record_type": recordType,
		"result":      result,
		"queried_at":  time.Now(),
	})

	return &sdk.Result{
		Output:   output,
		JSON:     jsonData,
		ExitCode: 0,
	}, nil
}

func (m *Module) handleForward(ctx context.Context, path []string, args []string) (*sdk.Result, error) {
	if len(path) < 2 {
		return &sdk.Result{
			Output:   "Usage: squawk forward {status|start|stop}",
			ExitCode: 1,
		}, nil
	}

	subcmd := path[1]

	m.mu.Lock()
	defer m.mu.Unlock()

	switch subcmd {
	case "status":
		var status string
		if m.forwarder != nil && m.forwarder.IsRunning() {
			status = "running"
		} else {
			status = "stopped"
		}
		jsonData, _ := json.Marshal(map[string]string{"status": status})
		return &sdk.Result{
			Output:   fmt.Sprintf("Forwarder: %s", status),
			JSON:     jsonData,
			ExitCode: 0,
		}, nil

	case "start":
		if m.forwarder == nil {
			return &sdk.Result{
				Output:   "Forwarder not configured",
				ExitCode: 1,
			}, nil
		}
		if err := m.forwarder.Start(ctx); err != nil {
			return &sdk.Result{
				Output:   fmt.Sprintf("Failed to start forwarder: %v", err),
				ExitCode: 1,
			}, nil
		}
		return &sdk.Result{
			Output:   "Forwarder started",
			ExitCode: 0,
		}, nil

	case "stop":
		if m.forwarder == nil {
			return &sdk.Result{
				Output:   "Forwarder not configured",
				ExitCode: 1,
			}, nil
		}
		if err := m.forwarder.Stop(); err != nil {
			return &sdk.Result{
				Output:   fmt.Sprintf("Failed to stop forwarder: %v", err),
				ExitCode: 1,
			}, nil
		}
		return &sdk.Result{
			Output:   "Forwarder stopped",
			ExitCode: 0,
		}, nil

	default:
		return &sdk.Result{
			Output:   fmt.Sprintf("Unknown subcommand: %s", subcmd),
			ExitCode: 1,
		}, nil
	}
}

func (m *Module) handleConfig(ctx context.Context, args []string) (*sdk.Result, error) {
	m.mu.RLock()
	defer m.mu.RUnlock()

	// Never print the auth token: `config show` output lands on terminals, in
	// screenshots, and in support tickets.
	redacted := *m.config
	redacted.DOH.AuthToken = maskSecret(m.config.DOH.AuthToken)

	jsonData, err := json.MarshalIndent(redacted, "", "  ")
	if err != nil {
		return nil, fmt.Errorf("render config: %w", err)
	}
	return &sdk.Result{
		Output:   string(jsonData),
		JSON:     jsonData,
		ExitCode: 0,
	}, nil
}

// maskSecret renders a secret as a non-reversible hint (e.g. "****abcd"),
// or "" when unset — never the value itself.
func maskSecret(s string) string {
	switch {
	case s == "":
		return ""
	case len(s) <= 4:
		return "****"
	default:
		return "****" + s[len(s)-4:]
	}
}

func (m *Module) handleCache(ctx context.Context, path []string, args []string) (*sdk.Result, error) {
	if len(path) < 2 {
		return &sdk.Result{
			Output:   "Usage: squawk cache {stats|flush}",
			ExitCode: 1,
		}, nil
	}

	subcmd := path[1]

	switch subcmd {
	case "stats":
		jsonData, _ := json.Marshal(map[string]interface{}{
			"cache_enabled": m.config.Cache.Enabled,
			"note":          "Cache statistics from DoH client not currently exposed",
		})
		return &sdk.Result{
			Output:   "Cache stats: cache " + map[bool]string{true: "enabled", false: "disabled"}[m.config.Cache.Enabled],
			JSON:     jsonData,
			ExitCode: 0,
		}, nil

	case "flush":
		jsonData, _ := json.Marshal(map[string]string{"status": "cache flushed (client-level cache not directly accessible)"})
		return &sdk.Result{
			Output:   "Cache flushed",
			JSON:     jsonData,
			ExitCode: 0,
		}, nil

	default:
		return &sdk.Result{
			Output:   fmt.Sprintf("Unknown subcommand: %s", subcmd),
			ExitCode: 1,
		}, nil
	}
}

func (m *Module) handleLicense(ctx context.Context, args []string) (*sdk.Result, error) {
	ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()

	licenseConfig := &config.LicenseConfig{
		ValidateOnline: true,
	}

	// Fetch license info from host secrets if available
	if userToken, err := m.host.Secrets().Get("user_token"); err == nil {
		licenseConfig.UserToken = string(userToken)
	}

	validator := license.NewValidator(licenseConfig)
	isValid, err := validator.IsValid(ctx)

	var status string //nolint:ineffassign // status is assigned in all branches below
	if err != nil {
		status = fmt.Sprintf("error: %v", err)
	} else if isValid {
		status = "valid"
	} else {
		status = "invalid or expired"
	}

	jsonData, _ := json.Marshal(map[string]interface{}{
		"status":      status,
		"checked_at":  time.Now(),
		"feature_key": "penguin.squawk",
	})

	return &sdk.Result{
		Output:   fmt.Sprintf("License status: %s", status),
		JSON:     jsonData,
		ExitCode: 0,
	}, nil
}

func (m *Module) handleTime(ctx context.Context, args []string) (*sdk.Result, error) {
	jsonData, _ := json.Marshal(map[string]interface{}{
		"ntp_enabled": false,
		"nts_enabled": false,
		"checked_at":  time.Now(),
		"note":        "NTP/NTS not currently exposed by squawk-client-go at module level",
	})

	return &sdk.Result{
		Output:   "NTP/NTS status: not configured",
		JSON:     jsonData,
		ExitCode: 0,
	}, nil
}

func (m *Module) registerMetrics(registerer prometheus.Registerer) {
	m.metrics = &SquawkMetrics{
		queriesTotal: prometheus.NewCounter(prometheus.CounterOpts{
			Name:      "squawk_queries_total",
			Namespace: "penguin_module",
			Subsystem: "squawk",
			Help:      "Total number of DNS queries issued",
		}),
		forwarderUp: prometheus.NewGauge(prometheus.GaugeOpts{
			Name:      "squawk_forwarder_up",
			Namespace: "penguin_module",
			Subsystem: "squawk",
			Help:      "Whether the DNS forwarder is running (1 = running, 0 = stopped)",
		}),
		cacheEntries: prometheus.NewGauge(prometheus.GaugeOpts{
			Name:      "squawk_cache_entries",
			Namespace: "penguin_module",
			Subsystem: "squawk",
			Help:      "Number of entries in the DNS cache",
		}),
		dnsApplied: prometheus.NewGauge(prometheus.GaugeOpts{
			Name:      "squawk_dns_applied",
			Namespace: "penguin_module",
			Subsystem: "squawk",
			Help:      "Whether system DNS resolver is managed (1 = managed, 0 = not managed)",
		}),
		healthStatus: prometheus.NewGauge(prometheus.GaugeOpts{
			Name:      "squawk_health_status",
			Namespace: "penguin_module",
			Subsystem: "squawk",
			Help:      "Module health status (0 = healthy, 1 = degraded, 2 = unhealthy)",
		}),
	}

	registerer.MustRegister(
		m.metrics.queriesTotal,
		m.metrics.forwarderUp,
		m.metrics.cacheEntries,
		m.metrics.dnsApplied,
		m.metrics.healthStatus,
	)
}
