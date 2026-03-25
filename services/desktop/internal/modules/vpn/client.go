package vpn

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"time"

	"github.com/penguintechinc/penguin/services/desktop/internal/module"
	"github.com/sirupsen/logrus"
)

type ConnectionState int

const (
	StateDisconnected ConnectionState = iota
	StateConnecting
	StateConnected
	StateDisconnecting
)

type Client struct {
	managerURL string
	apiKey     string
	authToken  string
	clientName string
	configDir  string
	dataDir    string
	logger     *logrus.Logger
	httpClient *http.Client

	mu          sync.RWMutex
	state       ConnectionState
	clientID    string
	assignedIP  string
	connectedAt time.Time
	cancel      context.CancelFunc
}

type registrationResponse struct {
	ClientID     string `json:"client_id"`
	APIKey       string `json:"api_key"`
	Cluster      string `json:"cluster"`
	Certificates struct {
		CA   string `json:"ca"`
		Cert string `json:"cert"`
		Key  string `json:"key"`
	} `json:"certificates"`
}

type wireguardKeysResponse struct {
	IP       string `json:"ip"`
	Config   string `json:"config"`
	DNS      string `json:"dns"`
	Endpoint string `json:"endpoint"`
}

func NewClient(deps module.Dependencies, logger *logrus.Logger) *Client {
	return &Client{
		configDir:  deps.ConfigDir,
		dataDir:    deps.DataDir,
		authToken:  deps.AuthToken,
		logger:     logger,
		httpClient: &http.Client{Timeout: 30 * time.Second},
		state:      StateDisconnected,
	}
}

func (c *Client) Configure(managerURL, apiKey, clientName string) {
	c.managerURL = strings.TrimRight(managerURL, "/")
	c.apiKey = apiKey
	c.clientName = clientName
}

func (c *Client) Connect(ctx context.Context) error {
	c.mu.Lock()
	if c.state != StateDisconnected {
		c.mu.Unlock()
		return fmt.Errorf("already connected or connecting")
	}
	c.state = StateConnecting
	c.mu.Unlock()

	// Step 1: Register
	regResp, err := c.register(ctx)
	if err != nil {
		c.setState(StateDisconnected)
		return fmt.Errorf("registration failed: %w", err)
	}
	c.clientID = regResp.ClientID

	// Step 2: Save certificates
	if err := c.saveCertificates(regResp); err != nil {
		c.setState(StateDisconnected)
		return fmt.Errorf("saving certificates: %w", err)
	}

	// Step 3: Get WireGuard config
	wgResp, err := c.getWireGuardConfig(ctx)
	if err != nil {
		c.setState(StateDisconnected)
		return fmt.Errorf("getting wireguard config: %w", err)
	}

	// Step 4: Write WireGuard config and bring up interface
	if err := c.setupWireGuard(wgResp); err != nil {
		c.setState(StateDisconnected)
		return fmt.Errorf("wireguard setup: %w", err)
	}

	c.mu.Lock()
	c.state = StateConnected
	c.assignedIP = wgResp.IP
	c.connectedAt = time.Now()
	c.mu.Unlock()

	// Step 5: Start monitoring
	monCtx, cancel := context.WithCancel(ctx)
	c.cancel = cancel
	go c.monitor(monCtx)

	c.logger.WithField("ip", wgResp.IP).Info("VPN connected")
	return nil
}

func (c *Client) Disconnect(ctx context.Context) error {
	c.mu.Lock()
	if c.state != StateConnected {
		c.mu.Unlock()
		return nil
	}
	c.state = StateDisconnecting
	c.mu.Unlock()

	if c.cancel != nil {
		c.cancel()
	}

	if err := c.teardownWireGuard(); err != nil {
		c.logger.WithError(err).Warn("Error tearing down WireGuard")
	}

	c.setState(StateDisconnected)
	c.logger.Info("VPN disconnected")
	return nil
}

func (c *Client) IsConnected() bool {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.state == StateConnected
}

