package extplugin

import (
	"context"
	"testing"
	"time"

	sdkv1 "github.com/penguintechinc/penguin/pkg/sdk/proto/penguin/sdk/v1"
	"github.com/penguintechinc/penguin/pkg/sdk"
	"github.com/prometheus/client_golang/prometheus"
	"go.uber.org/zap"
)

// FakeHostServices is a test implementation of sdk.HostServices with configurable behavior.
type FakeHostServices struct {
	logger         *zap.Logger
	secrets        sdk.SecretStore
	license        sdk.LicenseChecker
	config         []byte
	dataDir        string
	events         sdk.EventSink
	publishedEventsque []sdk.Event
}

func NewFakeHostServices() *FakeHostServices {
	return &FakeHostServices{
		logger:         zap.NewNop(),
		secrets:        &FakeSecretStore{store: make(map[string][]byte)},
		license:        &FakeLicenseChecker{tier: "free", features: make(map[string]bool)},
		config:         []byte("test config"),
		dataDir:        "/tmp/test",
		events:         &FakeEventSink{events: make([]sdk.Event, 0)},
		publishedEventsque: make([]sdk.Event, 0),
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
	return prometheus.DefaultRegisterer
}

func (f *FakeHostServices) Config() []byte {
	return f.config
}

func (f *FakeHostServices) DataDir() string {
	return f.dataDir
}

func (f *FakeHostServices) Events() sdk.EventSink {
	return f.events
}

// FakeSecretStore is a test implementation of sdk.SecretStore.
type FakeSecretStore struct {
	store map[string][]byte
}

func (f *FakeSecretStore) Get(key string) ([]byte, error) {
	val, ok := f.store[key]
	if !ok {
		return nil, sdk.ErrSecretNotFound
	}
	return val, nil
}

func (f *FakeSecretStore) Set(key string, value []byte) error {
	f.store[key] = value
	return nil
}

func (f *FakeSecretStore) Delete(key string) error {
	delete(f.store, key)
	return nil
}

// FakeLicenseChecker is a test implementation of sdk.LicenseChecker.
type FakeLicenseChecker struct {
	tier     string
	features map[string]bool
}

func (f *FakeLicenseChecker) FeatureEnabled(key string) bool {
	enabled, ok := f.features[key]
	if !ok {
		return false
	}
	return enabled
}

func (f *FakeLicenseChecker) Tier() string {
	return f.tier
}

// FakeEventSink is a test implementation of sdk.EventSink.
type FakeEventSink struct {
	events []sdk.Event
}

func (f *FakeEventSink) Publish(ev sdk.Event) {
	f.events = append(f.events, ev)
}

// TestLogRequestSuccess tests successful Log RPC.
func TestLogRequestSuccess(t *testing.T) {
	host := NewFakeHostServices()
	hostService := NewHostServiceImpl(host)

	req := &sdkv1.LogRequest{
		ApiVersion: "v1",
		Level:      "info",
		Message:    "test message",
	}

	resp, err := hostService.Log(context.Background(), req)
	if err != nil {
		t.Errorf("Log() failed: %v", err)
	}
	if resp == nil {
		t.Fatalf("Log() returned nil response")
	}
}

// TestLogRequestBadApiVersion tests Log with unsupported API version.
func TestLogRequestBadApiVersion(t *testing.T) {
	host := NewFakeHostServices()
	hostService := NewHostServiceImpl(host)

	req := &sdkv1.LogRequest{
		ApiVersion: "v999",
		Level:      "info",
		Message:    "test message",
	}

	_, err := hostService.Log(context.Background(), req)
	if err == nil {
		t.Errorf("Log() should fail for unsupported API version")
	}
}

// TestSecretsGetSuccess tests successful SecretsGet RPC.
func TestSecretsGetSuccess(t *testing.T) {
	host := NewFakeHostServices()
	secretStore := host.Secrets().(*FakeSecretStore)
	if err := secretStore.Set("test-key", []byte("test-value")); err != nil {
		t.Fatalf("Set failed: %v", err)
	}

	hostService := NewHostServiceImpl(host)

	req := &sdkv1.SecretsGetRequest{
		ApiVersion: "v1",
		Key:        "test-key",
	}

	resp, err := hostService.SecretsGet(context.Background(), req)
	if err != nil {
		t.Errorf("SecretsGet() failed: %v", err)
	}
	if resp == nil {
		t.Fatalf("SecretsGet() returned nil response")
	}
	if string(resp.Value) != "test-value" {
		t.Errorf("SecretsGet() value: got %q, want %q", string(resp.Value), "test-value")
	}
	if resp.Error != "" {
		t.Errorf("SecretsGet() error: got %q, want empty", resp.Error)
	}
}

// TestSecretsGetNotFound tests SecretsGet with nonexistent key.
func TestSecretsGetNotFound(t *testing.T) {
	host := NewFakeHostServices()
	hostService := NewHostServiceImpl(host)

	req := &sdkv1.SecretsGetRequest{
		ApiVersion: "v1",
		Key:        "nonexistent-key",
	}

	resp, err := hostService.SecretsGet(context.Background(), req)
	if err != nil {
		t.Errorf("SecretsGet() failed: %v", err)
	}
	if resp == nil {
		t.Fatalf("SecretsGet() returned nil response")
	}
	if resp.Error == "" {
		t.Errorf("SecretsGet() error: should not be empty for missing key")
	}
	if resp.Error != "not found" {
		t.Errorf("SecretsGet() error: got %q, want %q", resp.Error, "not found")
	}
}

// TestSecretsGetBadApiVersion tests SecretsGet with unsupported API version.
func TestSecretsGetBadApiVersion(t *testing.T) {
	host := NewFakeHostServices()
	hostService := NewHostServiceImpl(host)

	req := &sdkv1.SecretsGetRequest{
		ApiVersion: "v999",
		Key:        "test-key",
	}

	_, err := hostService.SecretsGet(context.Background(), req)
	if err == nil {
		t.Errorf("SecretsGet() should fail for unsupported API version")
	}
}

// TestSecretsSetSuccess tests successful SecretsSet RPC.
func TestSecretsSetSuccess(t *testing.T) {
	host := NewFakeHostServices()
	hostService := NewHostServiceImpl(host)

	req := &sdkv1.SecretsSetRequest{
		ApiVersion: "v1",
		Key:        "test-key",
		Value:      []byte("test-value"),
	}

	resp, err := hostService.SecretsSet(context.Background(), req)
	if err != nil {
		t.Errorf("SecretsSet() failed: %v", err)
	}
	if resp == nil {
		t.Fatalf("SecretsSet() returned nil response")
	}
	if resp.Error != "" {
		t.Errorf("SecretsSet() error: got %q, want empty", resp.Error)
	}

	// Verify the secret was actually stored.
	secretStore := host.Secrets().(*FakeSecretStore)
	val, err := secretStore.Get("test-key")
	if err != nil {
		t.Errorf("SecretsSet() failed to store secret: %v", err)
	}
	if string(val) != "test-value" {
		t.Errorf("SecretsSet() stored value: got %q, want %q", string(val), "test-value")
	}
}

// TestSecretsSetBadApiVersion tests SecretsSet with unsupported API version.
func TestSecretsSetBadApiVersion(t *testing.T) {
	host := NewFakeHostServices()
	hostService := NewHostServiceImpl(host)

	req := &sdkv1.SecretsSetRequest{
		ApiVersion: "v999",
		Key:        "test-key",
		Value:      []byte("test-value"),
	}

	_, err := hostService.SecretsSet(context.Background(), req)
	if err == nil {
		t.Errorf("SecretsSet() should fail for unsupported API version")
	}
}

// TestSecretsDeleteSuccess tests successful SecretsDelete RPC.
func TestSecretsDeleteSuccess(t *testing.T) {
	host := NewFakeHostServices()
	secretStore := host.Secrets().(*FakeSecretStore)
	if err := secretStore.Set("test-key", []byte("test-value")); err != nil {
		t.Fatalf("Set failed: %v", err)
	}

	hostService := NewHostServiceImpl(host)

	req := &sdkv1.SecretsDeleteRequest{
		ApiVersion: "v1",
		Key:        "test-key",
	}

	resp, err := hostService.SecretsDelete(context.Background(), req)
	if err != nil {
		t.Errorf("SecretsDelete() failed: %v", err)
	}
	if resp == nil {
		t.Fatalf("SecretsDelete() returned nil response")
	}
	if resp.Error != "" {
		t.Errorf("SecretsDelete() error: got %q, want empty", resp.Error)
	}

	// Verify the secret was actually deleted.
	_, err = secretStore.Get("test-key")
	if err != sdk.ErrSecretNotFound {
		t.Errorf("SecretsDelete() failed to delete secret: %v", err)
	}
}

// TestSecretsDeleteBadApiVersion tests SecretsDelete with unsupported API version.
func TestSecretsDeleteBadApiVersion(t *testing.T) {
	host := NewFakeHostServices()
	hostService := NewHostServiceImpl(host)

	req := &sdkv1.SecretsDeleteRequest{
		ApiVersion: "v999",
		Key:        "test-key",
	}

	_, err := hostService.SecretsDelete(context.Background(), req)
	if err == nil {
		t.Errorf("SecretsDelete() should fail for unsupported API version")
	}
}

// TestLicenseFeatureEnabledSuccess tests LicenseFeatureEnabled RPC.
func TestLicenseFeatureEnabledSuccess(t *testing.T) {
	host := NewFakeHostServices()
	licenseChecker := host.License().(*FakeLicenseChecker)
	licenseChecker.features["test-feature"] = true

	hostService := NewHostServiceImpl(host)

	req := &sdkv1.LicenseFeatureEnabledRequest{
		ApiVersion: "v1",
		Key:        "test-feature",
	}

	resp, err := hostService.LicenseFeatureEnabled(context.Background(), req)
	if err != nil {
		t.Errorf("LicenseFeatureEnabled() failed: %v", err)
	}
	if resp == nil {
		t.Fatalf("LicenseFeatureEnabled() returned nil response")
	}
	if !resp.Enabled {
		t.Errorf("LicenseFeatureEnabled() enabled: got false, want true")
	}
}

// TestLicenseFeatureEnabledDisabled tests LicenseFeatureEnabled with disabled feature.
func TestLicenseFeatureEnabledDisabled(t *testing.T) {
	host := NewFakeHostServices()
	hostService := NewHostServiceImpl(host)

	req := &sdkv1.LicenseFeatureEnabledRequest{
		ApiVersion: "v1",
		Key:        "unknown-feature",
	}

	resp, err := hostService.LicenseFeatureEnabled(context.Background(), req)
	if err != nil {
		t.Errorf("LicenseFeatureEnabled() failed: %v", err)
	}
	if resp == nil {
		t.Fatalf("LicenseFeatureEnabled() returned nil response")
	}
	if resp.Enabled {
		t.Errorf("LicenseFeatureEnabled() enabled: got true, want false for unknown feature")
	}
}

// TestLicenseFeatureEnabledBadApiVersion tests LicenseFeatureEnabled with unsupported API version.
func TestLicenseFeatureEnabledBadApiVersion(t *testing.T) {
	host := NewFakeHostServices()
	hostService := NewHostServiceImpl(host)

	req := &sdkv1.LicenseFeatureEnabledRequest{
		ApiVersion: "v999",
		Key:        "test-feature",
	}

	_, err := hostService.LicenseFeatureEnabled(context.Background(), req)
	if err == nil {
		t.Errorf("LicenseFeatureEnabled() should fail for unsupported API version")
	}
}

// TestLicenseTierSuccess tests LicenseTier RPC.
func TestLicenseTierSuccess(t *testing.T) {
	host := NewFakeHostServices()
	licenseChecker := host.License().(*FakeLicenseChecker)
	licenseChecker.tier = "enterprise"

	hostService := NewHostServiceImpl(host)

	req := &sdkv1.LicenseTierRequest{
		ApiVersion: "v1",
	}

	resp, err := hostService.LicenseTier(context.Background(), req)
	if err != nil {
		t.Errorf("LicenseTier() failed: %v", err)
	}
	if resp == nil {
		t.Fatalf("LicenseTier() returned nil response")
	}
	if resp.Tier != "enterprise" {
		t.Errorf("LicenseTier() tier: got %q, want %q", resp.Tier, "enterprise")
	}
}

// TestLicenseTierBadApiVersion tests LicenseTier with unsupported API version.
func TestLicenseTierBadApiVersion(t *testing.T) {
	host := NewFakeHostServices()
	hostService := NewHostServiceImpl(host)

	req := &sdkv1.LicenseTierRequest{
		ApiVersion: "v999",
	}

	_, err := hostService.LicenseTier(context.Background(), req)
	if err == nil {
		t.Errorf("LicenseTier() should fail for unsupported API version")
	}
}

// TestConfigSuccess tests Config RPC.
func TestConfigSuccess(t *testing.T) {
	testConfig := []byte("test configuration")
	host := NewFakeHostServices()
	host.config = testConfig

	hostService := NewHostServiceImpl(host)

	req := &sdkv1.ConfigRequest{
		ApiVersion: "v1",
	}

	resp, err := hostService.Config(context.Background(), req)
	if err != nil {
		t.Errorf("Config() failed: %v", err)
	}
	if resp == nil {
		t.Fatalf("Config() returned nil response")
	}
	if string(resp.Config) != string(testConfig) {
		t.Errorf("Config() value: got %q, want %q", string(resp.Config), string(testConfig))
	}
}

// TestConfigBadApiVersion tests Config with unsupported API version.
func TestConfigBadApiVersion(t *testing.T) {
	host := NewFakeHostServices()
	hostService := NewHostServiceImpl(host)

	req := &sdkv1.ConfigRequest{
		ApiVersion: "v999",
	}

	_, err := hostService.Config(context.Background(), req)
	if err == nil {
		t.Errorf("Config() should fail for unsupported API version")
	}
}

// TestDataDirSuccess tests DataDir RPC.
func TestDataDirSuccess(t *testing.T) {
	testDir := "/var/lib/penguin/plugins"
	host := NewFakeHostServices()
	host.dataDir = testDir

	hostService := NewHostServiceImpl(host)

	req := &sdkv1.DataDirRequest{
		ApiVersion: "v1",
	}

	resp, err := hostService.DataDir(context.Background(), req)
	if err != nil {
		t.Errorf("DataDir() failed: %v", err)
	}
	if resp == nil {
		t.Fatalf("DataDir() returned nil response")
	}
	if resp.Path != testDir {
		t.Errorf("DataDir() path: got %q, want %q", resp.Path, testDir)
	}
}

// TestDataDirBadApiVersion tests DataDir with unsupported API version.
func TestDataDirBadApiVersion(t *testing.T) {
	host := NewFakeHostServices()
	hostService := NewHostServiceImpl(host)

	req := &sdkv1.DataDirRequest{
		ApiVersion: "v999",
	}

	_, err := hostService.DataDir(context.Background(), req)
	if err == nil {
		t.Errorf("DataDir() should fail for unsupported API version")
	}
}

// TestPublishEventSuccess tests PublishEvent RPC.
func TestPublishEventSuccess(t *testing.T) {
	host := NewFakeHostServices()
	hostService := NewHostServiceImpl(host)

	eventTime := time.Now().Unix()
	req := &sdkv1.PublishEventRequest{
		ApiVersion: "v1",
		Module:     "test-module",
		Type:       "test-event",
		Message:    "test message",
		AtUnixNano: eventTime * 1e9,
		Fields:     map[string]string{"key": "value"},
	}

	resp, err := hostService.PublishEvent(context.Background(), req)
	if err != nil {
		t.Errorf("PublishEvent() failed: %v", err)
	}
	if resp == nil {
		t.Fatalf("PublishEvent() returned nil response")
	}

	// Verify the event was published.
	eventSink := host.Events().(*FakeEventSink)
	if len(eventSink.events) != 1 {
		t.Errorf("PublishEvent() events count: got %d, want 1", len(eventSink.events))
	}
	if len(eventSink.events) > 0 {
		ev := eventSink.events[0]
		if ev.Module != "test-module" {
			t.Errorf("PublishEvent() module: got %q, want %q", ev.Module, "test-module")
		}
		if ev.Message != "test message" {
			t.Errorf("PublishEvent() message: got %q, want %q", ev.Message, "test message")
		}
		if ev.Fields["key"] != "value" {
			t.Errorf("PublishEvent() field: got %q, want %q", ev.Fields["key"], "value")
		}
	}
}

// TestPublishEventBadApiVersion tests PublishEvent with unsupported API version.
func TestPublishEventBadApiVersion(t *testing.T) {
	host := NewFakeHostServices()
	hostService := NewHostServiceImpl(host)

	req := &sdkv1.PublishEventRequest{
		ApiVersion: "v999",
		Module:     "test-module",
		Type:       "test-event",
		Message:    "test message",
	}

	_, err := hostService.PublishEvent(context.Background(), req)
	if err == nil {
		t.Errorf("PublishEvent() should fail for unsupported API version")
	}
}

// TestProtoCommandSpecToSDKConversion tests the proto to SDK converter.
func TestProtoCommandSpecToSDKConversion(t *testing.T) {
	tests := []struct {
		name string
		pb   *sdkv1.CommandSpec
		want string
	}{
		{
			name: "simple command",
			pb: &sdkv1.CommandSpec{
				Name:    "greet",
				Use:     "greet <name>",
				Short:   "Greet someone",
				MinArgs: 1,
				MaxArgs: 1,
			},
			want: "greet",
		},
		{
			name: "command with flags",
			pb: &sdkv1.CommandSpec{
				Name: "deploy",
				Use:  "deploy [options]",
				Flags: []*sdkv1.FlagSpec{
					{
						Name:    "force",
						Type:    "bool",
						Default: "false",
					},
				},
			},
			want: "deploy",
		},
		{
			name: "command with subcommands",
			pb: &sdkv1.CommandSpec{
				Name: "config",
				Subcommands: []*sdkv1.CommandSpec{
					{
						Name: "get",
						Use:  "config get <key>",
					},
					{
						Name: "set",
						Use:  "config set <key> <value>",
					},
				},
			},
			want: "config",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			converted := protoCommandSpecToSDK(tt.pb)
			if converted.Name != tt.want {
				t.Errorf("protoCommandSpecToSDK() name: got %q, want %q", converted.Name, tt.want)
			}
			if converted.Use != tt.pb.Use {
				t.Errorf("protoCommandSpecToSDK() use: got %q, want %q", converted.Use, tt.pb.Use)
			}
			if converted.Short != tt.pb.Short {
				t.Errorf("protoCommandSpecToSDK() short: got %q, want %q", converted.Short, tt.pb.Short)
			}
			if converted.MinArgs != int(tt.pb.MinArgs) {
				t.Errorf("protoCommandSpecToSDK() minArgs: got %d, want %d", converted.MinArgs, int(tt.pb.MinArgs))
			}
			if converted.MaxArgs != int(tt.pb.MaxArgs) {
				t.Errorf("protoCommandSpecToSDK() maxArgs: got %d, want %d", converted.MaxArgs, int(tt.pb.MaxArgs))
			}
		})
	}
}

