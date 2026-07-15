package tobogganing

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"sync"
	"time"

	"github.com/golang-jwt/jwt/v5"
	"github.com/penguintechinc/penguin/pkg/sdk"
	"go.uber.org/zap"
)

// AuthManager handles JWT token acquisition, renewal, and storage.
type AuthManager struct {
	managerURL   string
	secrets      sdk.SecretStore
	logger       *zap.Logger
	httpClient   *http.Client
	mu           sync.RWMutex
	token        string
	refreshToken string
	expiresAt    time.Time
}

// TokenResponse is the API response when requesting or refreshing a token.
type TokenResponse struct {
	AccessToken  string    `json:"access_token"`
	RefreshToken string    `json:"refresh_token"`
	ExpiresAt    time.Time `json:"expires_at"`
	TokenType    string    `json:"token_type"`
}

// NewAuthManager creates a new auth manager.
func NewAuthManager(managerURL string, secrets sdk.SecretStore, logger *zap.Logger) (*AuthManager, error) {
	mgr := &AuthManager{
		managerURL: managerURL,
		secrets:    secrets,
		logger:     logger,
		httpClient: &http.Client{
			Timeout: 30 * time.Second,
		},
	}

	// Try to load cached token from secrets
	_ = mgr.loadCachedToken()

	return mgr, nil
}

// EnsureValidToken ensures we have a valid token, obtaining one if needed.
func (a *AuthManager) EnsureValidToken(ctx context.Context) error {
	a.mu.Lock()
	defer a.mu.Unlock()

	// If we have a valid token, we're done
	if a.token != "" && time.Now().Before(a.expiresAt) {
		return nil
	}

	// Try to refresh using refresh token
	if a.refreshToken != "" {
		if err := a.refreshTokenLocked(ctx); err == nil {
			return nil
		}
		a.logger.Debug("refresh token failed, will obtain new token via API key")
	}

	// Fall back to obtaining new token via API key
	apiKey, err := a.getAPIKey()
	if err != nil {
		return fmt.Errorf("no API key found: %w", err)
	}

	return a.obtainTokenLocked(ctx, apiKey)
}

// RefreshToken refreshes the access token using the stored refresh token.
func (a *AuthManager) RefreshToken(ctx context.Context) error {
	a.mu.Lock()
	defer a.mu.Unlock()

	if a.refreshToken == "" {
		return fmt.Errorf("no refresh token available")
	}

	return a.refreshTokenLocked(ctx)
}

// IsTokenExpired checks if the token will expire within the given threshold.
func (a *AuthManager) IsTokenExpired(threshold time.Duration) bool {
	a.mu.RLock()
	defer a.mu.RUnlock()

	if a.token == "" {
		return true
	}

	return time.Until(a.expiresAt) < threshold
}

// RevokeToken revokes the current token on the server.
func (a *AuthManager) RevokeToken(ctx context.Context) error {
	a.mu.RLock()
	token := a.token
	a.mu.RUnlock()

	if token == "" {
		return nil // Already revoked or never obtained
	}

	// POST /api/v1/auth/revoke with the token
	req, err := http.NewRequestWithContext(ctx, "POST", a.managerURL+"/api/v1/auth/revoke", nil)
	if err != nil {
		return fmt.Errorf("failed to create revoke request: %w", err)
	}

	req.Header.Set("Authorization", "Bearer "+token)

	resp, err := a.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("revoke request failed: %w", err)
	}
	defer func() { _ = resp.Body.Close() }()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("revoke failed with status %d", resp.StatusCode)
	}

	// Clear cached token
	a.mu.Lock()
	a.token = ""
	a.refreshToken = ""
	a.expiresAt = time.Time{}
	a.mu.Unlock()

	_ = a.secrets.Delete("access_token")
	_ = a.secrets.Delete("refresh_token")

	return nil
}

// GetToken returns the current access token (for internal use).
func (a *AuthManager) GetToken() string {
	a.mu.RLock()
	defer a.mu.RUnlock()
	return a.token
}

// Private helpers

