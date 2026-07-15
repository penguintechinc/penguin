package tobogganing

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/penguintechinc/penguin/pkg/sdk"
	"go.uber.org/zap/zaptest"
	"gopkg.in/yaml.v3"
)

func TestModuleInfo(t *testing.T) {
	m := New()
	info := m.Info()

	if info.Name != "tobogganing" {
		t.Errorf("expected name 'tobogganing', got %q", info.Name)
	}

	// Tobogganing is core product (Free tier): must load without license server
	if info.LicenseFeature != "" {
		t.Errorf("expected no module-level license gate, got %q", info.LicenseFeature)
	}

	if info.Description == "" {
		t.Errorf("expected non-empty description")
	}
}

func TestModuleInit(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	config := &ModuleConfig{
		ManagerURL: "http://localhost:8080",
		NodeID:     "test-node",
	}
	configYAML, _ := yaml.Marshal(config)
	host.config = configYAML

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := m.Init(ctx, host)
	if err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Verify module state
	mod := m.(*Module)
	if mod.host == nil {
		t.Errorf("host not set")
	}
	if mod.authMgr == nil {
		t.Errorf("auth manager not created")
	}
	if mod.vpnMgr == nil {
		t.Errorf("vpn manager not created")
	}
}

func TestModuleInitMissingConfig(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())
	host.config = []byte(`{}`) // Empty config

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := m.Init(ctx, host)
	if err == nil {
		t.Errorf("expected error for missing manager_url")
	}
}

func TestModuleInitialization(t *testing.T) {
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

	// Verify that the module is in a valid initialized state
	if mod.host == nil {
		t.Errorf("host not set after Init")
	}
	if mod.authMgr == nil {
		t.Errorf("authMgr not created after Init")
	}
	if mod.vpnMgr == nil {
		t.Errorf("vpnMgr not created after Init")
	}

	// Stop without Start should be idempotent
	if err := mod.Stop(ctx); err != nil {
		t.Errorf("Stop without Start failed: %v", err)
	}
}

func TestModuleStatus(t *testing.T) {
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
		t.Errorf("Status failed: %v", err)
	}

	if status.Detail == nil {
		t.Errorf("Status detail is nil")
	}

	if status.Detail["tunnel"] != "down" {
		t.Errorf("expected tunnel=down, got %q", status.Detail["tunnel"])
	}
}

func TestModuleHealth(t *testing.T) {
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

	health := m.Health(ctx)
	if health.Level != sdk.Unhealthy && health.Level != sdk.Healthy && health.Level != sdk.Degraded {
		t.Errorf("invalid health level: %d", health.Level)
	}
}

func TestModuleCommands(t *testing.T) {
	m := New()
	cmds := m.Commands()

	if len(cmds) == 0 {
		t.Errorf("expected commands, got none")
	}

	// Check for required commands
	cmdNames := make(map[string]bool)
	for _, cmd := range cmds {
		cmdNames[cmd.Name] = true
	}

	required := []string{"connect", "disconnect", "status", "rotate"}
	for _, req := range required {
		if !cmdNames[req] {
			t.Errorf("missing command: %s", req)
		}
	}

	// Check tray flags
	for _, cmd := range cmds {
		if cmd.Name == "connect" || cmd.Name == "disconnect" {
			if !cmd.Tray {
				t.Errorf("command %s should have Tray=true", cmd.Name)
			}
		}
	}
}

func TestConfigSchema(t *testing.T) {
	m := New()
	schema := m.ConfigSchema()

	if len(schema) == 0 {
		t.Errorf("expected non-empty schema")
	}

	// Try to unmarshal to verify it's valid JSON
	var s interface{}
	if err := json.Unmarshal(schema, &s); err != nil {
		t.Errorf("invalid JSON schema: %v", err)
	}
}

func TestDispatchStatus(t *testing.T) {
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

	result, err := m.Dispatch(ctx, []string{"status"}, nil, nil)
	if err != nil {
		t.Errorf("Dispatch failed: %v", err)
	}

	if result.ExitCode != 0 {
		t.Errorf("expected exit code 0, got %d", result.ExitCode)
	}

	if result.Output == "" {
		t.Errorf("expected non-empty output")
	}
}

