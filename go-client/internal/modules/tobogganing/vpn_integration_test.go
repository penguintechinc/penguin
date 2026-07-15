//go:build integration
// +build integration

package tobogganing

import (
	"context"
	"net"
	"os"
	"path/filepath"
	"testing"
	"time"

	"go.uber.org/zap/zaptest"
	"golang.zx2c4.com/wireguard/wgctrl/wgtypes"
)

// TestVPNManagerIntegrationWithFakeController tests VPNManager with FakeWGController
func TestVPNManagerIntegrationWithFakeController(t *testing.T) {
	if os.Geteuid() != 0 {
		t.Skip("requires root/CAP_NET_ADMIN")
	}

	logger := zaptest.NewLogger(t)
	dataDir := t.TempDir()

	config := &ModuleConfig{
		InterfaceName: "wg0",
		Keepalive:     25,
		Embedded:      true,
	}

	vpnMgr := &VPNManager{
		config:        config,
		dataDir:       dataDir,
		logger:        logger,
		wgClient:      NewFakeWGController(),
		lastHandshake: time.Time{},
	}

	t.Cleanup(func() {
		if vpnMgr.wgClient != nil {
			vpnMgr.wgClient.Close()
		}
	})

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Verify initial state
	if vpnMgr.IsConnected() {
		t.Errorf("should not be connected initially")
	}

	// Test Disconnect (should be no-op initially)
	err := vpnMgr.Disconnect(ctx)
	if err != nil {
		t.Fatalf("Disconnect failed: %v", err)
	}
}

// TestVPNManagerIntegrationDeviceCreation tests WireGuard device creation via fake controller
func TestVPNManagerIntegrationDeviceCreation(t *testing.T) {
	if os.Geteuid() != 0 {
		t.Skip("requires root/CAP_NET_ADMIN")
	}

	logger := zaptest.NewLogger(t)
	dataDir := t.TempDir()

	config := &ModuleConfig{
		InterfaceName: "wg0",
		Keepalive:     25,
	}

	fakeController := NewFakeWGController()
	vpnMgr := &VPNManager{
		config:        config,
		dataDir:       dataDir,
		logger:        logger,
		wgClient:      fakeController,
		lastHandshake: time.Time{},
	}

	t.Cleanup(func() {
		vpnMgr.wgClient.Close()
	})

	// Create a WireGuard configuration
	privateKey, err := wgtypes.GeneratePrivateKey()
	if err != nil {
		t.Fatalf("failed to generate private key: %v", err)
	}

	// Parse a test server public key
	serverPublicKey, err := wgtypes.ParseKey("8KwBAj0FhS0XPAw5iMEQjdKVV9FBb2dChvfTlnPL1Rg=")
	if err != nil {
		t.Fatalf("failed to parse server public key: %v", err)
	}

	// Create endpoint
	serverAddr, err := net.ResolveUDPAddr("udp", "127.0.0.1:51820")
	if err != nil {
		t.Fatalf("failed to resolve endpoint: %v", err)
	}

	// Parse allowed IPs
	_, ipNet, err := net.ParseCIDR("0.0.0.0/0")
	if err != nil {
		t.Fatalf("failed to parse CIDR: %v", err)
	}

	keepalive := time.Duration(config.Keepalive) * time.Second

	// Create config
	wgCfg := &wgtypes.Config{
		PrivateKey: &privateKey,
		ListenPort: nil,
		Peers: []wgtypes.PeerConfig{
			{
				PublicKey:                   serverPublicKey,
				Endpoint:                    serverAddr,
				AllowedIPs:                  []net.IPNet{*ipNet},
				PersistentKeepaliveInterval: &keepalive,
			},
		},
	}

	// Apply configuration via fake controller
	err = fakeController.Configure(config.InterfaceName, wgCfg)
	if err != nil {
		t.Fatalf("Configure failed: %v", err)
	}

	// Verify config was applied
	appliedConfig := fakeController.GetLastConfig(config.InterfaceName)
	if appliedConfig == nil {
		t.Errorf("config not applied")
	}

	if len(appliedConfig.Peers) != 1 {
		t.Errorf("expected 1 peer, got %d", len(appliedConfig.Peers))
	}

	// Verify we can read the device back
	device, err := fakeController.Device(config.InterfaceName)
	if err != nil {
		t.Fatalf("Device failed: %v", err)
	}

	if device != nil && device.Name != config.InterfaceName {
		t.Errorf("device name mismatch")
	}
}

// TestVPNManagerIntegrationDeviceList tests listing WireGuard devices
func TestVPNManagerIntegrationDeviceList(t *testing.T) {
	if os.Geteuid() != 0 {
		t.Skip("requires root/CAP_NET_ADMIN")
	}

	fakeController := NewFakeWGController()

	// Manually add a device to the fake controller
	fakeController.devices["wg0"] = &wgtypes.Device{
		Name: "wg0",
	}

	devices, err := fakeController.Devices()
	if err != nil {
		t.Fatalf("Devices failed: %v", err)
	}

	if len(devices) != 1 || devices[0] != "wg0" {
		t.Errorf("expected [wg0], got %v", devices)
	}
}

// TestVPNManagerLastHandshakeTracking tests handshake time tracking
func TestVPNManagerLastHandshakeTracking(t *testing.T) {
	if os.Geteuid() != 0 {
		t.Skip("requires root/CAP_NET_ADMIN")
	}

	logger := zaptest.NewLogger(t)
	dataDir := t.TempDir()

	config := &ModuleConfig{
		InterfaceName: "wg0",
		Keepalive:     25,
	}

	now := time.Now()
	vpnMgr := &VPNManager{
		config:        config,
		dataDir:       dataDir,
		logger:        logger,
		wgClient:      NewFakeWGController(),
		lastHandshake: now,
	}

	// Read last handshake
	lastHandshake := vpnMgr.LastHandshake()

	if !lastHandshake.Equal(now) {
		t.Errorf("last handshake time mismatch")
	}
}

// TestVPNManagerConfigPersistence tests saving and retrieving config
func TestVPNManagerConfigPersistence(t *testing.T) {
	if os.Geteuid() != 0 {
		t.Skip("requires root/CAP_NET_ADMIN")
	}

	logger := zaptest.NewLogger(t)
	dataDir := t.TempDir()

	config := &ModuleConfig{
		ManagerURL:    "http://localhost:8080",
		NodeID:        "test-node",
		InterfaceName: "wg0",
	}

	vpnMgr := &VPNManager{
		config:        config,
		dataDir:       dataDir,
		logger:        logger,
		wgClient:      NewFakeWGController(),
		lastHandshake: time.Time{},
	}

	// Save a certificate
	certData := []byte("test-certificate")
	err := vpnMgr.SaveCert(certData)
	if err != nil {
		t.Fatalf("SaveCert failed: %v", err)
	}

	// Verify file was created
	certPath := filepath.Join(dataDir, "tunnel.crt")
	stat, err := os.Stat(certPath)
	if err != nil {
		t.Fatalf("cert file not created: %v", err)
	}

	if stat.Size() != int64(len(certData)) {
		t.Errorf("cert file size mismatch")
	}
}
