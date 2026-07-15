package tobogganing

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/penguintechinc/penguin/pkg/sdk"
	"github.com/prometheus/client_golang/prometheus"
	"go.uber.org/zap/zaptest"
	"gopkg.in/yaml.v3"
)

// TestFakeLicenseCheckerFeatureEnabled tests the fake license checker.
func TestFakeLicenseCheckerFeatureEnabled(t *testing.T) {
	lc := &FakeLicenseChecker{featureEnabled: true}
	if !lc.FeatureEnabled("test.feature") {
		t.Errorf("expected feature to be enabled")
	}

	lc.featureEnabled = false
	if lc.FeatureEnabled("test.feature") {
		t.Errorf("expected feature to be disabled")
	}
}

// TestFakeLicenseCheckerTier tests the fake license checker tier.
func TestFakeLicenseCheckerTier(t *testing.T) {
	lc := &FakeLicenseChecker{tier: "professional"}
	if lc.Tier() != "professional" {
		t.Errorf("expected tier 'professional', got %q", lc.Tier())
	}

	lc.tier = "enterprise"
	if lc.Tier() != "enterprise" {
		t.Errorf("expected tier 'enterprise', got %q", lc.Tier())
	}
}

// TestFakeEventSinkPublish tests the fake event sink.
func TestFakeEventSinkPublish(t *testing.T) {
	sink := &FakeEventSink{events: []sdk.Event{}}
	ev := sdk.Event{
		Module:  "test",
		Type:    sdk.EventStateChanged,
		Message: "test_event",
		Fields:  map[string]string{"key": "value"},
	}
	sink.Publish(ev)
	if len(sink.events) != 1 {
		t.Errorf("expected 1 event, got %d", len(sink.events))
	}
	if sink.events[0].Message != "test_event" {
		t.Errorf("expected event message 'test_event', got %q", sink.events[0].Message)
	}
}

// TestFakeHostServicesDevice tests the device method.
func TestFakeHostServicesDevice(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())
	_ = host
	// host.Device() is not part of FakeHostServices, so we test the fakes here
}

// TestFakeHTTPClientDoJSON tests the fake HTTP client.
func TestFakeHTTPClientDoJSON(t *testing.T) {
	client := &FakeHTTPClient{
		responses: make(map[string]interface{}),
		errors:    make(map[string]error),
	}

	ctx := context.Background()
	var respBody map[string]string

	// Test successful response
	client.responses["http://example.com/api"] = map[string]string{"key": "value"}
	err := client.DoJSON(ctx, "GET", "http://example.com/api", "token", nil, &respBody)
	if err != nil {
		t.Errorf("expected no error, got %v", err)
	}
	if respBody["key"] != "value" {
		t.Errorf("expected key=value, got %q", respBody["key"])
	}

	// Test error response
	testErr := fmt.Errorf("test error")
	client.errors["http://example.com/error"] = testErr
	err = client.DoJSON(ctx, "GET", "http://example.com/error", "token", nil, &respBody)
	if err != testErr {
		t.Errorf("expected error, got %v", err)
	}
}

// TestExtractTokenExpiryMalformed tests extractTokenExpiry with malformed token.
func TestExtractTokenExpiryMalformed(t *testing.T) {
	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}
	authMgr, _ := NewAuthManager("http://localhost:8080", secrets, logger)

	// Test with empty token
	_, err := authMgr.extractTokenExpiry("")
	if err == nil {
		t.Errorf("expected error for empty token")
	}

	// Test with invalid JWT
	_, err = authMgr.extractTokenExpiry("not.a.jwt")
	if err == nil {
		t.Errorf("expected error for invalid JWT")
	}

	// Test with JWT missing exp claim. This is the canonical public jwt.io example
	// token (sub=1234567890, HS256 over the demo secret) — a fixed test vector, not
	// a credential.
	invalidJWT := "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U" // gitleaks:allow
	_, err = authMgr.extractTokenExpiry(invalidJWT)
	if err == nil {
		t.Errorf("expected error for JWT without exp")
	}
}

