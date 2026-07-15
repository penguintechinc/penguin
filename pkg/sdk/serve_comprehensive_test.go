package sdk

import (
	"context"
	"errors"
	"math"
	"testing"
	"time"

	v1 "github.com/penguintechinc/penguin/pkg/sdk/proto/penguin/sdk/v1"
	"google.golang.org/grpc"
)

// fakeModule implements Module for testing.
type fakeModule struct {
	infoResp         ModuleInfo
	initErr          error
	startErr         error
	stopErr          error
	statusResp       Status
	statusErr        error
	healthResp       HealthReport
	commandsResp     []CommandSpec
	dispatchResp     *Result
	dispatchErr      error
	configSchemaResp []byte
}

func (m *fakeModule) Info() ModuleInfo {
	return m.infoResp
}

func (m *fakeModule) Init(ctx context.Context, host HostServices) error {
	return m.initErr
}

func (m *fakeModule) Start(ctx context.Context) error {
	return m.startErr
}

func (m *fakeModule) Stop(ctx context.Context) error {
	return m.stopErr
}

func (m *fakeModule) Status(ctx context.Context) (Status, error) {
	return m.statusResp, m.statusErr
}

func (m *fakeModule) Health(ctx context.Context) HealthReport {
	return m.healthResp
}

func (m *fakeModule) Commands() []CommandSpec {
	return m.commandsResp
}

func (m *fakeModule) Dispatch(ctx context.Context, path []string, flags map[string]string, args []string) (*Result, error) {
	return m.dispatchResp, m.dispatchErr
}

func (m *fakeModule) ConfigSchema() []byte {
	return m.configSchemaResp
}

// fakeHostServiceClient implements v1.HostServiceClient for testing.
type fakeHostServiceClient struct {
	configResp     *v1.ConfigResponse
	configErr      error
	dataDirResp    *v1.DataDirResponse
	dataDirErr     error
	secretsGetErr  error
	secretsGetVal  []byte
	secretsSetErr  error
	secretsDelErr  error
	licenseFeated  bool
	licenseFeaErr  error
	licenseTier    string
	licenseTierErr error
	eventErr       error
}

func (f *fakeHostServiceClient) Log(ctx context.Context, req *v1.LogRequest, opts ...grpc.CallOption) (*v1.LogResponse, error) {
	return &v1.LogResponse{}, nil
}

func (f *fakeHostServiceClient) Config(ctx context.Context, req *v1.ConfigRequest, opts ...grpc.CallOption) (*v1.ConfigResponse, error) {
	if f.configErr != nil {
		return nil, f.configErr
	}
	return f.configResp, nil
}

func (f *fakeHostServiceClient) DataDir(ctx context.Context, req *v1.DataDirRequest, opts ...grpc.CallOption) (*v1.DataDirResponse, error) {
	if f.dataDirErr != nil {
		return nil, f.dataDirErr
	}
	return f.dataDirResp, nil
}

func (f *fakeHostServiceClient) SecretsGet(ctx context.Context, req *v1.SecretsGetRequest, opts ...grpc.CallOption) (*v1.SecretsGetResponse, error) {
	errMsg := ""
	if f.secretsGetErr != nil {
		if errors.Is(f.secretsGetErr, ErrSecretNotFound) {
			errMsg = "not found"
		} else {
			errMsg = f.secretsGetErr.Error()
		}
	}
	return &v1.SecretsGetResponse{Value: f.secretsGetVal, Error: errMsg}, nil
}

func (f *fakeHostServiceClient) SecretsSet(ctx context.Context, req *v1.SecretsSetRequest, opts ...grpc.CallOption) (*v1.SecretsSetResponse, error) {
	errMsg := ""
	if f.secretsSetErr != nil {
		errMsg = f.secretsSetErr.Error()
	}
	return &v1.SecretsSetResponse{Error: errMsg}, nil
}

func (f *fakeHostServiceClient) SecretsDelete(ctx context.Context, req *v1.SecretsDeleteRequest, opts ...grpc.CallOption) (*v1.SecretsDeleteResponse, error) {
	errMsg := ""
	if f.secretsDelErr != nil {
		errMsg = f.secretsDelErr.Error()
	}
	return &v1.SecretsDeleteResponse{Error: errMsg}, nil
}

