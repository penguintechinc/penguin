package articdbm

import (
	"context"
	"io"
	"net/http"
	"strings"
	"time"

	"github.com/penguintechinc/penguin-libs/packages/penguin-desktop"
	"github.com/sirupsen/logrus"
)

// Client is a REST client for the ArticDBM proxy service.
type Client struct {
	api       *desktop.JSONClient
	authToken string
	logger    *logrus.Logger
}

// NewClient creates an ArticDBM client.
func NewClient(baseURL, authToken string, logger *logrus.Logger) *Client {
	baseURL = strings.TrimRight(baseURL, "/")
	c := &Client{
		api:       desktop.NewJSONClient(baseURL, 30*time.Second),
		authToken: authToken,
		logger:    logger,
	}
	if authToken != "" {
		c.api.GetToken = func() string {
			return authToken
		}
	}
	return c
}

// ListProxies returns all proxy instances.
func (c *Client) ListProxies(ctx context.Context) ([]Proxy, error) {
	var proxies []Proxy
	if err := c.api.DoJSON(ctx, "GET", "/api/v1/proxies", nil, &proxies); err != nil {
		return nil, err
	}
	return proxies, nil
}

// GetProxy returns a single proxy by name.
func (c *Client) GetProxy(ctx context.Context, name string) (*Proxy, error) {
	var proxy Proxy
	if err := c.api.DoJSON(ctx, "GET", "/api/v1/proxies/"+name, nil, &proxy); err != nil {
		return nil, err
	}
	return &proxy, nil
}

// CreateProxy creates a new proxy instance.
func (c *Client) CreateProxy(ctx context.Context, req *CreateProxyRequest) (*Proxy, error) {
	var proxy Proxy
	if err := c.api.DoJSON(ctx, "POST", "/api/v1/proxies", req, &proxy); err != nil {
		return nil, err
	}
	return &proxy, nil
}

// DeleteProxy removes a proxy instance.
func (c *Client) DeleteProxy(ctx context.Context, name string) error {
	return c.api.DoJSON(ctx, "DELETE", "/api/v1/proxies/"+name, nil, nil)
}

// GetMetrics returns proxy metrics.
func (c *Client) GetMetrics(ctx context.Context) (string, error) {
	req, err := http.NewRequestWithContext(ctx, "GET", c.api.BaseURL+"/metrics", nil)
	if err != nil {
		return "", err
	}
	resp, err := http.DefaultClient.Do(req)
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
	req, err := http.NewRequestWithContext(ctx, "GET", c.api.BaseURL+"/health", nil)
	if err != nil {
		return "", err
	}
	resp, err := http.DefaultClient.Do(req)
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
