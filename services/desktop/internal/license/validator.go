package license

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/sirupsen/logrus"
)

// Validator handles license validation with caching.
type Validator struct {
	serverURL  string
	licenseKey string
	userToken  string
	httpClient *http.Client
	logger     *logrus.Logger

	mu       sync.RWMutex
	cache    *cacheEntry
	cacheTTL time.Duration
}

type cacheEntry struct {
	valid     bool
	response  *ValidationResponse
	checkedAt time.Time
}

// ValidationResponse from license server.
type ValidationResponse struct {
	Valid     bool     `json:"valid"`
	Message   string   `json:"message"`
	ExpiresAt string   `json:"expires_at,omitempty"`
	Features  []string `json:"features,omitempty"`
	Plan      string   `json:"plan,omitempty"`
}

// NewValidator creates a license validator.
func NewValidator(serverURL, licenseKey, userToken string, cacheTTL time.Duration, logger *logrus.Logger) *Validator {
	return &Validator{
		serverURL:  strings.TrimRight(serverURL, "/"),
		licenseKey: licenseKey,
		userToken:  userToken,
		cacheTTL:   cacheTTL,
		httpClient: &http.Client{Timeout: 15 * time.Second},
		logger:     logger,
	}
}

// Validate checks license validity, using cache when available.
func (v *Validator) Validate(ctx context.Context) (*ValidationResponse, error) {
	v.mu.RLock()
	if v.cache != nil && time.Since(v.cache.checkedAt) < v.cacheTTL {
		resp := v.cache.response
		v.mu.RUnlock()
		return resp, nil
	}
	v.mu.RUnlock()

	var resp *ValidationResponse
	var err error

	if v.userToken != "" {
		resp, err = v.validateToken(ctx)
	} else if v.licenseKey != "" {
		resp, err = v.validateKey(ctx)
	} else {
		return nil, fmt.Errorf("no license key or user token configured")
	}

	if err != nil {
		// Use cached result during network errors (grace period)
		v.mu.RLock()
		if v.cache != nil && time.Since(v.cache.checkedAt) < 24*time.Hour {
			cached := v.cache.response
			v.mu.RUnlock()
			v.logger.WithError(err).Warn("License validation failed, using cached result")
			return cached, nil
		}
		v.mu.RUnlock()
		return nil, err
	}

	v.mu.Lock()
	v.cache = &cacheEntry{valid: resp.Valid, response: resp, checkedAt: time.Now()}
	v.mu.Unlock()

	return resp, nil
}

func (v *Validator) validateKey(ctx context.Context) (*ValidationResponse, error) {
	payload := fmt.Sprintf(`{"license_key":%q}`, v.licenseKey)
	req, err := http.NewRequestWithContext(ctx, "POST", v.serverURL+"/api/validate", strings.NewReader(payload))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("User-Agent", "PenguinClient/0.1.0")

	resp, err := v.httpClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("license validation request: %w", err)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("reading response: %w", err)
	}

	var result ValidationResponse
	if err := json.Unmarshal(body, &result); err != nil {
		return nil, fmt.Errorf("parsing response: %w", err)
	}
	return &result, nil
}

func (v *Validator) validateToken(ctx context.Context) (*ValidationResponse, error) {
	req, err := http.NewRequestWithContext(ctx, "POST", v.serverURL+"/api/validate_token", nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer "+v.userToken)
	req.Header.Set("User-Agent", "PenguinClient/0.1.0")

	resp, err := v.httpClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("token validation request: %w", err)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("reading response: %w", err)
	}

	var result ValidationResponse
	if err := json.Unmarshal(body, &result); err != nil {
		return nil, fmt.Errorf("parsing response: %w", err)
	}
	return &result, nil
}

// IsValid returns whether the license is currently valid.
func (v *Validator) IsValid(ctx context.Context) bool {
	resp, err := v.Validate(ctx)
	if err != nil {
		return false
	}
	return resp.Valid
}

// ClearCache clears the validation cache.
func (v *Validator) ClearCache() {
	v.mu.Lock()
	v.cache = nil
	v.mu.Unlock()
}