// TestLoadCachedTokenPresent tests loading a cached token.
func TestLoadCachedTokenPresent(t *testing.T) {
	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}

	// Pre-populate with cached token
	cachedToken := "cached-access-token"
	_ = secrets.Set("access_token", []byte(cachedToken))

	authMgr, _ := NewAuthManager("http://localhost:8080", secrets, logger)

	// Token should be loaded
	if authMgr.GetToken() != cachedToken {
		t.Errorf("expected cached token, got %q", authMgr.GetToken())
	}
}

// TestLoadCachedTokenAbsent tests loading when no cached token exists.
func TestLoadCachedTokenAbsent(t *testing.T) {
	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}

	authMgr, _ := NewAuthManager("http://localhost:8080", secrets, logger)

	// Token should be empty
	if authMgr.GetToken() != "" {
		t.Errorf("expected empty token, got %q", authMgr.GetToken())
	}
}

// TestLoadCachedTokenCorruptJSON tests loading corrupted cached token.
func TestLoadCachedTokenCorruptJSON(t *testing.T) {
	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}

	// Set corrupted token (not a valid JWT)
	_ = secrets.Set("access_token", []byte("not-a-jwt"))

	authMgr, _ := NewAuthManager("http://localhost:8080", secrets, logger)

	// Should still load (parseUnverified doesn't fail hard)
	_ = authMgr.GetToken()
}

// TestEnsureValidTokenValidCached tests that a valid cached token is used.
func TestEnsureValidTokenValidCached(t *testing.T) {
	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}

	authMgr, _ := NewAuthManager("http://localhost:8080", secrets, logger)

	// Manually set a valid future token
	authMgr.mu.Lock()
	authMgr.token = "valid-token"
	authMgr.expiresAt = time.Now().Add(1 * time.Hour)
	authMgr.mu.Unlock()

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	// Should not fail since token is valid
	err := authMgr.EnsureValidToken(ctx)
	if err != nil {
		t.Errorf("expected no error with valid cached token, got %v", err)
	}
}

// TestEnsureValidTokenExpiredWithRefreshToken tests refresh when access token expires.
func TestEnsureValidTokenExpiredWithRefreshToken(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/auth/refresh" && r.Method == "POST" {
			w.Header().Set("Content-Type", "application/json")
			tokenResp := TokenResponse{
				AccessToken:  "refreshed-token",
				RefreshToken: "new-refresh-token",
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

	// Set expired access token with valid refresh token
	authMgr.mu.Lock()
	authMgr.token = "expired-token"
	authMgr.refreshToken = "valid-refresh-token"
	authMgr.expiresAt = time.Now().Add(-1 * time.Hour) // Expired
	authMgr.mu.Unlock()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := authMgr.EnsureValidToken(ctx)
	if err != nil {
		t.Errorf("expected no error, got %v", err)
	}

	// Token should be refreshed
	if authMgr.GetToken() != "refreshed-token" {
		t.Errorf("expected refreshed token, got %q", authMgr.GetToken())
	}
}

// TestEnsureValidTokenNoTokenNoKey tests obtaining token when none exists and no API key.
func TestEnsureValidTokenNoTokenNoKey(t *testing.T) {
	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)} // No API key

	authMgr, _ := NewAuthManager("http://localhost:8080", secrets, logger)

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	err := authMgr.EnsureValidToken(ctx)
	if err == nil {
		t.Errorf("expected error when no token and no API key")
	}
}

// TestRefreshTokenNoRefreshToken tests refresh when no refresh token is available.
func TestRefreshTokenNoRefreshToken(t *testing.T) {
	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}

	authMgr, _ := NewAuthManager("http://localhost:8080", secrets, logger)

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	// Should fail since no refresh token
	err := authMgr.RefreshToken(ctx)
	if err == nil {
		t.Errorf("expected error when no refresh token")
	}
}