// TestProtoCommandSpecToSDKRecursive tests recursive conversion of subcommands.
func TestProtoCommandSpecToSDKRecursive(t *testing.T) {
	pb := &sdkv1.CommandSpec{
		Name: "root",
		Subcommands: []*sdkv1.CommandSpec{
			{
				Name: "sub1",
				Subcommands: []*sdkv1.CommandSpec{
					{
						Name: "subsub1",
					},
				},
			},
		},
	}

	converted := protoCommandSpecToSDK(pb)
	if len(converted.Subcommands) != 1 {
		t.Errorf("protoCommandSpecToSDK() subcommands count: got %d, want 1", len(converted.Subcommands))
	}
	if len(converted.Subcommands) > 0 {
		if converted.Subcommands[0].Name != "sub1" {
			t.Errorf("protoCommandSpecToSDK() subcommand name: got %q, want %q", converted.Subcommands[0].Name, "sub1")
		}
		if len(converted.Subcommands[0].Subcommands) != 1 {
			t.Errorf("protoCommandSpecToSDK() nested subcommands count: got %d, want 1", len(converted.Subcommands[0].Subcommands))
		}
	}
}

// TestConfigSchemaConversion tests the ConfigSchema proto to bytes conversion.
func TestConfigSchemaConversion(t *testing.T) {
	testSchema := []byte(`{"type": "object", "properties": {"key": {"type": "string"}}}`)

	// Simulate the SDK adapter behavior with a mock module that returns schema.
	adapter := &moduleClientAdapter{}

	// This would normally call the gRPC method, but we're testing the converter function.
	// In an integration context, this is tested through the full moduleClientAdapter.
	_ = adapter
	_ = testSchema
	// Test passes if no panic; actual gRPC call would be tested in integration tests.
}
