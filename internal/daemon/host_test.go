package daemon

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/99designs/keyring"
	"github.com/penguintechinc/penguin/internal/secrets"
	"github.com/penguintechinc/penguin/internal/telemetry"
	"github.com/penguintechinc/penguin/pkg/sdk"
)

// TestNewHost tests NewHost creation.
func TestNewHost(t *testing.T) {
	tmpDir := t.TempDir()
	tel, _ := telemetry.New("info")

	fakeSecretStore := &fakeSecretStore{}
	fakeLicense := &fakeLicenseChecker{}
	eventSink := &fakeEventSink{}

	host := NewHost(
		"test-module",
		tel,
		fakeSecretStore,
		fakeLicense,
		tmpDir,
		eventSink,
		nil,
	)

	if host == nil {
		t.Error("expected non-nil host")
	}

	if host.Logger() == nil {
		t.Error("expected non-nil logger")
	}

	if host.Secrets() == nil {
		t.Error("expected non-nil secrets")
	}

	if host.License() == nil {
		t.Error("expected non-nil license")
	}

	if host.Metrics() == nil {
		t.Error("expected non-nil metrics")
	}

	if host.DataDir() == "" {
		t.Error("expected non-empty datadir")
	}

	if host.Events() == nil {
		t.Error("expected non-nil events")
	}
}

// TestNewHostNamespacedSecrets tests that NewHost wraps secrets.Store.
func TestNewHostNamespacedSecrets(t *testing.T) {
	tmpDir := t.TempDir()
	tel, _ := telemetry.New("info")

	// Create a real secrets.Store
	storeDir := filepath.Join(tmpDir, "secrets")
	_ = os.MkdirAll(storeDir, 0o700)

	store, _ := secrets.Open(secrets.Config{
		ServiceName:      "test",
		FileDir:          storeDir,
		FilePasswordFunc: func() ([]byte, error) { return []byte("test"), nil },
		AllowedBackends:  []keyring.BackendType{keyring.FileBackend},
	})

	fakeLicense := &fakeLicenseChecker{}
	eventSink := &fakeEventSink{}

	host := NewHost(
		"test-module",
		tel,
		store,
		fakeLicense,
		tmpDir,
		eventSink,
		nil,
	)

	if host == nil {
		t.Error("expected non-nil host")
	}

	// Verify the secrets were namespaced
	if host.Secrets() == nil {
		t.Error("expected non-nil namespaced secrets")
	}
}

// TestNewHostDataDirCreation tests that NewHost creates the data directory.
func TestNewHostDataDirCreation(t *testing.T) {
	tmpDir := t.TempDir()
	tel, _ := telemetry.New("info")

	fakeSecretStore := &fakeSecretStore{}
	fakeLicense := &fakeLicenseChecker{}
	eventSink := &fakeEventSink{}

	host := NewHost(
		"my-module",
		tel,
		fakeSecretStore,
		fakeLicense,
		tmpDir,
		eventSink,
		nil,
	)

	expectedDir := filepath.Join(tmpDir, "my-module")
	if host.DataDir() != expectedDir {
		t.Errorf("expected datadir %s, got %s", expectedDir, host.DataDir())
	}

	// Check that the directory was created
	if _, err := os.Stat(expectedDir); err != nil {
		t.Errorf("datadir not created: %v", err)
	}
}

// TestHostAccessors tests all Host accessor methods.
func TestHostAccessors(t *testing.T) {
	tmpDir := t.TempDir()
	tel, _ := telemetry.New("info")

	fakeSecretStore := &fakeSecretStore{}
	fakeLicense := &fakeLicenseChecker{}
	eventSink := &fakeEventSink{}

	host := NewHost(
		"test-module",
		tel,
		fakeSecretStore,
		fakeLicense,
		tmpDir,
		eventSink,
		nil,
	)

	// Test all accessors
	_ = host.Logger()
	_ = host.Secrets()
	_ = host.License()
	_ = host.Metrics()
	_ = host.DataDir()
	_ = host.Events()
}

// TestHostConfigAccessor tests that Config() returns the validated config bytes.
func TestHostConfigAccessor(t *testing.T) {
	tmpDir := t.TempDir()
	tel, _ := telemetry.New("info")

	fakeSecretStore := &fakeSecretStore{}
	fakeLicense := &fakeLicenseChecker{}
	eventSink := &fakeEventSink{}

	// Test with nil config
	host := NewHost(
		"test-module",
		tel,
		fakeSecretStore,
		fakeLicense,
		tmpDir,
		eventSink,
		nil,
	)

	if host.Config() != nil {
		t.Errorf("Config() should return nil when none provided, got %v", host.Config())
	}

	// Test with actual config bytes
	configBytes := []byte("key: value\nport: 8080")
	host2 := NewHost(
		"another-module",
		tel,
		fakeSecretStore,
		fakeLicense,
		tmpDir,
		eventSink,
		configBytes,
	)

	returnedConfig := host2.Config()
	if returnedConfig == nil {
		t.Error("Config() should return non-nil config when provided")
	}
	if string(returnedConfig) != string(configBytes) {
		t.Errorf("Config() returned wrong bytes: got %q, want %q", string(returnedConfig), string(configBytes))
	}
}

// TestHostConfigInterface tests Config() through the sdk.HostServices interface.
func TestHostConfigInterface(t *testing.T) {
	tmpDir := t.TempDir()
	tel, _ := telemetry.New("info")

	fakeSecretStore := &fakeSecretStore{}
	fakeLicense := &fakeLicenseChecker{}
	eventSink := &fakeEventSink{}

	configBytes := []byte("setting: production")

	var hostSvc sdk.HostServices = NewHost(
		"test-module",
		tel,
		fakeSecretStore,
		fakeLicense,
		tmpDir,
		eventSink,
		configBytes,
	)

	// Call through interface
	returnedConfig := hostSvc.Config()
	if returnedConfig == nil {
		t.Error("Config() through interface should return non-nil")
	}
	if string(returnedConfig) != string(configBytes) {
		t.Errorf("Config() through interface returned wrong bytes")
	}
}

// fakeSecretStore is a minimal implementation of sdk.SecretStore for testing.
type fakeSecretStore struct{}

func (f *fakeSecretStore) Get(key string) ([]byte, error) {
	return nil, nil
}

func (f *fakeSecretStore) Set(key string, value []byte) error {
	return nil
}

func (f *fakeSecretStore) Delete(key string) error {
	return nil
}

func (f *fakeSecretStore) Namespaced(prefix string) sdk.SecretStore {
	return f
}

// fakeLicenseChecker is a minimal implementation of sdk.LicenseChecker.
type fakeLicenseChecker struct{}

func (f *fakeLicenseChecker) FeatureEnabled(key string) bool {
	return true
}

func (f *fakeLicenseChecker) Tier() string {
	return "community"
}

// fakeEventSink is a minimal implementation of sdk.EventSink.
type fakeEventSink struct{}

func (f *fakeEventSink) Publish(ev sdk.Event) {}