// TestRefreshTokenServerError tests refresh when server returns error.
func TestRefreshTokenServerError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/auth/refresh" {
			w.WriteHeader(http.StatusInternalServerError)
			return
		}
		http.NotFound(w, r)
	}))
	defer server.Close()

	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}

	authMgr, _ := NewAuthManager(server.URL, secrets, logger)

	authMgr.mu.Lock()
	authMgr.refreshToken = "test-refresh-token"
	authMgr.mu.Unlock()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := authMgr.RefreshToken(ctx)
	if err == nil {
		t.Errorf("expected error on server error")
	}
}

// TestRevokeTokenNoToken tests revoke when no token is set.
func TestRevokeTokenNoToken(t *testing.T) {
	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}

	authMgr, _ := NewAuthManager("http://localhost:8080", secrets, logger)

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	// Should succeed even without token
	err := authMgr.RevokeToken(ctx)
	if err != nil {
		t.Errorf("expected no error when revoking missing token, got %v", err)
	}
}

// TestRevokeTokenServerError tests revoke when server returns error.
func TestRevokeTokenServerError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/auth/revoke" {
			w.WriteHeader(http.StatusForbidden)
			return
		}
		http.NotFound(w, r)
	}))
	defer server.Close()

	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}

	authMgr, _ := NewAuthManager(server.URL, secrets, logger)

	authMgr.mu.Lock()
	authMgr.token = "test-token"
	authMgr.mu.Unlock()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := authMgr.RevokeToken(ctx)
	if err == nil {
		t.Errorf("expected error on revoke server error")
	}
}

// TestObtainTokenHTTPError tests obtain token when HTTP request fails.
func TestObtainTokenHTTPError(t *testing.T) {
	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}
	_ = secrets.Set("api_key", []byte("test-api-key"))

	authMgr, _ := NewAuthManager("http://localhost:9999", secrets, logger) // Invalid port

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	err := authMgr.EnsureValidToken(ctx)
	if err == nil {
		t.Errorf("expected error on network failure")
	}
}

// TestObtainTokenNon200 tests obtain token when server returns non-200.
func TestObtainTokenNon200(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/auth/token" {
			w.WriteHeader(http.StatusUnauthorized)
			_, _ = w.Write([]byte("unauthorized")) // #nosec G104 -- test code
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
		t.Errorf("expected error on non-200 response")
	}
}

// TestInitRegisterMetricsCustomRegistry tests Init with metrics registration.
func TestInitRegisterMetricsCustomRegistry(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	// Use a fresh registry to track registrations
	host.metrics = prometheus.NewRegistry()

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
	if mod.metrics == nil {
		t.Errorf("metrics should be registered")
	}
}

// TestRecoverFromCrash tests the recover from crash function.
func TestRecoverFromCrash(t *testing.T) {
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

	// recoverFromCrash is called in Init, should not panic
	mod := m.(*Module)
	mod.recoverFromCrash() // Call again to test it's idempotent
}

// TestInitialConnectTokenSuccess tests initial connect with successful token.
func TestInitialConnectTokenSuccess(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/auth/token" && r.Method == "POST" {
			w.Header().Set("Content-Type", "application/json")
			tokenResp := TokenResponse{
				AccessToken: "test-token",
				TokenType:   "Bearer",
				ExpiresAt:   time.Now().Add(1 * time.Hour),
			}
			_ = json.NewEncoder(w).Encode(tokenResp) // #nosec G117 -- test mock
			return
		}
		http.NotFound(w, r)
	}))
	defer server.Close()

	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())
	_ = host.Secrets().Set("api_key", []byte("test-api-key"))

	config := &ModuleConfig{
		ManagerURL: server.URL,
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
	mod.initialConnect(ctx)
	// Should obtain token without error
}

