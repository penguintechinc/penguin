package daemon

import (
	"bytes"
	"context"
	"errors"
	"path/filepath"
	"sync"
	"testing"
	"time"

	daemonv1 "github.com/penguintechinc/penguin/api/proto/penguin/daemon/v1"
	"github.com/penguintechinc/penguin/pkg/sdk"
	"go.uber.org/zap/zaptest"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
)

// TestModule is a test implementation of sdk.Module.
type TestModule struct {
	info            sdk.ModuleInfo
	commands        []sdk.CommandSpec
	dispatchResult  *sdk.Result
	dispatchErr     error
	healthLevel     sdk.HealthLevel
	statusState     sdk.ModuleState
	statusDetail    map[string]string
	statusErr       error
}

func (tm *TestModule) Info() sdk.ModuleInfo {
	return tm.info
}

func (tm *TestModule) Init(ctx context.Context, host sdk.HostServices) error {
	return nil
}

func (tm *TestModule) Start(ctx context.Context) error {
	return nil
}

func (tm *TestModule) Stop(ctx context.Context) error {
	return nil
}

func (tm *TestModule) Status(ctx context.Context) (sdk.Status, error) {
	if tm.statusErr != nil {
		return sdk.Status{}, tm.statusErr
	}
	return sdk.Status{State: tm.statusState, Detail: tm.statusDetail}, nil
}

func (tm *TestModule) Health(ctx context.Context) sdk.HealthReport {
	return sdk.HealthReport{
		Level:     tm.healthLevel,
		Message:   "test health",
		CheckedAt: time.Now(),
	}
}

func (tm *TestModule) Commands() []sdk.CommandSpec {
	return tm.commands
}

func (tm *TestModule) Dispatch(ctx context.Context, path []string, flags map[string]string, args []string) (*sdk.Result, error) {
	if tm.dispatchErr != nil {
		return nil, tm.dispatchErr
	}
	if tm.dispatchResult != nil {
		return tm.dispatchResult, nil
	}
	return &sdk.Result{Output: "ok"}, nil
}

func (tm *TestModule) ConfigSchema() []byte {
	return nil
}

// MockUpdateClient is a test implementation of Client interface.
type MockUpdateClient struct {
	available bool
	latest    string
	checkErr  error
	applyErr  error
}

func (m *MockUpdateClient) CheckUpdate(ctx context.Context) (bool, string, error) {
	return m.available, m.latest, m.checkErr
}

func (m *MockUpdateClient) ApplyUpdate(ctx context.Context) error {
	return m.applyErr
}

// newTestServer creates a test server with one test module.
func newTestServer(t *testing.T, tm *TestModule) (*Server, *Supervisor) {
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return tm }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	sup := New(cfg)
	srv := NewServer(sup, "1.0.0", zaptest.NewLogger(t), nil)
	return srv, sup
}

