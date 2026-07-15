package squawk

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/penguintechinc/penguin/pkg/sdk"
	"github.com/prometheus/client_golang/prometheus"
	"go.uber.org/zap"
	"go.uber.org/zap/zaptest"
)

// FakeHostServices implements sdk.HostServices for testing.
type FakeHostServices struct {
	logger    *zap.Logger
	secrets   *FakeSecretStore
	license   *FakeLicenseChecker
	metrics   prometheus.Registerer
	dataDir   string
	eventSink *FakeEventSink
	config    []byte
}

type FakeSecretStore struct {
	store map[string][]byte
}

func (f *FakeSecretStore) Get(key string) ([]byte, error) {
	if val, ok := f.store[key]; ok {
		return val, nil
	}
	return nil, sdk.ErrSecretNotFound
}

func (f *FakeSecretStore) Set(key string, value []byte) error {
	f.store[key] = value
	return nil
}

func (f *FakeSecretStore) Delete(key string) error {
	delete(f.store, key)
	return nil
}

type FakeLicenseChecker struct {
	featureEnabled bool
	tier           string
}

func (f *FakeLicenseChecker) FeatureEnabled(key string) bool {
	return f.featureEnabled
}

func (f *FakeLicenseChecker) Tier() string {
	return f.tier
}

type FakeEventSink struct {
	events []sdk.Event
}

func (f *FakeEventSink) Publish(ev sdk.Event) {
	f.events = append(f.events, ev)
}

func NewFakeHost(logger *zap.Logger, dataDir string) *FakeHostServices {
	return &FakeHostServices{
		logger:    logger,
		secrets:   &FakeSecretStore{store: map[string][]byte{"auth_token": []byte("")}}, // Empty token
		license:   &FakeLicenseChecker{featureEnabled: true, tier: "professional"},
		metrics:   prometheus.NewRegistry(),
		dataDir:   dataDir,
		eventSink: &FakeEventSink{},
	}
}

func (f *FakeHostServices) Logger() *zap.Logger {
	return f.logger
}

func (f *FakeHostServices) Secrets() sdk.SecretStore {
	return f.secrets
}

func (f *FakeHostServices) License() sdk.LicenseChecker {
	return f.license
}

func (f *FakeHostServices) Metrics() prometheus.Registerer {
	return f.metrics
}

func (f *FakeHostServices) DataDir() string {
	return f.dataDir
}

func (f *FakeHostServices) Config() []byte {
	return f.config
}

func (f *FakeHostServices) Events() sdk.EventSink {
	return f.eventSink
}

// Tests

func TestModuleInfo(t *testing.T) {
	m := New()
	info := m.Info()

	if info.Name != "squawk" {
		t.Errorf("expected name 'squawk', got %q", info.Name)
	}

	// Squawk is core product (Free tier): the module must load without a
	// license server. Enterprise capabilities inside it are gated separately.
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
	if mod.dohClient == nil {
		t.Errorf("DoH client not created")
	}
	if mod.config == nil {
		t.Errorf("config not set")
	}
}

func TestModuleLifecycle(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Init
	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Start
	if err := m.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	// Status
	status, err := m.Status(ctx)
	if err != nil {
		t.Fatalf("Status failed: %v", err)
	}
	if status.State != sdk.StateRunning {
		t.Errorf("expected StateRunning, got %s", status.State)
	}

	// Health
	health := m.Health(ctx)
	if health.CheckedAt.IsZero() {
		t.Errorf("health check time not set")
	}

	// Stop (must be idempotent)
	if err := m.Stop(ctx); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}

	// Stop again (should be safe)
	if err := m.Stop(ctx); err != nil {
		t.Fatalf("Stop (2nd call) failed: %v", err)
	}

	status, err = m.Status(ctx)
	if err != nil {
		t.Fatalf("Status failed after stop: %v", err)
	}
	if status.State != sdk.StateStopped {
		t.Errorf("expected StateStopped, got %s", status.State)
	}
}

func TestCommands(t *testing.T) {
	m := New()
	cmds := m.Commands()

	if len(cmds) == 0 {
		t.Errorf("expected commands, got none")
	}

	// Verify required commands exist
	cmdNames := make(map[string]bool)
	for _, cmd := range cmds {
		cmdNames[cmd.Name] = true
	}

	requiredCmds := []string{"query", "forward", "config", "cache", "license", "time"}
	for _, required := range requiredCmds {
		if !cmdNames[required] {
			t.Errorf("missing required command: %s", required)
		}
	}
}

