package waddlebot

import (
	"context"
	"fmt"
	"net/http"
	"runtime"
	"sync"
	"time"

	desktop "github.com/penguintechinc/penguin/services/desktop/pkg/desktop"
	"github.com/sirupsen/logrus"
)

// Client handles communication with the WaddleBot bridge API.
type Client struct {
	api       *desktop.JSONClient
	config    BridgeConfig
	logger    *logrus.Logger
	mu        sync.RWMutex
	bridgeID  string
	connected bool
	lastPoll  time.Time
}

// NewClient creates a bridge API client for the given configuration.
// Community and User IDs are injected as custom headers on every request.
func NewClient(cfg BridgeConfig, logger *logrus.Logger) *Client {
	c := &Client{
		config: cfg,
		logger: logger,
	}
	c.api = desktop.NewJSONClient(cfg.APIURL, 30*time.Second)
	communityID := cfg.CommunityID
	userID := cfg.UserID
	c.api.ExtraHeaders = func(r *http.Request) {
		r.Header.Set("X-Community-ID", communityID)
		r.Header.Set("X-User-ID", userID)
	}
	return c
}

// SetToken configures the Bearer token used for all subsequent requests.
func (c *Client) SetToken(token string) {
	c.api.GetToken = func() string { return token }
}

// Register sends a registration request to the WaddleBot bridge API.
// On success the bridge ID is stored and the client is marked connected.
func (c *Client) Register(ctx context.Context) error {
	req := RegistrationRequest{
		CommunityID: c.config.CommunityID,
		UserID:      c.config.UserID,
		Version:     "0.1.0",
		Platform:    runtime.GOOS,
		Capabilities: []string{
			"obs",
			"scripting",
			"local_execution",
		},
		Modules: []ModuleInfo{
			{
				Name:    "obs",
				Version: "1.0.0",
				Actions: []ActionInfo{
					{Name: "switch_scene", Description: "Switch OBS scene"},
					{Name: "toggle_source", Description: "Toggle source visibility"},
					{Name: "start_stream", Description: "Start streaming"},
					{Name: "stop_stream", Description: "Stop streaming"},
					{Name: "start_recording", Description: "Start recording"},
					{Name: "stop_recording", Description: "Stop recording"},
				},
			},
			{
				Name:    "scripting",
				Version: "1.0.0",
				Actions: []ActionInfo{
					{Name: "run_script", Description: "Execute a Lua, Python, or Bash script"},
				},
			},
		},
	}

	var resp RegistrationResponse
	if err := c.api.DoJSON(ctx, "POST", "/api/bridge/register", req, &resp); err != nil {
		return fmt.Errorf("register bridge: %w", err)
	}

	c.mu.Lock()
	c.bridgeID = resp.BridgeID
	c.connected = true
	c.mu.Unlock()

	c.logger.WithField("bridge_id", resp.BridgeID).Info("bridge registered")
	return nil
}

// Poll retrieves pending actions from the WaddleBot bridge API.
func (c *Client) Poll(ctx context.Context) (*PollResponse, error) {
	var resp PollResponse
	if err := c.api.DoJSON(ctx, "GET", "/api/bridge/poll", nil, &resp); err != nil {
		return nil, fmt.Errorf("poll: %w", err)
	}

	c.mu.Lock()
	c.lastPoll = time.Now()
	c.mu.Unlock()

	return &resp, nil
}

// SendResponse reports the result of an executed action back to the server.
func (c *Client) SendResponse(ctx context.Context, resp ActionResponse) error {
	if err := c.api.DoJSON(ctx, "POST", "/api/bridge/response", resp, nil); err != nil {
		return fmt.Errorf("send response: %w", err)
	}
	return nil
}

// Heartbeat sends a keepalive signal to the bridge API.
func (c *Client) Heartbeat(ctx context.Context) error {
	if err := c.api.DoJSON(ctx, "POST", "/api/bridge/heartbeat", nil, nil); err != nil {
		return fmt.Errorf("heartbeat: %w", err)
	}
	return nil
}

// Unregister removes the bridge registration from the API.
func (c *Client) Unregister(ctx context.Context) error {
	err := c.api.DoJSON(ctx, "POST", "/api/bridge/unregister", nil, nil)

	c.mu.Lock()
	c.connected = false
	c.bridgeID = ""
	c.mu.Unlock()

	if err != nil {
		return fmt.Errorf("unregister bridge: %w", err)
	}
	return nil
}

// IsConnected returns true if the bridge is currently registered and connected.
func (c *Client) IsConnected() bool {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.connected
}

// GetStatus returns a snapshot of the current bridge connection state.
func (c *Client) GetStatus() BridgeStatus {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return BridgeStatus{
		Connected:   c.connected,
		CommunityID: c.config.CommunityID,
		UserID:      c.config.UserID,
		BridgeID:    c.bridgeID,
		LastPoll:    c.lastPoll,
	}
}