// TestInitialConnectTokenFailure tests initial connect with failed token.
func TestInitialConnectTokenFailure(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.NotFound(w, r)
	}))
	defer server.Close()

	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	config := &ModuleConfig{
		ManagerURL: server.URL,
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
	mod.initialConnect(ctx)
	// Should fail gracefully (no API key)
}

// TestStatusConnected tests status when tunnel is connected.
func TestStatusConnected(t *testing.T) {
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

	// Manually mark as connected
	mod.vpnMgr.mu.Lock()
	mod.vpnMgr.connected = true
	mod.vpnMgr.mu.Unlock()

	status, err := m.Status(ctx)
	if err != nil {
		t.Fatalf("Status failed: %v", err)
	}

	if status.Detail["tunnel"] != "up" {
		t.Errorf("expected tunnel=up, got %q", status.Detail["tunnel"])
	}
}

// TestStatusJSONFormatting tests the JSON formatting in cmdStatus.
func TestStatusJSONFormatting(t *testing.T) {
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

	result, err := m.Dispatch(ctx, []string{"status"}, map[string]string{"json": "true"}, nil)
	if err != nil {
		t.Fatalf("Dispatch failed: %v", err)
	}

	if result.ExitCode != 0 {
		t.Errorf("expected exit code 0, got %d", result.ExitCode)
	}

	// Verify JSON is valid
	var data map[string]interface{}
	if err := json.Unmarshal(result.JSON, &data); err != nil {
		t.Errorf("invalid JSON: %v", err)
	}

	if _, ok := data["state"]; !ok {
		t.Errorf("missing state in JSON")
	}
}

// TestCmdConnectFailure tests cmdConnect when connection fails.
func TestCmdConnectFailure(t *testing.T) {
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

	result, err := m.Dispatch(ctx, []string{"connect"}, nil, nil)
	if err != nil {
		t.Fatalf("Dispatch failed: %v", err)
	}

	// Connection should fail (no manager)
	if result.ExitCode == 0 {
		t.Errorf("expected non-zero exit code for failed connection")
	}
}

// TestCmdDisconnectSuccess tests cmdDisconnect.
func TestCmdDisconnectSuccess(t *testing.T) {
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

	result, err := m.Dispatch(ctx, []string{"disconnect"}, nil, nil)
	if err != nil {
		t.Fatalf("Dispatch failed: %v", err)
	}

	if result.ExitCode != 0 {
		t.Errorf("expected exit code 0, got %d", result.ExitCode)
	}
}

// TestCmdRotateSuccess tests cmdRotate.
func TestCmdRotateSuccess(t *testing.T) {
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
	_ = host.Secrets().Set("api_key", []byte("test-api-key"))

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

	// Set token directly since API key isn't in secrets
	mod.authMgr.mu.Lock()
	mod.authMgr.token = "test-token"
	mod.authMgr.expiresAt = time.Now().Add(1 * time.Hour)
	mod.authMgr.mu.Unlock()

	result, err := m.Dispatch(ctx, []string{"rotate"}, map[string]string{"force": "false"}, nil)
	if err != nil {
		t.Fatalf("Dispatch failed: %v", err)
	}

	if result.ExitCode != 0 {
		t.Errorf("expected exit code 0 for rotate, got %d: %s", result.ExitCode, result.Output)
	}
}

// TestAuthRefreshLoopWithCancel tests the auth refresh loop behavior.
func TestAuthRefreshLoopWithCancel(t *testing.T) {
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

	// Start the module
	if err := m.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	// Immediately stop to test loop cancellation
	if err := m.Stop(ctx); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}
}

// TestMonitorLoopHealthCheck tests the monitor loop health updates.
func TestMonitorLoopHealthCheck(t *testing.T) {
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

	// Manually call updateHealthProbe
	mod.updateHealthProbe()

	health := m.Health(ctx)
	if health.Level == 0 {
		t.Errorf("health level should be set")
	}
}

