// Package desktop provides shared utilities for Penguin desktop modules,
// including a JSON HTTP client, a periodic tick worker, and an embedded script engine.
package desktop

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

// JSONClient wraps an http.Client with JSON marshaling, auth headers, and a base URL.
// It consolidates the ~37-line doJSON pattern duplicated across desktop modules.
type JSONClient struct {
	// BaseURL is the root URL prepended to every request path.
	BaseURL string

	// HTTPClient is the underlying HTTP client. If nil, http.DefaultClient is used.
	HTTPClient *http.Client

	// GetToken is called before each request to obtain a Bearer token.
	// When nil no Authorization header is added.
	GetToken func() string

	// ExtraHeaders is called with each request to allow per-request header injection.
	// When nil no extra headers are added.
	ExtraHeaders func(*http.Request)
}

// NewJSONClient creates a JSONClient with a plain timeout and no auth.
func NewJSONClient(baseURL string, timeout time.Duration) *JSONClient {
	return &JSONClient{
		BaseURL: strings.TrimRight(baseURL, "/"),
		HTTPClient: &http.Client{
			Timeout: timeout,
		},
	}
}

// NewJSONClientWithToken creates a JSONClient with a fixed static Bearer token.
func NewJSONClientWithToken(baseURL string, timeout time.Duration, token string) *JSONClient {
	c := NewJSONClient(baseURL, timeout)
	c.GetToken = func() string { return token }
	return c
}

// DoJSON performs an HTTP request and handles JSON marshaling/unmarshaling.
//
// method is the HTTP verb (GET, POST, PUT, DELETE, PATCH).
// path is appended to BaseURL; a leading slash is added automatically if missing.
// body is marshaled to JSON and sent as the request body; pass nil for no body.
// result is populated by unmarshaling the response body; pass nil to discard.
//
// HTTP responses with status >= 400 are returned as errors containing the status
// code and any response body text.
// HTTP 204 No Content responses are treated as success with no body decoding.
func (c *JSONClient) DoJSON(ctx context.Context, method, path string, body, result any) error {
	// Build full URL
	if !strings.HasPrefix(path, "/") {
		path = "/" + path
	}
	url := c.BaseURL + path

	// Marshal body
	var bodyReader io.Reader
	if body != nil {
		data, err := json.Marshal(body)
		if err != nil {
			return fmt.Errorf("httpclient: marshal request body: %w", err)
		}
		bodyReader = bytes.NewReader(data)
	}

	// Build request
	req, err := http.NewRequestWithContext(ctx, method, url, bodyReader)
	if err != nil {
		return fmt.Errorf("httpclient: create request %s %s: %w", method, url, err)
	}

	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	req.Header.Set("Accept", "application/json")

	// Auth header
	if c.GetToken != nil {
		if tok := c.GetToken(); tok != "" {
			req.Header.Set("Authorization", "Bearer "+tok)
		}
	}

	// Extra headers
	if c.ExtraHeaders != nil {
		c.ExtraHeaders(req)
	}

	// Execute
	client := c.HTTPClient
	if client == nil {
		client = http.DefaultClient
	}
	resp, err := client.Do(req)
	if err != nil {
		return fmt.Errorf("httpclient: execute %s %s: %w", method, url, err)
	}
	defer resp.Body.Close()

	// Error response
	if resp.StatusCode >= 400 {
		respBody, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("httpclient: %s %s returned %d: %s", method, url, resp.StatusCode, strings.TrimSpace(string(respBody)))
	}

	// No content
	if resp.StatusCode == http.StatusNoContent || result == nil {
		return nil
	}

	// Decode response
	if err := json.NewDecoder(resp.Body).Decode(result); err != nil {
		return fmt.Errorf("httpclient: decode response from %s %s: %w", method, url, err)
	}

	return nil
}