func (f *fakeHostServiceClient) LicenseFeatureEnabled(ctx context.Context, req *v1.LicenseFeatureEnabledRequest, opts ...grpc.CallOption) (*v1.LicenseFeatureEnabledResponse, error) {
	if f.licenseFeaErr != nil {
		return nil, f.licenseFeaErr
	}
	return &v1.LicenseFeatureEnabledResponse{Enabled: f.licenseFeated}, nil
}

func (f *fakeHostServiceClient) LicenseTier(ctx context.Context, req *v1.LicenseTierRequest, opts ...grpc.CallOption) (*v1.LicenseTierResponse, error) {
	if f.licenseTierErr != nil {
		return nil, f.licenseTierErr
	}
	return &v1.LicenseTierResponse{Tier: f.licenseTier}, nil
}

func (f *fakeHostServiceClient) PublishEvent(ctx context.Context, req *v1.PublishEventRequest, opts ...grpc.CallOption) (*v1.PublishEventResponse, error) {
	return &v1.PublishEventResponse{}, f.eventErr
}

// TestModuleServiceImpl_Info tests the Info RPC.
func TestModuleServiceImpl_Info(t *testing.T) {
	tests := []struct {
		name       string
		apiVersion string
		info       ModuleInfo
		wantErr    bool
	}{
		{
			name:       "happy path v1",
			apiVersion: "v1",
			info: ModuleInfo{
				Name:           "testmod",
				Version:        "1.0.0",
				Description:    "Test module",
				LicenseFeature: "test.feature",
			},
			wantErr: false,
		},
		{
			name:       "wrong api version",
			apiVersion: "v2",
			wantErr:    true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			m := &fakeModule{infoResp: tt.info}
			impl := &ModuleServiceImpl{m: m}

			resp, err := impl.Info(context.Background(), &v1.InfoRequest{ApiVersion: tt.apiVersion})
			if (err != nil) != tt.wantErr {
				t.Errorf("Info() error = %v, wantErr %v", err, tt.wantErr)
			}
			if !tt.wantErr {
				if resp.Name != tt.info.Name || resp.Version != tt.info.Version {
					t.Errorf("Info() got %+v, want %+v", resp, tt.info)
				}
			}
		})
	}
}

// TestModuleServiceImpl_Init tests the Init RPC.
func TestModuleServiceImpl_Init(t *testing.T) {
	tests := []struct {
		name       string
		apiVersion string
		wantErr    bool
	}{
		{
			name:       "happy path v1",
			apiVersion: "v1",
			wantErr:    false,
		},
		{
			name:       "wrong api version",
			apiVersion: "v2",
			wantErr:    true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			impl := &ModuleServiceImpl{m: &fakeModule{}}
			_, err := impl.Init(context.Background(), &v1.InitRequest{ApiVersion: tt.apiVersion})
			if (err != nil) != tt.wantErr {
				t.Errorf("Init() error = %v, wantErr %v", err, tt.wantErr)
			}
		})
	}
}

// TestModuleServiceImpl_Start tests the Start RPC.
func TestModuleServiceImpl_Start(t *testing.T) {
	tests := []struct {
		name       string
		apiVersion string
		moduleErr  error
		wantErr    bool
		wantErrMsg string
	}{
		{
			name:       "happy path",
			apiVersion: "v1",
			wantErr:    false,
		},
		{
			name:       "module error",
			apiVersion: "v1",
			moduleErr:  errors.New("start failed"),
			wantErr:    false, // RPC succeeds, error in response
			wantErrMsg: "start failed",
		},
		{
			name:       "wrong api version",
			apiVersion: "v2",
			wantErr:    true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			m := &fakeModule{startErr: tt.moduleErr}
			impl := &ModuleServiceImpl{m: m}

			resp, err := impl.Start(context.Background(), &v1.StartRequest{ApiVersion: tt.apiVersion})
			if (err != nil) != tt.wantErr {
				t.Errorf("Start() error = %v, wantErr %v", err, tt.wantErr)
			}
			if !tt.wantErr && resp.Error != tt.wantErrMsg {
				t.Errorf("Start() error msg = %q, want %q", resp.Error, tt.wantErrMsg)
			}
		})
	}
}