// TestUpdateHealthProbeConnected tests health probe when connected.
func TestUpdateHealthProbeConnected(t *testing.T) {
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

	// Mark tunnel as connected with recent handshake
	mod.vpnMgr.mu.Lock()
	mod.vpnMgr.connected = true
	mod.vpnMgr.lastHandshake = time.Now()
	mod.vpnMgr.mu.Unlock()

	mod.updateHealthProbe()

	health := m.Health(ctx)
	if health.Level != sdk.Healthy {
		t.Errorf("expected Healthy, got %d", health.Level)
	}
}

// TestUpdateHealthProbeDegraded tests health probe when handshake is stale.
func TestUpdateHealthProbeDegraded(t *testing.T) {
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

	// Mark tunnel as connected with old handshake
	mod.vpnMgr.mu.Lock()
	mod.vpnMgr.connected = true
	mod.vpnMgr.lastHandshake = time.Now().Add(-3 * time.Minute)
	mod.vpnMgr.mu.Unlock()

	mod.updateHealthProbe()

	health := m.Health(ctx)
	if health.Level != sdk.Degraded {
		t.Errorf("expected Degraded, got %d", health.Level)
	}
}

// TestUpdateHealthProbeDisconnected tests health probe when disconnected.
func TestUpdateHealthProbeDisconnected(t *testing.T) {
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

	// Ensure not connected
	mod.vpnMgr.mu.Lock()
	mod.vpnMgr.connected = false
	mod.vpnMgr.mu.Unlock()

	mod.updateHealthProbe()

	health := m.Health(ctx)
	if health.Level != sdk.Unhealthy {
		t.Errorf("expected Unhealthy, got %d", health.Level)
	}
}

// TestEstablishTunnelSuccess tests establishing tunnel.
func TestEstablishTunnelSuccess(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/auth/token" && r.Method == "POST" {
			w.Header().Set("Content-Type", "application/json")
			tokenResp := TokenResponse{
				AccessToken: "test-token",
				TokenType:   "Bearer",
				ExpiresAt:   time.Now().Add(1 * time.Hour),
			}
			_ = json.NewEncoder(w).Encode(tokenResp) // #nosec G117 -- test mock
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
			_ = json.NewEncoder(w).Encode(cfg) // #nosec G117 -- test mock
			return
		}
		http.NotFound(w, r)
	}))
	defer server.Close()

	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())
	_ = host.Secrets().Set("api_key", []byte("test-api-key"))

	config := &ModuleConfig{
		ManagerURL: server.URL,
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
	mod.vpnMgr.wgClient = NewFakeWGController()
	mod.mu.Lock()
	mod.running = true
	mod.mu.Unlock()

	err := mod.establishTunnel(ctx)
	if err != nil {
		t.Errorf("expected no error, got %v", err)
	}

	if !mod.vpnMgr.IsConnected() {
		t.Errorf("expected tunnel to be connected")
	}
}

// TestEstablishTunnelNotRunning tests tunnel establishment when module not running.
func TestEstablishTunnelNotRunning(t *testing.T) {
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

	// Module not running
	err := mod.establishTunnel(ctx)
	if err == nil {
		t.Errorf("expected error when not running")
	}
}

// TestNewVPNManagerEmbedded tests VPNManager with embedded flag.
func TestNewVPNManagerEmbedded(t *testing.T) {
	logger := zaptest.NewLogger(t)
	config := &ModuleConfig{
		ManagerURL: "http://localhost:8080",
		NodeID:     "test-node",
		Embedded:   true,
	}

	vpnMgr := NewVPNManager(config, t.TempDir(), logger)
	if vpnMgr == nil {
		t.Errorf("expected VPNManager to be created")
	}
}