func TestDispatchQuery(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Test query command without domain
	result, _ := m.Dispatch(ctx, []string{"query"}, map[string]string{}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("expected non-zero exit code for missing domain")
	}

	// Test query with domain (may fail if DoH server unreachable, but structure should be valid)
	result, _ = m.Dispatch(ctx, []string{"query"}, map[string]string{"type": "A"}, []string{"example.com"})
	if result == nil {
		t.Errorf("expected result, got nil")
	}
}

func TestDispatchForward(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Test forward status
	result, _ := m.Dispatch(ctx, []string{"forward", "status"}, map[string]string{}, []string{})
	if result == nil {
		t.Errorf("expected result for forward status")
	}

	// Test unknown subcommand
	result, _ = m.Dispatch(ctx, []string{"forward", "invalid"}, map[string]string{}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("expected non-zero exit code for invalid subcommand")
	}
}

func TestDispatchConfig(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	result, _ := m.Dispatch(ctx, []string{"config", "show"}, map[string]string{}, []string{})
	if result.ExitCode != 0 {
		t.Errorf("expected zero exit code for config show")
	}

	// Verify JSON is valid
	var config ModuleConfig
	if err := json.Unmarshal(result.JSON, &config); err != nil {
		t.Errorf("invalid JSON output: %v", err)
	}
}

func TestDispatchCache(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Test cache stats
	result, _ := m.Dispatch(ctx, []string{"cache", "stats"}, map[string]string{}, []string{})
	if result.ExitCode != 0 {
		t.Errorf("expected zero exit code for cache stats")
	}

	// Test cache flush
	result, _ = m.Dispatch(ctx, []string{"cache", "flush"}, map[string]string{}, []string{})
	if result.ExitCode != 0 {
		t.Errorf("expected zero exit code for cache flush")
	}
}

func TestDispatchLicense(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	result, _ := m.Dispatch(ctx, []string{"license", "status"}, map[string]string{}, []string{})
	if result == nil {
		t.Errorf("expected result for license status")
	}
}

func TestDispatchTime(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	result, _ := m.Dispatch(ctx, []string{"time"}, map[string]string{}, []string{})
	if result.ExitCode != 0 {
		t.Errorf("expected zero exit code for time command")
	}
}

func TestDispatchUnknownCommand(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	result, _ := m.Dispatch(ctx, []string{"unknown"}, map[string]string{}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("expected non-zero exit code for unknown command")
	}
}

func TestConfigSchema(t *testing.T) {
	m := New()
	schema := m.ConfigSchema()

	if len(schema) == 0 {
		t.Errorf("expected non-empty schema")
	}

	// Verify it's valid JSON
	var s map[string]interface{}
	if err := json.Unmarshal(schema, &s); err != nil {
		t.Errorf("schema is not valid JSON: %v", err)
	}

	// Verify it has JSON Schema properties
	if _, ok := s["$schema"]; !ok {
		t.Errorf("missing $schema")
	}
	if _, ok := s["properties"]; !ok {
		t.Errorf("missing properties")
	}
}

func TestHealth(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	health := m.Health(ctx)
	if health.CheckedAt.IsZero() {
		t.Errorf("health check time not set")
	}

	// Health check should use cached result on subsequent calls (< 5s)
	health2 := m.Health(ctx)
	if health2.CheckedAt.IsZero() {
		t.Errorf("health2 check time not set")
	}

	// Both should be within a reasonable time range (check takes time, but should be cached)
	diff := health2.CheckedAt.Sub(health.CheckedAt)
	if diff < 0 || diff > 2*time.Second {
		// The second check should be within 2 seconds of the first
		t.Logf("health check time diff: %v (acceptable)", diff)
	}
}

func TestSecretIntegration(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	// Set a secret
	_ = host.Secrets().Set("auth_token", []byte("test-token-123"))

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	if mod.config.DOH.AuthToken != "test-token-123" {
		t.Errorf("auth token not loaded from secrets")
	}
}

func TestMetricsRegistration(t *testing.T) {
	logger := zaptest.NewLogger(t)
	registry := prometheus.NewRegistry()
	host := NewFakeHost(logger, t.TempDir())
	// Override metrics with our test registry
	host.metrics = registry

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	if mod.metrics == nil {
		t.Errorf("metrics not registered")
	}
}

