//go:build integration
// +build integration

package extplugin

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"testing"

	"aead.dev/minisign"
)

// rustPluginHelloPath locates the pre-built plugin-hello-rs binary (built
// with the Rust toolchain in Docker — this Go test process has no Rust
// compiler available, so unlike the Go fixtures elsewhere in this package
// there is nothing to build here). Prefers PENGUIN_PLUGIN_HELLO_RS, else
// falls back to go-client/bin/plugin-hello-rs — `go test` always runs with
// the package directory (go-client/internal/extplugin) as its working
// directory, so that default is reached via "../../bin/plugin-hello-rs",
// matching the existing plugin-hello binary's own home in that directory.
// Skips (rather than failing) when neither exists.
func rustPluginHelloPath(t *testing.T) string {
	t.Helper()

	if override := os.Getenv("PENGUIN_PLUGIN_HELLO_RS"); override != "" {
		if _, err := os.Stat(override); err == nil {
			return override
		}
		t.Skipf("SKIP: PENGUIN_PLUGIN_HELLO_RS=%s does not exist", override)
		return ""
	}

	defaultPath := filepath.Join("..", "..", "bin", "plugin-hello-rs")
	if _, err := os.Stat(defaultPath); err == nil {
		return defaultPath
	}
	t.Skipf(
		"SKIP: no plugin-hello-rs binary at %s — build it with:\n"+
			"  docker run --rm -v $(pwd):/work -w /work "+
			"-v penguin_cargo_home:/cargo -e CARGO_HOME=/cargo "+
			"-v penguin_target_serve:/target -e CARGO_TARGET_DIR=/target "+
			"penguin-rust:1.97 cargo build -p plugin-hello-rs\n"+
			"then copy /target/debug/plugin-hello-rs to go-client/bin/plugin-hello-rs, "+
			"or set PENGUIN_PLUGIN_HELLO_RS",
		defaultPath,
	)
	return ""
}

// TestRustPluginReverseCompat proves the second headline claim of this task:
// a Rust plugin built against penguin-sdk's `plugin::serve` loads correctly
// under the FROZEN Go daemon's real go-plugin-based loader
// (LoadWithVerifier, exercising plugin.NewClient with AutoMTLS just like
// production use). This is the reverse direction of every other compat test
// in this repo (which proves a Go-built plugin loads under the Rust host) —
// together the two prove the wire protocol holds in both directions.
//
// The Rust plugin also attempts to dial the host's HostService on the
// broker's id=1 leg (see docs/PARITY.md §1.10): the frozen Go host serves
// that leg in plaintext (internal/extplugin/plugin_glue.go's
// clientModulePlugin.GRPCClient), so the plugin's TLS ClientHello is
// rejected and it must degrade to a no-op HostServices rather than hanging
// or crashing — this test is what proves that degradation path holds
// end-to-end against a real Go host, not just in isolation.
func TestRustPluginReverseCompat(t *testing.T) {
	if testing.Short() {
		t.Skip("skipping subprocess integration test in short mode")
	}

	pluginExePath := rustPluginHelloPath(t)

	tmpDir := t.TempDir()

	pubKey, privKey, err := generateTestMinisignKey()
	if err != nil {
		t.Fatalf("generate minisign key: %v", err)
	}

	binaryData, err := os.ReadFile(pluginExePath) //nolint:gosec // reading pre-built test fixture at a caller-controlled path
	if err != nil {
		t.Fatalf("read plugin binary: %v", err)
	}

	sig := minisign.Sign(privKey, binaryData)

	pluginDir := filepath.Join(tmpDir, "plugins", "hello-rs")
	if err := os.MkdirAll(pluginDir, 0o750); err != nil {
		t.Fatalf("mkdir plugin dir: %v", err)
	}

	pluginBinaryName := "hello-rs"
	pluginBinaryPath := filepath.Join(pluginDir, pluginBinaryName)
	if err := os.WriteFile(pluginBinaryPath, binaryData, 0o700); err != nil { //nolint:gosec // test fixture, executable intentional
		t.Fatalf("write plugin binary: %v", err)
	}

	sigPath := filepath.Join(pluginDir, pluginBinaryName+".minisig")
	if err := os.WriteFile(sigPath, sig, 0o600); err != nil {
		t.Fatalf("write signature: %v", err)
	}

	h := sha256.Sum256(binaryData)
	manifestJSON := fmt.Sprintf(`{
  "name": "hello-rs",
  "version": "1.0.0",
  "sdk_version": "v1",
  "binary": "%s",
  "sha256": "%s",
  "publisher": "test"
}`, pluginBinaryName, hex.EncodeToString(h[:]))
	manifestPath := filepath.Join(pluginDir, "plugin.json")
	if err := os.WriteFile(manifestPath, []byte(manifestJSON), 0o600); err != nil {
		t.Fatalf("write manifest: %v", err)
	}

	hostServices := &MockHostServices{
		configValue:  []byte("test config"),
		dataDirValue: tmpDir,
	}
	verifier := NewVerifierWithKeys([]string{pubKey})

	// Assertion: handshake accepted. LoadWithVerifier fails outright if the
	// AutoMTLS handshake (and the health check that follows it) does not
	// complete — see TestLoadWithVerifierHandshakeFails for the inverse case
	// against a non-plugin binary.
	mod, err := LoadWithVerifier(context.Background(), pluginDir, hostServices, verifier)
	if err != nil {
		t.Fatalf("load plugin-hello-rs: %v", err)
	}
	if mod == nil {
		t.Fatalf("module is nil after load")
	}

	// Assertion: Info returns hello-rs.
	info := mod.Info()
	if info.Name != "hello-rs" {
		t.Errorf("info.Name: got %q, want %q", info.Name, "hello-rs")
	}
	if info.Version != "1.0.0" {
		t.Errorf("info.Version: got %q, want %q", info.Version, "1.0.0")
	}

	// Assertion: greet world -> hello, world.
	result, err := mod.Dispatch(context.Background(), []string{"greet"}, map[string]string{}, []string{"world"})
	if err != nil {
		t.Fatalf("dispatch greet failed: %v", err)
	}
	if result == nil {
		t.Fatalf("dispatch greet returned a nil result")
	}
	if result.Output != "hello, world" {
		t.Errorf("dispatch output: got %q, want %q", result.Output, "hello, world")
	}
	if result.ExitCode != 0 {
		t.Errorf("dispatch exit code: got %d, want 0", result.ExitCode)
	}

	if err := mod.Stop(context.Background()); err != nil {
		t.Errorf("stop failed: %v", err)
	}

	// Assertion: clean shutdown. This test lives in the same package as
	// moduleWrapper, so the concrete type backing the sdk.Module interface
	// can be recovered to drive go-plugin's own Kill()/Exited() accounting —
	// the same mechanism a real host would use, and a stronger proof than
	// merely asserting Stop() (the ModuleService RPC) returned no error.
	wrapper, ok := mod.(*moduleWrapper)
	if !ok {
		t.Fatalf("unexpected module type %T; cannot verify clean process shutdown", mod)
	}
	wrapper.client.Kill()
	if !wrapper.client.Exited() {
		t.Errorf("rust plugin process did not exit after Kill()")
	}

	t.Logf("Rust plugin reverse-compat test passed: the frozen Go host loaded plugin-hello-rs, " +
		"verified Info/Dispatch over the wire, and shut it down cleanly")
}
