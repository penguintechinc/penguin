package articdbm

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

// Client is a REST client for the ArticDBM proxy service.
type Client struct {
	baseURL    string
	authToken  string
	httpClient *http.Client
	logger     *logrus.Logger
}

// NewClient creates an ArticDBM client.
func NewClient(baseURL, authToken string, logger *logrus.Logger) *Client {
	return &Client{
		baseURL:    strings.TrimRight(baseURL, "/"),
		authToken:  authToken,
		httpClient: &http.Client{Timeout: 30 * time.Second},
		logger:     logger,
	}
}

// ListProxies returns all proxy instances.
func (c *Client) ListProxies(ctx context.Context) ([]Proxy, error) {
	var proxies []Proxy
	if err := c.doJSON(ctx, "GET", c.baseURL+"/api/v1/proxies", nil, &proxies); err != nil {
		return nil, err
	}
	return proxies, nil
}

// GetProxy returns a single proxy by name.
func (c *Client) GetProxy(ctx context.Context, name string) (*Proxy, error) {
	var proxy Proxy
	if err := c.doJSON(ctx, "GET", c.baseURL+"/api/v1/proxies/"+name, nil, &proxy); err != nil {
		return nil, err
	}
	return &proxy, nil
}

// CreateProxy creates a new proxy instance.
func (c *Client) CreateProxy(ctx context.Context, req *CreateProxyRequest) (*Proxy, error) {
	var proxy Proxy
	if err := c.doJSON(ctx, "POST", c.baseURL+"/api/v1/proxies", req, &proxy); err != nil {
		return nil, err
	}
	return &proxy, nil
}

// DeleteProxy removes a proxy instance.
func (c *Client) DeleteProxy(ctx context.Context, name string) error {
	return c.doJSON(ctx, "DELETE", c.baseURL+"/api/v1/proxies/"+name, nil, nil)
}

// GetMetrics returns proxy metrics.
func (c *Client) GetMetrics(ctx context.Context) (string, error) {
	req, err := http.NewRequestWithContext(ctx, "GET", c.baseURL+"/metrics", nil)
	if err != nil {
		return "", err
	}
	resp, err := c.httpClient.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return "", err
	}
	return string(body), nil
}

// HealthCheck checks the proxy health.
func (c *Client) HealthCheck(ctx context.Context) (string, error) {
	req, err := http.NewRequestWithContext(ctx, "GET", c.baseURL+"/health", nil)
	if err != nil {
		return "", err
	}
	resp, err := c.httpClient.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return "", err
	}
	return string(body), nil
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