// TestModuleServiceImpl_Stop tests the Stop RPC.
func TestModuleServiceImpl_Stop(t *testing.T) {
	tests := []struct {
		name       string
		apiVersion string
		moduleErr  error
		wantErrMsg string
	}{
		{
			name:       "happy path",
			apiVersion: "v1",
		},
		{
			name:       "module error",
			apiVersion: "v1",
			moduleErr:  errors.New("stop failed"),
			wantErrMsg: "stop failed",
		},
		{
			name:       "wrong api version",
			apiVersion: "v2",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			m := &fakeModule{stopErr: tt.moduleErr}
			impl := &ModuleServiceImpl{m: m}

			resp, err := impl.Stop(context.Background(), &v1.StopRequest{ApiVersion: tt.apiVersion})
			if tt.apiVersion != "v1" {
				if err == nil {
					t.Error("Stop() expected error for wrong api_version")
				}
				return
			}
			if resp.Error != tt.wantErrMsg {
				t.Errorf("Stop() error msg = %q, want %q", resp.Error, tt.wantErrMsg)
			}
		})
	}
}

// TestModuleServiceImpl_Status tests the Status RPC.
func TestModuleServiceImpl_Status(t *testing.T) {
	tests := []struct {
		name       string
		apiVersion string
		status     Status
		moduleErr  error
		wantErrMsg string
		wantErr    bool
	}{
		{
			name:       "happy path",
			apiVersion: "v1",
			status:     Status{State: StateRunning, Detail: map[string]string{"endpoint": "us-east"}},
			wantErr:    false,
		},
		{
			name:       "status error",
			apiVersion: "v1",
			status:     Status{State: StateFailed},
			moduleErr:  errors.New("status check failed"),
			wantErrMsg: "status check failed",
			wantErr:    false,
		},
		{
			name:       "wrong api version",
			apiVersion: "v2",
			wantErr:    true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			m := &fakeModule{statusResp: tt.status, statusErr: tt.moduleErr}
			impl := &ModuleServiceImpl{m: m}

			resp, err := impl.Status(context.Background(), &v1.StatusRequest{ApiVersion: tt.apiVersion})
			if (err != nil) != tt.wantErr {
				t.Errorf("Status() error = %v, wantErr %v", err, tt.wantErr)
			}
			if !tt.wantErr {
				if resp.State != string(tt.status.State) {
					t.Errorf("Status() state = %q, want %q", resp.State, tt.status.State)
				}
				if resp.Error != tt.wantErrMsg {
					t.Errorf("Status() error msg = %q, want %q", resp.Error, tt.wantErrMsg)
				}
			}
		})
	}
}

// TestModuleServiceImpl_Health tests the Health RPC.
func TestModuleServiceImpl_Health(t *testing.T) {
	tests := []struct {
		name       string
		apiVersion string
		wantErr    bool
	}{
		{
			name:       "happy path v1",
			apiVersion: "v1",
			wantErr:    false,
		},
		{
			name:       "wrong api version",
			apiVersion: "v2",
			wantErr:    true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			now := time.Now()
			health := HealthReport{
				Level:     Healthy,
				Message:   "healthy",
				CheckedAt: now,
			}

			m := &fakeModule{healthResp: health}
			impl := &ModuleServiceImpl{m: m}

			resp, err := impl.Health(context.Background(), &v1.HealthRequest{ApiVersion: tt.apiVersion})
			if (err != nil) != tt.wantErr {
				t.Errorf("Health() error = %v, wantErr %v", err, tt.wantErr)
			}
			if !tt.wantErr {
				if resp.Level != int32(Healthy) || resp.Message != "healthy" {
					t.Errorf("Health() got level=%d msg=%q, want level=%d msg=%q", resp.Level, resp.Message, int32(Healthy), "healthy")
				}
			}
		})
	}
}

