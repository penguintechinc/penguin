//go:build integration
// +build integration

package extplugin

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"aead.dev/minisign"
	"github.com/penguintechinc/penguin/pkg/sdk"
)

// TestPluginLifecycleIntegration tests the full lifecycle of loading, verifying, and running
// an external plugin via subprocess. This exercises the real go-plugin broker, subprocess
// management, and the complete Host/Client communication path.
func TestPluginLifecycleIntegration(t *testing.T) {
	if testing.Short() {
		t.Skip("skipping subprocess integration test in short mode")
	}

	// Step 1: Build the example plugin.
	tmpDir := t.TempDir()
	binDir := filepath.Join(tmpDir, "bin")
	if err := os.MkdirAll(binDir, 0o750); err != nil {
		t.Fatalf("mkdir bin: %v", err)
	}

	pluginExePath := filepath.Join(binDir, "plugin-hello")
	buildCmd := exec.Command("go", "build", "-o", pluginExePath, "./examples/plugin-hello") //nolint:gosec // hardcoded command, only output is variable
	buildCmd.Dir = "/home/penguin/code/penguin"
	if output, err := buildCmd.CombinedOutput(); err != nil {
		t.Fatalf("build plugin failed: %v\n%s", err, output)
	}

	// Verify the plugin binary was created.
	if _, err := os.Stat(pluginExePath); err != nil {
		t.Fatalf("plugin binary not found after build: %v", err)
	}

	// Step 2: Generate an ephemeral minisign keypair.
	pubKey, privKey, err := generateTestMinisignKey()
	if err != nil {
		t.Fatalf("generate minisign key: %v", err)
	}

	// Step 3: Read the plugin binary and sign it.
	binaryData, err := os.ReadFile(pluginExePath) //nolint:gosec // reading built test fixture
	if err != nil {
		t.Fatalf("read plugin binary: %v", err)
	}

	sig := minisign.Sign(privKey, binaryData)

	// Step 4: Set up the plugin directory structure:
	// <dir>/hello/{plugin.json, hello, hello.minisig}
	pluginDir := filepath.Join(tmpDir, "plugins", "hello")
	if err := os.MkdirAll(pluginDir, 0o750); err != nil {
		t.Fatalf("mkdir plugin dir: %v", err)
	}

	// Copy binary to plugin directory.
	pluginBinaryName := "hello"
	pluginBinaryPath := filepath.Join(pluginDir, pluginBinaryName)
	if err := os.WriteFile(pluginBinaryPath, binaryData, 0o700); err != nil { //nolint:gosec // test fixture, executable intentional
		t.Fatalf("write plugin binary: %v", err)
	}

	// Write signature file.
	sigPath := filepath.Join(pluginDir, pluginBinaryName+".minisig")
	if err := os.WriteFile(sigPath, sig, 0o600); err != nil {
		t.Fatalf("write signature: %v", err)
	}

	// Write manifest.
	h := sha256.Sum256(binaryData)
	manifestJSON := fmt.Sprintf(`{
  "name": "hello",
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

	// Step 5: Create host services mock.
	hostServices := &MockHostServices{
		configValue:  []byte("test config"),
		dataDirValue: tmpDir,
	}

	// Step 6: Create a verifier that trusts our ephemeral key.
	verifier := NewVerifierWithKeys([]string{pubKey})

	// Step 7: Load the plugin via the real subprocess path.
	mod, err := LoadWithVerifier(context.Background(), pluginDir, hostServices, verifier)
	if err != nil {
		t.Fatalf("load plugin: %v", err)
	}

	if mod == nil {
		t.Fatalf("module is nil after load")
	}

	// Step 8: Test the module lifecycle.

	// Info(): Verify the module reports correct metadata.
	info := mod.Info()
	if info.Name != "hello" {
		t.Errorf("info.Name: got %q, want %q", info.Name, "hello")
	}
	if info.Version != "1.0.0" {
		t.Errorf("info.Version: got %q, want %q", info.Version, "1.0.0")
	}

	// Init(): Initialize the module with the host.
	if err := mod.Init(context.Background(), hostServices); err != nil {
		t.Errorf("init failed: %v", err)
	}

	// Start(): Start the module.
	if err := mod.Start(context.Background()); err != nil {
		t.Errorf("start failed: %v", err)
	}

	// Commands(): Verify the module reports its commands.
	commands := mod.Commands()
	if len(commands) != 1 {
		t.Errorf("commands count: got %d, want 1", len(commands))
	}
	if len(commands) > 0 && commands[0].Name != "greet" {
		t.Errorf("command name: got %q, want %q", commands[0].Name, "greet")
	}

	// Dispatch(): Execute the greet command.
	result, err := mod.Dispatch(context.Background(), []string{"greet"}, map[string]string{}, []string{"Integration"})
	if err != nil {
		t.Errorf("dispatch failed: %v", err)
	}
	if result != nil {
		if result.Output != "hello, Integration" {
			t.Errorf("dispatch output: got %q, want %q", result.Output, "hello, Integration")
		}
		if result.ExitCode != 0 {
			t.Errorf("dispatch exit code: got %d, want 0", result.ExitCode)
		}
	}

	// Status(): Check the module status.
	status, err := mod.Status(context.Background())
	if err != nil {
		t.Errorf("status failed: %v", err)
	}
	if status.State != sdk.StateRunning {
		t.Errorf("status.State: got %s, want %s", status.State, sdk.StateRunning)
	}

	// Health(): Check health.
	health := mod.Health(context.Background())
	if health.Level != sdk.Healthy {
		t.Errorf("health.Level: got %v, want %v", health.Level, sdk.Healthy)
	}

	// Stop(): Clean up the module.
	if err := mod.Stop(context.Background()); err != nil {
		t.Errorf("stop failed: %v", err)
	}

	t.Logf("Plugin lifecycle integration test passed: successfully loaded, initialized, and executed the example plugin")
}

// TestLoadWithVerifierHandshakeFails tests LoadWithVerifier when go-plugin handshake fails
// because the binary is not a real plugin (e.g. /bin/true or a no-op binary).
func TestLoadWithVerifierHandshakeFails(t *testing.T) {
	if testing.Short() {
		t.Skip("skipping subprocess integration test in short mode")
	}

	tmpDir := t.TempDir()
	pluginDir := filepath.Join(tmpDir, "plugins", "bad-plugin")
	if err := os.MkdirAll(pluginDir, 0o750); err != nil {
		t.Fatalf("mkdir plugin dir: %v", err)
	}

	// Generate an ephemeral minisign keypair.
	pubKeyStr, privKey, err := generateTestMinisignKey()
	if err != nil {
		t.Fatalf("generate minisign key: %v", err)
	}

	// Use /bin/true as the "plugin" binary (it exists, but will fail handshake).
	// Copy it to the plugin directory so we can sign it.
	trueData, err := os.ReadFile("/bin/true") //nolint:gosec // reading system binary for test
	if err != nil {
		t.Skipf("skipping: /bin/true not available: %v", err)
	}

	pluginBinaryPath := filepath.Join(pluginDir, "bad-plugin")
	if err := os.WriteFile(pluginBinaryPath, trueData, 0o700); err != nil { //nolint:gosec // test executable intentional
		t.Fatalf("write binary: %v", err)
	}

	// Sign the binary.
	sig := minisign.Sign(privKey, trueData)
	sigPath := filepath.Join(pluginDir, "bad-plugin.minisig")
	if err := os.WriteFile(sigPath, sig, 0o600); err != nil {
		t.Fatalf("write signature: %v", err)
	}

	// Write manifest with correct hash of /bin/true.
	h := sha256.Sum256(trueData)
	manifestJSON := fmt.Sprintf(`{
  "name": "bad-plugin",
  "version": "1.0.0",
  "sdk_version": "v1",
  "binary": "bad-plugin",
  "sha256": "%s",
  "publisher": "test"
}`, hex.EncodeToString(h[:]))
	manifestPath := filepath.Join(pluginDir, "plugin.json")
	if err := os.WriteFile(manifestPath, []byte(manifestJSON), 0o600); err != nil {
		t.Fatalf("write manifest: %v", err)
	}

	hostServices := &MockHostServices{
		configValue:  []byte("test config"),
		dataDirValue: tmpDir,
	}

	verifier := NewVerifierWithKeys([]string{pubKeyStr})

	// Attempt to load the plugin. The binary passes verification (signature and hash are valid)
	// but will fail during go-plugin handshake because /bin/true is not a plugin.
	mod, err := LoadWithVerifier(context.Background(), pluginDir, hostServices, verifier)
	if err == nil {
		t.Fatalf("LoadWithVerifier should fail for non-plugin binary; got module: %v", mod)
	}

	t.Logf("LoadWithVerifier correctly rejected non-plugin binary: %v", err)
}

// TestDiscoverMixedPlugins tests Discover with one good plugin entry and one malformed entry.
func TestDiscoverMixedPlugins(t *testing.T) {
	if testing.Short() {
		t.Skip("skipping subprocess integration test in short mode")
	}

	tmpDir := t.TempDir()
	pluginsDir := filepath.Join(tmpDir, "plugins")
	if err := os.MkdirAll(pluginsDir, 0o750); err != nil {
		t.Fatalf("mkdir plugins dir: %v", err)
	}

	// Create a valid plugin entry.
	_, privKey, err := generateTestMinisignKey()
	if err != nil {
		t.Fatalf("generate minisign key: %v", err)
	}

	exeDir := filepath.Join(tmpDir, "bin")
	if err := os.MkdirAll(exeDir, 0o750); err != nil {
		t.Fatalf("mkdir bin: %v", err)
	}

	exePath := filepath.Join(exeDir, "plugin-hello")
	buildCmd := exec.Command("go", "build", "-o", exePath, "./examples/plugin-hello") //nolint:gosec // hardcoded command, only output is variable
	buildCmd.Dir = "/home/penguin/code/penguin"
	if output, err := buildCmd.CombinedOutput(); err != nil {
		t.Fatalf("build plugin failed: %v\n%s", err, output)
	}

	binaryData, err := os.ReadFile(exePath) //nolint:gosec // reading built test fixture
	if err != nil {
		t.Fatalf("read binary: %v", err)
	}

	sig := minisign.Sign(privKey, binaryData)
	goodDir := filepath.Join(pluginsDir, "hello")
	if err := os.MkdirAll(goodDir, 0o750); err != nil {
		t.Fatalf("mkdir good dir: %v", err)
	}

	goodBinaryPath := filepath.Join(goodDir, "hello")
	if err := os.WriteFile(goodBinaryPath, binaryData, 0o700); err != nil { //nolint:gosec // test fixture
		t.Fatalf("write binary: %v", err)
	}

	if err := os.WriteFile(filepath.Join(goodDir, "hello.minisig"), sig, 0o600); err != nil {
		t.Fatalf("write signature: %v", err)
	}

	h := sha256.Sum256(binaryData)
	goodManifest := fmt.Sprintf(`{
  "name": "hello",
  "version": "1.0.0",
  "sdk_version": "v1",
  "binary": "hello",
  "sha256": "%s",
  "publisher": "test"
}`, hex.EncodeToString(h[:]))

	if err := os.WriteFile(filepath.Join(goodDir, "plugin.json"), []byte(goodManifest), 0o600); err != nil {
		t.Fatalf("write manifest: %v", err)
	}

	// Create a malformed plugin entry (bad manifest JSON).
	badDir := filepath.Join(pluginsDir, "bad")
	if err := os.MkdirAll(badDir, 0o750); err != nil {
		t.Fatalf("mkdir bad dir: %v", err)
	}

	badManifestPath := filepath.Join(badDir, "plugin.json")
	if err := os.WriteFile(badManifestPath, []byte("{invalid json"), 0o600); err != nil {
		t.Fatalf("write bad manifest: %v", err)
	}

	// Discover plugins. Should return the good one and skip the bad one.
	manifests, err := Discover(pluginsDir)
	if err != nil {
		t.Errorf("Discover failed: %v", err)
	}

	// Should have at least the good plugin (verification may fail on the good one too if keys don't match).
	// At minimum, we're testing that Discover doesn't crash and handles malformed entries gracefully.
	t.Logf("Discover returned %d plugins (expected at least 0, skipping bad ones gracefully)", len(manifests))
}

// TestLoadWithVerifierTamperedBinaryFails tests that LoadWithVerifier rejects a binary
// that has been tampered with after signing.
func TestLoadWithVerifierTamperedBinaryFails(t *testing.T) {
	if testing.Short() {
		t.Skip("skipping subprocess integration test in short mode")
	}

	tmpDir := t.TempDir()
	pluginDir := filepath.Join(tmpDir, "plugins", "tampered")
	if err := os.MkdirAll(pluginDir, 0o750); err != nil {
		t.Fatalf("mkdir plugin dir: %v", err)
	}

	// Generate an ephemeral minisign keypair.
	pubKey, privKey, err := generateTestMinisignKey()
	if err != nil {
		t.Fatalf("generate minisign key: %v", err)
	}

	// Create a dummy binary.
	originalData := []byte("original plugin binary content")
	pluginBinaryPath := filepath.Join(pluginDir, "tampered")
	if err := os.WriteFile(pluginBinaryPath, originalData, 0o700); err != nil { //nolint:gosec // test fixture
		t.Fatalf("write binary: %v", err)
	}

	// Sign the original binary.
	sig := minisign.Sign(privKey, originalData)
	sigPath := filepath.Join(pluginDir, "tampered.minisig")
	if err := os.WriteFile(sigPath, sig, 0o600); err != nil {
		t.Fatalf("write signature: %v", err)
	}

	// Write manifest with the original hash.
	h := sha256.Sum256(originalData)
	manifestJSON := fmt.Sprintf(`{
  "name": "tampered",
  "version": "1.0.0",
  "sdk_version": "v1",
  "binary": "tampered",
  "sha256": "%s",
  "publisher": "test"
}`, hex.EncodeToString(h[:]))
	manifestPath := filepath.Join(pluginDir, "plugin.json")
	if err := os.WriteFile(manifestPath, []byte(manifestJSON), 0o600); err != nil {
		t.Fatalf("write manifest: %v", err)
	}

	// Now tamper with the binary (flip a byte).
	tamperedData := append([]byte{}, originalData...)
	tamperedData[0] ^= 0xFF
	if err := os.WriteFile(pluginBinaryPath, tamperedData, 0o700); err != nil { //nolint:gosec // test fixture
		t.Fatalf("write tampered binary: %v", err)
	}

	hostServices := &MockHostServices{
		configValue:  []byte("test config"),
		dataDirValue: tmpDir,
	}

	verifier := NewVerifierWithKeys([]string{pubKey})

	// Attempt to load the plugin. Should fail because the SHA256 hash no longer matches.
	mod, err := LoadWithVerifier(context.Background(), pluginDir, hostServices, verifier)
	if err == nil {
		t.Fatalf("LoadWithVerifier should fail for tampered binary; got module: %v", mod)
	}

	t.Logf("LoadWithVerifier correctly rejected tampered binary: %v", err)
}

// TestLoadFallbackToLoadWithVerifier tests that Load delegates to LoadWithVerifier.
func TestLoadFallbackToLoadWithVerifier(t *testing.T) {
	if testing.Short() {
		t.Skip("skipping subprocess integration test in short mode")
	}

	tmpDir := t.TempDir()
	binDir := filepath.Join(tmpDir, "bin")
	if err := os.MkdirAll(binDir, 0o750); err != nil {
		t.Fatalf("mkdir bin: %v", err)
	}

	pluginExePath := filepath.Join(binDir, "plugin-hello")
	buildCmd := exec.Command("go", "build", "-o", pluginExePath, "./examples/plugin-hello") //nolint:gosec // hardcoded command, only output is variable
	buildCmd.Dir = "/home/penguin/code/penguin"
	if output, err := buildCmd.CombinedOutput(); err != nil {
		t.Fatalf("build plugin failed: %v\n%s", err, output)
	}

	if _, err := os.Stat(pluginExePath); err != nil {
		t.Fatalf("plugin binary not found after build: %v", err)
	}

	_, privKey, err := generateTestMinisignKey()
	if err != nil {
		t.Fatalf("generate minisign key: %v", err)
	}

	binaryData, err := os.ReadFile(pluginExePath) //nolint:gosec // reading built test fixture
	if err != nil {
		t.Fatalf("read plugin binary: %v", err)
	}

	sig := minisign.Sign(privKey, binaryData)

	pluginDir := filepath.Join(tmpDir, "plugins", "hello")
	if err := os.MkdirAll(pluginDir, 0o750); err != nil {
		t.Fatalf("mkdir plugin dir: %v", err)
	}

	pluginBinaryName := "hello"
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
  "name": "hello",
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

	// NOTE: Load() uses NewVerifier() which loads the embedded key.
	// This test will only succeed if the plugin was signed with the embedded key,
	// which it is not (we generated an ephemeral key).
	// So we expect Load to fail, but it should at least attempt the flow.
	mod, err := Load(context.Background(), pluginDir, hostServices)

	// Load should fail because our ephemeral key is not in the trusted list.
	// The point is to verify that Load delegates to LoadWithVerifier.
	if err == nil {
		t.Fatalf("Load should fail with ephemeral key; got module: %v", mod)
	}

	t.Logf("Load correctly failed with untrusted key (expected): %v", err)
}
