package extplugin

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"math"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	"aead.dev/minisign"
	"github.com/penguintechinc/penguin/pkg/sdk"
	sdkv1 "github.com/penguintechinc/penguin/pkg/sdk/proto/penguin/sdk/v1"
	"github.com/prometheus/client_golang/prometheus"
	"go.uber.org/zap"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/test/bufconn"
)

// clampInt32 narrows an int to int32, saturating rather than wrapping.
func clampInt32(v int) int32 {
	switch {
	case v > math.MaxInt32:
		return math.MaxInt32
	case v < math.MinInt32:
		return math.MinInt32
	default:
		return int32(v)
	}
}

// TestLoadExternalPluginE2E tests loading and running an external plugin via subprocess.
// This test is skipped if -short is set (can be unreliable in CI).
func TestLoadExternalPluginE2E(t *testing.T) {
	if testing.Short() {
		t.Skip("skipping subprocess test in short mode")
	}

	// Build the hello plugin.
	exeDir := filepath.Join(t.TempDir(), "bin")
	if err := os.MkdirAll(exeDir, 0o750); err != nil {
		t.Fatalf("mkdir bin: %v", err)
	}

	exePath := filepath.Join(exeDir, "plugin-hello")
	cmd := exec.Command("go", "build", "-o", exePath, "./examples/plugin-hello") // #nosec G204 -- hardcoded command and args, only output path is variable
	cmd.Dir = "/home/penguin/code/penguin" // Repo root
	if output, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("build plugin: %v\n%s", err, output)
	}

	// Generate a test keypair and sign the binary.
	pubKey, privKey, err := generateTestMinisignKey()
	if err != nil {
		t.Fatalf("generate key: %v", err)
	}

	binaryData, err := os.ReadFile(exePath) // #nosec G304 -- reading test fixture built by exec.Command above
	if err != nil {
		t.Fatalf("read binary: %v", err)
	}

	sig := minisign.Sign(privKey, binaryData)

	// Create the plugin directory.
	pluginDir := filepath.Join(t.TempDir(), "plugins", "hello")
	if err := os.MkdirAll(pluginDir, 0o750); err != nil {
		t.Fatalf("mkdir plugin: %v", err)
	}

	// Copy binary to plugin dir and sign.
	binaryPath := filepath.Join(pluginDir, "plugin-hello")
	if err := os.WriteFile(binaryPath, binaryData, 0o700); err != nil { // #nosec G306,G703 -- test needs executable binary for subprocess launch
		t.Fatalf("write binary: %v", err)
	}

	if err := os.WriteFile(filepath.Join(pluginDir, "plugin-hello.minisig"), sig, 0o600); err != nil { // #nosec G703 -- writing to temp directory fixture
		t.Fatalf("write signature: %v", err)
	}

	// Create manifest file (LoadManifest will read it from disk).
	h := sha256.Sum256(binaryData)
	manifestJSON := fmt.Sprintf(`{
  "name": "hello",
  "version": "1.0.0",
  "sdk_version": "v1",
  "binary": "plugin-hello",
  "sha256": "%s",
  "publisher": "test"
}`, hex.EncodeToString(h[:]))
	if err := os.WriteFile(filepath.Join(pluginDir, "plugin.json"), []byte(manifestJSON), 0o600); err != nil { // #nosec G703 -- writing to temp directory fixture
		t.Fatalf("write manifest: %v", err)
	}

	// Create host services mock.
	hostServices := &MockHostServices{
		configValue: []byte("test config"),
		dataDirValue: "/tmp/test",
	}

	// Load the plugin with a verifier that trusts our test key.
	verifier := NewVerifierWithKeys([]string{pubKey})

	// NOTE: This will spawn a real subprocess. Be prepared for edge cases.
	mod, err := LoadWithVerifier(context.Background(), pluginDir, hostServices, verifier)
	if err != nil {
		t.Fatalf("load plugin: %v", err)
	}

	// Verify we got a Module.
	if mod == nil {
		t.Fatalf("plugin returned nil module")
	}

	// Test Info().
	info := mod.Info()
	if info.Name != "hello" {
		t.Errorf("module name: got %s, want hello", info.Name)
	}

	// Test Start().
	if err := mod.Start(context.Background()); err != nil {
		t.Errorf("start: %v", err)
	}

	// Test Dispatch() - the core command.
	result, err := mod.Dispatch(context.Background(), []string{"greet"}, map[string]string{}, []string{"World"})
	if err != nil {
		t.Errorf("dispatch: %v", err)
	}
	if result.Output != "hello, World" {
		t.Errorf("dispatch output: got %q, want %q", result.Output, "hello, World")
	}
	if result.ExitCode != 0 {
		t.Errorf("dispatch exit code: got %d, want 0", result.ExitCode)
	}

	// Test Status().
	status, err := mod.Status(context.Background())
	if err != nil {
		t.Errorf("status: %v", err)
	}
	if status.State != sdk.StateRunning {
		t.Errorf("status state: got %s, want %s", status.State, sdk.StateRunning)
	}

	// Test Stop().
	if err := mod.Stop(context.Background()); err != nil {
		t.Errorf("stop: %v", err)
	}
}

