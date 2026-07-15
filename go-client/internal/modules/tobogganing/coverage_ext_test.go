package tobogganing

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/golang-jwt/jwt/v5"
	"github.com/penguintechinc/penguin/pkg/sdk"
	"go.uber.org/zap/zaptest"
	"golang.zx2c4.com/wireguard/wgctrl/wgtypes"
	"gopkg.in/yaml.v3"
)

// TestExtractTokenExpiryValidJWT tests extractTokenExpiry with a valid JWT.
func TestExtractTokenExpiryValidJWT(t *testing.T) {
	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}
	authMgr, _ := NewAuthManager("http://localhost:8080", secrets, logger)

	// Create a valid JWT with exp claim using a signing method
	expTime := time.Now().Add(1 * time.Hour)
	token := jwt.NewWithClaims(jwt.SigningMethodHS256, jwt.MapClaims{
		"exp": float64(expTime.Unix()),
	})

	tokenString, err := token.SignedString([]byte("secret"))
	if err != nil {
		t.Fatalf("failed to create token: %v", err)
	}

	expiry, err := authMgr.extractTokenExpiry(tokenString)
	if err != nil {
		t.Errorf("expected no error, got %v", err)
	}

	if expiry.IsZero() {
		t.Errorf("expected non-zero expiry")
	}

	// Verify expiry is close to what we set
	diff := expiry.Sub(expTime)
	if diff < -1*time.Second || diff > 1*time.Second {
		t.Errorf("expiry time mismatch: expected ~%v, got %v (diff: %v)", expTime, expiry, diff)
	}
}

// TestRefreshTokenLockedSuccess tests refreshTokenLocked with successful response.
func TestRefreshTokenLockedSuccess(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/auth/refresh" && r.Method == "POST" {
			w.Header().Set("Content-Type", "application/json")
			tokenResp := TokenResponse{
				AccessToken:  "new-access",
				RefreshToken: "new-refresh",
				TokenType:    "Bearer",
				ExpiresAt:    time.Now().Add(1 * time.Hour),
			}
			_ = json.NewEncoder(w).Encode(tokenResp) // #nosec G117 -- test mock
			return
		}
		http.NotFound(w, r)
	}))
	defer server.Close()

	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}

	authMgr, _ := NewAuthManager(server.URL, secrets, logger)

	// Set refresh token
	authMgr.mu.Lock()
	authMgr.refreshToken = "old-refresh"
	authMgr.mu.Unlock()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := authMgr.RefreshToken(ctx)
	if err != nil {
		t.Errorf("expected no error, got %v", err)
	}

	if authMgr.GetToken() != "new-access" {
		t.Errorf("expected 'new-access', got %q", authMgr.GetToken())
	}
}

// TestObtainTokenLockedWithExpiryExtraction tests obtainTokenLocked with JWT exp extraction.
func TestObtainTokenLockedWithExpiryExtraction(t *testing.T) {
	// Create a valid JWT
	token := jwt.NewWithClaims(jwt.SigningMethodHS256, jwt.MapClaims{
		"exp": float64(time.Now().Add(2 * time.Hour).Unix()),
	})
	tokenString, _ := token.SignedString([]byte("secret"))

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/auth/token" && r.Method == "POST" {
			w.Header().Set("Content-Type", "application/json")
			// Return response without ExpiresAt (should extract from JWT)
			tokenResp := TokenResponse{
				AccessToken:  tokenString,
				RefreshToken: "test-refresh",
				TokenType:    "Bearer",
				// ExpiresAt is zero, should extract from JWT
			}
			_ = json.NewEncoder(w).Encode(tokenResp) // #nosec G117 -- test mock
			return
		}
		http.NotFound(w, r)
	}))
	defer server.Close()

	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}
	_ = secrets.Set("api_key", []byte("test-api-key"))

	authMgr, _ := NewAuthManager(server.URL, secrets, logger)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := authMgr.EnsureValidToken(ctx)
	if err != nil {
		t.Errorf("expected no error, got %v", err)
	}

	// Token should be set
	if authMgr.GetToken() != tokenString {
		t.Errorf("expected token to match")
	}
}

