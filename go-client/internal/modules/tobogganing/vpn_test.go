package tobogganing

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"testing"
	"time"

	"go.uber.org/zap/zaptest"
	"golang.zx2c4.com/wireguard/wgctrl/wgtypes"
)

func TestVPNManagerConnect(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/auth/token" && r.Method == "POST" {
			w.Header().Set("Content-Type", "application/json")
			tokenResp := TokenResponse{
				AccessToken: "test-token",
				TokenType:   "Bearer",
				ExpiresAt:   time.Now().Add(1 * time.Hour),
			}
			_ = json.NewEncoder(w).Encode(tokenResp) // #nosec G117 -- test mock server response encoding
			return
		}
		if r.URL.Path == "/api/v1/config" && r.Method == "GET" {
			w.Header().Set("Content-Type", "application/json")
			cfg := TunnelConfig{
				TunnelAddress:   "10.0.0.2/32",
				ServerPublicKey: "sMlWwt2d4gkKsPl6gWAGqtEgp2Xo2S4xyJ1wFjNsFEs=",
				ServerEndpoint:  "203.0.113.1:51820",
				AllowedIPs:      []string{"10.0.0.0/24"},
				DNS:             []string{"1.1.1.1"},
			}
			_ = json.NewEncoder(w).Encode(cfg)
			return
		}
		http.NotFound(w, r)
	}))
	defer server.Close()

	logger := zaptest.NewLogger(t)
	config := &ModuleConfig{
		ManagerURL:    server.URL,
		NodeID:        "test-node",
		InterfaceName: "wg0",
		MTU:           1420,
		Keepalive:     25,
		Embedded:      true,
	}

	vpnMgr := NewVPNManager(config, t.TempDir(), logger)
	// Override with fake controller
	vpnMgr.wgClient = NewFakeWGController()

	secrets := &FakeSecretStore{store: make(map[string][]byte)}
	_ = secrets.Set("api_key", []byte("test-api-key"))
	authMgr, _ := NewAuthManager(server.URL, secrets, logger)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Obtain token first
	_ = authMgr.EnsureValidToken(ctx)

	// Connect to tunnel
	if err := vpnMgr.Connect(ctx, authMgr); err != nil {
		t.Fatalf("Connect failed: %v", err)
	}

	if !vpnMgr.IsConnected() {
		t.Errorf("expected tunnel to be connected")
	}
}

func TestVPNManagerDisconnect(t *testing.T) {
	logger := zaptest.NewLogger(t)
	config := &ModuleConfig{
		ManagerURL:    "http://localhost:8080",
		NodeID:        "test-node",
		InterfaceName: "wg0",
	}

	vpnMgr := NewVPNManager(config, t.TempDir(), logger)
	vpnMgr.wgClient = NewFakeWGController()

	ctx := context.Background()

	// Mark as connected
	vpnMgr.mu.Lock()
	vpnMgr.connected = true
	vpnMgr.mu.Unlock()

	if !vpnMgr.IsConnected() {
		t.Errorf("tunnel should be connected")
	}

	// Disconnect
	if err := vpnMgr.Disconnect(ctx); err != nil {
		t.Fatalf("Disconnect failed: %v", err)
	}

	if vpnMgr.IsConnected() {
		t.Errorf("tunnel should be disconnected")
	}

	// Disconnect again should be idempotent
	if err := vpnMgr.Disconnect(ctx); err != nil {
		t.Fatalf("Disconnect not idempotent: %v", err)
	}
}

func TestVPNManagerGetConfig(t *testing.T) {
	logger := zaptest.NewLogger(t)
	config := &ModuleConfig{
		ManagerURL:    "http://localhost:8080",
		NodeID:        "test-node",
		InterfaceName: "wg0",
	}

	vpnMgr := NewVPNManager(config, t.TempDir(), logger)

	ctx := context.Background()
	if cfg := vpnMgr.GetConfig(ctx); cfg == nil {
		t.Errorf("expected non-nil config")
	}
}