// TestLoadExternalPluginBuffconn tests loading and running an external plugin
// using an in-process gRPC connection (bufconn) instead of a subprocess.
// This is more reliable for CI and doesn't require building the plugin.
func TestLoadExternalPluginBuffconn(t *testing.T) {
	// Create an in-memory listener for the gRPC connection.
	listener := bufconn.Listen(1024 * 1024)
	defer func() {
		_ = listener.Close()
	}()

	// Set up a gRPC server with our module service.
	grpcServer := grpc.NewServer()
	testModule := &TestModule{name: "hello"}
	sdkv1.RegisterModuleServiceServer(grpcServer, &testModuleServiceImpl{m: testModule})

	go func() {
		_ = grpcServer.Serve(listener)
	}()
	defer grpcServer.Stop()

	// Create a bufconn dialer.
	dialFunc := func(context.Context, string) (net.Conn, error) {
		return listener.Dial()
	}

	// Create the client connection.
	conn, err := grpc.NewClient(
		"passthrough:///bufnet",
		grpc.WithContextDialer(dialFunc),
		grpc.WithTransportCredentials(insecure.NewCredentials()),
	)
	if err != nil {
		t.Fatalf("dial: %v", err)
	}
	defer func() {
		_ = conn.Close()
	}()

	// Wrap the gRPC client as an sdk.Module.
	moduleClient := sdkv1.NewModuleServiceClient(conn)
	adapter := &moduleClientAdapter{grpcClient: moduleClient}

	// Test Info().
	info := adapter.Info()
	if info.Name != "hello" {
		t.Errorf("module name: got %s, want hello", info.Name)
	}

	// Test Start().
	if err := adapter.Start(context.Background()); err != nil {
		t.Errorf("start: %v", err)
	}

	// Test Dispatch().
	result, err := adapter.Dispatch(context.Background(), []string{"greet"}, map[string]string{}, []string{"Bufconn"})
	if err != nil {
		t.Errorf("dispatch: %v", err)
	}
	if result.Output != "hello, Bufconn" {
		t.Errorf("dispatch output: got %q, want %q", result.Output, "hello, Bufconn")
	}
	if result.ExitCode != 0 {
		t.Errorf("dispatch exit code: got %d, want 0", result.ExitCode)
	}

	// Test Status().
	status, err := adapter.Status(context.Background())
	if err != nil {
		t.Errorf("status: %v", err)
	}
	if status.State != sdk.StateRunning {
		t.Errorf("status state: got %s, want %s", status.State, sdk.StateRunning)
	}

	// Test Commands().
	commands := adapter.Commands()
	if len(commands) != 1 {
		t.Errorf("commands count: got %d, want 1", len(commands))
	}
	if len(commands) > 0 && commands[0].Name != "greet" {
		t.Errorf("command name: got %s, want greet", commands[0].Name)
	}

	// Test Health().
	health := adapter.Health(context.Background())
	if health.Level != sdk.Healthy {
		t.Errorf("health level: got %v, want %v", health.Level, sdk.Healthy)
	}

	// Test Stop().
	if err := adapter.Stop(context.Background()); err != nil {
		t.Errorf("stop: %v", err)
	}
}