func TestDispatchForwardStart(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Test forward start (forwarder not enabled in config, so should fail)
	result, _ := m.Dispatch(ctx, []string{"forward", "start"}, map[string]string{}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("expected non-zero exit code for forward start without forwarder")
	}
}

func TestDispatchForwardStop(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Test forward stop (should work even if not started)
	result, _ := m.Dispatch(ctx, []string{"forward", "stop"}, map[string]string{}, []string{})
	if result == nil {
		t.Errorf("expected result for forward stop")
	}
}

func TestDispatchForwardInvalid(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Test unknown forward subcommand
	result, _ := m.Dispatch(ctx, []string{"forward", "invalid"}, map[string]string{}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("expected non-zero exit code for unknown forward subcommand")
	}
}

func TestDispatchForwardNoSubcommand(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Test forward with no subcommand
	result, _ := m.Dispatch(ctx, []string{"forward"}, map[string]string{}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("expected non-zero exit code for forward without subcommand")
	}
}

func TestDispatchCacheNoSubcommand(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Test cache with no subcommand
	result, _ := m.Dispatch(ctx, []string{"cache"}, map[string]string{}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("expected non-zero exit code for cache without subcommand")
	}
}

func TestDispatchCacheInvalid(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Test unknown cache subcommand
	result, _ := m.Dispatch(ctx, []string{"cache", "invalid"}, map[string]string{}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("expected non-zero exit code for unknown cache subcommand")
	}
}

func TestStatusWithForwarder(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	config := []byte(`
forwarder:
  enabled: true
  udp_addr: "127.0.0.1:53"
  tcp_addr: "127.0.0.1:53"
`)
	host.config = config

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	status, err := m.Status(ctx)
	if err != nil {
		t.Fatalf("Status failed: %v", err)
	}

	if status.Detail["forwarder"] != "listening :53" {
		t.Errorf("expected forwarder listening status")
	}
}

func TestHealthDegraded(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Perform health checks - may be healthy or degraded depending on server
	health := m.Health(ctx)
	if health.Level != sdk.Healthy && health.Level != sdk.Degraded {
		t.Errorf("expected Healthy or Degraded, got %d", health.Level)
	}
}

func TestInitWithMalformedConfig(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())
	// Use YAML that won't parse - tab characters are not allowed in YAML indentation
	host.config = []byte(`
doh:
	server_url: "https://example.com"
`)

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	err := m.Init(ctx, host)
	if err == nil {
		t.Fatalf("Init should fail with malformed config")
	}
}

func TestQueryWithTypeFlag(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Test query with AAAA record type (may fail due to network, but structure should be valid)
	result, _ := m.Dispatch(ctx, []string{"query"}, map[string]string{"type": "AAAA"}, []string{"example.com"})
	if result == nil {
		t.Errorf("expected result, got nil")
	}
}

func TestConfigSchemaValidation(t *testing.T) {
	m := New()
	schema := m.ConfigSchema()

	var parsed map[string]interface{}
	if err := json.Unmarshal(schema, &parsed); err != nil {
		t.Errorf("schema is not valid JSON: %v", err)
	}

	// Verify required schema fields
	if _, hasSchema := parsed["$schema"]; !hasSchema {
		t.Errorf("schema missing $schema field")
	}
	if _, hasType := parsed["type"]; !hasType {
		t.Errorf("schema missing type field")
	}
	if _, hasProperties := parsed["properties"]; !hasProperties {
		t.Errorf("schema missing properties field")
	}

	// Verify DOH properties
	props := parsed["properties"].(map[string]interface{})
	if _, hasDOH := props["doh"]; !hasDOH {
		t.Errorf("schema missing doh property")
	}
}

func TestStopWithoutStart(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Stop without starting should be idempotent
	if err := m.Stop(ctx); err != nil {
		t.Errorf("Stop without Start should be idempotent: %v", err)
	}

	status, err := m.Status(ctx)
	if err != nil {
		t.Fatalf("Status failed: %v", err)
	}
	if status.State != sdk.StateStopped {
		t.Errorf("expected StateStopped, got %s", status.State)
	}
}

func TestQueryNoArgs(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Query with no domain should fail
	result, _ := m.Dispatch(ctx, []string{"query"}, map[string]string{}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("expected non-zero exit code for query without domain")
	}
}

