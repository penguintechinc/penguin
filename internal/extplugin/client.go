package extplugin

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"time"

	"github.com/hashicorp/go-plugin"
	sdkv1 "github.com/penguintechinc/penguin/pkg/sdk/proto/penguin/sdk/v1"
	"github.com/penguintechinc/penguin/pkg/sdk"
)

// Client is deprecated and not used. Plugin management is handled via LoadWithVerifier.
type Client struct {
}

// Load verifies a plugin binary and launches it as an external process.
// The plugin's Module is returned wrapped in an adapter that implements
// the same sdk.Module interface.
//
// Load requires that the plugin directory contain:
// - plugin.json (manifest)
// - <binary> (the plugin executable)
// - <binary>.minisig (minisign signature)
//
// All files must pass verification (ownership, permissions, SHA256, signature).
func Load(ctx context.Context, pluginDir string, host sdk.HostServices) (sdk.Module, error) {
	verifier := NewVerifier()
	return LoadWithVerifier(ctx, pluginDir, host, verifier)
}

// LoadWithVerifier is like Load but allows injecting a custom verifier (for testing).
func LoadWithVerifier(ctx context.Context, pluginDir string, host sdk.HostServices, verifier *Verifier) (sdk.Module, error) {
	// Load and parse the manifest.
	manifest, err := LoadManifest(pluginDir)
	if err != nil {
		return nil, fmt.Errorf("load manifest: %w", err)
	}

	// Verify the plugin.
	if err := verifier.Verify(pluginDir, manifest); err != nil {
		return nil, fmt.Errorf("verify plugin: %w", err)
	}

	// Launch the plugin subprocess using go-plugin.
	binaryPath := manifest.BinaryPath(pluginDir)

	pluginClient := plugin.NewClient(&plugin.ClientConfig{
		HandshakeConfig: plugin.HandshakeConfig{
			ProtocolVersion:  sdk.PluginProtocolVersion,
			MagicCookieKey:   sdk.PluginHandshakeConfigMagicCookieKey,
			MagicCookieValue: sdk.PluginHandshakeConfigMagicCookieValue,
		},
		Plugins: map[string]plugin.Plugin{
			"module": &clientModulePlugin{hostServices: host},
		},
		Cmd:              exec.Command(binaryPath), // #nosec G204 -- binary path is signature-verified and ownership-checked by verifier.Verify before use
		AutoMTLS:         true,
		AllowedProtocols: []plugin.Protocol{plugin.ProtocolGRPC},
	})

	// Connect to the plugin.
	rpcClient, err := pluginClient.Client()
	if err != nil {
		return nil, fmt.Errorf("connect to plugin: %w", err)
	}

	// Get the module interface from the plugin.
	raw, err := rpcClient.Dispense("module")
	if err != nil {
		pluginClient.Kill()
		return nil, fmt.Errorf("dispense module: %w", err)
	}

	m, ok := raw.(sdk.Module)
	if !ok {
		pluginClient.Kill()
		return nil, fmt.Errorf("plugin did not return a Module")
	}

	// Wrap the module in an adapter that handles plugin lifecycle.
	return &moduleWrapper{
		mod:    m,
		client: pluginClient,
	}, nil
}

// Discover scans a plugins directory for valid plugin manifests.
// Returns a list of manifests and skips any directories that are world-writable
// (logging a note but not failing).
func Discover(pluginsDir string) ([]Manifest, error) {
	entries, err := os.ReadDir(pluginsDir)
	if err != nil {
		return nil, fmt.Errorf("read plugins dir: %w", err)
	}

	var manifests []Manifest
	verifier := NewVerifier()

	for _, entry := range entries {
		if !entry.IsDir() {
			continue
		}

		pluginDir := filepath.Join(pluginsDir, entry.Name())

		// Skip world-writable directories (potential tamper).
		if info, err := os.Stat(pluginDir); err == nil && info.Mode()&0o002 != 0 {
			// Log a note (would use logger in real code).
			fmt.Fprintf(os.Stderr, "skipping world-writable plugin dir: %s\n", pluginDir)
			continue
		}

		// Load the manifest without verifying the binary yet.
		m, err := LoadManifest(pluginDir)
		if err != nil {
			// Skip malformed manifests (log and continue).
			fmt.Fprintf(os.Stderr, "skipping plugin %s: %v\n", entry.Name(), err)
			continue
		}

		// Verify the binary to ensure it's valid before listing it.
		if err := verifier.Verify(pluginDir, m); err != nil {
			fmt.Fprintf(os.Stderr, "skipping plugin %s: verification failed: %v\n", entry.Name(), err)
			continue
		}

		manifests = append(manifests, *m)
	}

	return manifests, nil
}

// moduleClientAdapter adapts the gRPC ModuleServiceClient to sdk.Module.
type moduleClientAdapter struct {
	grpcClient sdkv1.ModuleServiceClient
	broker     *plugin.GRPCBroker
}

func (a *moduleClientAdapter) Info() sdk.ModuleInfo {
	resp, err := a.grpcClient.Info(context.Background(), &sdkv1.InfoRequest{ApiVersion: "v1"})
	if err != nil {
		return sdk.ModuleInfo{}
	}
	return sdk.ModuleInfo{
		Name:           resp.Name,
		Version:        resp.Version,
		Description:    resp.Description,
		LicenseFeature: resp.LicenseFeature,
	}
}

func (a *moduleClientAdapter) Init(ctx context.Context, host sdk.HostServices) error {
	// Init is already called during GRPCClient, so this is a no-op.
	// (The plugin's GRPCClient wraps the module and calls Init.)
	return nil
}