// TestConnectErrorsOnFetchConfig tests Connect error handling on config fetch.
func TestConnectErrorsOnFetchConfig(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/config" {
			w.WriteHeader(http.StatusInternalServerError)
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

	// Set token directly
	authMgr.mu.Lock()
	authMgr.token = "test-token"
	authMgr.mu.Unlock()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := vpnMgr.Connect(ctx, authMgr)
	if err == nil {
		t.Errorf("expected error on config fetch failure")
	}
}

// TestFetchTunnelConfigNoToken tests fetching config without token.
func TestFetchTunnelConfigNoToken(t *testing.T) {
	logger := zaptest.NewLogger(t)
	config := &ModuleConfig{
		ManagerURL: "http://localhost:8080",
		NodeID:     "test-node",
	}

	vpnMgr := NewVPNManager(config, t.TempDir(), logger)

	secrets := &FakeSecretStore{store: make(map[string][]byte)}
	authMgr, _ := NewAuthManager("http://localhost:8080", secrets, logger)

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	_, err := vpnMgr.fetchTunnelConfig(ctx, authMgr)
	if err == nil {
		t.Errorf("expected error when no token")
	}
}

// TestFetchTunnelConfigBadJSON tests fetching config with bad JSON response.
func TestFetchTunnelConfigBadJSON(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/config" {
			w.Header().Set("Content-Type", "application/json")
			_, _ = w.Write([]byte("not json")) // #nosec G104 -- test code
			return
		}
		http.NotFound(w, r)
	}))
	defer server.Close()

	logger := zaptest.NewLogger(t)
	config := &ModuleConfig{
		ManagerURL: server.URL,
		NodeID:     "test-node",
	}

	vpnMgr := NewVPNManager(config, t.TempDir(), logger)

	secrets := &FakeSecretStore{store: make(map[string][]byte)}
	authMgr, _ := NewAuthManager(server.URL, secrets, logger)

	// Set token directly
	authMgr.mu.Lock()
	authMgr.token = "test-token"
	authMgr.mu.Unlock()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	_, err := vpnMgr.fetchTunnelConfig(ctx, authMgr)
	if err == nil {
		t.Errorf("expected error on bad JSON")
	}
}

// TestRealHTTPClientDoJSONNetworkError tests network error handling.
func TestRealHTTPClientDoJSONNetworkError(t *testing.T) {
	client := &realHTTPClient{}
	ctx, cancel := context.WithTimeout(context.Background(), 1*time.Millisecond)
	defer cancel()

	var respBody interface{}
	err := client.DoJSON(ctx, "GET", "http://localhost:9999", "token", nil, &respBody)
	if err == nil {
		t.Errorf("expected error on network failure")
	}
}

// TestRealHTTPClientDoJSONNon200 tests non-200 status handling.
func TestRealHTTPClientDoJSONNon200(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
	}))
	defer server.Close()

	client := &realHTTPClient{}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	var respBody interface{}
	err := client.DoJSON(ctx, "GET", server.URL, "token", nil, &respBody)
	if err == nil {
		t.Errorf("expected error on non-200 status")
	}
}

// TestConnectAlreadyConnected tests Connect when already connected.
func TestConnectAlreadyConnected(t *testing.T) {
	logger := zaptest.NewLogger(t)
	config := &ModuleConfig{
		ManagerURL: "http://localhost:8080",
		NodeID:     "test-node",
	}

	vpnMgr := NewVPNManager(config, t.TempDir(), logger)
	vpnMgr.wgClient = NewFakeWGController()

	// Mark as connected
	vpnMgr.mu.Lock()
	vpnMgr.connected = true
	vpnMgr.mu.Unlock()

	secrets := &FakeSecretStore{store: make(map[string][]byte)}
	authMgr, _ := NewAuthManager("http://localhost:8080", secrets, logger)

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	err := vpnMgr.Connect(ctx, authMgr)
	if err == nil {
		t.Errorf("expected error when already connected")
	}
}
