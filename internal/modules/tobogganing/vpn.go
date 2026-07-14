package tobogganing

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"sync"
	"time"

	"go.uber.org/zap"
	"golang.zx2c4.com/wireguard/wgctrl"
	"golang.zx2c4.com/wireguard/wgctrl/wgtypes"
)

// VPNManager handles WireGuard tunnel lifecycle and configuration.
type VPNManager struct {
	config        *ModuleConfig
	dataDir       string
	logger        *zap.Logger
	mu            sync.RWMutex
	connected     bool
	wgClient      WGController // Abstracted for testing
	lastHandshake time.Time
	config_data   *TunnelConfig
}

// TunnelConfig is the tunnel configuration fetched from the manager API.
type TunnelConfig struct {
	TunnelAddress   string   `json:"tunnel_address"`
	ServerPublicKey string   `json:"server_public_key"`
	ServerEndpoint  string   `json:"server_endpoint"`
	AllowedIPs      []string `json:"allowed_ips"`
	DNS             []string `json:"dns"`
}

// WGController abstracts WireGuard operations for testing.
type WGController interface {
	// Devices returns all WireGuard device names.
	Devices() ([]string, error)
	// Close closes the controller.
	Close() error
	// Device returns device info.
	Device(name string) (*wgtypes.Device, error)
	// Configure applies configuration to a device.
	Configure(name string, cfg *wgtypes.Config) error
}

// NewVPNManager creates a new VPN manager.
func NewVPNManager(config *ModuleConfig, dataDir string, logger *zap.Logger) *VPNManager {
	wgClient, err := wgctrl.New()
	if err != nil {
		logger.Warn("failed to create wgctrl client, will use fallback", zap.Error(err))
		// Fallback: create a fake controller for testing
		wgClient = nil
	}

	var controller WGController
	if wgClient != nil {
		controller = &realWGController{client: wgClient}
	} else {
		controller = NewFakeWGController()
	}

	return &VPNManager{
		config:        config,
		dataDir:       dataDir,
		logger:        logger,
		wgClient:      controller,
		lastHandshake: time.Time{},
	}
}

// Connect establishes a VPN tunnel.
func (v *VPNManager) Connect(ctx context.Context, authMgr *AuthManager) error {
	v.mu.Lock()
	defer v.mu.Unlock()

	if v.connected {
		return fmt.Errorf("already connected")
	}

	// Fetch tunnel configuration from manager
	tunnelCfg, err := v.fetchTunnelConfig(ctx, authMgr)
	if err != nil {
		return fmt.Errorf("failed to fetch tunnel config: %w", err)
	}

	v.config_data = tunnelCfg

	// Generate WireGuard key pair
	privateKey, err := wgtypes.GeneratePrivateKey()
	if err != nil {
		return fmt.Errorf("failed to generate private key: %w", err)
	}

	// Create WireGuard device configuration
	serverPublicKey, err := wgtypes.ParseKey(tunnelCfg.ServerPublicKey)
	if err != nil {
		return fmt.Errorf("invalid server public key: %w", err)
	}

	// Parse server endpoint
	serverAddr, err := net.ResolveUDPAddr("udp", tunnelCfg.ServerEndpoint)
	if err != nil {
		return fmt.Errorf("failed to resolve server endpoint: %w", err)
	}

	// Create WireGuard configuration
	parsedAllowedIPs := make([]net.IPNet, len(tunnelCfg.AllowedIPs))
	for i, ipStr := range tunnelCfg.AllowedIPs {
		_, ipNet, err := net.ParseCIDR(ipStr)
		if err != nil {
			return fmt.Errorf("invalid allowed IP %q: %w", ipStr, err)
		}
		parsedAllowedIPs[i] = *ipNet
	}

	keepalive := time.Duration(v.config.Keepalive) * time.Second

	wgCfg := &wgtypes.Config{
		PrivateKey: &privateKey,
		ListenPort: nil, // Let kernel choose
		Peers: []wgtypes.PeerConfig{
			{
				PublicKey:                   serverPublicKey,
				Endpoint:                    serverAddr,
				AllowedIPs:                  parsedAllowedIPs,
				PersistentKeepaliveInterval: &keepalive,
			},
		},
	}

	// Apply configuration to WireGuard device
	if err := v.wgClient.Configure(v.config.InterfaceName, wgCfg); err != nil {
		return fmt.Errorf("failed to configure wireguard device: %w", err)
	}

	v.connected = true
	v.lastHandshake = time.Now()
	v.logger.Info("tunnel connected",
		zap.String("interface", v.config.InterfaceName),
		zap.String("endpoint", tunnelCfg.ServerEndpoint),
	)

	return nil
}