func (a *moduleClientAdapter) Start(ctx context.Context) error {
	resp, err := a.grpcClient.Start(ctx, &sdkv1.StartRequest{ApiVersion: "v1"})
	if err != nil {
		return fmt.Errorf("start: %w", err)
	}
	if resp.Error != "" {
		return fmt.Errorf("start: %s", resp.Error)
	}
	return nil
}

func (a *moduleClientAdapter) Stop(ctx context.Context) error {
	resp, err := a.grpcClient.Stop(ctx, &sdkv1.StopRequest{ApiVersion: "v1"})
	if err != nil {
		return fmt.Errorf("stop: %w", err)
	}
	if resp.Error != "" {
		return fmt.Errorf("stop: %s", resp.Error)
	}
	return nil
}

func (a *moduleClientAdapter) Status(ctx context.Context) (sdk.Status, error) {
	resp, err := a.grpcClient.Status(ctx, &sdkv1.StatusRequest{ApiVersion: "v1"})
	if err != nil {
		return sdk.Status{}, fmt.Errorf("status: %w", err)
	}
	if resp.Error != "" {
		return sdk.Status{}, fmt.Errorf("status: %s", resp.Error)
	}
	return sdk.Status{
		State:  sdk.ModuleState(resp.State),
		Detail: resp.Detail,
	}, nil
}

func (a *moduleClientAdapter) Health(ctx context.Context) sdk.HealthReport {
	resp, err := a.grpcClient.Health(ctx, &sdkv1.HealthRequest{ApiVersion: "v1"})
	if err != nil {
		return sdk.HealthReport{Level: sdk.Unhealthy, Message: err.Error()}
	}
	return sdk.HealthReport{
		Level:     sdk.HealthLevel(resp.Level),
		Message:   resp.Message,
		CheckedAt: timeFromUnixNano(resp.CheckedAtUnixNano),
	}
}

func (a *moduleClientAdapter) Commands() []sdk.CommandSpec {
	resp, err := a.grpcClient.Commands(context.Background(), &sdkv1.CommandsRequest{ApiVersion: "v1"})
	if err != nil {
		return nil
	}
	specs := make([]sdk.CommandSpec, len(resp.Commands))
	for i, pbSpec := range resp.Commands {
		specs[i] = protoCommandSpecToSDK(pbSpec)
	}
	return specs
}

func (a *moduleClientAdapter) Dispatch(ctx context.Context, path []string, flags map[string]string, args []string) (*sdk.Result, error) {
	resp, err := a.grpcClient.Dispatch(ctx, &sdkv1.DispatchRequest{
		ApiVersion: "v1",
		Path:       path,
		Flags:      flags,
		Args:       args,
	})
	if err != nil {
		return nil, fmt.Errorf("dispatch: %w", err)
	}
	if resp.Error != "" {
		return nil, fmt.Errorf("dispatch: %s", resp.Error)
	}
	return &sdk.Result{
		Output:   resp.Output,
		JSON:     resp.Json,
		ExitCode: int(resp.ExitCode),
	}, nil
}

func (a *moduleClientAdapter) ConfigSchema() []byte {
	resp, err := a.grpcClient.ConfigSchema(context.Background(), &sdkv1.ConfigSchemaRequest{ApiVersion: "v1"})
	if err != nil {
		return nil
	}
	return resp.Schema
}

// moduleWrapper manages the plugin subprocess lifecycle.
type moduleWrapper struct {
	mod    sdk.Module
	client *plugin.Client
}

func (w *moduleWrapper) Info() sdk.ModuleInfo              { return w.mod.Info() }
func (w *moduleWrapper) Init(ctx context.Context, host sdk.HostServices) error {
	return w.mod.Init(ctx, host)
}
func (w *moduleWrapper) Start(ctx context.Context) error      { return w.mod.Start(ctx) }
func (w *moduleWrapper) Stop(ctx context.Context) error       { return w.mod.Stop(ctx) }
func (w *moduleWrapper) Status(ctx context.Context) (sdk.Status, error) {
	return w.mod.Status(ctx)
}
func (w *moduleWrapper) Health(ctx context.Context) sdk.HealthReport {
	return w.mod.Health(ctx)
}
func (w *moduleWrapper) Commands() []sdk.CommandSpec { return w.mod.Commands() }
func (w *moduleWrapper) Dispatch(ctx context.Context, path []string, flags map[string]string, args []string) (*sdk.Result, error) {
	return w.mod.Dispatch(ctx, path, flags, args)
}
func (w *moduleWrapper) ConfigSchema() []byte { return w.mod.ConfigSchema() }

// Helper to convert proto CommandSpec to SDK.
func protoCommandSpecToSDK(pbSpec *sdkv1.CommandSpec) sdk.CommandSpec {
	flags := make([]sdk.FlagSpec, len(pbSpec.Flags))
	for i, pbFlag := range pbSpec.Flags {
		flags[i] = sdk.FlagSpec{
			Name:      pbFlag.Name,
			Shorthand: pbFlag.Shorthand,
			Usage:     pbFlag.Usage,
			Default:   pbFlag.Default,
			Type:      sdk.FlagType(pbFlag.Type),
		}
	}

	subcmds := make([]sdk.CommandSpec, len(pbSpec.Subcommands))
	for i, pbSub := range pbSpec.Subcommands {
		subcmds[i] = protoCommandSpecToSDK(pbSub)
	}

	return sdk.CommandSpec{
		Name:         pbSpec.Name,
		Use:          pbSpec.Use,
		Short:        pbSpec.Short,
		Flags:        flags,
		Subcommands:  subcmds,
		Tray:         pbSpec.Tray,
		MinArgs:      int(pbSpec.MinArgs),
		MaxArgs:      int(pbSpec.MaxArgs),
	}
}

// Helper to convert unix-nanos to time.Time.
func timeFromUnixNano(nanos int64) time.Time {
	return time.Unix(0, nanos)
}