func (a *AuthManager) obtainTokenLocked(ctx context.Context, apiKey string) error {
	reqBody := map[string]string{
		"api_key": apiKey,
	}

	jsonData, _ := json.Marshal(reqBody)

	req, err := http.NewRequestWithContext(ctx, "POST", a.managerURL+"/api/v1/auth/token", bytes.NewReader(jsonData))
	if err != nil {
		return fmt.Errorf("failed to create token request: %w", err)
	}

	req.Header.Set("Content-Type", "application/json")

	resp, err := a.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("token request failed: %w", err)
	}
	defer func() { _ = resp.Body.Close() }()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("token request failed with status %d: %s", resp.StatusCode, string(body))
	}

	var tokenResp TokenResponse
	if err := json.NewDecoder(resp.Body).Decode(&tokenResp); err != nil {
		return fmt.Errorf("failed to parse token response: %w", err)
	}

	// If ExpiresAt is not provided, extract from JWT
	if tokenResp.ExpiresAt.IsZero() && tokenResp.AccessToken != "" {
		if exp, err := a.extractTokenExpiry(tokenResp.AccessToken); err == nil {
			tokenResp.ExpiresAt = exp
		}
	}

	a.token = tokenResp.AccessToken
	a.refreshToken = tokenResp.RefreshToken
	a.expiresAt = tokenResp.ExpiresAt

	// Cache tokens in secrets
	_ = a.secrets.Set("access_token", []byte(tokenResp.AccessToken))
	if tokenResp.RefreshToken != "" {
		_ = a.secrets.Set("refresh_token", []byte(tokenResp.RefreshToken))
	}

	a.logger.Debug("obtained new access token")
	return nil
}

func (a *AuthManager) refreshTokenLocked(ctx context.Context) error {
	reqBody := map[string]string{
		"refresh_token": a.refreshToken,
	}

	jsonData, _ := json.Marshal(reqBody)

	req, err := http.NewRequestWithContext(ctx, "POST", a.managerURL+"/api/v1/auth/refresh", bytes.NewReader(jsonData))
	if err != nil {
		return fmt.Errorf("failed to create refresh request: %w", err)
	}

	req.Header.Set("Content-Type", "application/json")

	resp, err := a.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("refresh request failed: %w", err)
	}
	defer func() { _ = resp.Body.Close() }()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("refresh failed with status %d", resp.StatusCode)
	}

	var tokenResp TokenResponse
	if err := json.NewDecoder(resp.Body).Decode(&tokenResp); err != nil {
		return fmt.Errorf("failed to parse refresh response: %w", err)
	}

	// If ExpiresAt is not provided, extract from JWT
	if tokenResp.ExpiresAt.IsZero() && tokenResp.AccessToken != "" {
		if exp, err := a.extractTokenExpiry(tokenResp.AccessToken); err == nil {
			tokenResp.ExpiresAt = exp
		}
	}

	a.token = tokenResp.AccessToken
	if tokenResp.RefreshToken != "" {
		a.refreshToken = tokenResp.RefreshToken
	}
	a.expiresAt = tokenResp.ExpiresAt

	// Update cached tokens
	_ = a.secrets.Set("access_token", []byte(tokenResp.AccessToken))
	if tokenResp.RefreshToken != "" {
		_ = a.secrets.Set("refresh_token", []byte(tokenResp.RefreshToken))
	}

	a.logger.Debug("refreshed access token")
	return nil
}

func (a *AuthManager) extractTokenExpiry(tokenString string) (time.Time, error) {
	token, _, err := new(jwt.Parser).ParseUnverified(tokenString, jwt.MapClaims{})
	if err != nil {
		return time.Time{}, fmt.Errorf("failed to parse token: %w", err)
	}

	if claims, ok := token.Claims.(jwt.MapClaims); ok {
		if exp, ok := claims["exp"].(float64); ok {
			return time.Unix(int64(exp), 0), nil
		}
	}

	return time.Time{}, fmt.Errorf("no expiry found in token")
}

func (a *AuthManager) getAPIKey() (string, error) {
	val, err := a.secrets.Get("api_key")
	if err != nil {
		return "", err
	}
	return string(val), nil
}

func (a *AuthManager) loadCachedToken() error {
	val, err := a.secrets.Get("access_token")
	if err != nil {
		return err
	}

	a.mu.Lock()
	defer a.mu.Unlock()

	a.token = string(val)

	// Try to extract expiry from token
	if exp, err := a.extractTokenExpiry(a.token); err == nil {
		a.expiresAt = exp
	}

	// Load refresh token if available
	if refVal, err := a.secrets.Get("refresh_token"); err == nil {
		a.refreshToken = string(refVal)
	}

	return nil
}
