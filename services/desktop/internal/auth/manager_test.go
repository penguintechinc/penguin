package auth

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/golang-jwt/jwt/v5"
	"github.com/sirupsen/logrus"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func newTestManager(t *testing.T, handler http.Handler) (*Manager, *httptest.Server) {
	t.Helper()
	server := httptest.NewServer(handler)
	logger := logrus.New()
	logger.SetOutput(io.Discard)
	manager := NewManager(server.URL, logger)
	return manager, server
}

func TestManager_Login(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		assert.Equal(t, "/api/v1/auth/login", r.URL.Path)
		var reqBody map[string]string
		err := json.NewDecoder(r.Body).Decode(&reqBody)
		require.NoError(t, err)
		assert.Equal(t, "testuser", reqBody["username"])
		assert.Equal(t, "testpass", reqBody["password"])

		resp := TokenResponse{
			AccessToken:  "access-token",
			RefreshToken: "refresh-token",
			ExpiresAt:    time.Now().Add(1 * time.Hour),
			TokenType:    "Bearer",
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	})

	manager, server := newTestManager(t, handler)
	defer server.Close()

	err := manager.Login("testuser", "testpass")
	require.NoError(t, err)

	assert.True(t, manager.IsAuthenticated())
	token, err := manager.AccessToken()
	require.NoError(t, err)
	assert.Equal(t, "access-token", token)
}

func TestManager_GetToken(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		assert.Equal(t, "/api/v1/auth/token", r.URL.Path)
		var reqBody map[string]string
		err := json.NewDecoder(r.Body).Decode(&reqBody)
		require.NoError(t, err)
		assert.Equal(t, "node-1", reqBody["node_id"])
		assert.Equal(t, "type-a", reqBody["node_type"])
		assert.Equal(t, "api-key", reqBody["api_key"])

		resp := TokenResponse{
			AccessToken:  "service-token",
			RefreshToken: "service-refresh",
			ExpiresAt:    time.Now().Add(1 * time.Hour),
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	})

	manager, server := newTestManager(t, handler)
	defer server.Close()

	err := manager.GetToken("node-1", "type-a", "api-key")
	require.NoError(t, err)
	assert.Equal(t, "service-token", manager.accessToken)
}

func TestManager_Refresh(t *testing.T) {
	var refreshCount int
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/auth/refresh" {
			refreshCount++
			var reqBody map[string]string
			err := json.NewDecoder(r.Body).Decode(&reqBody)
			require.NoError(t, err)
			assert.Equal(t, "old-refresh-token", reqBody["refresh_token"])

			resp := TokenResponse{
				AccessToken:  "new-access-token",
				RefreshToken: "new-refresh-token",
				ExpiresAt:    time.Now().Add(1 * time.Hour),
			}
			json.NewEncoder(w).Encode(resp)
		}
	})

	manager, server := newTestManager(t, handler)
	defer server.Close()

	manager.refreshToken = "old-refresh-token"
	err := manager.Refresh()
	require.NoError(t, err)

	assert.Equal(t, 1, refreshCount)
	assert.Equal(t, "new-access-token", manager.accessToken)
	assert.Equal(t, "new-refresh-token", manager.refreshToken)
}

func TestManager_AccessToken_AutoRefresh(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/v1/auth/refresh" {
			resp := TokenResponse{AccessToken: "refreshed-token", ExpiresAt: time.Now().Add(1 * time.Hour)}
			json.NewEncoder(w).Encode(resp)
		}
	})

	manager, server := newTestManager(t, handler)
	defer server.Close()

	// Set a token that is about to expire
	manager.accessToken = "expiring-token"
	manager.refreshToken = "a-refresh-token" // Provide a refresh token
	manager.expiresAt = time.Now().Add(4 * time.Minute)

	token, err := manager.AccessToken()
	require.NoError(t, err)
	assert.Equal(t, "refreshed-token", token)
}

