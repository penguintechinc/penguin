package waddleperf

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	"github.com/sirupsen/logrus"
)

// Client communicates with WaddlePerf's testServer and managerServer.
type Client struct {
	testServerURL string
	managerURL    string
	apiKey        string
	device        DeviceInfo
	httpClient    *http.Client
	logger        *logrus.Logger
}

// NewClient creates a WaddlePerf API client.
func NewClient(testServerURL, managerURL, apiKey string, device DeviceInfo, logger *logrus.Logger) *Client {
	return &Client{
		testServerURL: strings.TrimRight(testServerURL, "/"),
		managerURL:    strings.TrimRight(managerURL, "/"),
		apiKey:        apiKey,
		device:        device,
		httpClient:    &http.Client{Timeout: 60 * time.Second},
		logger:        logger,
	}
}

// RunHTTPTest executes an HTTP network test via the testServer.
func (c *Client) RunHTTPTest(ctx context.Context, target, protocol string) (*TestResult, error) {
	return c.runTest(ctx, TestHTTP, target, protocol)
}

// RunTCPTest executes a TCP network test via the testServer.
func (c *Client) RunTCPTest(ctx context.Context, target, protocol string) (*TestResult, error) {
	return c.runTest(ctx, TestTCP, target, protocol)
}

// RunUDPTest executes a UDP network test via the testServer.
func (c *Client) RunUDPTest(ctx context.Context, target, protocol string) (*TestResult, error) {
	return c.runTest(ctx, TestUDP, target, protocol)
}

// RunICMPTest executes an ICMP network test via the testServer.
func (c *Client) RunICMPTest(ctx context.Context, target, protocol string) (*TestResult, error) {
	return c.runTest(ctx, TestICMP, target, protocol)
}

// UploadResult sends a test result to the managerServer.
func (c *Client) UploadResult(ctx context.Context, result TestResult) error {
	payload := uploadPayload{Result: result, Device: c.device}
	return c.doJSON(ctx, "POST", c.managerURL+"/api/v1/results/upload", payload, nil)
}

// GetRecentResults retrieves recent test results from the managerServer.
func (c *Client) GetRecentResults(ctx context.Context, limit int) ([]TestResult, error) {
	url := fmt.Sprintf("%s/api/v1/statistics/recent?limit=%d", c.managerURL, limit)
	var results []TestResult
	if err := c.doJSON(ctx, "GET", url, nil, &results); err != nil {
		return nil, err
	}
	return results, nil
}

// HealthCheck verifies connectivity to both testServer and managerServer.
func (c *Client) HealthCheck(ctx context.Context) (testOK, managerOK bool) {
	testOK = c.checkHealth(ctx, c.testServerURL+"/health")
	managerOK = c.checkHealth(ctx, c.managerURL+"/health")
	return
}

func (c *Client) runTest(ctx context.Context, testType TestType, target, protocol string) (*TestResult, error) {
	body := map[string]string{
		"target":   target,
		"protocol": protocol,
	}
	endpoint := fmt.Sprintf("%s/api/v1/test/%s", c.testServerURL, string(testType))
	var result TestResult
	if err := c.doJSON(ctx, "POST", endpoint, body, &result); err != nil {
		return nil, fmt.Errorf("running %s test: %w", testType, err)
	}
	return &result, nil
}

func (c *Client) checkHealth(ctx context.Context, url string) bool {
	req, err := http.NewRequestWithContext(ctx, "GET", url, nil)
	if err != nil {
		return false
	}
	resp, err := c.httpClient.Do(req)
	if err != nil {
		return false
	}
	defer resp.Body.Close()
	return resp.StatusCode < 400
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
	if c.apiKey != "" {
		req.Header.Set("Authorization", "Bearer "+c.apiKey)
	}

	// Device identification headers.
	req.Header.Set("X-Device-Serial", c.device.Serial)
	req.Header.Set("X-Device-Hostname", c.device.Hostname)
	req.Header.Set("X-Device-OS", c.device.OS)
	req.Header.Set("X-Device-OS-Version", c.device.Version)

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