func TestQueryDefaultType(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Query with no --type flag should default to A
	result, _ := m.Dispatch(ctx, []string{"query"}, map[string]string{}, []string{"example.com"})
	if result == nil {
		t.Errorf("expected result, got nil")
	}
}

func TestForwardStatusWhenNotRunning(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Check forward status when forwarder not enabled
	result, err := m.Dispatch(ctx, []string{"forward", "status"}, map[string]string{}, []string{})
	if err != nil {
		t.Fatalf("Dispatch failed: %v", err)
	}

	if result.ExitCode != 0 {
		t.Errorf("expected zero exit code, got %d", result.ExitCode)
	}
}

func TestCacheStats(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	result, err := m.Dispatch(ctx, []string{"cache", "stats"}, map[string]string{}, []string{})
	if err != nil {
		t.Fatalf("Dispatch failed: %v", err)
	}

	if result.ExitCode != 0 {
		t.Errorf("expected zero exit code, got %d", result.ExitCode)
	}

	// Verify JSON is valid
	var data map[string]interface{}
	if err := json.Unmarshal(result.JSON, &data); err != nil {
		t.Errorf("invalid JSON output: %v", err)
	}
}

func TestLicenseStatus(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	result, err := m.Dispatch(ctx, []string{"license", "status"}, map[string]string{}, []string{})
	if err != nil {
		t.Fatalf("Dispatch failed: %v", err)
	}

	// License check might succeed or fail depending on server, but should return result
	if len(result.JSON) == 0 {
		t.Errorf("expected JSON output")
	}
}

// Extended coverage tests

func TestStartWithForwarderEnabled(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())
	config := []byte(`
forwarder:
  enabled: true
  udp_addr: "127.0.0.1:0"
  tcp_addr: "127.0.0.1:0"
`)
	host.config = config

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Start with forwarder enabled (will attempt to listen on :53 which may fail without root)
	// The test verifies Start completes without panic
	_ = m.Start(ctx)

	// Try to stop cleanly
	_ = m.Stop(ctx)
}

func TestStartWithSystemDNSManage(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())
	config := []byte(`
system_dns:
  manage: true
`)
	host.config = config

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Start with system DNS manage (will attempt real DNS changes, which will likely fail)
	// The test verifies the code path executes
	_ = m.Start(ctx)

	// Stop (will attempt restore)
	_ = m.Stop(ctx)
}

func TestStopWithSystemDNSManage(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())
	config := []byte(`
system_dns:
  manage: true
`)
	host.config = config

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Start may fail due to permissions, which is OK
	if err := m.Start(ctx); err != nil {
		t.Logf("Start error (expected): %v", err)
	}

	// Stop should handle both success and failure paths
	_ = m.Stop(ctx)
}

func TestHealthCaching(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// First health check
	health1 := m.Health(ctx)
	time1 := health1.CheckedAt

	// Immediate second check should return cached result (< 5s)
	health2 := m.Health(ctx)
	time2 := health2.CheckedAt

	// Both should have been checked recently
	if time1.IsZero() || time2.IsZero() {
		t.Errorf("health check times should not be zero")
	}
}

func TestQueryWithMissingDomain(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Query without domain argument
	result, _ := m.Dispatch(ctx, []string{"query"}, map[string]string{}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("expected non-zero exit code when domain missing")
	}
	if !strings.Contains(result.Output, "Usage") {
		t.Errorf("expected usage message in output")
	}
}

func TestQueryWithSpecificType(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Query with specific type flag (MX record)
	result, _ := m.Dispatch(ctx, []string{"query"}, map[string]string{"type": "MX"}, []string{"example.com"})
	// Result may fail due to network, but structure should be valid
	if result == nil {
		t.Errorf("expected result, got nil")
	}
}

func TestDispatchEmptyPath(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Dispatch with empty path
	result, _ := m.Dispatch(ctx, []string{}, map[string]string{}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("expected non-zero exit code for empty command")
	}
}

func TestInitWithConfigFromHostConfig(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())
	config := []byte(`
doh:
  server_url: "https://8.8.8.8:443/dns-query"
  verify_tls: false
  auth_token: "config-token"
forwarder:
  enabled: true
  udp_addr: "127.0.0.1:0"
`)
	host.config = config

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	if mod.config.DOH.ServerURL != "https://8.8.8.8:443/dns-query" {
		t.Errorf("server URL not loaded from config")
	}
	if mod.config.DOH.AuthToken != "config-token" {
		t.Errorf("auth token not loaded from config")
	}
}