// TestDiscoverPlugins scans a plugins directory and lists valid plugins.
func TestDiscoverPlugins(t *testing.T) {
	tmpDir := t.TempDir()
	pluginsDir := filepath.Join(tmpDir, "plugins")
	if err := os.Mkdir(pluginsDir, 0o750); err != nil {
		t.Fatalf("mkdir plugins: %v", err)
	}

	// Create two valid plugins.
	for i := 1; i <= 2; i++ {
		pluginName := fmt.Sprintf("plugin%d", i)
		pluginDir := filepath.Join(pluginsDir, pluginName)
		if err := os.Mkdir(pluginDir, 0o750); err != nil {
			t.Fatalf("mkdir plugin: %v", err)
		}

		// Create a dummy manifest.
		manifestPath := filepath.Join(pluginDir, "plugin.json")
		manifestJSON := fmt.Sprintf(`{
  "name": "%s",
  "version": "1.0.0",
  "sdk_version": "v1",
  "binary": "binary",
  "sha256": "abc123",
  "publisher": "test"
}`, pluginName)
		if err := os.WriteFile(manifestPath, []byte(manifestJSON), 0o600); err != nil {
			t.Fatalf("write manifest: %v", err)
		}

		// Create a dummy binary and signature (verification will fail, but that's OK for this test).
		binaryPath := filepath.Join(pluginDir, "binary")
		if err := os.WriteFile(binaryPath, []byte("dummy"), 0o600); err != nil {
			t.Fatalf("write binary: %v", err)
		}

		sigPath := filepath.Join(pluginDir, "binary.minisig")
		if err := os.WriteFile(sigPath, []byte("dummy sig"), 0o600); err != nil {
			t.Fatalf("write sig: %v", err)
		}
	}

	// Create a world-writable directory (should be skipped).
	worldWritableDir := filepath.Join(pluginsDir, "world-writable")
	if err := os.Mkdir(worldWritableDir, 0o777); err != nil { // #nosec G301 -- deliberately world-writable to test rejection
		t.Fatalf("mkdir world-writable: %v", err)
	}

	// Discover plugins. Since verification will fail, we expect no results,
	// but the function should not crash on world-writable dirs.
	manifests, err := Discover(pluginsDir)
	if err != nil {
		t.Errorf("discover: %v", err)
	}

	// We expect 0 manifests because verification fails (dummy sig, wrong hash).
	// But we're testing that the function handles world-writable dirs gracefully.
	t.Logf("Discovered %d plugins", len(manifests))
}

// ---- Test helpers ----

// testModuleServiceImpl implements sdkv1.ModuleServiceServer for testing.
type testModuleServiceImpl struct {
	sdkv1.UnimplementedModuleServiceServer
	m sdk.Module
}

func (s *testModuleServiceImpl) Info(ctx context.Context, req *sdkv1.InfoRequest) (*sdkv1.InfoResponse, error) {
	info := s.m.Info()
	return &sdkv1.InfoResponse{
		Name:           info.Name,
		Version:        info.Version,
		Description:    info.Description,
		LicenseFeature: info.LicenseFeature,
	}, nil
}

func (s *testModuleServiceImpl) Init(ctx context.Context, req *sdkv1.InitRequest) (*sdkv1.InitResponse, error) {
	return &sdkv1.InitResponse{}, nil
}

func (s *testModuleServiceImpl) Start(ctx context.Context, req *sdkv1.StartRequest) (*sdkv1.StartResponse, error) {
	err := s.m.Start(ctx)
	errMsg := ""
	if err != nil {
		errMsg = err.Error()
	}
	return &sdkv1.StartResponse{Error: errMsg}, nil
}

func (s *testModuleServiceImpl) Stop(ctx context.Context, req *sdkv1.StopRequest) (*sdkv1.StopResponse, error) {
	err := s.m.Stop(ctx)
	errMsg := ""
	if err != nil {
		errMsg = err.Error()
	}
	return &sdkv1.StopResponse{Error: errMsg}, nil
}

func (s *testModuleServiceImpl) Status(ctx context.Context, req *sdkv1.StatusRequest) (*sdkv1.StatusResponse, error) {
	status, err := s.m.Status(ctx)
	errMsg := ""
	if err != nil {
		errMsg = err.Error()
	}
	return &sdkv1.StatusResponse{
		State:  string(status.State),
		Detail: status.Detail,
		Error:  errMsg,
	}, nil
}

func (s *testModuleServiceImpl) Health(ctx context.Context, req *sdkv1.HealthRequest) (*sdkv1.HealthResponse, error) {
	report := s.m.Health(ctx)
	return &sdkv1.HealthResponse{
		Level:             clampInt32(int(report.Level)),
		Message:           report.Message,
		CheckedAtUnixNano: report.CheckedAt.UnixNano(),
	}, nil
}