// TestRefreshTokenLockedWithExpiryExtraction tests refreshTokenLocked with exp extraction.
func TestRefreshTokenLockedWithExpiryExtraction(t *testing.T) {
	token := jwt.NewWithClaims(jwt.SigningMethodHS256, jwt.MapClaims{
		"exp": float64(time.Now().Add(2 * time.Hour).Unix()),
	})
	tokenString, _ := token.SignedString([]byte("secret"))

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/auth/refresh" && r.Method == "POST" {
			w.Header().Set("Content-Type", "application/json")
			tokenResp := TokenResponse{
				AccessToken:  tokenString,
				RefreshToken: "new-refresh",
				TokenType:    "Bearer",
				// ExpiresAt is zero, should extract from JWT
			}
			_ = json.NewEncoder(w).Encode(tokenResp) // #nosec G117 -- test mock
			return
		}
		http.NotFound(w, r)
	}))
	defer server.Close()

	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}

	authMgr, _ := NewAuthManager(server.URL, secrets, logger)

	authMgr.mu.Lock()
	authMgr.refreshToken = "old-refresh"
	authMgr.mu.Unlock()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := authMgr.RefreshToken(ctx)
	if err != nil {
		t.Errorf("expected no error, got %v", err)
	}
}

// TestInitMissingManagerURL tests Init with missing manager_url.
func TestInitMissingManagerURL(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	config := &ModuleConfig{
		// Missing ManagerURL
		NodeID: "test-node",
	}
	configYAML, _ := yaml.Marshal(config)
	host.config = configYAML

	m := New()
	ctx := context.Background()

	err := m.Init(ctx, host)
	if err == nil {
		t.Fatalf("Init should fail with missing manager_url")
	}
}

// TestRegisterMetricsAllMetrics tests that all metrics are registered.
func TestRegisterMetricsAllMetrics(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	config := &ModuleConfig{
		ManagerURL: "http://localhost:8080",
		NodeID:     "test-node",
	}
	configYAML, _ := yaml.Marshal(config)
	host.config = configYAML

	m := New()
	ctx := context.Background()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	if mod.metrics.tunnelUp == nil {
		t.Errorf("tunnelUp metric not initialized")
	}
	if mod.metrics.handshakeAge == nil {
		t.Errorf("handshakeAge metric not initialized")
	}
	if mod.metrics.rxBytes == nil {
		t.Errorf("rxBytes metric not initialized")
	}
	if mod.metrics.txBytes == nil {
		t.Errorf("txBytes metric not initialized")
	}
	if mod.metrics.tokenRefreshes == nil {
		t.Errorf("tokenRefreshes metric not initialized")
	}
	if mod.metrics.connErrors == nil {
		t.Errorf("connErrors metric not initialized")
	}
}

// TestStartAlreadyRunning tests Start when already running.
func TestStartAlreadyRunning(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	config := &ModuleConfig{
		ManagerURL: "http://localhost:8080",
		NodeID:     "test-node",
	}
	configYAML, _ := yaml.Marshal(config)
	host.config = configYAML

	m := New()
	ctx := context.Background()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// First Start
	if err := m.Start(ctx); err != nil {
		t.Fatalf("First Start failed: %v", err)
	}

	// Second Start should fail
	err := m.Start(ctx)
	if err == nil {
		t.Errorf("expected error on second Start")
	}

	_ = m.Stop(ctx)
}

// TestStatusRunning tests status when module is running.
func TestStatusRunning(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	config := &ModuleConfig{
		ManagerURL: "http://localhost:8080",
		NodeID:     "test-node",
	}
	configYAML, _ := yaml.Marshal(config)
	host.config = configYAML

	m := New()
	ctx := context.Background()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	if err := m.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	status, err := m.Status(ctx)
	if err != nil {
		t.Fatalf("Status failed: %v", err)
	}

	if status.State != sdk.StateRunning {
		t.Errorf("expected StateRunning, got %v", status.State)
	}

	_ = m.Stop(ctx)
}

// TestStatusNotRunning tests status when module is not running.
func TestStatusNotRunning(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	config := &ModuleConfig{
		ManagerURL: "http://localhost:8080",
		NodeID:     "test-node",
	}
	configYAML, _ := yaml.Marshal(config)
	host.config = configYAML

	m := New()
	ctx := context.Background()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	status, err := m.Status(ctx)
	if err != nil {
		t.Fatalf("Status failed: %v", err)
	}

	if status.State != sdk.StateDisabled {
		t.Errorf("expected StateDisabled, got %v", status.State)
	}
}

// TestStatusWithConfigDetail tests status includes config details.
func TestStatusWithConfigDetail(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	config := &ModuleConfig{
		ManagerURL: "http://manager.example.com",
		NodeID:     "node-123",
	}
	configYAML, _ := yaml.Marshal(config)
	host.config = configYAML

	m := New()
	ctx := context.Background()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)

	// Manually set tunnel config
	mod.vpnMgr.mu.Lock()
	mod.vpnMgr.config_data = &TunnelConfig{
		ServerEndpoint: "1.2.3.4:51820",
	}
	mod.vpnMgr.connected = true
	mod.vpnMgr.mu.Unlock()

	status, err := m.Status(ctx)
	if err != nil {
		t.Fatalf("Status failed: %v", err)
	}

	if status.Detail["endpoint"] != "http://manager.example.com" {
		t.Errorf("expected endpoint in detail")
	}

	if status.Detail["node_id"] != "node-123" {
		t.Errorf("expected node_id in detail")
	}

	if status.Detail["tunnel"] != "up" {
		t.Errorf("expected tunnel=up")
	}
}