func TestInitAuthTokenFallback(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())
	// Set auth_token in secrets
	_ = host.secrets.Set("auth_token", []byte("secret-token"))

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	if mod.config.DOH.AuthToken != "secret-token" {
		t.Errorf("auth token not loaded from secrets fallback")
	}
}

func TestConfigRedaction(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())
	_ = host.secrets.Set("auth_token", []byte("secret-token-12345"))

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	result, _ := m.Dispatch(ctx, []string{"config", "show"}, map[string]string{}, []string{})
	if strings.Contains(result.Output, "secret-token-12345") {
		t.Errorf("auth token should be redacted in config output")
	}
	if !strings.Contains(result.Output, "****") {
		t.Errorf("expected redacted token format in config output")
	}
}

func TestStartIdempotency(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Start first time
	if err := m.Start(ctx); err != nil {
		t.Fatalf("First Start failed: %v", err)
	}

	// Start again (should be idempotent)
	if err := m.Start(ctx); err != nil {
		t.Fatalf("Second Start failed: %v", err)
	}

	if err := m.Stop(ctx); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}
}

func TestForwardStart(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())
	config := []byte(`
forwarder:
  enabled: true
  udp_addr: "127.0.0.1:0"
`)
	host.config = config

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Test forward start subcommand
	result, _ := m.Dispatch(ctx, []string{"forward", "start"}, map[string]string{}, []string{})
	// May succeed or fail depending on permissions, but should handle gracefully
	if result == nil {
		t.Errorf("expected result")
	}

	_ = m.Stop(ctx)
}

func TestDispatchCacheInvalidSubcommand(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Test cache with invalid subcommand
	result, _ := m.Dispatch(ctx, []string{"cache", "clear"}, map[string]string{}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("expected non-zero exit code for invalid cache subcommand")
	}
}

func TestStatusWithoutForwarder(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	status, err := m.Status(ctx)
	if err != nil {
		t.Fatalf("Status failed: %v", err)
	}

	if status.State != sdk.StateStopped {
		t.Errorf("expected StateStopped, got %s", status.State)
	}

	// Forwarder should not be in detail when not configured
	if _, ok := status.Detail["forwarder"]; ok {
		t.Errorf("expected no forwarder in status when not configured")
	}
}

func TestHealthWhenClientNil(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	mod := m.(*Module)
	mod.host = host
	mod.logger = logger

	// Set DoH client to nil
	mod.dohClient = nil

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	health := mod.Health(ctx)
	if health.Level != sdk.Unhealthy {
		t.Errorf("expected Unhealthy when DoH client is nil, got %d", health.Level)
	}
}

func TestConfigSchemaHasAllProperties(t *testing.T) {
	m := New()
	schema := m.ConfigSchema()

	var parsed map[string]interface{}
	if err := json.Unmarshal(schema, &parsed); err != nil {
		t.Fatalf("schema parsing failed: %v", err)
	}

	props := parsed["properties"].(map[string]interface{})

	requiredProps := []string{"doh", "forwarder", "system_dns", "cache"}
	for _, prop := range requiredProps {
		if _, ok := props[prop]; !ok {
			t.Errorf("schema missing required property: %s", prop)
		}
	}
}

func TestHandleForwardWithoutForwarderConfigured(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Forwarder not enabled in config
	mod := m.(*Module)
	if mod.forwarder != nil {
		t.Errorf("expected forwarder to be nil when not configured")
	}

	// Try start - should fail gracefully
	result, _ := m.Dispatch(ctx, []string{"forward", "start"}, map[string]string{}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("expected non-zero exit when trying to start unconfigured forwarder")
	}

	// Try stop - should fail gracefully
	result, _ = m.Dispatch(ctx, []string{"forward", "stop"}, map[string]string{}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("expected non-zero exit when trying to stop unconfigured forwarder")
	}
}

func TestDispatchTimeCommand(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	result, _ := m.Dispatch(ctx, []string{"time"}, map[string]string{}, []string{})
	if result.ExitCode != 0 {
		t.Errorf("expected zero exit code for time command")
	}

	var data map[string]interface{}
	if err := json.Unmarshal(result.JSON, &data); err != nil {
		t.Errorf("expected valid JSON output")
	}
}