func TestVPNManagerLastHandshake(t *testing.T) {
	logger := zaptest.NewLogger(t)
	config := &ModuleConfig{
		ManagerURL:    "http://localhost:8080",
		NodeID:        "test-node",
		InterfaceName: "wg0",
	}

	vpnMgr := NewVPNManager(config, t.TempDir(), logger)

	now := time.Now()
	vpnMgr.mu.Lock()
	vpnMgr.lastHandshake = now
	vpnMgr.mu.Unlock()

	if vpnMgr.LastHandshake().Before(now.Add(-1 * time.Second)) {
		t.Errorf("LastHandshake time is incorrect")
	}
}

func TestVPNManagerSaveCert(t *testing.T) {
	logger := zaptest.NewLogger(t)
	config := &ModuleConfig{
		ManagerURL:    "http://localhost:8080",
		NodeID:        "test-node",
		InterfaceName: "wg0",
	}

	dataDir := t.TempDir()
	vpnMgr := NewVPNManager(config, dataDir, logger)

	certData := []byte("test-certificate-data")
	if err := vpnMgr.SaveCert(certData); err != nil {
		t.Fatalf("SaveCert failed: %v", err)
	}

	// Verify file was created
	certPath := dataDir + "/tunnel.crt"
	if _, err := os.Stat(certPath); err != nil {
		t.Errorf("cert file not created: %v", err)
	}
}

func TestFakeWGController(t *testing.T) {
	fakeCtrl := NewFakeWGController()

	// Configure a device
	cfg := &wgtypes.Config{}
	if err := fakeCtrl.Configure("wg0", cfg); err != nil {
		t.Fatalf("Configure failed: %v", err)
	}

	// Devices should list the configured device
	devices, err := fakeCtrl.Devices()
	if err != nil {
		t.Fatalf("Devices failed: %v", err)
	}

	if len(devices) == 0 {
		t.Errorf("expected configured device")
	}

	// Get device should work
	device, err := fakeCtrl.Device("wg0")
	if err != nil {
		t.Fatalf("Device failed: %v", err)
	}

	if device == nil {
		t.Errorf("expected device to be non-nil")
	}

	// GetLastConfig should return the config we set
	lastCfg := fakeCtrl.GetLastConfig("wg0")
	if lastCfg == nil {
		t.Errorf("expected last config to be non-nil")
	}

	// Close should succeed
	if err := fakeCtrl.Close(); err != nil {
		t.Fatalf("Close failed: %v", err)
	}
}

func TestVPNManagerRotateConfig(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/auth/token" && r.Method == "POST" {
			w.Header().Set("Content-Type", "application/json")
			tokenResp := TokenResponse{
				AccessToken: "test-token",
				TokenType:   "Bearer",
				ExpiresAt:   time.Now().Add(1 * time.Hour),
			}
			_ = json.NewEncoder(w).Encode(tokenResp) // #nosec G117 -- test mock server response encoding
			return
		}
		if r.URL.Path == "/api/v1/config" && r.Method == "GET" {
			w.Header().Set("Content-Type", "application/json")
			cfg := TunnelConfig{
				TunnelAddress:   "10.0.0.2/32",
				ServerPublicKey: "sMlWwt2d4gkKsPl6gWAGqtEgp2Xo2S4xyJ1wFjNsFEs=",
				ServerEndpoint:  "203.0.113.1:51820",
				AllowedIPs:      []string{"10.0.0.0/24"},
				DNS:             []string{"1.1.1.1"},
			}
			_ = json.NewEncoder(w).Encode(cfg)
			return
		}
		http.NotFound(w, r)
	}))
	defer server.Close()

	logger := zaptest.NewLogger(t)
	config := &ModuleConfig{
		ManagerURL:    server.URL,
		NodeID:        "test-node",
		InterfaceName: "wg0",
	}

	vpnMgr := NewVPNManager(config, t.TempDir(), logger)
	vpnMgr.wgClient = NewFakeWGController()

	secrets := &FakeSecretStore{store: make(map[string][]byte)}
	_ = secrets.Set("api_key", []byte("test-api-key"))
	authMgr, _ := NewAuthManager(server.URL, secrets, logger)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	_ = authMgr.EnsureValidToken(ctx)

	// Rotate config
	if err := vpnMgr.RotateConfig(ctx, authMgr, false); err != nil {
		t.Fatalf("RotateConfig failed: %v", err)
	}

	// Config should be updated
	if vpnMgr.config_data == nil {
		t.Errorf("expected config to be populated")
	}
}
