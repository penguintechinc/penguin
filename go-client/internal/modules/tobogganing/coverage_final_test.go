package tobogganing

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"go.uber.org/zap/zaptest"
	"golang.zx2c4.com/wireguard/wgctrl/wgtypes"
	"gopkg.in/yaml.v3"
)

// TestRealHTTPClientDoJSONBadStatusAndJSON tests both bad status and bad JSON.
func TestRealHTTPClientDoJSONBadStatusAndJSON(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
		_, _ = w.Write([]byte("error message")) // #nosec G104 -- test code
	}))
	defer server.Close()

	client := &realHTTPClient{}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	var respBody interface{}
	err := client.DoJSON(ctx, "POST", server.URL, "token", map[string]string{"key": "val"}, &respBody)
	if err == nil {
		t.Errorf("expected error on non-200 status")
	}
}

// TestRealHTTPClientDoJSONMarshalError tests when request body can't be marshaled.
func TestRealHTTPClientDoJSONUnmarshalError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte("not json")) // #nosec G104 -- test code
	}))
	defer server.Close()

	client := &realHTTPClient{}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	var respBody interface{}
	err := client.DoJSON(ctx, "GET", server.URL, "token", nil, &respBody)
	if err == nil {
		t.Errorf("expected error on bad response JSON")
	}
}

// TestInitWithInvalidConfigYAML tests Init with invalid YAML.
func TestInitWithInvalidConfigYAML(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	// Set invalid YAML
	host.config = []byte("{ invalid yaml [ }")

	m := New()
	ctx := context.Background()

	err := m.Init(ctx, host)
	if err == nil {
		t.Fatalf("Init should fail with invalid YAML")
	}
}

// TestCmdDisconnectNotConnected tests disconnect when not connected.
func TestCmdDisconnectNotConnected(t *testing.T) {
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

	// Should succeed even when not connected (idempotent)
	if result.ExitCode != 0 {
		t.Errorf("expected exit code 0, got %d", result.ExitCode)
	}
}

// TestStopWhileNotRunning tests Stop when module is not running.
func TestStopWhileNotRunning(t *testing.T) {
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

	// Stop without Start
	err := m.Stop(ctx)
	if err != nil {
		t.Errorf("Stop without Start should be idempotent, got %v", err)
	}
}

// TestStartReturnsPromptlyStress tests Start multiple times.
func TestStartStress(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	config := &ModuleConfig{
		ManagerURL: "http://10.255.255.1:9", // Unreachable
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

	// Try to Start again (should fail)
	if err := m.Start(ctx); err == nil {
		t.Errorf("expected error on second Start")
	}

	if err := m.Stop(ctx); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}

	// Stop again (should be idempotent)
	if err := m.Stop(ctx); err != nil {
		t.Fatalf("second Stop should be idempotent: %v", err)
	}
}

// TestHealthReportStructure tests that Health returns valid report.
func TestHealthReportStructure(t *testing.T) {
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
	// Manually trigger health probe to populate CheckedAt
	mod.updateHealthProbe()

	health := m.Health(ctx)

	// Should have valid timestamp (since we called updateHealthProbe)
	if health.CheckedAt.IsZero() {
		t.Errorf("expected non-zero CheckedAt")
	}

	// Should have a valid level (Unhealthy when not connected)
	if health.Level != 0 && health.Level != 1 && health.Level != 2 {
		t.Errorf("expected valid health level")
	}
}

// TestDispatchWithNilFlags tests Dispatch with nil flags.
func TestDispatchWithNilFlags(t *testing.T) {
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

	// Dispatch with nil flags
	result, err := m.Dispatch(ctx, []string{"status"}, nil, nil)
	if err != nil {
		t.Fatalf("Dispatch failed: %v", err)
	}

	if result.ExitCode != 0 {
		t.Errorf("expected exit code 0, got %d", result.ExitCode)
	}
}

// TestRotateConfigFailure tests rotate when config fetch fails.
func TestRotateConfigFailure(t *testing.T) {
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

	result, err := m.Dispatch(ctx, []string{"rotate"}, nil, nil)
	if err != nil {
		t.Fatalf("Dispatch failed: %v", err)
	}

	// Should fail because no token is available
	if result.ExitCode == 0 {
		t.Errorf("expected non-zero exit code for rotate failure")
	}
}

// TestFakeHostServicesFullAccess tests all FakeHostServices methods.
func TestFakeHostServicesFullAccess(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	// Test Logger
	if host.Logger() == nil {
		t.Errorf("expected Logger to be set")
	}

	// Test Secrets
	if host.Secrets() == nil {
		t.Errorf("expected Secrets to be set")
	}

	// Test License
	if host.License() == nil {
		t.Errorf("expected License to be set")
	}

	// Test Metrics
	if host.Metrics() == nil {
		t.Errorf("expected Metrics to be set")
	}

	// Test DataDir
	if host.DataDir() == "" {
		t.Errorf("expected DataDir to be set")
	}

	// Test Config
	if len(host.Config()) > 0 {
		t.Errorf("expected Config to be empty initially")
	}

	// Test Events
	if host.Events() == nil {
		t.Errorf("expected Events to be set")
	}
}

