package tobogganing

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"go.uber.org/zap/zaptest"
)

func TestAuthManagerObtainToken(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/auth/token" && r.Method == "POST" {
			w.Header().Set("Content-Type", "application/json")
			tokenResp := TokenResponse{
				AccessToken:  "test-access-token",
				RefreshToken: "test-refresh-token",
				TokenType:    "Bearer",
				ExpiresAt:    time.Now().Add(1 * time.Hour),
			}
			_ = json.NewEncoder(w).Encode(tokenResp) // #nosec G117 -- test mock server response encoding
			return
		}
		http.NotFound(w, r)
	}))
	defer server.Close()

	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}
	_ = secrets.Set("api_key", []byte("test-api-key"))

	authMgr, err := NewAuthManager(server.URL, secrets, logger)
	if err != nil {
		t.Fatalf("NewAuthManager failed: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := authMgr.EnsureValidToken(ctx); err != nil {
		t.Fatalf("EnsureValidToken failed: %v", err)
	}

	token := authMgr.GetToken()
	if token != "test-access-token" {
		t.Errorf("expected 'test-access-token', got %q", token)
	}

	if authMgr.IsTokenExpired(30 * time.Minute) {
		t.Errorf("token should not be expired")
	}
}

func TestAuthManagerRefreshToken(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/auth/token" && r.Method == "POST" {
			w.Header().Set("Content-Type", "application/json")
			tokenResp := TokenResponse{
				AccessToken:  "test-access-token",
				RefreshToken: "test-refresh-token",
				TokenType:    "Bearer",
				ExpiresAt:    time.Now().Add(1 * time.Hour),
			}
			_ = json.NewEncoder(w).Encode(tokenResp) // #nosec G117 -- test mock server response encoding
			return
		}
		if r.URL.Path == "/api/v1/auth/refresh" && r.Method == "POST" {
			w.Header().Set("Content-Type", "application/json")
			tokenResp := TokenResponse{
				AccessToken:  "refreshed-access-token",
				RefreshToken: "refreshed-refresh-token",
				TokenType:    "Bearer",
				ExpiresAt:    time.Now().Add(1 * time.Hour),
			}
			_ = json.NewEncoder(w).Encode(tokenResp) // #nosec G117 -- test mock server response encoding
			return
		}
		http.NotFound(w, r)
	}))
	defer server.Close()

	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}
	_ = secrets.Set("api_key", []byte("test-api-key"))

	authMgr, err := NewAuthManager(server.URL, secrets, logger)
	if err != nil {
		t.Fatalf("NewAuthManager failed: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Obtain initial token
	if err := authMgr.EnsureValidToken(ctx); err != nil {
		t.Fatalf("EnsureValidToken failed: %v", err)
	}

	// Refresh token
	if err := authMgr.RefreshToken(ctx); err != nil {
		t.Fatalf("RefreshToken failed: %v", err)
	}

	token := authMgr.GetToken()
	if token != "refreshed-access-token" {
		t.Errorf("expected 'refreshed-access-token', got %q", token)
	}
}

func TestAuthManagerRevokeToken(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/auth/token" && r.Method == "POST" {
			w.Header().Set("Content-Type", "application/json")
			tokenResp := TokenResponse{
				AccessToken:  "test-access-token",
				RefreshToken: "test-refresh-token",
				TokenType:    "Bearer",
				ExpiresAt:    time.Now().Add(1 * time.Hour),
			}
			_ = json.NewEncoder(w).Encode(tokenResp) // #nosec G117 -- test mock server response encoding
			return
		}
		if r.URL.Path == "/api/v1/auth/revoke" && r.Method == "POST" {
			w.WriteHeader(http.StatusOK)
			return
		}
		http.NotFound(w, r)
	}))
	defer server.Close()

	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}
	_ = secrets.Set("api_key", []byte("test-api-key"))

	authMgr, err := NewAuthManager(server.URL, secrets, logger)
	if err != nil {
		t.Fatalf("NewAuthManager failed: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Obtain token
	if err := authMgr.EnsureValidToken(ctx); err != nil {
		t.Fatalf("EnsureValidToken failed: %v", err)
	}

	// Revoke token
	if err := authMgr.RevokeToken(ctx); err != nil {
		t.Fatalf("RevokeToken failed: %v", err)
	}

	token := authMgr.GetToken()
	if token != "" {
		t.Errorf("expected empty token after revoke, got %q", token)
	}
}

func TestAuthManagerTokenExpiry(t *testing.T) {
	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}

	authMgr, err := NewAuthManager("http://localhost:8080", secrets, logger)
	if err != nil {
		t.Fatalf("NewAuthManager failed: %v", err)
	}

	// Token not set, should be expired
	if !authMgr.IsTokenExpired(0) {
		t.Errorf("empty token should be expired")
	}

	// Manually set token with future expiry
	authMgr.mu.Lock()
	authMgr.token = "test-token"
	authMgr.expiresAt = time.Now().Add(5 * time.Minute)
	authMgr.mu.Unlock()

	// Should not be expired with 4 minute threshold
	if authMgr.IsTokenExpired(4 * time.Minute) {
		t.Errorf("token should not be expired")
	}

	// Should be expired with 6 minute threshold
	if !authMgr.IsTokenExpired(6 * time.Minute) {
		t.Errorf("token should be expired")
	}
}

func TestAuthManagerNoAPIKey(t *testing.T) {
	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)} // No API key

	authMgr, err := NewAuthManager("http://localhost:8080", secrets, logger)
	if err != nil {
		t.Fatalf("NewAuthManager failed: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Should fail without API key
	err = authMgr.EnsureValidToken(ctx)
	if err == nil {
		t.Errorf("expected error when API key not set")
	}
}

func TestAuthManagerCacheToken(t *testing.T) {
	logger := zaptest.NewLogger(t)
	secrets := &FakeSecretStore{store: make(map[string][]byte)}

	// Pre-populate with a cached token
	token := "cached-token"
	expiryTime := time.Now().Add(30 * time.Minute)

	authMgr, err := NewAuthManager("http://localhost:8080", secrets, logger)
	if err != nil {
		t.Fatalf("NewAuthManager failed: %v", err)
	}

	// Manually set cache
	authMgr.mu.Lock()
	authMgr.token = token
	authMgr.expiresAt = expiryTime
	authMgr.mu.Unlock()

	// Token should be valid
	if authMgr.IsTokenExpired(5 * time.Minute) {
		t.Errorf("cached token should not be expired")
	}

	// GetToken should return cached token
	if authMgr.GetToken() != token {
		t.Errorf("expected cached token")
	}
}