func TestManager_Claims(t *testing.T) {
	// Create a sample JWT token
	claims := jwt.MapClaims{
		"sub":  "12345",
		"name": "Test User",
		"exp":  time.Now().Add(1 * time.Hour).Unix(),
	}
	token := jwt.NewWithClaims(jwt.SigningMethodHS256, claims)
	tokenString, err := token.SignedString([]byte("secret"))
	require.NoError(t, err)

	manager, server := newTestManager(t, nil)
	defer server.Close()
	manager.accessToken = tokenString

	parsedClaims, err := manager.Claims()
	require.NoError(t, err)
	assert.Equal(t, "12345", parsedClaims["sub"])
	assert.Equal(t, "Test User", parsedClaims["name"])
}

func TestManager_Logout(t *testing.T) {
	manager, server := newTestManager(t, nil)
	defer server.Close()

	manager.accessToken = "some-token"
	manager.expiresAt = time.Now().Add(1 * time.Hour)
	assert.True(t, manager.IsAuthenticated())

	manager.Logout()
	assert.False(t, manager.IsAuthenticated())
	assert.Empty(t, manager.accessToken)
}

// Negative Test Cases
func TestManager_Login_Fail(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusUnauthorized)
		fmt.Fprint(w, "Invalid credentials")
	})
	manager, server := newTestManager(t, handler)
	defer server.Close()

	err := manager.Login("user", "wrongpass")
	require.Error(t, err)
	assert.Contains(t, err.Error(), "login failed (status 401)")
}

func TestManager_Refresh_NoToken(t *testing.T) {
	manager, server := newTestManager(t, nil)
	defer server.Close()
	err := manager.Refresh()
	assert.EqualError(t, err, "no refresh token available")
}

func TestManager_Login_BadRequest(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusBadRequest)
	})
	manager, server := newTestManager(t, handler)
	defer server.Close()

	err := manager.Login("user", "pass")
	require.Error(t, err)
	assert.Contains(t, err.Error(), "login failed (status 400)")
}

func TestManager_GetToken_Fail(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusUnauthorized)
	})
	manager, server := newTestManager(t, handler)
	defer server.Close()

	err := manager.GetToken("node", "type", "key")
	require.Error(t, err)
	assert.Contains(t, err.Error(), "token request failed (status 401)")
}

func TestManager_Refresh_Fail(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusUnauthorized)
	})
	manager, server := newTestManager(t, handler)
	defer server.Close()
	manager.refreshToken = "some-token"
	err := manager.Refresh()
	require.Error(t, err)
	assert.Contains(t, err.Error(), "refresh failed (status 401)")
}

func TestManager_AccessToken_NotAuthenticated(t *testing.T) {
	manager, server := newTestManager(t, nil)
	defer server.Close()
	_, err := manager.AccessToken()
	assert.EqualError(t, err, "not authenticated")
}

func TestManager_Claims_NoToken(t *testing.T) {
	manager, server := newTestManager(t, nil)
	defer server.Close()
	_, err := manager.Claims()
	assert.EqualError(t, err, "not authenticated")
}

func TestManager_Claims_InvalidToken(t *testing.T) {
	manager, server := newTestManager(t, nil)
	defer server.Close()
	manager.accessToken = "invalid-token"
	_, err := manager.Claims()
	require.Error(t, err)
	assert.Contains(t, err.Error(), "parsing token:")
}

func TestManager_AccessToken_RefreshFail(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusUnauthorized)
	})
	manager, server := newTestManager(t, handler)
	defer server.Close()

	manager.accessToken = "expiring-token"
	manager.refreshToken = "a-refresh-token"
	manager.expiresAt = time.Now().Add(4 * time.Minute)

	_, err := manager.AccessToken()
	require.Error(t, err)
	assert.Contains(t, err.Error(), "refreshing token")
}

func TestManager_Login_InvalidJSON(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		fmt.Fprint(w, "not-json")
	})
	manager, server := newTestManager(t, handler)
	defer server.Close()

	err := manager.Login("user", "pass")
	require.Error(t, err)
	assert.Contains(t, err.Error(), "decoding login response")
}