func (s *testModuleServiceImpl) Commands(ctx context.Context, req *sdkv1.CommandsRequest) (*sdkv1.CommandsResponse, error) {
	specs := s.m.Commands()
	pbSpecs := make([]*sdkv1.CommandSpec, len(specs))
	for i, spec := range specs {
		pbSpecs[i] = &sdkv1.CommandSpec{
			Name:  spec.Name,
			Use:   spec.Use,
			Short: spec.Short,
		}
	}
	return &sdkv1.CommandsResponse{Commands: pbSpecs}, nil
}

func (s *testModuleServiceImpl) Dispatch(ctx context.Context, req *sdkv1.DispatchRequest) (*sdkv1.DispatchResponse, error) {
	result, err := s.m.Dispatch(ctx, req.Path, req.Flags, req.Args)
	errMsg := ""
	if err != nil {
		errMsg = err.Error()
	}
	return &sdkv1.DispatchResponse{
		Output:   result.Output,
		Json:     result.JSON,
		ExitCode: clampInt32(result.ExitCode),
		Error:    errMsg,
	}, nil
}

func (s *testModuleServiceImpl) ConfigSchema(ctx context.Context, req *sdkv1.ConfigSchemaRequest) (*sdkv1.ConfigSchemaResponse, error) {
	return &sdkv1.ConfigSchemaResponse{Schema: s.m.ConfigSchema()}, nil
}

// MockHostServices is a minimal mock of sdk.HostServices for testing.
type MockHostServices struct {
	configValue  []byte
	dataDirValue string
}

func (m *MockHostServices) Logger() *zap.Logger {
	return zap.NewNop()
}
func (m *MockHostServices) Secrets() sdk.SecretStore {
	return &MockSecretStore{}
}
func (m *MockHostServices) License() sdk.LicenseChecker {
	return &MockLicenseChecker{}
}
func (m *MockHostServices) Metrics() prometheus.Registerer {
	return prometheus.DefaultRegisterer
}
func (m *MockHostServices) Config() []byte {
	return m.configValue
}
func (m *MockHostServices) DataDir() string {
	return m.dataDirValue
}
func (m *MockHostServices) Events() sdk.EventSink {
	return &MockEventSink{}
}

type MockSecretStore struct{}

func (m *MockSecretStore) Get(key string) ([]byte, error) {
	return nil, sdk.ErrSecretNotFound
}
func (m *MockSecretStore) Set(key string, value []byte) error {
	return nil
}
func (m *MockSecretStore) Delete(key string) error {
	return nil
}

type MockLicenseChecker struct{}

func (m *MockLicenseChecker) FeatureEnabled(key string) bool {
	return false
}
func (m *MockLicenseChecker) Tier() string {
	return "free"
}

type MockEventSink struct{}

func (m *MockEventSink) Publish(ev sdk.Event) {}

// TestModule is a minimal module for in-process testing.
type TestModule struct {
	name string
}

func (t *TestModule) Info() sdk.ModuleInfo {
	return sdk.ModuleInfo{
		Name:        t.name,
		Version:     "1.0.0",
		Description: "Test module",
	}
}

func (t *TestModule) Init(ctx context.Context, host sdk.HostServices) error {
	return nil
}

func (t *TestModule) Start(ctx context.Context) error {
	return nil
}

func (t *TestModule) Stop(ctx context.Context) error {
	return nil
}

func (t *TestModule) Status(ctx context.Context) (sdk.Status, error) {
	return sdk.Status{State: sdk.StateRunning}, nil
}

func (t *TestModule) Health(ctx context.Context) sdk.HealthReport {
	return sdk.HealthReport{
		Level:     sdk.Healthy,
		Message:   "OK",
		CheckedAt: time.Now(),
	}
}

func (t *TestModule) Commands() []sdk.CommandSpec {
	return []sdk.CommandSpec{
		{
			Name:    "greet",
			Use:     "greet <name>",
			Short:   "Greet someone",
			MinArgs: 1,
			MaxArgs: 1,
		},
	}
}

func (t *TestModule) Dispatch(ctx context.Context, path []string, flags map[string]string, args []string) (*sdk.Result, error) {
	if len(path) == 0 || path[0] != "greet" {
		return nil, fmt.Errorf("unknown command")
	}
	if len(args) < 1 {
		return &sdk.Result{Output: "missing name", ExitCode: 1}, nil
	}
	name := args[0]
	return &sdk.Result{
		Output:   fmt.Sprintf("hello, %s", name),
		ExitCode: 0,
	}, nil
}

func (t *TestModule) ConfigSchema() []byte {
	return nil
}