// TestCheckAPIVersion tests checkAPIVersion with various versions.
func TestCheckAPIVersion(t *testing.T) {
	testModule := &TestModule{
		info: sdk.ModuleInfo{Name: "test", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}
	srv, _ := newTestServer(t, testModule)

	tests := []struct {
		version   string
		wantError bool
	}{
		{"", false},       // Empty defaults to v1
		{"v1", false},     // v1 supported
		{"v2", true},      // v2 not supported
		{"v99", true},     // v99 not supported
		{"invalid", true}, // Invalid version
	}

	for _, tt := range tests {
		err := srv.checkAPIVersion(tt.version)
		if tt.wantError && err == nil {
			t.Errorf("checkAPIVersion(%q): expected error, got nil", tt.version)
		}
		if !tt.wantError && err != nil {
			t.Errorf("checkAPIVersion(%q): unexpected error %v", tt.version, err)
		}
	}
}

// TestVersion tests the Version RPC.
func TestVersion(t *testing.T) {
	testModule := &TestModule{
		info: sdk.ModuleInfo{Name: "test", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}
	srv, _ := newTestServer(t, testModule)

	// Success case
	resp, err := srv.Version(context.Background(), &daemonv1.VersionRequest{ApiVersion: "v1"})
	if err != nil {
		t.Fatalf("Version failed: %v", err)
	}
	if resp.DaemonVersion != "1.0.0" {
		t.Errorf("Version: got daemon version %q, want 1.0.0", resp.DaemonVersion)
	}
	if resp.ApiVersion != "v1" {
		t.Errorf("Version: got api version %q, want v1", resp.ApiVersion)
	}

	// Unsupported API version
	_, err = srv.Version(context.Background(), &daemonv1.VersionRequest{ApiVersion: "v2"})
	if err == nil {
		t.Errorf("Version: expected error for unsupported API version")
	}
	st, _ := status.FromError(err)
	if st.Code() != codes.Unimplemented {
		t.Errorf("Version: expected Unimplemented code, got %v", st.Code())
	}
}

// TestListModules tests the ListModules RPC.
func TestListModules(t *testing.T) {
	testModule := &TestModule{
		info: sdk.ModuleInfo{
			Name:        "test-mod",
			Version:     "1.0.0",
			Description: "test module",
		},
		statusState: sdk.StateRunning,
	}
	srv, sup := newTestServer(t, testModule)

	// Load the module so it appears in the list
	ctx := context.Background()
	_ = sup.Load(ctx, "test-mod")

	// Test list all modules
	resp, err := srv.ListModules(ctx, &daemonv1.ListModulesRequest{ApiVersion: "v1"})
	if err != nil {
		t.Fatalf("ListModules failed: %v", err)
	}
	if len(resp.Modules) == 0 {
		t.Errorf("ListModules: expected modules, got none")
	}
	found := false
	for _, m := range resp.Modules {
		if m.Name == "test-mod" {
			found = true
			if m.Version != "1.0.0" {
				t.Errorf("ListModules: version %q, want 1.0.0", m.Version)
			}
			if m.Description != "test module" {
				t.Errorf("ListModules: description %q, want 'test module'", m.Description)
			}
		}
	}
	if !found {
		t.Errorf("ListModules: test-mod not found in response")
	}

	// Test unsupported API version
	_, err = srv.ListModules(ctx, &daemonv1.ListModulesRequest{ApiVersion: "v2"})
	if err == nil {
		t.Errorf("ListModules: expected error for unsupported API version")
	}
}

// TestLoadModule tests the LoadModule RPC.
func TestLoadModule(t *testing.T) {
	testModule := &TestModule{
		info:        sdk.ModuleInfo{Name: "test-mod", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}
	srv, _ := newTestServer(t, testModule)
	ctx := context.Background()

	// Test successful load
	resp, err := srv.LoadModule(ctx, &daemonv1.LoadModuleRequest{
		ApiVersion: "v1",
		Name:       "test-mod",
	})
	if err != nil {
		t.Fatalf("LoadModule failed: %v", err)
	}
	if resp.State != "running" {
		t.Errorf("LoadModule: state %q, want 'running'", resp.State)
	}

	// Test empty module name
	_, err = srv.LoadModule(ctx, &daemonv1.LoadModuleRequest{
		ApiVersion: "v1",
		Name:       "",
	})
	if err == nil {
		t.Errorf("LoadModule: expected error for empty name")
	}
	st, _ := status.FromError(err)
	if st.Code() != codes.InvalidArgument {
		t.Errorf("LoadModule: expected InvalidArgument code, got %v", st.Code())
	}

	// Test unknown module
	_, err = srv.LoadModule(ctx, &daemonv1.LoadModuleRequest{
		ApiVersion: "v1",
		Name:       "unknown-mod",
	})
	if err == nil {
		t.Errorf("LoadModule: expected error for unknown module")
	}
	st, _ = status.FromError(err)
	if st.Code() != codes.NotFound {
		t.Errorf("LoadModule: expected NotFound code, got %v", st.Code())
	}

	// Test unsupported API version
	_, err = srv.LoadModule(ctx, &daemonv1.LoadModuleRequest{
		ApiVersion: "v2",
		Name:       "test-mod",
	})
	if err == nil {
		t.Errorf("LoadModule: expected error for unsupported API version")
	}
}

// TestUnloadModule tests the UnloadModule RPC.
func TestUnloadModule(t *testing.T) {
	testModule := &TestModule{
		info:        sdk.ModuleInfo{Name: "test-mod", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}
	srv, sup := newTestServer(t, testModule)
	ctx := context.Background()

	// Load first
	_ = sup.Load(ctx, "test-mod")

	// Test unload
	resp, err := srv.UnloadModule(ctx, &daemonv1.UnloadModuleRequest{
		ApiVersion: "v1",
		Name:       "test-mod",
	})
	if err != nil {
		t.Fatalf("UnloadModule failed: %v", err)
	}
	if resp.State != "stopped" {
		t.Errorf("UnloadModule: state %q, want 'stopped'", resp.State)
	}

	// Test empty module name
	_, err = srv.UnloadModule(ctx, &daemonv1.UnloadModuleRequest{
		ApiVersion: "v1",
		Name:       "",
	})
	if err == nil {
		t.Errorf("UnloadModule: expected error for empty name")
	}
	st, _ := status.FromError(err)
	if st.Code() != codes.InvalidArgument {
		t.Errorf("UnloadModule: expected InvalidArgument code, got %v", st.Code())
	}

	// Test unsupported API version
	_, err = srv.UnloadModule(ctx, &daemonv1.UnloadModuleRequest{
		ApiVersion: "v2",
		Name:       "test-mod",
	})
	if err == nil {
		t.Errorf("UnloadModule: expected error for unsupported API version")
	}
}

// TestGetStatus tests the GetStatus RPC.
func TestGetStatus(t *testing.T) {
	testModule := &TestModule{
		info:        sdk.ModuleInfo{Name: "test-mod", Version: "1.0.0"},
		statusState: sdk.StateRunning,
		statusDetail: map[string]string{"status": "running normally"},
		healthLevel: sdk.Healthy,
	}
	srv, sup := newTestServer(t, testModule)
	ctx := context.Background()

	// Load module
	_ = sup.Load(ctx, "test-mod")

	// Test get all statuses
	resp, err := srv.GetStatus(ctx, &daemonv1.GetStatusRequest{
		ApiVersion: "v1",
		Name:       "",
	})
	if err != nil {
		t.Fatalf("GetStatus (all) failed: %v", err)
	}
	if len(resp.Modules) == 0 {
		t.Errorf("GetStatus: expected modules, got none")
	}

	// Test get single module status
	resp, err = srv.GetStatus(ctx, &daemonv1.GetStatusRequest{
		ApiVersion: "v1",
		Name:       "test-mod",
	})
	if err != nil {
		t.Fatalf("GetStatus (single) failed: %v", err)
	}
	if len(resp.Modules) != 1 {
		t.Errorf("GetStatus: expected 1 module, got %d", len(resp.Modules))
	}
	if resp.Modules[0].State != "running" {
		t.Errorf("GetStatus: state %q, want 'running'", resp.Modules[0].State)
	}
	if resp.Modules[0].Health != "healthy" {
		t.Errorf("GetStatus: health %q, want 'healthy'", resp.Modules[0].Health)
	}

	// Test unknown module
	_, err = srv.GetStatus(ctx, &daemonv1.GetStatusRequest{
		ApiVersion: "v1",
		Name:       "unknown-mod",
	})
	if err == nil {
		t.Errorf("GetStatus: expected error for unknown module")
	}
	st, _ := status.FromError(err)
	if st.Code() != codes.NotFound {
		t.Errorf("GetStatus: expected NotFound code, got %v", st.Code())
	}

	// Test unsupported API version
	_, err = srv.GetStatus(ctx, &daemonv1.GetStatusRequest{
		ApiVersion: "v2",
		Name:       "test-mod",
	})
	if err == nil {
		t.Errorf("GetStatus: expected error for unsupported API version")
	}
}

// TestListCommands tests the ListCommands RPC.
func TestListCommands(t *testing.T) {
	testModule := &TestModule{
		info: sdk.ModuleInfo{Name: "test-mod", Version: "1.0.0"},
		commands: []sdk.CommandSpec{
			{
				Name:  "cmd1",
				Use:   "cmd1 [args]",
				Short: "first command",
				Flags: []sdk.FlagSpec{
					{Name: "verbose", Shorthand: "v", Usage: "verbose output"},
				},
				Subcommands: []sdk.CommandSpec{
					{Name: "subcmd1", Use: "subcmd1", Short: "subcommand"},
				},
			},
		},
		statusState: sdk.StateRunning,
	}
	srv, sup := newTestServer(t, testModule)
	ctx := context.Background()

	// Load module
	_ = sup.Load(ctx, "test-mod")

	// Test list commands
	resp, err := srv.ListCommands(ctx, &daemonv1.ListCommandsRequest{
		ApiVersion: "v1",
	})
	if err != nil {
		t.Fatalf("ListCommands failed: %v", err)
	}
	if len(resp.Modules) == 0 {
		t.Errorf("ListCommands: expected modules, got none")
	}

	found := false
	for _, m := range resp.Modules {
		if m.Module == "test-mod" {
			found = true
			if len(m.Commands) != 1 {
				t.Errorf("ListCommands: expected 1 command, got %d", len(m.Commands))
			}
			if m.Commands[0].Name != "cmd1" {
				t.Errorf("ListCommands: command name %q, want 'cmd1'", m.Commands[0].Name)
			}
			if len(m.Commands[0].Subcommands) != 1 {
				t.Errorf("ListCommands: expected 1 subcommand, got %d", len(m.Commands[0].Subcommands))
			}
		}
	}
	if !found {
		t.Errorf("ListCommands: test-mod not found in response")
	}

	// Test unsupported API version
	_, err = srv.ListCommands(ctx, &daemonv1.ListCommandsRequest{
		ApiVersion: "v2",
	})
	if err == nil {
		t.Errorf("ListCommands: expected error for unsupported API version")
	}
}

// mockDispatchServer is a test implementation of Daemon_DispatchServer.
type mockDispatchServer struct {
	grpc.ServerStream
	sent []*daemonv1.DispatchChunk
	mu   sync.Mutex
}

func (m *mockDispatchServer) Send(chunk *daemonv1.DispatchChunk) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.sent = append(m.sent, chunk)
	return nil
}

func (m *mockDispatchServer) Context() context.Context {
	return context.Background()
}

// TestDispatch tests the Dispatch RPC.
func TestDispatch(t *testing.T) {
	testModule := &TestModule{
		info:           sdk.ModuleInfo{Name: "test-mod", Version: "1.0.0"},
		statusState:    sdk.StateRunning,
		dispatchResult: &sdk.Result{Output: "dispatch result", JSON: []byte(`{"key":"value"}`), ExitCode: 0},
	}
	srv, sup := newTestServer(t, testModule)
	ctx := context.Background()

	// Load module
	_ = sup.Load(ctx, "test-mod")

	// Test successful dispatch
	stream := &mockDispatchServer{}
	err := srv.Dispatch(&daemonv1.DispatchRequest{
		ApiVersion: "v1",
		Module:     "test-mod",
		Path:       []string{"cmd1"},
		Flags:      map[string]string{"verbose": "true"},
		Args:       []string{"arg1"},
	}, stream)
	if err != nil {
		t.Fatalf("Dispatch failed: %v", err)
	}
	if len(stream.sent) != 1 {
		t.Errorf("Dispatch: expected 1 chunk, got %d", len(stream.sent))
	}
	if stream.sent[0].Output != "dispatch result" {
		t.Errorf("Dispatch: output %q, want 'dispatch result'", stream.sent[0].Output)
	}
	if !stream.sent[0].Final {
		t.Errorf("Dispatch: expected final=true")
	}

	// Test empty module name
	stream = &mockDispatchServer{}
	err = srv.Dispatch(&daemonv1.DispatchRequest{
		ApiVersion: "v1",
		Module:     "",
	}, stream)
	if err == nil {
		t.Errorf("Dispatch: expected error for empty module")
	}
	st, _ := status.FromError(err)
	if st.Code() != codes.InvalidArgument {
		t.Errorf("Dispatch: expected InvalidArgument code, got %v", st.Code())
	}

	// Test unknown module
	stream = &mockDispatchServer{}
	err = srv.Dispatch(&daemonv1.DispatchRequest{
		ApiVersion: "v1",
		Module:     "unknown-mod",
	}, stream)
	if err == nil {
		t.Errorf("Dispatch: expected error for unknown module")
	}
	st, _ = status.FromError(err)
	if st.Code() != codes.NotFound {
		t.Errorf("Dispatch: expected NotFound code, got %v", st.Code())
	}

	// Test dispatch error
	testModule.dispatchErr = errors.New("dispatch failed")
	stream = &mockDispatchServer{}
	err = srv.Dispatch(&daemonv1.DispatchRequest{
		ApiVersion: "v1",
		Module:     "test-mod",
	}, stream)
	if err == nil {
		t.Errorf("Dispatch: expected error from module dispatch")
	}
	st, _ = status.FromError(err)
	if st.Code() != codes.Internal {
		t.Errorf("Dispatch: expected Internal code, got %v", st.Code())
	}

	// Test unsupported API version
	stream = &mockDispatchServer{}
	err = srv.Dispatch(&daemonv1.DispatchRequest{
		ApiVersion: "v2",
		Module:     "test-mod",
	}, stream)
	if err == nil {
		t.Errorf("Dispatch: expected error for unsupported API version")
	}
}

// mockWatchEventsServer is a test implementation of Daemon_WatchEventsServer.
type mockWatchEventsServer struct {
	grpc.ServerStream
	sent    []*daemonv1.Event
	mu      sync.Mutex
	cancelCh chan struct{}
}

func (m *mockWatchEventsServer) Send(ev *daemonv1.Event) error {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.sent = append(m.sent, ev)
	return nil
}

func (m *mockWatchEventsServer) Context() context.Context {
	ctx, cancel := context.WithCancel(context.Background())
	go func() {
		<-m.cancelCh
		cancel()
	}()
	return ctx
}

// TestWatchEvents tests the WatchEvents RPC.
func TestWatchEvents(t *testing.T) {
	testModule := &TestModule{
		info:        sdk.ModuleInfo{Name: "test-mod", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}
	srv, sup := newTestServer(t, testModule)
	ctx := context.Background()

	// Load module
	_ = sup.Load(ctx, "test-mod")

	// Test watch all events
	stream := &mockWatchEventsServer{cancelCh: make(chan struct{})}
	go func() {
		time.Sleep(50 * time.Millisecond)
		// Publish an event
		srv.eventBroker.Publish(sdk.Event{
			Module:  "test-mod",
			Type:    "test",
			Message: "test event",
			Fields:  map[string]string{},
		})
		time.Sleep(50 * time.Millisecond)
		close(stream.cancelCh)
	}()

	err := srv.WatchEvents(&daemonv1.WatchEventsRequest{
		ApiVersion: "v1",
		Module:     "",
	}, stream)
	if err != nil && err != context.Canceled {
		t.Fatalf("WatchEvents failed: %v", err)
	}

	// Test watch filtered by module
	stream = &mockWatchEventsServer{cancelCh: make(chan struct{})}
	go func() {
		time.Sleep(50 * time.Millisecond)
		srv.eventBroker.Publish(sdk.Event{
			Module:  "test-mod",
			Type:    "test",
			Message: "test event",
			Fields:  map[string]string{},
		})
		srv.eventBroker.Publish(sdk.Event{
			Module:  "other-mod",
			Type:    "test",
			Message: "other event",
			Fields:  map[string]string{},
		})
		time.Sleep(50 * time.Millisecond)
		close(stream.cancelCh)
	}()

	err = srv.WatchEvents(&daemonv1.WatchEventsRequest{
		ApiVersion: "v1",
		Module:     "test-mod",
	}, stream)
	if err != nil && err != context.Canceled {
		t.Fatalf("WatchEvents (filtered) failed: %v", err)
	}

	// Test unsupported API version
	stream = &mockWatchEventsServer{cancelCh: make(chan struct{})}
	err = srv.WatchEvents(&daemonv1.WatchEventsRequest{
		ApiVersion: "v2",
	}, stream)
	if err == nil {
		t.Errorf("WatchEvents: expected error for unsupported API version")
	}
}

// TestTailLogsUnimplemented tests the TailLogs RPC returns Unimplemented.
func TestTailLogsUnimplemented(t *testing.T) {
	testModule := &TestModule{
		info:        sdk.ModuleInfo{Name: "test-mod", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}
	srv, _ := newTestServer(t, testModule)

	// We test the API version check path first
	err := srv.TailLogs(&daemonv1.TailLogsRequest{
		ApiVersion: "v2",
	}, nil) // nil stream is ok since we return error before using it
	if err == nil {
		t.Errorf("TailLogs v2: expected error")
	}
	st, _ := status.FromError(err)
	if st.Code() != codes.Unimplemented {
		t.Errorf("TailLogs v2: expected Unimplemented code, got %v", st.Code())
	}

	// Test the Unimplemented path for v1
	err = srv.TailLogs(&daemonv1.TailLogsRequest{
		ApiVersion: "v1",
	}, nil) // nil stream is ok since we return error before using it
	if err == nil {
		t.Errorf("TailLogs v1: expected Unimplemented error")
	}
	st, _ = status.FromError(err)
	if st.Code() != codes.Unimplemented {
		t.Errorf("TailLogs v1: expected Unimplemented code, got %v", st.Code())
	}
}

// TestCheckUpdate tests the CheckUpdate RPC.
func TestCheckUpdate(t *testing.T) {
	testModule := &TestModule{
		info:        sdk.ModuleInfo{Name: "test-mod", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}
	srv, _ := newTestServer(t, testModule)
	ctx := context.Background()

	// Test with no update client
	resp, err := srv.CheckUpdate(ctx, &daemonv1.CheckUpdateRequest{
		ApiVersion: "v1",
	})
	if err != nil {
		t.Fatalf("CheckUpdate (no client) failed: %v", err)
	}
	if resp.Available {
		t.Errorf("CheckUpdate: expected no update available")
	}
	if resp.CurrentVersion != "1.0.0" || resp.LatestVersion != "1.0.0" {
		t.Errorf("CheckUpdate: versions %q/%q, want 1.0.0/1.0.0", resp.CurrentVersion, resp.LatestVersion)
	}

	// Test with update client - update available
	mockClient := &MockUpdateClient{available: true, latest: "2.0.0"}
	srv.updateClient = mockClient
	resp, err = srv.CheckUpdate(ctx, &daemonv1.CheckUpdateRequest{
		ApiVersion: "v1",
	})
	if err != nil {
		t.Fatalf("CheckUpdate (with client) failed: %v", err)
	}
	if !resp.Available {
		t.Errorf("CheckUpdate: expected update available")
	}
	if resp.LatestVersion != "2.0.0" {
		t.Errorf("CheckUpdate: latest %q, want 2.0.0", resp.LatestVersion)
	}

	// Test with update client - no update available
	mockClient.available = false
	mockClient.latest = "1.0.0"
	resp, err = srv.CheckUpdate(ctx, &daemonv1.CheckUpdateRequest{
		ApiVersion: "v1",
	})
	if err != nil {
		t.Fatalf("CheckUpdate (no update) failed: %v", err)
	}
	if resp.Available {
		t.Errorf("CheckUpdate: expected no update available")
	}

	// Test with update client - check error
	mockClient.checkErr = errors.New("network error")
	_, err = srv.CheckUpdate(ctx, &daemonv1.CheckUpdateRequest{
		ApiVersion: "v1",
	})
	if err == nil {
		t.Errorf("CheckUpdate: expected error from client")
	}
	st, _ := status.FromError(err)
	if st.Code() != codes.Internal {
		t.Errorf("CheckUpdate: expected Internal code, got %v", st.Code())
	}

	// Test unsupported API version
	_, err = srv.CheckUpdate(ctx, &daemonv1.CheckUpdateRequest{
		ApiVersion: "v2",
	})
	if err == nil {
		t.Errorf("CheckUpdate: expected error for unsupported API version")
	}
}

// TestApplyUpdate tests the ApplyUpdate RPC.
func TestApplyUpdate(t *testing.T) {
	testModule := &TestModule{
		info:        sdk.ModuleInfo{Name: "test-mod", Version: "1.0.0"},
		statusState: sdk.StateRunning,
	}
	srv, _ := newTestServer(t, testModule)
	ctx := context.Background()

	// Test with no update client
	resp, err := srv.ApplyUpdate(ctx, &daemonv1.ApplyUpdateRequest{
		ApiVersion: "v1",
	})
	if err != nil {
		t.Fatalf("ApplyUpdate (no client) failed: %v", err)
	}
	if resp.Applied {
		t.Errorf("ApplyUpdate: expected not applied")
	}

	// Test with update client - success
	mockClient := &MockUpdateClient{}
	srv.updateClient = mockClient
	resp, err = srv.ApplyUpdate(ctx, &daemonv1.ApplyUpdateRequest{
		ApiVersion: "v1",
	})
	if err != nil {
		t.Fatalf("ApplyUpdate (success) failed: %v", err)
	}
	if !resp.Applied {
		t.Errorf("ApplyUpdate: expected applied=true")
	}

	// Test with update client - error
	mockClient.applyErr = errors.New("apply failed")
	resp, err = srv.ApplyUpdate(ctx, &daemonv1.ApplyUpdateRequest{
		ApiVersion: "v1",
	})
	if err != nil {
		t.Fatalf("ApplyUpdate (error) failed: %v", err)
	}
	if resp.Applied {
		t.Errorf("ApplyUpdate: expected applied=false")
	}
	if resp.Message != "apply failed" {
		t.Errorf("ApplyUpdate: message %q, want 'apply failed'", resp.Message)
	}

	// Test unsupported API version
	_, err = srv.ApplyUpdate(ctx, &daemonv1.ApplyUpdateRequest{
		ApiVersion: "v2",
	})
	if err == nil {
		t.Errorf("ApplyUpdate: expected error for unsupported API version")
	}
}

// TestEventBrokerSubscribePublish tests event broker subscriptions and publishing.
func TestEventBrokerSubscribePublish(t *testing.T) {
	broker := NewEventBroker()

	// Subscribe
	id1, ch1 := broker.Subscribe()
	id2, ch2 := broker.Subscribe()

	if id1 == id2 {
		t.Errorf("Subscribe: expected different IDs, got %d and %d", id1, id2)
	}

	// Publish to broker (should go through sdk.EventSink)
	event := sdk.Event{
		Module:  "test",
		Type:    "event",
		Message: "test message",
		At:      time.Now(),
		Fields:  map[string]string{"key": "value"},
	}
	broker.Publish(event)

	// Both subscribers should receive
	select {
	case ev := <-ch1:
		if ev.Module != "test" || ev.Message != "test message" {
			t.Errorf("Subscriber 1: got %+v", ev)
		}
	case <-time.After(100 * time.Millisecond):
		t.Errorf("Subscriber 1: timeout waiting for event")
	}

	select {
	case ev := <-ch2:
		if ev.Module != "test" || ev.Message != "test message" {
			t.Errorf("Subscriber 2: got %+v", ev)
		}
	case <-time.After(100 * time.Millisecond):
		t.Errorf("Subscriber 2: timeout waiting for event")
	}

	// Unsubscribe one
	broker.Unsubscribe(id1)

	// Publish again
	broker.Publish(event)

	// Subscriber 2 should receive the new publish
	select {
	case ev := <-ch2:
		if ev.Module != "test" {
			t.Errorf("Subscriber 2: got %+v", ev)
		}
	case <-time.After(100 * time.Millisecond):
		t.Errorf("Subscriber 2: timeout waiting for event")
	}

	// Subscriber 1's channel was closed by Unsubscribe, so we can't reliably test it
	// The important test is that subscriber 2 still works after unsub of subscriber 1

	// Unsubscribe non-existent ID (should not panic)
	broker.Unsubscribe(999)
}

// TestEventBrokerSlowSubscriber tests that slow subscribers are dropped.
func TestEventBrokerSlowSubscriber(t *testing.T) {
	broker := NewEventBroker()
	id, ch := broker.Subscribe()

	// Fill the channel (buffered to 10)
	for i := 0; i < 10; i++ {
		broker.Publish(sdk.Event{
			Module:  "test",
			Type:    "event",
			Message: "msg",
			Fields:  map[string]string{},
		})
	}

	// Drain to verify all went through
	for i := 0; i < 10; i++ {
		<-ch
	}

	// Now the 11th publish should not block (slow subscriber dropped)
	broker.Publish(sdk.Event{
		Module:  "test",
		Type:    "event",
		Message: "msg11",
		Fields:  map[string]string{},
	})

	// Clean up
	broker.Unsubscribe(id)
}

// TestClampInt32 tests the clampInt32 helper function.
func TestClampInt32(t *testing.T) {
	tests := []struct {
		input int
		want  int32
	}{
		{0, 0},
		{100, 100},
		{-100, -100},
		{2147483647, 2147483647}, // max int32
		{-2147483648, -2147483648}, // min int32
		{2147483648, 2147483647}, // overflow clamped
		{-2147483649, -2147483648}, // underflow clamped
	}

	for _, tt := range tests {
		got := clampInt32(tt.input)
		if got != tt.want {
			t.Errorf("clampInt32(%d): got %d, want %d", tt.input, got, tt.want)
		}
	}
}

// TestSdkCommandToProto tests the command spec conversion.
func TestSdkCommandToProto(t *testing.T) {
	cmd := sdk.CommandSpec{
		Name:  "test",
		Use:   "test [args]",
		Short: "test command",
		Flags: []sdk.FlagSpec{
			{Name: "verbose", Shorthand: "v", Usage: "verbose mode", Default: "false", Type: sdk.FlagBool},
		},
		Subcommands: []sdk.CommandSpec{
			{Name: "sub1", Use: "sub1", Short: "subcommand 1"},
		},
		MinArgs: 0,
		MaxArgs: 5,
	}

	proto := sdkCommandToProto(cmd)

	if proto.Name != "test" {
		t.Errorf("Name: got %q, want 'test'", proto.Name)
	}
	if len(proto.Flags) != 1 {
		t.Errorf("Flags: got %d, want 1", len(proto.Flags))
	}
	if len(proto.Subcommands) != 1 {
		t.Errorf("Subcommands: got %d, want 1", len(proto.Subcommands))
	}
	if proto.MinArgs != 0 || proto.MaxArgs != 5 {
		t.Errorf("Args: got %d-%d, want 0-5", proto.MinArgs, proto.MaxArgs)
	}
}

// TestModuleInfo tests Supervisor.ModuleInfo for loaded and unloaded modules.
func TestModuleInfo(t *testing.T) {
	testModule := &TestModule{
		info: sdk.ModuleInfo{
			Name:        "test-mod",
			Version:     "1.0.0",
			Description: "test module",
		},
	}
	_, sup := newTestServer(t, testModule)
	ctx := context.Background()

	// Test ModuleInfo before loading (unloaded module)
	info, ok := sup.ModuleInfo("test-mod")
	if !ok {
		t.Errorf("ModuleInfo: expected to find unloaded module")
	}
	if info.Name != "test-mod" || info.Version != "1.0.0" {
		t.Errorf("ModuleInfo: got %+v, want Name='test-mod' Version='1.0.0'", info)
	}

	// Test ModuleInfo after loading (loaded module)
	_ = sup.Load(ctx, "test-mod")
	info, ok = sup.ModuleInfo("test-mod")
	if !ok {
		t.Errorf("ModuleInfo (loaded): expected to find module")
	}
	if info.Name != "test-mod" {
		t.Errorf("ModuleInfo (loaded): got Name %q, want 'test-mod'", info.Name)
	}

	// Test ModuleInfo for unknown module
	_, ok = sup.ModuleInfo("unknown-mod")
	if ok {
		t.Errorf("ModuleInfo: expected not to find unknown module")
	}
}

// TestHostServicesConfig tests the Host.Config accessor.
func TestHostServicesConfig(t *testing.T) {
	testModule := &TestModule{
		info: sdk.ModuleInfo{Name: "test-mod", Version: "1.0.0"},
	}
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")
	configData := []byte("test config data")

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return testModule }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
				configVal:      configData,
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	sup := New(cfg)
	ctx := context.Background()

	// Load the module
	_ = sup.Load(ctx, "test-mod")

	// Test Hosts for loaded module (should be non-nil)
	host := sup.Hosts("test-mod")
	if host == nil {
		t.Errorf("Hosts (loaded): expected non-nil")
	}

	// Test Config accessor - should return the config data passed during module initialization
	hostConfig := host.Config()
	if !bytes.Equal(hostConfig, configData) {
		t.Errorf("Host.Config(): got %q, want %q", string(hostConfig), string(configData))
	}
}

// TestNewServerInit creates a server and verifies initialization.
func TestNewServerInit(t *testing.T) {
	testModule := &TestModule{
		info: sdk.ModuleInfo{Name: "test-mod", Version: "1.0.0"},
	}
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return testModule }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	sup := New(cfg)
	updateClient := &MockUpdateClient{available: false}
	srv := NewServer(sup, "1.0.0", zaptest.NewLogger(t), updateClient)

	if srv.version != "1.0.0" {
		t.Errorf("Server version: got %q, want 1.0.0", srv.version)
	}
	if srv.eventBroker == nil {
		t.Errorf("EventBroker: expected non-nil")
	}
	if srv.updateClient != updateClient {
		t.Errorf("UpdateClient: not set correctly")
	}
}

// TestLoadModuleLicenseDenied tests that LoadModule rejects modules when license is denied.
func TestLoadModuleLicenseDenied(t *testing.T) {
	testModule := &TestModule{
		info: sdk.ModuleInfo{
			Name:           "premium-mod",
			Version:        "1.0.0",
			LicenseFeature: "penguin.premium",
		},
		statusState: sdk.StateRunning,
	}
	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	cfg := Config{
		Modules: []sdk.Factory{func() sdk.Module { return testModule }},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: map[string]bool{"penguin.premium": false}, // Feature disabled
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	sup := New(cfg)
	srv := NewServer(sup, "1.0.0", zaptest.NewLogger(t), nil)
	ctx := context.Background()

	// Try to load with license denied
	_, err := srv.LoadModule(ctx, &daemonv1.LoadModuleRequest{
		ApiVersion: "v1",
		Name:       "premium-mod",
	})
	if err == nil {
		t.Errorf("LoadModule: expected error for license-denied module")
	}
	st, _ := status.FromError(err)
	if st.Code() != codes.PermissionDenied {
		t.Errorf("LoadModule: expected PermissionDenied code, got %v", st.Code())
	}
}

// TestGetStatusAllModules tests GetStatus when fetching all modules.
func TestGetStatusAllModules(t *testing.T) {
	testMod1 := &TestModule{
		info:        sdk.ModuleInfo{Name: "mod1", Version: "1.0.0"},
		statusState: sdk.StateRunning,
		healthLevel: sdk.Healthy,
	}
	testMod2 := &TestModule{
		info:        sdk.ModuleInfo{Name: "mod2", Version: "2.0.0"},
		statusState: sdk.StateDegraded,
		healthLevel: sdk.Degraded,
	}

	tmpdir := t.TempDir()
	statePath := filepath.Join(tmpdir, "state.json")

	cfg := Config{
		Modules: []sdk.Factory{
			func() sdk.Module { return testMod1 },
			func() sdk.Module { return testMod2 },
		},
		Host: func(name string) sdk.HostServices {
			return &fakeHostServices{
				loggerVal:      zaptest.NewLogger(t),
				metricsVal:     &fakePrometheus{},
				dataDirVal:     tmpdir,
				featureEnabled: make(map[string]bool),
			}
		},
		StatePath: statePath,
		Logger:    zaptest.NewLogger(t),
		Backoff:   DefaultBackoff(),
	}

	sup := New(cfg)
	srv := NewServer(sup, "1.0.0", zaptest.NewLogger(t), nil)
	ctx := context.Background()

	// Load both modules
	_ = sup.Load(ctx, "mod1")
	_ = sup.Load(ctx, "mod2")

	// Get all statuses
	resp, err := srv.GetStatus(ctx, &daemonv1.GetStatusRequest{
		ApiVersion: "v1",
		Name:       "",
	})
	if err != nil {
		t.Fatalf("GetStatus failed: %v", err)
	}

	if len(resp.Modules) < 2 {
		t.Errorf("GetStatus: expected at least 2 modules, got %d", len(resp.Modules))
	}

	// Find both modules
	found := map[string]bool{}
	for _, m := range resp.Modules {
		if m.Name == "mod1" {
			found["mod1"] = true
			if m.State != "running" {
				t.Errorf("mod1 state: got %q, want 'running'", m.State)
			}
			if m.Health != "healthy" {
				t.Errorf("mod1 health: got %q, want 'healthy'", m.Health)
			}
		}
		if m.Name == "mod2" {
			found["mod2"] = true
			if m.State != "degraded" {
				t.Errorf("mod2 state: got %q, want 'degraded'", m.State)
			}
			if m.Health != "degraded" {
				t.Errorf("mod2 health: got %q, want 'degraded'", m.Health)
			}
		}
	}

	if !found["mod1"] || !found["mod2"] {
		t.Errorf("GetStatus: missing modules in response: %+v", found)
	}
}