// TestCmdConnectSuccess tests successful connection.
func TestCmdConnectSuccess(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/config" && r.Method == "GET" {
			w.Header().Set("Content-Type", "application/json")
			cfg := TunnelConfig{
				TunnelAddress:   "10.0.0.2/32",
				ServerPublicKey: "sMlWwt2d4gkKsPl6gWAGqtEgp2Xo2S4xyJ1wFjNsFEs=",
				ServerEndpoint:  "203.0.113.1:51820",
				AllowedIPs:      []string{"10.0.0.0/24"},
				DNS:             []string{"1.1.1.1"},
			}
			_ = json.NewEncoder(w).Encode(cfg) // #nosec G117 -- test mock
			return
		}
		http.NotFound(w, r)
	}))
	defer server.Close()

	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	config := &ModuleConfig{
		ManagerURL:    server.URL,
		NodeID:        "test-node",
		InterfaceName: "wg0",
	}
	configYAML, _ := yaml.Marshal(config)
	host.config = configYAML

	m := New()
	ctx := context.Background()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	mod.vpnMgr.wgClient = NewFakeWGController()

	// Start the module to mark it as running
	if err := m.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	// Set token directly
	mod.authMgr.mu.Lock()
	mod.authMgr.token = "test-token"
	mod.authMgr.expiresAt = time.Now().Add(1 * time.Hour)
	mod.authMgr.mu.Unlock()

	result, err := m.Dispatch(ctx, []string{"connect"}, nil, nil)
	if err != nil {
		t.Fatalf("Dispatch failed: %v", err)
	}

	if result.ExitCode != 0 {
		t.Errorf("expected exit code 0, got %d: %s", result.ExitCode, result.Output)
	}

	_ = m.Stop(ctx)
}

// TestRevokeTokenNetworkError tests revoke with network error.
func TestRevokeTokenNetworkError(t *testing.T) {
	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}

	authMgr, _ := NewAuthManager("http://localhost:9999", secrets, logger) // Invalid port

	authMgr.mu.Lock()
	authMgr.token = "test-token"
	authMgr.mu.Unlock()

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	err := authMgr.RevokeToken(ctx)
	if err == nil {
		t.Errorf("expected error on network failure")
	}
}

// TestLoadCachedTokenWithRefreshToken tests loading both access and refresh tokens.
func TestLoadCachedTokenWithRefreshToken(t *testing.T) {
	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}

	// Pre-populate with both tokens
	_ = secrets.Set("access_token", []byte("cached-access"))
	_ = secrets.Set("refresh_token", []byte("cached-refresh"))

	authMgr, _ := NewAuthManager("http://localhost:8080", secrets, logger)

	authMgr.mu.Lock()
	refreshToken := authMgr.refreshToken
	authMgr.mu.Unlock()

	if refreshToken != "cached-refresh" {
		t.Errorf("expected refresh token to be loaded, got %q", refreshToken)
	}
}

// TestNewVPNManagerFailsToInit tests NewVPNManager when wgctrl.New fails.
func TestNewVPNManagerFallback(t *testing.T) {
	logger := zaptest.NewLogger(t)
	config := &ModuleConfig{
		ManagerURL: "http://localhost:8080",
		NodeID:     "test-node",
	}

	vpnMgr := NewVPNManager(config, t.TempDir(), logger)

	// Should have a controller (real or fake)
	if vpnMgr.wgClient == nil {
		t.Errorf("expected wgClient to be set")
	}
}

// TestConnectInvalidServerPublicKey tests Connect with invalid server public key.
func TestConnectInvalidServerPublicKey(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/config" && r.Method == "GET" {
			w.Header().Set("Content-Type", "application/json")
			cfg := TunnelConfig{
				TunnelAddress:   "10.0.0.2/32",
				ServerPublicKey: "invalid-key",
				ServerEndpoint:  "203.0.113.1:51820",
				AllowedIPs:      []string{"10.0.0.0/24"},
				DNS:             []string{"1.1.1.1"},
			}
			_ = json.NewEncoder(w).Encode(cfg) // #nosec G117 -- test mock
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
	authMgr, _ := NewAuthManager(server.URL, secrets, logger)

	authMgr.mu.Lock()
	authMgr.token = "test-token"
	authMgr.mu.Unlock()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := vpnMgr.Connect(ctx, authMgr)
	if err == nil {
		t.Errorf("expected error on invalid server public key")
	}
}