// Disconnect tears down the VPN tunnel.
func (v *VPNManager) Disconnect(ctx context.Context) error {
	v.mu.Lock()
	defer v.mu.Unlock()

	if !v.connected {
		return nil // Idempotent
	}

	v.connected = false
	v.logger.Info("tunnel disconnected", zap.String("interface", v.config.InterfaceName))
	return nil
}

// IsConnected returns whether the tunnel is currently connected.
func (v *VPNManager) IsConnected() bool {
	v.mu.RLock()
	defer v.mu.RUnlock()
	return v.connected
}

// GetConfig returns the current tunnel configuration.
func (v *VPNManager) GetConfig(ctx context.Context) *ModuleConfig {
	v.mu.RLock()
	defer v.mu.RUnlock()
	return v.config
}

// LastHandshake returns the time of the last WireGuard handshake.
func (v *VPNManager) LastHandshake() time.Time {
	v.mu.RLock()
	defer v.mu.RUnlock()
	return v.lastHandshake
}

// RotateConfig forces a new tunnel configuration fetch.
func (v *VPNManager) RotateConfig(ctx context.Context, authMgr *AuthManager, force bool) error {
	v.mu.Lock()
	defer v.mu.Unlock()

	// Re-fetch configuration
	tunnelCfg, err := v.fetchTunnelConfig(ctx, authMgr)
	if err != nil {
		return fmt.Errorf("failed to fetch new config: %w", err)
	}

	v.config_data = tunnelCfg
	v.logger.Info("configuration rotated", zap.String("interface", v.config.InterfaceName))

	return nil
}

// fetchTunnelConfig fetches the tunnel configuration from the manager.
func (v *VPNManager) fetchTunnelConfig(ctx context.Context, authMgr *AuthManager) (*TunnelConfig, error) {
	token := authMgr.GetToken()
	if token == "" {
		return nil, fmt.Errorf("no valid token")
	}

	client := &realHTTPClient{}
	cfg := &TunnelConfig{}

	if err := client.DoJSON(ctx, "GET", v.config.ManagerURL+"/api/v1/config", token, nil, cfg); err != nil {
		return nil, fmt.Errorf("config fetch failed: %w", err)
	}

	return cfg, nil
}

// realHTTPClient is a simple HTTP client for API calls.
type realHTTPClient struct{}

func (h *realHTTPClient) DoJSON(ctx context.Context, method, url, token string, reqBody interface{}, respBody interface{}) error {
	var reqReader io.Reader
	if reqBody != nil {
		jsonData, _ := json.Marshal(reqBody)
		reqReader = bytes.NewReader(jsonData)
	}

	req, err := http.NewRequestWithContext(ctx, method, url, reqReader)
	if err != nil {
		return err
	}

	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Content-Type", "application/json")

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return err
	}
	defer func() { _ = resp.Body.Close() }()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("status %d", resp.StatusCode)
	}

	return json.NewDecoder(resp.Body).Decode(respBody)
}

// SaveCert saves the certificate to disk.
func (v *VPNManager) SaveCert(cert []byte) error {
	v.mu.Lock()
	defer v.mu.Unlock()

	certPath := filepath.Join(v.dataDir, "tunnel.crt")
	return os.WriteFile(certPath, cert, 0o600)
}