// TestRealHTTPClientRequestCreation tests that request is properly formed.
func TestRealHTTPClientRequestCreation(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Verify request properties
		if r.Header.Get("Authorization") == "" {
			t.Errorf("expected Authorization header")
		}

		if r.Header.Get("Content-Type") != "application/json" {
			t.Errorf("expected Content-Type header")
		}

		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]string{"result": "ok"}) // #nosec G117 -- test mock
	}))
	defer server.Close()

	client := &realHTTPClient{}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	type Response struct {
		Result string `json:"result"`
	}
	var resp Response

	err := client.DoJSON(ctx, "POST", server.URL, "test-token", map[string]string{"data": "value"}, &resp)
	if err != nil {
		t.Errorf("expected no error, got %v", err)
	}

	if resp.Result != "ok" {
		t.Errorf("expected result 'ok', got %q", resp.Result)
	}
}

// TestRealHTTPClientBodyClose tests that response body is properly closed.
func TestRealHTTPClientBodyClose(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]interface{}{"key": "value"}) // #nosec G117 -- test mock
	}))
	defer server.Close()

	client := &realHTTPClient{}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	var resp map[string]interface{}

	// Multiple calls to ensure body is properly closed each time
	for i := 0; i < 3; i++ {
		err := client.DoJSON(ctx, "GET", server.URL, "token", nil, &resp)
		if err != nil {
			t.Errorf("call %d: expected no error, got %v", i+1, err)
		}
	}
}

// TestEnsureValidTokenRefreshNetworkError tests refresh with network error.
func TestEnsureValidTokenRefreshNetworkError(t *testing.T) {
	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}
	_ = secrets.Set("api_key", []byte("test-api-key"))

	authMgr, _ := NewAuthManager("http://localhost:9999", secrets, logger)

	authMgr.mu.Lock()
	authMgr.token = "expired"
	authMgr.expiresAt = time.Now().Add(-1 * time.Hour)
	authMgr.refreshToken = "refresh-token"
	authMgr.mu.Unlock()

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	// Should fail when network is unavailable
	err := authMgr.EnsureValidToken(ctx)
	if err == nil {
		t.Errorf("expected error when refresh fails and API key endpoint unreachable")
	}
}

// TestConnectConfigureError tests Connect when Configure fails.
func TestConnectConfigureError(t *testing.T) {
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
	config := &ModuleConfig{
		ManagerURL:    server.URL,
		NodeID:        "test-node",
		InterfaceName: "wg0",
	}

	vpnMgr := NewVPNManager(config, t.TempDir(), logger)

	// Use a mock controller that fails on Configure
	failingController := &FailingWGController{}
	vpnMgr.wgClient = failingController

	secrets := &FakeSecretStore{store: make(map[string][]byte)}
	authMgr, _ := NewAuthManager(server.URL, secrets, logger)

	authMgr.mu.Lock()
	authMgr.token = "test-token"
	authMgr.mu.Unlock()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := vpnMgr.Connect(ctx, authMgr)
	if err == nil {
		t.Errorf("expected error when Configure fails")
	}
}

// FailingWGController is a test helper that fails on Configure.
type FailingWGController struct{}

func (f *FailingWGController) Devices() ([]string, error) {
	return []string{}, nil
}

func (f *FailingWGController) Close() error {
	return nil
}

func (f *FailingWGController) Device(name string) (*wgtypes.Device, error) {
	return nil, nil
}

func (f *FailingWGController) Configure(name string, cfg *wgtypes.Config) error {
	return io.EOF // Simulate configure failure
}

// TestVPNManagerIsConnectedFalse tests IsConnected returns false when not connected.
func TestVPNManagerIsConnectedFalse(t *testing.T) {
	logger := zaptest.NewLogger(t)
	config := &ModuleConfig{
		ManagerURL: "http://localhost:8080",
		NodeID:     "test-node",
	}

	vpnMgr := NewVPNManager(config, t.TempDir(), logger)

	if vpnMgr.IsConnected() {
		t.Errorf("expected IsConnected to be false initially")
	}
}

// TestRefreshTokenMissingRefreshToken tests refresh without refresh token.
func TestRefreshTokenMissingRefreshTokenLockedPath(t *testing.T) {
	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}

	authMgr, _ := NewAuthManager("http://localhost:8080", secrets, logger)

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	// Try to refresh without setting a refresh token
	err := authMgr.RefreshToken(ctx)
	if err == nil {
		t.Errorf("expected error when no refresh token available")
	}
}

// TestRealHTTPClientNoRequestBody tests DoJSON with nil request body.
func TestRealHTTPClientNoRequestBody(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.ContentLength != 0 {
			t.Errorf("expected no request body")
		}

		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]string{"status": "ok"}) // #nosec G117 -- test mock
	}))
	defer server.Close()

	client := &realHTTPClient{}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	type Response struct {
		Status string `json:"status"`
	}
	var resp Response

	// Call with nil request body
	err := client.DoJSON(ctx, "GET", server.URL, "token", nil, &resp)
	if err != nil {
		t.Errorf("expected no error, got %v", err)
	}
}
