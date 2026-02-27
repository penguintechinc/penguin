package skauswatch

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"runtime"
	"strings"
	"sync"
	"time"

	"github.com/sirupsen/logrus"
)

// Client communicates with the SkaUsWatch security monitoring service.
type Client struct {
	baseURL    string
	authToken  string
	httpClient *http.Client
	logger     *logrus.Logger

	mu         sync.Mutex
	endpointID string
	registered bool

	stopCh chan struct{}
	wg     sync.WaitGroup
}

// NewClient creates a SkaUsWatch API client.
func NewClient(baseURL, authToken string, logger *logrus.Logger) *Client {
	return &Client{
		baseURL:    strings.TrimRight(baseURL, "/"),
		authToken:  authToken,
		httpClient: &http.Client{Timeout: 30 * time.Second},
		logger:     logger,
		stopCh:     make(chan struct{}),
	}
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
	if err := c.doJSON(ctx, "POST", c.baseURL+"/api/v1/endpoints", reqBody, &resp); err != nil {
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
	c.wg.Add(1)
	go c.checkinWorker()
}

// StopCheckin stops the background checkin worker.
func (c *Client) StopCheckin() {
	close(c.stopCh)
	c.wg.Wait()
}

// Checkin sends a heartbeat to the SkaUsWatch service.
func (c *Client) Checkin(ctx context.Context) error {
	c.mu.Lock()
	id := c.endpointID
	c.mu.Unlock()
	if id == "" {
		return fmt.Errorf("endpoint not registered")
	}
	return c.doJSON(ctx, "POST", c.baseURL+"/api/v1/endpoints/"+id+"/checkin", nil, nil)
}

// GetAlerts retrieves threat alerts, optionally filtered by status.
func (c *Client) GetAlerts(ctx context.Context, status string) ([]ThreatAlert, error) {
	url := c.baseURL + "/api/v1/alerts"
	if status != "" {
		url += "?status=" + status
	}
	var alerts []ThreatAlert
	if err := c.doJSON(ctx, "GET", url, nil, &alerts); err != nil {
		return nil, err
	}
	return alerts, nil
}

// DismissAlert marks an alert as dismissed.
func (c *Client) DismissAlert(ctx context.Context, id string) error {
	return c.doJSON(ctx, "PUT", c.baseURL+"/api/v1/alerts/"+id+"/dismiss", nil, nil)
}

// StartScan initiates a security scan of the given type.
func (c *Client) StartScan(ctx context.Context, scanType string) (*ScanResult, error) {
	body := map[string]string{"type": scanType}
	var result ScanResult
	if err := c.doJSON(ctx, "POST", c.baseURL+"/api/v1/scans", body, &result); err != nil {
		return nil, err
	}
	return &result, nil
}

// GetScanStatus returns the current state of a scan.
func (c *Client) GetScanStatus(ctx context.Context, id string) (*ScanResult, error) {
	var result ScanResult
	if err := c.doJSON(ctx, "GET", c.baseURL+"/api/v1/scans/"+id, nil, &result); err != nil {
		return nil, err
	}
	return &result, nil
}

// GetQuarantine lists quarantined files.
func (c *Client) GetQuarantine(ctx context.Context) ([]QuarantineEntry, error) {
	var entries []QuarantineEntry
	if err := c.doJSON(ctx, "GET", c.baseURL+"/api/v1/quarantine", nil, &entries); err != nil {
		return nil, err
	}
	return entries, nil
}

// RestoreFile restores a quarantined file.
func (c *Client) RestoreFile(ctx context.Context, id string) error {
	return c.doJSON(ctx, "POST", c.baseURL+"/api/v1/quarantine/"+id+"/restore", nil, nil)
}

// DeleteFile permanently deletes a quarantined file.
func (c *Client) DeleteFile(ctx context.Context, id string) error {
	return c.doJSON(ctx, "DELETE", c.baseURL+"/api/v1/quarantine/"+id, nil, nil)
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
	if err := c.doJSON(ctx, "GET", c.baseURL+"/api/v1/endpoints/"+id, nil, &status); err != nil {
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

func (c *Client) checkinWorker() {
	defer c.wg.Done()
	ticker := time.NewTicker(60 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-c.stopCh:
			return
		case <-ticker.C:
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			if err := c.Checkin(ctx); err != nil {
				c.logger.WithError(err).Warn("skauswatch checkin failed")
			}
			cancel()
		}
	}
}

func (c *Client) doJSON(ctx context.Context, method, url string, body, result interface{}) error {
	var bodyReader io.Reader
	if body != nil {
		data, err := json.Marshal(body)
		if err != nil {
			return fmt.Errorf("marshaling request: %w", err)
		}
		bodyReader = bytes.NewReader(data)
	}

	req, err := http.NewRequestWithContext(ctx, method, url, bodyReader)
	if err != nil {
		return fmt.Errorf("creating request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")
	if c.authToken != "" {
		req.Header.Set("Authorization", "Bearer "+c.authToken)
	}

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode >= 400 {
		respBody, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("API error (status %d): %s", resp.StatusCode, string(respBody))
	}

	if result != nil && resp.StatusCode != http.StatusNoContent {
		if err := json.NewDecoder(resp.Body).Decode(result); err != nil {
			return fmt.Errorf("decoding response: %w", err)
		}
	}
	return nil
}