// TestModuleServiceImpl_Commands tests the Commands RPC.
func TestModuleServiceImpl_Commands(t *testing.T) {
	specs := []CommandSpec{
		{
			Name: "cmd1",
			Use:  "usage1",
			Flags: []FlagSpec{
				{Name: "flag1", Shorthand: "f", Usage: "flag usage"},
			},
		},
		{
			Name: "cmd2",
			Use:  "usage2",
			Subcommands: []CommandSpec{
				{
					Name: "subcmd",
					Use:  "sub usage",
					Flags: []FlagSpec{
						{Name: "subflag", Type: FlagString},
					},
				},
			},
		},
	}

	tests := []struct {
		name       string
		apiVersion string
		wantErr    bool
	}{
		{
			name:       "happy path v1",
			apiVersion: "v1",
			wantErr:    false,
		},
		{
			name:       "wrong api version",
			apiVersion: "v2",
			wantErr:    true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			m := &fakeModule{commandsResp: specs}
			impl := &ModuleServiceImpl{m: m}

			resp, err := impl.Commands(context.Background(), &v1.CommandsRequest{ApiVersion: tt.apiVersion})
			if (err != nil) != tt.wantErr {
				t.Errorf("Commands() error = %v, wantErr %v", err, tt.wantErr)
			}
			if !tt.wantErr {
				if len(resp.Commands) != len(specs) {
					t.Errorf("Commands() got %d commands, want %d", len(resp.Commands), len(specs))
				}
			}
		})
	}
}

// TestModuleServiceImpl_Dispatch tests the Dispatch RPC.
func TestModuleServiceImpl_Dispatch(t *testing.T) {
	tests := []struct {
		name       string
		apiVersion string
		result     *Result
		dispErr    error
		wantExit   int32
		wantErrMsg string
	}{
		{
			name:       "happy path with result",
			apiVersion: "v1",
			result: &Result{
				Output:   "output",
				JSON:     []byte(`{"key":"value"}`),
				ExitCode: 0,
			},
			wantExit: 0,
		},
		{
			name:       "dispatch error",
			apiVersion: "v1",
			result: &Result{
				Output:   "",
				ExitCode: 1,
			},
			dispErr:    errors.New("dispatch failed"),
			wantExit:   1,
			wantErrMsg: "dispatch failed",
		},
		{
			name:       "nil result with error (bug fix test)",
			apiVersion: "v1",
			result:     nil,
			dispErr:    errors.New("command failed"),
			wantExit:   0, // default when result is nil
			wantErrMsg: "command failed",
		},
		{
			name:       "nil result no error",
			apiVersion: "v1",
			result:     nil,
			wantExit:   0,
		},
		{
			name:       "wrong api version",
			apiVersion: "v2",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			m := &fakeModule{dispatchResp: tt.result, dispatchErr: tt.dispErr}
			impl := &ModuleServiceImpl{m: m}

			resp, err := impl.Dispatch(context.Background(), &v1.DispatchRequest{
				ApiVersion: tt.apiVersion,
				Path:       []string{"cmd"},
				Flags:      map[string]string{},
				Args:       []string{},
			})
			if tt.apiVersion != "v1" {
				if err == nil {
					t.Error("Dispatch() expected error for wrong api_version")
				}
				return
			}
			if resp.ExitCode != tt.wantExit {
				t.Errorf("Dispatch() exit code = %d, want %d", resp.ExitCode, tt.wantExit)
			}
			if resp.Error != tt.wantErrMsg {
				t.Errorf("Dispatch() error msg = %q, want %q", resp.Error, tt.wantErrMsg)
			}
		})
	}
}

// TestModuleServiceImpl_ConfigSchema tests the ConfigSchema RPC.
func TestModuleServiceImpl_ConfigSchema(t *testing.T) {
	schema := []byte(`{"type":"object"}`)

	tests := []struct {
		name       string
		apiVersion string
		wantErr    bool
	}{
		{
			name:       "happy path v1",
			apiVersion: "v1",
			wantErr:    false,
		},
		{
			name:       "wrong api version",
			apiVersion: "v2",
			wantErr:    true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			m := &fakeModule{configSchemaResp: schema}
			impl := &ModuleServiceImpl{m: m}

			resp, err := impl.ConfigSchema(context.Background(), &v1.ConfigSchemaRequest{ApiVersion: tt.apiVersion})
			if (err != nil) != tt.wantErr {
				t.Errorf("ConfigSchema() error = %v, wantErr %v", err, tt.wantErr)
			}
			if !tt.wantErr {
				if string(resp.Schema) != string(schema) {
					t.Errorf("ConfigSchema() got %s, want %s", resp.Schema, schema)
				}
			}
		})
	}
}