func (c *Client) StatusDetails() map[string]string {
	c.mu.RLock()
	defer c.mu.RUnlock()
	details := map[string]string{
		"state": fmt.Sprintf("%d", c.state),
		"ip":    c.assignedIP,
	}
	if !c.connectedAt.IsZero() {
		details["uptime"] = time.Since(c.connectedAt).Truncate(time.Second).String()
	}
	return details
}

func (c *Client) setState(s ConnectionState) {
	c.mu.Lock()
	c.state = s
	c.mu.Unlock()
}

func (c *Client) register(ctx context.Context) (*registrationResponse, error) {
	payload := fmt.Sprintf(`{"name":%q,"type":"desktop","platform":%q,"architecture":%q}`,
		c.clientName, runtime.GOOS, runtime.GOARCH)
	req, err := http.NewRequestWithContext(ctx, "POST", c.managerURL+"/api/v1/clients/register", strings.NewReader(payload))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")
	if c.authToken != "" {
		req.Header.Set("Authorization", "Bearer "+c.authToken)
	}

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK && resp.StatusCode != http.StatusCreated {
		body, _ := io.ReadAll(resp.Body)
		return nil, fmt.Errorf("registration failed (status %d): %s", resp.StatusCode, string(body))
	}

	var result registrationResponse
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, err
	}
	return &result, nil
}

func (c *Client) saveCertificates(reg *registrationResponse) error {
	certDir := filepath.Join(c.dataDir, "certs")
	if err := os.MkdirAll(certDir, 0700); err != nil {
		return err
	}
	files := map[string]string{
		"ca.pem":   reg.Certificates.CA,
		"cert.pem": reg.Certificates.Cert,
		"key.pem":  reg.Certificates.Key,
	}
	for name, data := range files {
		if data == "" {
			continue
		}
		if err := os.WriteFile(filepath.Join(certDir, name), []byte(data), 0600); err != nil {
			return fmt.Errorf("writing %s: %w", name, err)
		}
	}
	return nil
}

func (c *Client) getWireGuardConfig(ctx context.Context) (*wireguardKeysResponse, error) {
	req, err := http.NewRequestWithContext(ctx, "POST", c.managerURL+"/api/v1/wireguard/keys", nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")
	if c.authToken != "" {
		req.Header.Set("Authorization", "Bearer "+c.authToken)
	}

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return nil, fmt.Errorf("wireguard keys failed (status %d): %s", resp.StatusCode, string(body))
	}

	var result wireguardKeysResponse
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, err
	}
	return &result, nil
}

func (c *Client) setupWireGuard(wg *wireguardKeysResponse) error {
	var confPath, iface string
	switch runtime.GOOS {
	case "linux":
		confPath = "/etc/wireguard/wg0.conf"
		iface = "wg0"
	case "darwin":
		confPath = "/usr/local/etc/wireguard/utun1.conf"
		iface = "utun1"
	case "windows":
		confPath = `C:\Program Files\WireGuard\Data\Configurations\penguin.conf`
		iface = "penguin"
	default:
		return fmt.Errorf("unsupported platform: %s", runtime.GOOS)
	}

	dir := filepath.Dir(confPath)
	if err := os.MkdirAll(dir, 0700); err != nil {
		return fmt.Errorf("creating wireguard config dir: %w", err)
	}
	if err := os.WriteFile(confPath, []byte(wg.Config), 0600); err != nil {
		return fmt.Errorf("writing wireguard config: %w", err)
	}

	cmd := exec.Command("wg-quick", "up", iface)
	if output, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("wg-quick up failed: %s: %w", string(output), err)
	}
	return nil
}

func (c *Client) teardownWireGuard() error {
	var iface string
	switch runtime.GOOS {
	case "linux":
		iface = "wg0"
	case "darwin":
		iface = "utun1"
	case "windows":
		iface = "penguin"
	default:
		return fmt.Errorf("unsupported platform: %s", runtime.GOOS)
	}
	cmd := exec.Command("wg-quick", "down", iface)
	if output, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("wg-quick down failed: %s: %w", string(output), err)
	}
	return nil
}

func (c *Client) monitor(ctx context.Context) {
	ticker := time.NewTicker(30 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			c.logger.Debug("VPN health check")
		}
	}
}
