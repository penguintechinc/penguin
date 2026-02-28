package skauswatch

import (
	"context"
	"fmt"
	"os"
	"runtime"
	"strings"
	"sync"
	"time"

	desktop "github.com/penguintechinc/penguin-libs/packages/penguin-desktop"
	"github.com/sirupsen/logrus"
)

// Client communicates with the SkaUsWatch security monitoring service.
type Client struct {
	api    *desktop.JSONClient
	logger *logrus.Logger

	mu         sync.Mutex
	endpointID string
	registered bool

	checkinWorker *desktop.TickWorker
}

// NewClient creates a SkaUsWatch API client.
func NewClient(baseURL, authToken string, logger *logrus.Logger) *Client {
	c := &Client{
		logger: logger,
	}
	c.api = desktop.NewJSONClientWithToken(strings.TrimRight(baseURL, "/"), 30*time.Second, authToken)
	c.checkinWorker = &desktop.TickWorker{
		Interval: 60 * time.Second,
		Timeout:  10 * time.Second,
		Action:   c.Checkin,
		OnError:  func(err error) { logger.WithError(err).Warn("skauswatch checkin failed") },
	}
	return c
}

// RegisterEndpoint registers this device with the SkaUsWatch service.
func (c *Client) RegisterEndpoint(ctx context.Context) error {
	hostname, _ := os.Hostname()
	reqBody := registerRequest{
		Hostname:     hostname,
		OS:           runtime.GOOS + "/" + runtime.GOARCH,
		AgentVersion: "0.1.0",
	}
	var resp registerResponse
	if err := c.api.DoJSON(ctx, "POST", "/api/v1/endpoints", reqBody, &resp); err != nil {
		return fmt.Errorf("registering endpoint: %w", err)
	}
	c.mu.Lock()
	c.endpointID = resp.EndpointID
	c.registered = true
	c.mu.Unlock()
	return nil
}

// StartCheckin begins the background checkin worker.
func (c *Client) StartCheckin() {
	c.checkinWorker.Start()
}

// StopCheckin stops the background checkin worker.
func (c *Client) StopCheckin() {
	c.checkinWorker.Stop()
}

// Checkin sends a heartbeat to the SkaUsWatch service.
func (c *Client) Checkin(ctx context.Context) error {
	c.mu.Lock()
	id := c.endpointID
	c.mu.Unlock()
	if id == "" {
		return fmt.Errorf("endpoint not registered")
	}
	return c.api.DoJSON(ctx, "POST", "/api/v1/endpoints/"+id+"/checkin", nil, nil)
}

// GetAlerts retrieves threat alerts, optionally filtered by status.
func (c *Client) GetAlerts(ctx context.Context, status string) ([]ThreatAlert, error) {
	path := "/api/v1/alerts"
	if status != "" {
		path += "?status=" + status
	}
	var alerts []ThreatAlert
	if err := c.api.DoJSON(ctx, "GET", path, nil, &alerts); err != nil {
		return nil, err
	}
	return alerts, nil
}

// DismissAlert marks an alert as dismissed.
func (c *Client) DismissAlert(ctx context.Context, id string) error {
	return c.api.DoJSON(ctx, "PUT", "/api/v1/alerts/"+id+"/dismiss", nil, nil)
}

// StartScan initiates a security scan of the given type.
func (c *Client) StartScan(ctx context.Context, scanType string) (*ScanResult, error) {
	body := map[string]string{"type": scanType}
	var result ScanResult
	if err := c.api.DoJSON(ctx, "POST", "/api/v1/scans", body, &result); err != nil {
		return nil, err
	}
	return &result, nil
}

// GetScanStatus returns the current state of a scan.
func (c *Client) GetScanStatus(ctx context.Context, id string) (*ScanResult, error) {
	var result ScanResult
	if err := c.api.DoJSON(ctx, "GET", "/api/v1/scans/"+id, nil, &result); err != nil {
		return nil, err
	}
	return &result, nil
}

// GetQuarantine lists quarantined files.
func (c *Client) GetQuarantine(ctx context.Context) ([]QuarantineEntry, error) {
	var entries []QuarantineEntry
	if err := c.api.DoJSON(ctx, "GET", "/api/v1/quarantine", nil, &entries); err != nil {
		return nil, err
	}
	return entries, nil
}

// RestoreFile restores a quarantined file.
func (c *Client) RestoreFile(ctx context.Context, id string) error {
	return c.api.DoJSON(ctx, "POST", "/api/v1/quarantine/"+id+"/restore", nil, nil)
}

// DeleteFile permanently deletes a quarantined file.
func (c *Client) DeleteFile(ctx context.Context, id string) error {
	return c.api.DoJSON(ctx, "DELETE", "/api/v1/quarantine/"+id, nil, nil)
}

// GetEndpointStatus returns the status of this endpoint.
func (c *Client) GetEndpointStatus(ctx context.Context) (*EndpointStatus, error) {
	c.mu.Lock()
	id := c.endpointID
	c.mu.Unlock()
	if id == "" {
		return &EndpointStatus{Registered: false, AgentVersion: "0.1.0"}, nil
	}
	var status EndpointStatus
	if err := c.api.DoJSON(ctx, "GET", "/api/v1/endpoints/"+id, nil, &status); err != nil {
		return nil, err
	}
	return &status, nil
}

// IsRegistered returns whether the endpoint has been registered.
func (c *Client) IsRegistered() bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.registered
}