// TestConnectInvalidAllowedIP tests Connect with invalid allowed IP.
func TestConnectInvalidAllowedIP(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/config" && r.Method == "GET" {
			w.Header().Set("Content-Type", "application/json")
			cfg := TunnelConfig{
				TunnelAddress:   "10.0.0.2/32",
				ServerPublicKey: "sMlWwt2d4gkKsPl6gWAGqtEgp2Xo2S4xyJ1wFjNsFEs=",
				ServerEndpoint:  "203.0.113.1:51820",
				AllowedIPs:      []string{"invalid-cidr"},
				DNS:             []string{"1.1.1.1"},
			}
			_ = json.NewEncoder(w).Encode(cfg) // #nosec G117 -- test mock
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
	authMgr, _ := NewAuthManager(server.URL, secrets, logger)

	authMgr.mu.Lock()
	authMgr.token = "test-token"
	authMgr.mu.Unlock()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := vpnMgr.Connect(ctx, authMgr)
	if err == nil {
		t.Errorf("expected error on invalid allowed IP")
	}
}

// TestConnectInvalidServerEndpoint tests Connect with invalid server endpoint.
func TestConnectInvalidServerEndpoint(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/config" && r.Method == "GET" {
			w.Header().Set("Content-Type", "application/json")
			cfg := TunnelConfig{
				TunnelAddress:   "10.0.0.2/32",
				ServerPublicKey: "sMlWwt2d4gkKsPl6gWAGqtEgp2Xo2S4xyJ1wFjNsFEs=",
				ServerEndpoint:  "invalid-endpoint",
				AllowedIPs:      []string{"10.0.0.0/24"},
				DNS:             []string{"1.1.1.1"},
			}
			_ = json.NewEncoder(w).Encode(cfg) // #nosec G117 -- test mock
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
	authMgr, _ := NewAuthManager(server.URL, secrets, logger)

	authMgr.mu.Lock()
	authMgr.token = "test-token"
	authMgr.mu.Unlock()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := vpnMgr.Connect(ctx, authMgr)
	if err == nil {
		t.Errorf("expected error on invalid server endpoint")
	}
}

// TestFakeWGControllerGetLastConfig tests the GetLastConfig method.
func TestFakeWGControllerGetLastConfig(t *testing.T) {
	fake := NewFakeWGController()

	cfg := &wgtypes.Config{}
	_ = fake.Configure("wg0", cfg)

	lastCfg := fake.GetLastConfig("wg0")
	if lastCfg == nil {
		t.Errorf("expected config to be stored")
	}

	lastCfg2 := fake.GetLastConfig("wg1")
	if lastCfg2 != nil {
		t.Errorf("expected nil for non-existent device")
	}
}

// TestFakeSecretStoreDelete tests the Delete method.
func TestFakeSecretStoreDelete(t *testing.T) {
	store := &FakeSecretStore{store: make(map[string][]byte)}

	_ = store.Set("key", []byte("value"))
	_, err := store.Get("key")
	if err != nil {
		t.Errorf("key should exist after Set")
	}

	_ = store.Delete("key")
	_, err = store.Get("key")
	if err == nil {
		t.Errorf("key should not exist after Delete")
	}
}

// TestRefreshTokenMissingAccessTokenInResponse tests refresh when response is missing access_token.
func TestRefreshTokenBadResponseFormat(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/auth/refresh" && r.Method == "POST" {
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte("not a json object")) // #nosec G104 -- test code
			return
		}
		http.NotFound(w, r)
	}))
	defer server.Close()

	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}

	authMgr, _ := NewAuthManager(server.URL, secrets, logger)

	authMgr.mu.Lock()
	authMgr.refreshToken = "test-refresh"
	authMgr.mu.Unlock()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := authMgr.RefreshToken(ctx)
	if err == nil {
		t.Errorf("expected error on bad response format")
	}
}

// TestObtainTokenBadResponseFormat tests obtain when response is invalid JSON.
func TestObtainTokenBadResponseFormat(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/auth/token" && r.Method == "POST" {
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte("not json")) // #nosec G104 -- test code
			return
		}
		http.NotFound(w, r)
	}))
	defer server.Close()

	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}
	_ = secrets.Set("api_key", []byte("test-api-key"))

	authMgr, _ := NewAuthManager(server.URL, secrets, logger)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := authMgr.EnsureValidToken(ctx)
	if err == nil {
		t.Errorf("expected error on bad response format")
	}
}