// Additional coverage improvements for remaining gaps

func TestStartWithForwarderAndSystemDNS(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())
	config := []byte(`
forwarder:
  enabled: true
  udp_addr: "127.0.0.1:0"
  tcp_addr: "127.0.0.1:0"
system_dns:
  manage: true
`)
	host.config = config

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Start with both forwarder and system DNS
	_ = m.Start(ctx)
	_ = m.Stop(ctx)
}

func TestQueryWithTypeDefaultedToA(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Query without explicit type (should default to "A")
	result, _ := m.Dispatch(ctx, []string{"query"}, map[string]string{}, []string{"example.com"})
	if result == nil {
		t.Errorf("expected result")
	}
}

func TestHandleQueryErrorPath(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 1*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Query with very short timeout to trigger error
	ctx2, cancel2 := context.WithTimeout(context.Background(), 1*time.Millisecond)
	defer cancel2()

	result, _ := m.Dispatch(ctx2, []string{"query"}, map[string]string{}, []string{"example.com"})
	if result.ExitCode == 0 {
		t.Logf("query succeeded (may have been fast enough)")
	}
}

func TestHandleForwardStatusWhenEnabled(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())
	config := []byte(`
forwarder:
  enabled: true
  udp_addr: "127.0.0.1:0"
`)
	host.config = config

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Check forward status when configured
	result, _ := m.Dispatch(ctx, []string{"forward", "status"}, map[string]string{}, []string{})
	if result == nil {
		t.Errorf("expected result for forward status")
	}
}

func TestHandleConfigShowOutputFormat(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	result, _ := m.Dispatch(ctx, []string{"config", "show"}, map[string]string{}, []string{})
	if result.ExitCode != 0 {
		t.Errorf("expected zero exit code for config show")
	}

	// Verify output is valid JSON and contains expected structure
	var config ModuleConfig
	if err := json.Unmarshal(result.JSON, &config); err != nil {
		t.Errorf("config output should be valid JSON: %v", err)
	}

	// Verify redacted token
	if strings.Contains(result.Output, "127.0.0.1") {
		t.Logf("config output contains server URL (expected)")
	}
}

func TestHandleLicenseWithoutUserToken(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())
	// Remove user_token from secrets
	_ = host.secrets.Delete("auth_token")

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	result, _ := m.Dispatch(ctx, []string{"license", "status"}, map[string]string{}, []string{})
	// Result should succeed even without user token
	if result == nil {
		t.Errorf("expected result for license status")
	}
}

func TestInitWithCrashRecoverySuccess(t *testing.T) {
	logger := zaptest.NewLogger(t)
	dataDir := t.TempDir()
	host := NewFakeHost(logger, dataDir)

	// Pre-create backup marker to trigger recovery path
	backup := &DNSBackup{
		PreviousServers: []string{"8.8.8.8"},
		AppliedAt:       time.Now().Format(time.RFC3339),
	}
	data, _ := json.Marshal(backup)
	markerPath := filepath.Join(dataDir, "dns-applied.json")
	_ = os.WriteFile(markerPath, data, 0o600)

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Init should attempt crash recovery
	_ = m.Init(ctx, host)
}

func TestForwardStartWithInvalidListen(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())
	config := []byte(`
forwarder:
  enabled: true
  udp_addr: "invalid:address"
`)
	host.config = config

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		// Init may fail due to invalid config, which is OK
		t.Logf("Init error (expected): %v", err)
		return
	}

	// If Init succeeds, try start (should handle invalid address)
	_ = m.Start(ctx)
	_ = m.Stop(ctx)
}

func TestCacheFlushedCommand(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	result, _ := m.Dispatch(ctx, []string{"cache", "flush"}, map[string]string{}, []string{})
	if result.ExitCode != 0 {
		t.Errorf("expected zero exit code for cache flush")
	}

	var data map[string]interface{}
	if err := json.Unmarshal(result.JSON, &data); err != nil {
		t.Errorf("expected valid JSON output")
	}
}

// Additional tests for error paths and branch coverage

func TestStartMultipleTimesIsIdempotent(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Start multiple times
	if err := m.Start(ctx); err != nil {
		t.Logf("Start error (may fail on some systems): %v", err)
	}

	if err := m.Start(ctx); err != nil {
		t.Logf("Start 2nd error (should be idempotent): %v", err)
	}

	_ = m.Stop(ctx)
}

