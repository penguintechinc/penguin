package auth

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/golang-jwt/jwt/v5"
	"github.com/sirupsen/logrus"
)

// Manager handles JWT authentication across services.
type Manager struct {
	serverURL  string
	httpClient *http.Client
	logger     *logrus.Logger

	mu           sync.RWMutex
	accessToken  string
	refreshToken string
	expiresAt    time.Time
}

// TokenResponse represents a token response from the auth server.
type TokenResponse struct {
	AccessToken  string    `json:"access_token"`
	RefreshToken string    `json:"refresh_token"`
	ExpiresAt    time.Time `json:"expires_at"`
	TokenType    string    `json:"token_type"`
}

// NewManager creates a new auth manager.
func NewManager(serverURL string, logger *logrus.Logger) *Manager {
	return &Manager{
		serverURL: strings.TrimRight(serverURL, "/"),
		httpClient: &http.Client{
			Timeout: 30 * time.Second,
		},
		logger: logger,
	}
}

// Login authenticates with username and password.
func (m *Manager) Login(username, password string) error {
	payload := fmt.Sprintf(`{"username":%q,"password":%q}`, username, password)
	req, err := http.NewRequest("POST", m.serverURL+"/api/v1/auth/login", strings.NewReader(payload))
	if err != nil {
		return fmt.Errorf("creating login request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")

	resp, err := m.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("login request: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("login failed (status %d): %s", resp.StatusCode, string(body))
	}

	var tokenResp TokenResponse
	if err := json.NewDecoder(resp.Body).Decode(&tokenResp); err != nil {
		return fmt.Errorf("decoding login response: %w", err)
	}

	m.mu.Lock()
	m.accessToken = tokenResp.AccessToken
	m.refreshToken = tokenResp.RefreshToken
	m.expiresAt = tokenResp.ExpiresAt
	m.mu.Unlock()

	m.logger.Info("Authentication successful")
	return nil
}

// GetToken authenticates with node credentials (for service-to-service).
func (m *Manager) GetToken(nodeID, nodeType, apiKey string) error {
	payload := fmt.Sprintf(`{"node_id":%q,"node_type":%q,"api_key":%q}`, nodeID, nodeType, apiKey)
	req, err := http.NewRequest("POST", m.serverURL+"/api/v1/auth/token", strings.NewReader(payload))
	if err != nil {
		return fmt.Errorf("creating token request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")

	resp, err := m.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("token request: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("token request failed (status %d): %s", resp.StatusCode, string(body))
	}

	var tokenResp TokenResponse
	if err := json.NewDecoder(resp.Body).Decode(&tokenResp); err != nil {
		return fmt.Errorf("decoding token response: %w", err)
	}

	m.mu.Lock()
	m.accessToken = tokenResp.AccessToken
	m.refreshToken = tokenResp.RefreshToken
	m.expiresAt = tokenResp.ExpiresAt
	m.mu.Unlock()

	return nil
}

// Refresh refreshes the access token using the refresh token.
func (m *Manager) Refresh() error {
	m.mu.RLock()
	rt := m.refreshToken
	m.mu.RUnlock()

	if rt == "" {
		return fmt.Errorf("no refresh token available")
	}

	payload := fmt.Sprintf(`{"refresh_token":%q}`, rt)
	req, err := http.NewRequest("POST", m.serverURL+"/api/v1/auth/refresh", strings.NewReader(payload))
	if err != nil {
		return fmt.Errorf("creating refresh request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")

	resp, err := m.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("refresh request: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("refresh failed (status %d): %s", resp.StatusCode, string(body))
	}

	var tokenResp TokenResponse
	if err := json.NewDecoder(resp.Body).Decode(&tokenResp); err != nil {
		return fmt.Errorf("decoding refresh response: %w", err)
	}

	m.mu.Lock()
	m.accessToken = tokenResp.AccessToken
	m.refreshToken = tokenResp.RefreshToken
	m.expiresAt = tokenResp.ExpiresAt
	m.mu.Unlock()

	m.logger.Debug("Token refreshed successfully")
	return nil
}

// AccessToken returns the current access token, refreshing if needed.
func (m *Manager) AccessToken() (string, error) {
	m.mu.RLock()
	token := m.accessToken
	expires := m.expiresAt
	m.mu.RUnlock()

	if token == "" {
		return "", fmt.Errorf("not authenticated")
	}

	// Refresh if expiring within 5 minutes
	if time.Until(expires) < 5*time.Minute {
		if err := m.Refresh(); err != nil {
			return "", fmt.Errorf("refreshing token: %w", err)
		}
		m.mu.RLock()
		token = m.accessToken
		m.mu.RUnlock()
	}

	return token, nil
}

// IsAuthenticated checks if we have a valid token.
func (m *Manager) IsAuthenticated() bool {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.accessToken != "" && time.Now().Before(m.expiresAt)
}

// Claims returns the parsed claims from the current access token.
func (m *Manager) Claims() (jwt.MapClaims, error) {
	m.mu.RLock()
	token := m.accessToken
	m.mu.RUnlock()

	if token == "" {
		return nil, fmt.Errorf("not authenticated")
	}

	// Parse without verification (server-side validation already done)
	parsed, _, err := jwt.NewParser().ParseUnverified(token, jwt.MapClaims{})
	if err != nil {
		return nil, fmt.Errorf("parsing token: %w", err)
	}

	claims, ok := parsed.Claims.(jwt.MapClaims)
	if !ok {
		return nil, fmt.Errorf("invalid claims format")
	}
	return claims, nil
}

// Logout clears stored tokens.
func (m *Manager) Logout() {
	m.mu.Lock()
	m.accessToken = ""
	m.refreshToken = ""
	m.expiresAt = time.Time{}
	m.mu.Unlock()
	m.logger.Info("Logged out")
}