func TestDispatchStatusJSON(t *testing.T) {
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

	flags := map[string]string{"json": "true"}
	result, err := m.Dispatch(ctx, []string{"status"}, flags, nil)
	if err != nil {
		t.Errorf("Dispatch failed: %v", err)
	}

	if result.ExitCode != 0 {
		t.Errorf("expected exit code 0, got %d", result.ExitCode)
	}

	// Verify JSON output
	var data map[string]interface{}
	if err := json.Unmarshal(result.JSON, &data); err != nil {
		t.Errorf("invalid JSON output: %v", err)
	}

	if _, ok := data["state"]; !ok {
		t.Errorf("missing 'state' in JSON output")
	}
}

func TestDispatchUnknownCommand(t *testing.T) {
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

	result, err := m.Dispatch(ctx, []string{"unknown"}, nil, nil)
	if err != nil {
		t.Errorf("Dispatch failed: %v", err)
	}

	if result.ExitCode == 0 {
		t.Errorf("expected non-zero exit code for unknown command")
	}
}

func TestAuthWithMockServer(t *testing.T) {
	// Create a mock manager server
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
		http.NotFound(w, r)
	}))
	defer server.Close()

	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	// Set API key in secrets
	_ = host.Secrets().Set("api_key", []byte("test-api-key"))

	config := &ModuleConfig{
		ManagerURL: server.URL,
		NodeID:     "test-node",
	}
	configYAML, _ := yaml.Marshal(config)
	host.config = configYAML

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)

	// Attempt to get token
	if err := mod.authMgr.EnsureValidToken(ctx); err != nil {
		t.Fatalf("EnsureValidToken failed: %v", err)
	}

	token := mod.authMgr.GetToken()
	if token != "test-token" {
		t.Errorf("expected token 'test-token', got %q", token)
	}
}

func TestDispatchConnect(t *testing.T) {
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
		t.Errorf("Dispatch failed: %v", err)
	}

	// Connect may fail (no real manager), but should return a result
	if result == nil {
		t.Errorf("expected result, got nil")
	}
}

func TestDispatchDisconnect(t *testing.T) {
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
		t.Errorf("Dispatch failed: %v", err)
	}

	if result == nil {
		t.Errorf("expected result, got nil")
	}
}

func TestDispatchRotate(t *testing.T) {
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

	flags := map[string]string{"force": "false"}
	result, err := m.Dispatch(ctx, []string{"rotate"}, flags, nil)
	if err != nil {
		t.Errorf("Dispatch failed: %v", err)
	}

	if result == nil {
		t.Errorf("expected result, got nil")
	}
}

func TestDispatchRotateWithForce(t *testing.T) {
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

	flags := map[string]string{"force": "true"}
	result, err := m.Dispatch(ctx, []string{"rotate"}, flags, nil)
	if err != nil {
		t.Errorf("Dispatch failed: %v", err)
	}

	if result == nil {
		t.Errorf("expected result, got nil")
	}
}

func TestDispatchNoCommand(t *testing.T) {
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

	result, err := m.Dispatch(ctx, []string{}, nil, nil)
	if err != nil {
		t.Errorf("Dispatch failed: %v", err)
	}

	if result.ExitCode == 0 {
		t.Errorf("expected non-zero exit code for no command")
	}
}

func TestModuleStartStop(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	config := &ModuleConfig{
		ManagerURL: "http://localhost:8080",
		NodeID:     "test-node",
	}
	configYAML, _ := yaml.Marshal(config)
	host.config = configYAML

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Start module
	if err := m.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	// Stop module
	if err := m.Stop(ctx); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}

	// Stop again (should be idempotent)
	if err := m.Stop(ctx); err != nil {
		t.Fatalf("Second Stop failed: %v", err)
	}
}

func TestModuleHealthAfterInit(t *testing.T) {
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

	health := m.Health(ctx)
	// Health should have a level (initially may be Unhealthy or Healthy)
	if health.Level != sdk.Healthy && health.Level != sdk.Degraded && health.Level != sdk.Unhealthy {
		t.Errorf("invalid health level: %d", health.Level)
	}
}

func TestConfigSchemaValidation(t *testing.T) {
	m := New()
	schema := m.ConfigSchema()

	var s map[string]interface{}
	if err := json.Unmarshal(schema, &s); err != nil {
		t.Errorf("schema is not valid JSON: %v", err)
	}

	// Verify schema structure
	if _, hasSchema := s["$schema"]; !hasSchema {
		t.Errorf("schema missing $schema field")
	}

	props := s["properties"].(map[string]interface{})
	required := s["required"].([]interface{})

	// Verify required fields
	if len(required) != 2 {
		t.Errorf("expected 2 required fields, got %d", len(required))
	}

	// Verify properties
	if _, hasManagerURL := props["manager_url"]; !hasManagerURL {
		t.Errorf("schema missing manager_url property")
	}
	if _, hasNodeID := props["node_id"]; !hasNodeID {
		t.Errorf("schema missing node_id property")
	}
}