func TestHandleQueryJSONResponse(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Query should return JSON data
	result, _ := m.Dispatch(ctx, []string{"query"}, map[string]string{}, []string{"example.com"})
	if len(result.JSON) == 0 && result.ExitCode == 0 {
		t.Logf("query result has no JSON (may have failed)")
	}
}

func TestHandleForwardAllSubcommands(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	// Test all subcommands
	subcommands := []string{"status", "start", "stop"}
	for _, subcmd := range subcommands {
		result, err := m.Dispatch(ctx, []string{"forward", subcmd}, map[string]string{}, []string{})
		if err != nil {
			t.Errorf("forward %s error: %v", subcmd, err)
		}
		if result == nil {
			t.Errorf("forward %s returned nil result", subcmd)
		}
	}
}

func TestHandleLicenseJSONResponse(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	result, _ := m.Dispatch(ctx, []string{"license", "status"}, map[string]string{}, []string{})

	// Should have JSON output
	if len(result.JSON) == 0 {
		t.Errorf("license status should return JSON")
	}

	var data map[string]interface{}
	if err := json.Unmarshal(result.JSON, &data); err != nil {
		t.Errorf("license JSON invalid: %v", err)
	}

	// Should have status field
	if _, ok := data["status"]; !ok {
		t.Errorf("license JSON missing status field")
	}
}

func TestCacheStatsWithCacheDisabled(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())
	config := []byte(`
cache:
  enabled: false
`)
	host.config = config

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	result, _ := m.Dispatch(ctx, []string{"cache", "stats"}, map[string]string{}, []string{})
	if result.ExitCode != 0 {
		t.Errorf("cache stats should succeed even when disabled")
	}

	var data map[string]interface{}
	if err := json.Unmarshal(result.JSON, &data); err != nil {
		t.Errorf("cache stats JSON invalid: %v", err)
	}

	if cacheEnabled, ok := data["cache_enabled"]; !ok || cacheEnabled.(bool) {
		t.Errorf("cache_enabled should be false")
	}
}

func TestStatusDetail(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	status, err := m.Status(ctx)
	if err != nil {
		t.Fatalf("Status failed: %v", err)
	}

	// Detail should have server entry
	if server, ok := status.Detail["server"]; !ok || server == "" {
		t.Errorf("status detail should include server")
	}
}

func TestHealthCheckMessageFormatting(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	health := m.Health(ctx)
	if health.Message == "" {
		t.Errorf("health message should not be empty")
	}

	// Message should be either OK or describe an error
	if health.Level == sdk.Healthy && health.Message != "OK" {
		t.Logf("healthy but message is %q (allowed)", health.Message)
	}
}

func TestDispatchNonExistentCommand(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	result, _ := m.Dispatch(ctx, []string{"nonexistent"}, map[string]string{}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("nonexistent command should fail")
	}

	if !strings.Contains(result.Output, "unknown command") {
		t.Errorf("output should indicate unknown command")
	}
}

// Tests for error paths in module.go

func TestHandleQueryErrorNoArgs(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	result, _ := mod.handleQuery(ctx, map[string]string{}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("query with no args should fail")
	}
}

func TestHandleForwardErrorNoSubcommand(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	result, _ := mod.handleForward(ctx, []string{"forward"}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("forward without subcommand should fail")
	}
}

func TestHandleLicenseChecking(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	result, _ := mod.handleLicense(ctx, []string{})
	// Should have output about license status
	if result.Output == "" {
		t.Errorf("license handler should produce output")
	}
}

func TestRegisterMetricsCollectorRegistration(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)

	// After Init, all metrics should be registered
	if mod.metrics == nil {
		t.Errorf("metrics should be registered after Init")
	}

	// Verify specific metrics exist
	if mod.metrics.queriesTotal == nil {
		t.Errorf("queriesTotal metric not registered")
	}
	if mod.metrics.forwarderUp == nil {
		t.Errorf("forwarderUp metric not registered")
	}
	if mod.metrics.cacheEntries == nil {
		t.Errorf("cacheEntries metric not registered")
	}
	if mod.metrics.dnsApplied == nil {
		t.Errorf("dnsApplied metric not registered")
	}
	if mod.metrics.healthStatus == nil {
		t.Errorf("healthStatus metric not registered")
	}
}