// TestCommandSpecToProto_RecursiveSubcommands tests nested subcommands.
func TestCommandSpecToProto_RecursiveSubcommands(t *testing.T) {
	spec := CommandSpec{
		Name: "parent",
		Use:  "parent usage",
		Flags: []FlagSpec{
			{Name: "pflag", Type: FlagString, Default: "def"},
		},
		Subcommands: []CommandSpec{
			{
				Name: "child1",
				Use:  "child1 usage",
				Flags: []FlagSpec{
					{Name: "cflag", Shorthand: "c", Usage: "child flag"},
				},
				Subcommands: []CommandSpec{
					{
						Name:    "grandchild",
						Use:     "gc usage",
						MinArgs: 1,
						MaxArgs: 10,
					},
				},
			},
		},
	}

	pbSpec := commandSpecToProto(spec)

	if pbSpec.Name != "parent" {
		t.Errorf("commandSpecToProto() Name = %q, want parent", pbSpec.Name)
	}
	if len(pbSpec.Flags) != 1 || pbSpec.Flags[0].Default != "def" {
		t.Errorf("commandSpecToProto() flags mismatch")
	}
	if len(pbSpec.Subcommands) != 1 {
		t.Errorf("commandSpecToProto() subcommands count = %d, want 1", len(pbSpec.Subcommands))
	}

	child := pbSpec.Subcommands[0]
	if child.Name != "child1" || len(child.Subcommands) != 1 {
		t.Errorf("commandSpecToProto() child mismatch")
	}

	grandchild := child.Subcommands[0]
	if grandchild.Name != "grandchild" || grandchild.MinArgs != 1 || grandchild.MaxArgs != 10 {
		t.Errorf("commandSpecToProto() grandchild mismatch")
	}
}

// TestCommandSpecToProto_FlagTypes tests flag type conversion.
func TestCommandSpecToProto_FlagTypes(t *testing.T) {
	tests := []struct {
		name       string
		flagType   FlagType
		wantString string
	}{
		{"string type", FlagString, string(FlagString)},
		{"bool type", FlagBool, string(FlagBool)},
		{"int type", FlagInt, string(FlagInt)},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			spec := CommandSpec{
				Name: "test",
				Flags: []FlagSpec{
					{Name: "flag", Type: tt.flagType},
				},
			}
			pbSpec := commandSpecToProto(spec)
			if pbSpec.Flags[0].Type != tt.wantString {
				t.Errorf("Flag type = %q, want %q", pbSpec.Flags[0].Type, tt.wantString)
			}
		})
	}
}

// TestSecretsProxy tests the SecretStore proxy methods.
func TestSecretsProxy(t *testing.T) {
	tests := []struct {
		name      string
		operation string
		key       string
		value     []byte
		setErr    error
		getErr    error
		delErr    error
		wantErr   bool
		wantValue []byte
	}{
		{
			name:      "get success",
			operation: "get",
			key:       "mykey",
			wantValue: []byte("secret"),
		},
		{
			name:      "get not found",
			operation: "get",
			getErr:    ErrSecretNotFound,
			wantErr:   true,
		},
		{
			name:      "set success",
			operation: "set",
			key:       "mykey",
			value:     []byte("secret"),
		},
		{
			name:      "set error",
			operation: "set",
			setErr:    errors.New("write failed"),
			wantErr:   true,
		},
		{
			name:      "delete success",
			operation: "delete",
			key:       "mykey",
		},
		{
			name:      "delete error",
			operation: "delete",
			delErr:    errors.New("delete failed"),
			wantErr:   true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			fake := &fakeHostServiceClient{
				secretsGetVal: tt.wantValue,
				secretsGetErr: tt.getErr,
				secretsSetErr: tt.setErr,
				secretsDelErr: tt.delErr,
			}
			proxy := &SecretsProxy{hostClient: fake}

			switch tt.operation {
			case "get":
				val, err := proxy.Get(tt.key)
				if (err != nil) != tt.wantErr {
					t.Errorf("Get() error = %v, wantErr %v", err, tt.wantErr)
				}
				if !tt.wantErr && string(val) != string(tt.wantValue) {
					t.Errorf("Get() value = %s, want %s", val, tt.wantValue)
				}
			case "set":
				err := proxy.Set(tt.key, tt.value)
				if (err != nil) != tt.wantErr {
					t.Errorf("Set() error = %v, wantErr %v", err, tt.wantErr)
				}
			case "delete":
				err := proxy.Delete(tt.key)
				if (err != nil) != tt.wantErr {
					t.Errorf("Delete() error = %v, wantErr %v", err, tt.wantErr)
				}
			}
		})
	}
}

