package waddleperf

import (
	"context"
	"fmt"
	"net/http"
	"strings"
	"time"

	desktop "github.com/penguintechinc/penguin-libs/packages/penguin-desktop"
	"github.com/sirupsen/logrus"
)

// Client communicates with WaddlePerf's testServer and managerServer.
type Client struct {
	testAPI    *desktop.JSONClient
	managerAPI *desktop.JSONClient
	device     DeviceInfo
	logger     *logrus.Logger
}

// NewClient creates a WaddlePerf API client.
func NewClient(testServerURL, managerURL, apiKey string, device DeviceInfo, logger *logrus.Logger) *Client {
	deviceHeaders := func(r *http.Request) {
		r.Header.Set("X-Device-Serial", device.Serial)
		r.Header.Set("X-Device-Hostname", device.Hostname)
		r.Header.Set("X-Device-OS", device.OS)
		r.Header.Set("X-Device-OS-Version", device.Version)
	}

	testAPI := desktop.NewJSONClient(strings.TrimRight(testServerURL, "/"), 60*time.Second)
	testAPI.ExtraHeaders = deviceHeaders
	if apiKey != "" {
		testAPI.GetToken = func() string { return apiKey }
	}

	managerAPI := desktop.NewJSONClient(strings.TrimRight(managerURL, "/"), 60*time.Second)
	managerAPI.ExtraHeaders = deviceHeaders
	if apiKey != "" {
		managerAPI.GetToken = func() string { return apiKey }
	}

	return &Client{
		testAPI:    testAPI,
		managerAPI: managerAPI,
		device:     device,
		logger:     logger,
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
	return c.managerAPI.DoJSON(ctx, "POST", "/api/v1/results/upload", payload, nil)
}

// GetRecentResults retrieves recent test results from the managerServer.
func (c *Client) GetRecentResults(ctx context.Context, limit int) ([]TestResult, error) {
	path := fmt.Sprintf("/api/v1/statistics/recent?limit=%d", limit)
	var results []TestResult
	if err := c.managerAPI.DoJSON(ctx, "GET", path, nil, &results); err != nil {
		return nil, err
	}
	return results, nil
}

// HealthCheck verifies connectivity to both testServer and managerServer.
func (c *Client) HealthCheck(ctx context.Context) (testOK, managerOK bool) {
	testOK = c.checkHealth(ctx, c.testAPI, "/health")
	managerOK = c.checkHealth(ctx, c.managerAPI, "/health")
	return
}

func (c *Client) runTest(ctx context.Context, testType TestType, target, protocol string) (*TestResult, error) {
	body := map[string]string{
		"target":   target,
		"protocol": protocol,
	}
	path := fmt.Sprintf("/api/v1/test/%s", string(testType))
	var result TestResult
	if err := c.testAPI.DoJSON(ctx, "POST", path, body, &result); err != nil {
		return nil, fmt.Errorf("running %s test: %w", testType, err)
	}
	return &result, nil
}

func (c *Client) checkHealth(ctx context.Context, api *desktop.JSONClient, path string) bool {
	return api.DoJSON(ctx, "GET", path, nil, nil) == nil
}