func TestInitMissingNodeID(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	config := &ModuleConfig{
		ManagerURL: "http://localhost:8080",
		// Missing NodeID
	}
	configYAML, _ := yaml.Marshal(config)
	host.config = configYAML

	m := New()
	ctx := context.Background()

	err := m.Init(ctx, host)
	if err == nil {
		t.Fatalf("Init should fail with missing node_id")
	}
}

func TestStatusDetail(t *testing.T) {
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

	if status.Detail == nil {
		t.Errorf("Status.Detail should not be nil")
	}

	if _, ok := status.Detail["tunnel"]; !ok {
		t.Errorf("Status.Detail missing 'tunnel' key")
	}
}

// Tests for missing coverage paths

func TestAuthRefreshLoopWithCancellation(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	config := &ModuleConfig{
		ManagerURL: "http://localhost:8080",
		NodeID:     "test-node",
	}
	configYAML, _ := yaml.Marshal(config)
	host.config = configYAML

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	mod.running = true

	// Start authRefreshLoop in goroutine and let it run briefly
	go mod.authRefreshLoop(ctx)

	// Give it time to at least start
	time.Sleep(50 * time.Millisecond)

	// Cancel context to stop the loop
	cancel()

	// Give it time to clean up
	time.Sleep(50 * time.Millisecond)
}

func TestRegisterMetricsAll(t *testing.T) {
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

	// Verify all 6 metrics are registered
	if mod.metrics == nil {
		t.Errorf("metrics should be initialized")
	}

	// Count: tunnelUp, handshakeAge, rxBytes, txBytes, tokenRefreshes, connErrors = 6 total
	metricCount := 0
	if mod.metrics.tunnelUp != nil {
		metricCount++
	}
	if mod.metrics.handshakeAge != nil {
		metricCount++
	}
	if mod.metrics.rxBytes != nil {
		metricCount++
	}
	if mod.metrics.txBytes != nil {
		metricCount++
	}
	if mod.metrics.tokenRefreshes != nil {
		metricCount++
	}
	if mod.metrics.connErrors != nil {
		metricCount++
	}

	if metricCount != 6 {
		t.Errorf("expected 6 metrics registered, got %d", metricCount)
	}
}

func TestCmdDisconnect(t *testing.T) {
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

	result, err := mod.cmdDisconnect(ctx)
	if err != nil {
		t.Fatalf("cmdDisconnect error: %v", err)
	}

	if result.ExitCode != 0 {
		t.Errorf("cmdDisconnect should return exit code 0")
	}
	if !strings.Contains(result.Output, "disconnected") {
		t.Errorf("output should indicate disconnection")
	}
}

func TestMonitorLoopWithCancellation(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	config := &ModuleConfig{
		ManagerURL: "http://localhost:8080",
		NodeID:     "test-node",
	}
	configYAML, _ := yaml.Marshal(config)
	host.config = configYAML

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)

	// Start monitorLoop in goroutine
	go mod.monitorLoop(ctx)

	// Give it time to run at least once
	time.Sleep(50 * time.Millisecond)

	// Cancel context
	cancel()

	// Give it time to clean up
	time.Sleep(50 * time.Millisecond)
}

func TestStopModule(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	config := &ModuleConfig{
		ManagerURL: "http://localhost:8080",
		NodeID:     "test-node",
	}
	configYAML, _ := yaml.Marshal(config)
	host.config = configYAML

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Start module
	if err := m.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	// Stop module
	if err := m.Stop(ctx); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}

	mod := m.(*Module)
	if mod.running {
		t.Errorf("module should not be running after Stop")
	}
}

func TestCmdConnect(t *testing.T) {
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
	mod.running = true

	result, err := mod.cmdConnect(ctx)
	if err != nil {
		t.Fatalf("cmdConnect error: %v", err)
	}

	// Will likely fail without server, but shouldn't crash
	if result == nil {
		t.Errorf("cmdConnect should return a result")
	}
}