// TestLicenseProxy tests the LicenseChecker proxy.
func TestLicenseProxy(t *testing.T) {
	tests := []struct {
		name      string
		operation string
		enabled   bool
		tier      string
		wantTier  string
		wantBool  bool
	}{
		{
			name:      "feature enabled",
			operation: "enabled",
			enabled:   true,
			wantBool:  true,
		},
		{
			name:      "feature disabled",
			operation: "enabled",
			enabled:   false,
			wantBool:  false,
		},
		{
			name:      "tier professional",
			operation: "tier",
			tier:      "professional",
			wantTier:  "professional",
		},
		{
			name:      "tier enterprise",
			operation: "tier",
			tier:      "enterprise",
			wantTier:  "enterprise",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			fake := &fakeHostServiceClient{
				licenseFeated: tt.enabled,
				licenseTier:   tt.tier,
			}
			proxy := &LicenseProxy{hostClient: fake}

			switch tt.operation {
			case "enabled":
				result := proxy.FeatureEnabled("test.feature")
				if result != tt.wantBool {
					t.Errorf("FeatureEnabled() = %v, want %v", result, tt.wantBool)
				}
			case "tier":
				result := proxy.Tier()
				if result != tt.wantTier {
					t.Errorf("Tier() = %q, want %q", result, tt.wantTier)
				}
			}
		})
	}
}

// TestEventsProxy tests event publishing.
func TestEventsProxy(t *testing.T) {
	fake := &fakeHostServiceClient{}
	proxy := &EventsProxy{hostClient: fake}

	ev := Event{
		Module:  "testmod",
		Type:    EventStateChanged,
		Message: "state changed",
		At:      time.Now(),
		Fields:  map[string]string{"state": "running"},
	}

	proxy.Publish(ev) // should not panic
}

// TestHostServicesProxy tests the main HostServices proxy.
func TestHostServicesProxy(t *testing.T) {
	fake := &fakeHostServiceClient{
		configResp:  &v1.ConfigResponse{Config: []byte(`{"key":"value"}`)},
		dataDirResp: &v1.DataDirResponse{Path: "/var/lib/test"},
	}
	proxy := &HostServicesProxy{hostClient: fake}

	if proxy.Config() == nil || len(proxy.Config()) == 0 {
		t.Error("Config() should return config bytes")
	}
	if proxy.DataDir() != "/var/lib/test" {
		t.Errorf("DataDir() = %q, want /var/lib/test", proxy.DataDir())
	}

	// Logger returns nop
	logger := proxy.Logger()
	if logger == nil {
		t.Error("Logger() should return non-nil logger")
	}

	// Metrics returns registerer
	reg := proxy.Metrics()
	if reg == nil {
		t.Error("Metrics() should return non-nil registerer")
	}

	// Secrets returns proxy
	secrets := proxy.Secrets()
	if secrets == nil {
		t.Error("Secrets() should return non-nil SecretStore")
	}

	// License returns proxy
	license := proxy.License()
	if license == nil {
		t.Error("License() should return non-nil LicenseChecker")
	}

	// Events returns proxy
	events := proxy.Events()
	if events == nil {
		t.Error("Events() should return non-nil EventSink")
	}
}

// TestHostServicesProxy_ConfigEmpty tests Config with empty config.
func TestHostServicesProxy_ConfigEmpty(t *testing.T) {
	fake := &fakeHostServiceClient{
		configResp: &v1.ConfigResponse{Config: []byte{}},
	}
	proxy := &HostServicesProxy{hostClient: fake}

	result := proxy.Config()
	if result == nil {
		t.Error("Config() should return empty byte slice, not nil")
	}
}

// TestHostServicesProxy_ConfigError tests Config error path.
func TestHostServicesProxy_ConfigError(t *testing.T) {
	fake := &fakeHostServiceClient{
		configErr: errors.New("rpc failed"),
	}
	proxy := &HostServicesProxy{hostClient: fake}

	result := proxy.Config()
	if len(result) > 0 {
		t.Error("Config() with RPC error should return nil/empty")
	}
}

