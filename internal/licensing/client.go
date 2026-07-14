// Package licensing provides PenguinTech License Server integration with
// offline caching and graceful degradation.
package licensing

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"sync"
	"time"

	"go.uber.org/zap"
)

// Feature represents a licensed feature.
type Feature struct {
	Name        string            `json:"name"`
	Entitled    bool              `json:"entitled"`
	Units       int               `json:"units"`
	Description string            `json:"description"`
	Metadata    map[string]string `json:"metadata"`
}

// LicenseInfo represents the license state from the server.
type LicenseInfo struct {
	Valid          bool              `json:"valid"`
	Customer       string            `json:"customer"`
	Product        string            `json:"product"`
	LicenseVersion string            `json:"license_version"`
	LicenseKey     string            `json:"license_key"`
	ExpiresAt      time.Time         `json:"expires_at"`
	IssuedAt       time.Time         `json:"issued_at"`
	Tier           string            `json:"tier"`
	Features       []Feature         `json:"features"`
	Limits         map[string]int    `json:"limits"`
	Metadata       map[string]string `json:"metadata"`
	ServerID       string            `json:"server_id"`
	Message        string            `json:"message"`
}

// cacheFile represents the persisted cache format.
type cacheFile struct {
	Tier      string          `json:"tier"`
	Features  map[string]bool `json:"features"`
	FetchedAt time.Time       `json:"fetched_at"`
}

// Options configures a Client.
type Options struct {
	LicenseKey      string
	Product         string
	BaseURL         string
	CacheDir        string
	HTTPClient      *http.Client
	RefreshInterval time.Duration
	Logger          *zap.Logger
}

// Client implements sdk.LicenseChecker with offline caching and graceful degradation.
type Client struct {
	licenseKey      string
	product         string
	baseURL         string
	cacheDir        string
	httpClient      *http.Client
	refreshInterval time.Duration
	logger          *zap.Logger

	mu             sync.RWMutex
	cachedTier     string
	cachedFeatures map[string]bool
	cachedAt       time.Time
	stopCh         chan struct{}
	doneWg         sync.WaitGroup
}

// New creates a new license client with the given options.
func New(opts Options) *Client {
	if opts.Product == "" {
		opts.Product = "penguin"
	}
	if opts.BaseURL == "" {
		opts.BaseURL = "https://license.penguintech.io"
	}
	if opts.HTTPClient == nil {
		opts.HTTPClient = &http.Client{
			Timeout: 10 * time.Second,
		}
	}
	if opts.RefreshInterval == 0 {
		opts.RefreshInterval = 5 * time.Minute
	}
	if opts.Logger == nil {
		opts.Logger = zap.NewNop()
	}

	c := &Client{
		licenseKey:      opts.LicenseKey,
		product:         opts.Product,
		baseURL:         opts.BaseURL,
		cacheDir:        opts.CacheDir,
		httpClient:      opts.HTTPClient,
		refreshInterval: opts.RefreshInterval,
		logger:          opts.Logger,
		cachedFeatures:  make(map[string]bool),
		stopCh:          make(chan struct{}),
	}

	// Load cache from disk if available
	_ = c.loadCache()

	return c
}

// FeatureEnabled reports whether a feature flag is enabled.
// Unknown or unfetchable flags return false.
func (c *Client) FeatureEnabled(key string) bool {
	c.mu.RLock()
	defer c.mu.RUnlock()

	// If no license key configured, all features disabled
	if c.licenseKey == "" {
		return false
	}

	// Check cache first
	enabled, exists := c.cachedFeatures[key]
	if exists {
		return enabled
	}

	// Unknown flag defaults to false
	return false
}

// Tier returns the current license tier.
func (c *Client) Tier() string {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.cachedTier
}

// Validate fetches the current license state from the server.
// If the server is unreachable, returns cached state (graceful degradation).
func (c *Client) Validate(ctx context.Context) error {
	if c.licenseKey == "" {
		c.mu.Lock()
		c.cachedTier = ""
		c.cachedFeatures = make(map[string]bool)
		c.mu.Unlock()
		return nil
	}

	info, err := c.fetch(ctx)
	if err != nil {
		c.logger.Debug("failed to fetch license", zap.Error(err))
		// Graceful degradation: keep using cached state
		return nil
	}

	c.updateCache(info)
	return nil
}

