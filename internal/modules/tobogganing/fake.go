package tobogganing

import (
	"context"
	"encoding/json"

	"github.com/penguintechinc/penguin/pkg/sdk"
	"github.com/prometheus/client_golang/prometheus"
	"go.uber.org/zap"
	"golang.zx2c4.com/wireguard/wgctrl/wgtypes"
)

// FakeWGController is a test fake for WGController.
type FakeWGController struct {
	devices map[string]*wgtypes.Device
	configs map[string]*wgtypes.Config
}

// NewFakeWGController creates a new fake WireGuard controller.
func NewFakeWGController() *FakeWGController {
	return &FakeWGController{
		devices: make(map[string]*wgtypes.Device),
		configs: make(map[string]*wgtypes.Config),
	}
}

func (f *FakeWGController) Devices() ([]string, error) {
	names := make([]string, 0, len(f.devices))
	for name := range f.devices {
		names = append(names, name)
	}
	return names, nil
}

func (f *FakeWGController) Close() error {
	return nil
}

func (f *FakeWGController) Device(name string) (*wgtypes.Device, error) {
	if dev, ok := f.devices[name]; ok {
		return dev, nil
	}
	return nil, nil
}

func (f *FakeWGController) Configure(name string, cfg *wgtypes.Config) error {
	f.configs[name] = cfg
	// Create a minimal device entry
	if _, ok := f.devices[name]; !ok {
		f.devices[name] = &wgtypes.Device{
			Name: name,
		}
	}
	return nil
}

// GetLastConfig returns the last configuration applied to the device.
func (f *FakeWGController) GetLastConfig(name string) *wgtypes.Config {
	return f.configs[name]
}

// FakeSecretStore implements sdk.SecretStore for testing.
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

// FakeLicenseChecker implements sdk.LicenseChecker for testing.
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

// FakeEventSink implements sdk.EventSink for testing.
type FakeEventSink struct {
	events []sdk.Event
}

func (f *FakeEventSink) Publish(ev sdk.Event) {
	f.events = append(f.events, ev)
}

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

// NewFakeHost creates a new fake host for testing.
func NewFakeHost(logger *zap.Logger, dataDir string) *FakeHostServices {
	return &FakeHostServices{
		logger:    logger,
		secrets:   &FakeSecretStore{store: make(map[string][]byte)},
		license:   &FakeLicenseChecker{featureEnabled: true, tier: "professional"},
		metrics:   prometheus.NewRegistry(),
		dataDir:   dataDir,
		eventSink: &FakeEventSink{},
		config:    nil,
	}
}

// FakeHTTPClient is a test fake for HTTP calls.
type FakeHTTPClient struct {
	responses map[string]interface{}
	errors    map[string]error
}

func (f *FakeHTTPClient) DoJSON(ctx context.Context, method, url, token string, reqBody interface{}, respBody interface{}) error {
	if err, ok := f.errors[url]; ok && err != nil {
		return err
	}

	if resp, ok := f.responses[url]; ok {
		data, _ := json.Marshal(resp)
		_ = json.Unmarshal(data, respBody)
		return nil
	}

	return nil
}