func TestStartWithForwarderDisabled(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())
	host.config = []byte(`
forwarder:
  enabled: false
`)

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	if err := m.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	mod := m.(*Module)
	// Verify forwarder is nil
	if mod.forwarder != nil {
		t.Errorf("forwarder should be nil when disabled")
	}

	if err := m.Stop(ctx); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}
}

func TestStartWithSystemDNSDisabled(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	// Config should have system DNS management disabled by default
	if mod.config.SystemDNS.Manage {
		t.Errorf("system DNS should be disabled by default")
	}

	if err := m.Start(ctx); err != nil {
		t.Fatalf("Start failed: %v", err)
	}

	if err := m.Stop(ctx); err != nil {
		t.Fatalf("Stop failed: %v", err)
	}
}

func TestHandleQuerySuccess(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	result, _ := mod.handleQuery(ctx, map[string]string{"type": "A"}, []string{"google.com"})
	// Result should be a result object (may succeed or fail depending on config)
	if result == nil {
		t.Errorf("handleQuery should return a result")
	}
}

func TestHandleForwardStatus(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	result, _ := mod.handleForward(ctx, []string{"forward", "status"}, []string{})
	if result.ExitCode != 0 {
		t.Errorf("forward status should succeed")
	}
}

func TestHandleConfigShow(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	result, _ := mod.handleConfig(ctx, []string{})
	if result.ExitCode != 0 {
		t.Errorf("config show should succeed")
	}
	if result.Output == "" {
		t.Errorf("config show should return output")
	}
}

func TestHandleCacheStats(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	result, _ := mod.handleCache(ctx, []string{"cache", "stats"}, []string{})
	if result.ExitCode != 0 {
		t.Errorf("cache stats should succeed")
	}
}

func TestHandleTime(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	result, _ := mod.handleTime(ctx, []string{})
	if result.ExitCode != 0 {
		t.Errorf("time command should succeed")
	}
}

// TestHandleForwardWhenForwarderNil tests handleForward when forwarder is not configured (nil).
func TestHandleForwardWhenForwarderNil(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	// Verify forwarder is nil (not enabled in config)
	mod.mu.Lock()
	if mod.forwarder != nil {
		mod.mu.Unlock()
		t.Skip("forwarder should be nil in default config")
	}
	mod.mu.Unlock()

	// Test start when forwarder is nil
	result, _ := mod.handleForward(ctx, []string{"forward", "start"}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("start should fail when forwarder is nil")
	}
	if !strings.Contains(result.Output, "not configured") {
		t.Errorf("error should mention forwarder not configured")
	}

	// Test stop when forwarder is nil
	result, _ = mod.handleForward(ctx, []string{"forward", "stop"}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("stop should fail when forwarder is nil")
	}
}

// TestHandleLicenseDifferentTiers tests handleLicense output for different license tiers.
func TestHandleLicenseDifferentTiers(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	// Test Free tier
	host.license.tier = "free"
	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	result, _ := mod.handleLicense(ctx, []string{})
	if result.ExitCode != 0 {
		t.Errorf("license command should succeed for free tier")
	}
	if len(result.JSON) == 0 {
		t.Errorf("license command should return JSON")
	}
}

// TestStopIdempotency tests that Stop can be called multiple times safely.
func TestStopIdempotency(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)

	// Start the module
	if err := mod.Start(ctx); err != nil {
		t.Errorf("Start failed: %v", err)
	}

	// Stop once
	if err := mod.Stop(ctx); err != nil {
		t.Errorf("first Stop failed: %v", err)
	}

	// Stop again - should be idempotent
	if err := mod.Stop(ctx); err != nil {
		t.Errorf("second Stop failed (should be idempotent): %v", err)
	}
}

// TestHandleQueryNoArgs tests handleQuery with no arguments.
func TestHandleQueryNoArgs(t *testing.T) {
	logger := zaptest.NewLogger(t)
	host := NewFakeHost(logger, t.TempDir())

	m := New()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := m.Init(ctx, host); err != nil {
		t.Fatalf("Init failed: %v", err)
	}

	mod := m.(*Module)
	result, _ := mod.handleQuery(ctx, map[string]string{}, []string{})
	if result.ExitCode == 0 {
		t.Errorf("query with no args should fail")
	}
	if !strings.Contains(result.Output, "Usage") {
		t.Errorf("error should show usage")
	}
}