// Start begins background license refresh.
func (c *Client) Start(ctx context.Context) error {
	c.doneWg.Add(1)
	go func() {
		defer c.doneWg.Done()
		ticker := time.NewTicker(c.refreshInterval)
		defer ticker.Stop()

		// Do initial fetch
		_ = c.Validate(ctx)

		for {
			select {
			case <-ticker.C:
				_ = c.Validate(ctx)
			case <-c.stopCh:
				return
			case <-ctx.Done():
				return
			}
		}
	}()
	return nil
}

// Stop halts the background refresh loop.
func (c *Client) Stop() error {
	close(c.stopCh)
	c.doneWg.Wait()
	return nil
}

// fetch retrieves license state from the server.
func (c *Client) fetch(ctx context.Context) (*LicenseInfo, error) {
	url := c.baseURL + "/api/v2/validate"

	payload := map[string]string{"product": c.product}
	body, err := json.Marshal(payload)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal request: %w", err)
	}

	req, err := http.NewRequestWithContext(ctx, "POST", url, bytes.NewReader(body))
	if err != nil {
		return nil, fmt.Errorf("failed to create request: %w", err)
	}

	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer "+c.licenseKey)

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("license server unreachable: %w", err)
	}
	defer func() { _ = resp.Body.Close() }()

	respBody, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("failed to read response: %w", err)
	}

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("license server returned %d: %s", resp.StatusCode, string(respBody))
	}

	var info LicenseInfo
	if err := json.Unmarshal(respBody, &info); err != nil {
		return nil, fmt.Errorf("failed to parse response: %w", err)
	}

	return &info, nil
}

// updateCache updates in-memory cache and persists to disk.
func (c *Client) updateCache(info *LicenseInfo) {
	c.mu.Lock()
	defer c.mu.Unlock()

	c.cachedTier = info.Tier
	c.cachedFeatures = make(map[string]bool)
	for _, f := range info.Features {
		c.cachedFeatures[f.Name] = f.Entitled
	}
	c.cachedAt = time.Now()

	// Persist to disk (best-effort)
	_ = c.persistCache()
}

// cachePath returns the on-disk cache file location.
func (c *Client) cachePath() string {
	return filepath.Join(c.cacheDir, "license-cache.json")
}

// persistCache writes the cache to disk atomically (temp file + rename,
// 0600). Callers must hold c.mu.
func (c *Client) persistCache() error {
	if c.cacheDir == "" {
		return nil
	}

	data := cacheFile{
		Tier:      c.cachedTier,
		Features:  c.cachedFeatures,
		FetchedAt: c.cachedAt,
	}

	raw, err := json.MarshalIndent(data, "", "  ")
	if err != nil {
		return fmt.Errorf("marshal license cache: %w", err)
	}

	if err := os.MkdirAll(c.cacheDir, 0o700); err != nil {
		return fmt.Errorf("mkdir license cache dir: %w", err)
	}
	tmp, err := os.CreateTemp(c.cacheDir, ".license-cache-*")
	if err != nil {
		return fmt.Errorf("create license cache temp: %w", err)
	}
	tmpPath := tmp.Name()
	cleanup := func() {
		_ = tmp.Close()
		_ = os.Remove(tmpPath)
	}
	if err := tmp.Chmod(0o600); err != nil {
		cleanup()
		return fmt.Errorf("chmod license cache: %w", err)
	}
	if _, err := tmp.Write(raw); err != nil {
		cleanup()
		return fmt.Errorf("write license cache: %w", err)
	}
	if err := tmp.Close(); err != nil {
		_ = os.Remove(tmpPath)
		return fmt.Errorf("close license cache: %w", err)
	}
	if err := os.Rename(tmpPath, c.cachePath()); err != nil {
		_ = os.Remove(tmpPath)
		return fmt.Errorf("rename license cache: %w", err)
	}
	return nil
}

// loadCache restores the last-known license state from disk so that a daemon
// restart while the license server is unreachable keeps prior entitlements
// (graceful degradation). A missing or corrupt cache is not an error: the
// client simply starts with everything disabled.
func (c *Client) loadCache() error {
	if c.cacheDir == "" {
		return nil
	}

	raw, err := os.ReadFile(c.cachePath()) // #nosec G304 -- daemon-owned cache dir from config
	if err != nil {
		return nil // missing cache: start cold
	}

	var data cacheFile
	if err := json.Unmarshal(raw, &data); err != nil {
		return nil // corrupt cache: ignore, will be overwritten on next fetch
	}

	c.mu.Lock()
	defer c.mu.Unlock()
	c.cachedTier = data.Tier
	if data.Features != nil {
		c.cachedFeatures = data.Features
	}
	c.cachedAt = data.FetchedAt
	return nil
}