// TestHostServicesProxy_DataDirEmpty tests DataDir with empty path.
func TestHostServicesProxy_DataDirEmpty(t *testing.T) {
	fake := &fakeHostServiceClient{
		dataDirResp: &v1.DataDirResponse{Path: ""},
	}
	proxy := &HostServicesProxy{hostClient: fake}

	result := proxy.DataDir()
	if result != "" {
		t.Errorf("DataDir() with empty path = %q, want empty", result)
	}
}

// TestHostServicesProxy_DataDirError tests DataDir error path.
func TestHostServicesProxy_DataDirError(t *testing.T) {
	fake := &fakeHostServiceClient{
		dataDirErr: errors.New("rpc failed"),
	}
	proxy := &HostServicesProxy{hostClient: fake}

	result := proxy.DataDir()
	if result != "" {
		t.Error("DataDir() with RPC error should return empty string")
	}
}

// TestLicenseProxy_FeatureEnabledError tests FeatureEnabled when client returns error.
func TestLicenseProxy_FeatureEnabledError(t *testing.T) {
	fake := &fakeHostServiceClient{
		licenseFeaErr: errors.New("rpc failed"),
	}
	proxy := &LicenseProxy{hostClient: fake}

	result := proxy.FeatureEnabled("test.feature")
	if result {
		t.Error("FeatureEnabled() with error should return false")
	}
}

// TestLicenseProxy_FeatureEnabledSuccess tests FeatureEnabled success case.
func TestLicenseProxy_FeatureEnabledSuccess(t *testing.T) {
	fake := &fakeHostServiceClient{
		licenseFeated: true,
	}
	proxy := &LicenseProxy{hostClient: fake}

	result := proxy.FeatureEnabled("test.feature")
	if !result {
		t.Error("FeatureEnabled() should return true")
	}
}

// TestLicenseProxy_TierError tests Tier when client returns error.
func TestLicenseProxy_TierError(t *testing.T) {
	fake := &fakeHostServiceClient{
		licenseTierErr: errors.New("rpc failed"),
	}
	proxy := &LicenseProxy{hostClient: fake}

	result := proxy.Tier()
	if result != "" {
		t.Errorf("Tier() with error = %q, want empty", result)
	}
}

// TestDispatchNilResultNoPanic tests that Dispatch handles nil result without panic.
func TestDispatchNilResultNoPanic(t *testing.T) {
	m := &fakeModule{dispatchResp: nil, dispatchErr: errors.New("failed")}
	impl := &ModuleServiceImpl{m: m}

	resp, err := impl.Dispatch(context.Background(), &v1.DispatchRequest{
		ApiVersion: "v1",
		Path:       []string{"cmd"},
		Flags:      map[string]string{},
		Args:       []string{},
	})

	if err != nil {
		t.Errorf("Dispatch() error = %v, want nil", err)
	}
	if resp == nil {
		t.Error("Dispatch() response is nil")
		return
	}
	if resp.ExitCode != 0 {
		t.Errorf("Dispatch() exit code = %d, want 0 when result nil", resp.ExitCode)
	}
	if resp.Error != "failed" {
		t.Errorf("Dispatch() error = %q, want %q", resp.Error, "failed")
	}
}

// TestCommandSpecToProto_ArgBoundaryClamp tests argument min/max clamping.
func TestCommandSpecToProto_ArgBoundaryClamp(t *testing.T) {
	tests := []struct {
		name        string
		minArgs     int
		maxArgs     int
		wantMinArgs int32
		wantMaxArgs int32
	}{
		{"normal", 1, 10, 1, 10},
		{"zero min", 0, 5, 0, 5},
		{"overflow max", 1, math.MaxInt32 + 1000, 1, math.MaxInt32},
		{"underflow min", math.MinInt32 - 1000, 5, math.MinInt32, 5},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			spec := CommandSpec{
				Name:    "cmd",
				MinArgs: tt.minArgs,
				MaxArgs: tt.maxArgs,
			}
			pbSpec := commandSpecToProto(spec)
			if pbSpec.MinArgs != tt.wantMinArgs || pbSpec.MaxArgs != tt.wantMaxArgs {
				t.Errorf("clamp min=%d max=%d, want min=%d max=%d",
					pbSpec.MinArgs, pbSpec.MaxArgs, tt.wantMinArgs, tt.wantMaxArgs)
			}
		})
	}
}
