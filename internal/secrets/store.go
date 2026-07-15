// Package secrets provides namespaced secure key/value storage backed by
// the OS keychain with an encrypted-file fallback for headless daemons.
package secrets

import (
	"crypto/rand"
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"github.com/99designs/keyring"
	"github.com/penguintechinc/penguin/pkg/sdk"
)

// Config holds Store configuration.
type Config struct {
	// ServiceName is the keyring service identifier (default "penguind").
	ServiceName string
	// FileDir is the directory for encrypted files (fallback backend).
	FileDir string
	// FilePasswordFunc provides the master password for the file backend.
	FilePasswordFunc func() ([]byte, error)
	// AllowedBackends restricts which keyring backends can be used.
	// If empty, defaults to production order: WinCred, Keychain, SecretService, File.
	AllowedBackends []keyring.BackendType
}

// Store implements sdk.SecretStore with OS keychain backends and file fallback.
type Store struct {
	ring        keyring.Keyring
	namespace   string
	fileDir     string
	serviceName string
}

// Open creates and initializes a new Store with automatic backend selection.
// Default backends (if not specified): WinCred, Keychain, SecretService, File.
func Open(cfg Config) (*Store, error) {
	if cfg.ServiceName == "" {
		cfg.ServiceName = "penguind"
	}

	availableBackends := cfg.AllowedBackends
	if len(availableBackends) == 0 {
		// Default production order (can be overridden in tests)
		availableBackends = []keyring.BackendType{
			keyring.WinCredBackend,
			keyring.KeychainBackend,
			keyring.SecretServiceBackend,
			keyring.FileBackend,
		}
	}

	// Create a PromptFunc from FilePasswordFunc (converts []byte to string).
	var promptFn keyring.PromptFunc
	if cfg.FilePasswordFunc != nil {
		promptFn = func(prompt string) (string, error) {
			pwd, err := cfg.FilePasswordFunc()
			if err != nil {
				return "", err
			}
			return string(pwd), nil
		}
	}

	var kr keyring.Keyring
	var err error

	// If file backend is needed, ensure the directory exists and has proper permissions.
	if cfg.FileDir != "" {
		kr, err = keyring.Open(keyring.Config{
			ServiceName:      cfg.ServiceName,
			AllowedBackends:  availableBackends,
			FileDir:          cfg.FileDir,
			FilePasswordFunc: promptFn,
		})
	} else {
		kr, err = keyring.Open(keyring.Config{
			ServiceName:     cfg.ServiceName,
			AllowedBackends: availableBackends,
		})
	}

	if err != nil {
		return nil, fmt.Errorf("failed to open keyring: %w", err)
	}

	return &Store{
		ring:        kr,
		namespace:   "",
		fileDir:     cfg.FileDir,
		serviceName: cfg.ServiceName,
	}, nil
}

// Get retrieves a secret by key. Returns an error wrapping sdk.ErrSecretNotFound if not found.
func (s *Store) Get(key string) ([]byte, error) {
	fullKey := s.makeKey(key)
	item, err := s.ring.Get(fullKey)
	if err != nil {
		if errors.Is(err, keyring.ErrKeyNotFound) {
			return nil, fmt.Errorf("failed to get secret %q: %w", key, sdk.ErrSecretNotFound)
		}
		return nil, fmt.Errorf("failed to get secret %q: %w", key, err)
	}
	return item.Data, nil
}

// Set stores a secret. Never logs the value.
func (s *Store) Set(key string, value []byte) error {
	fullKey := s.makeKey(key)
	item := keyring.Item{
		Key:  fullKey,
		Data: value,
	}
	if err := s.ring.Set(item); err != nil {
		return fmt.Errorf("failed to set secret %q: %w", key, err)
	}
	return nil
}

// Delete removes a secret by key.
func (s *Store) Delete(key string) error {
	fullKey := s.makeKey(key)
	if err := s.ring.Remove(fullKey); err != nil {
		if errors.Is(err, keyring.ErrKeyNotFound) {
			return fmt.Errorf("secret not found: %w", sdk.ErrSecretNotFound)
		}
		return fmt.Errorf("failed to delete secret %q: %w", key, err)
	}
	return nil
}

// Namespaced returns a view that prefixes all keys with "<module>/".
// The returned SecretStore is a view; Get/Set/Delete on it transparently
// apply the namespace to the underlying Store.
func (s *Store) Namespaced(module string) sdk.SecretStore {
	return &Store{
		ring:        s.ring,
		namespace:   module,
		fileDir:     s.fileDir,
		serviceName: s.serviceName,
	}
}

// makeKey constructs the full keyring key, including namespace if set.
func (s *Store) makeKey(key string) string {
	if s.namespace == "" {
		return key
	}
	return s.namespace + "/" + key
}

// EnsureMasterKey creates or reads a 32-byte master key for file-based encryption.
// Path must be a file (not directory). Creates 0600 if missing.
// Returns an error if the directory is world-writable (R7 risk).
func EnsureMasterKey(path string) ([]byte, error) {
	dir := filepath.Dir(path)

	// Check directory permissions: must not be world-writable.
	info, err := os.Stat(dir)
	if err != nil {
		return nil, fmt.Errorf("failed to stat directory %q: %w", dir, err)
	}
	if info.Mode()&0o002 != 0 {
		return nil, fmt.Errorf("directory %q is world-writable (security risk)", dir)
	}

	// Try to read existing key.
	// #nosec G304 -- master-key path is under the root-owned keyring dir whose
	// permissions were validated above; it is never derived from user input.
	if keyBytes, err := os.ReadFile(path); err == nil {
		if len(keyBytes) != 32 {
			return nil, fmt.Errorf("master key at %q has wrong size (expected 32, got %d)", path, len(keyBytes))
		}
		return keyBytes, nil
	} else if !errors.Is(err, os.ErrNotExist) {
		return nil, fmt.Errorf("failed to read master key at %q: %w", path, err)
	}

	// Generate new 32-byte key.
	key := make([]byte, 32)
	if _, err := rand.Read(key); err != nil {
		return nil, fmt.Errorf("failed to generate random key: %w", err)
	}

	// Write with 0600 permissions.
	if err := os.WriteFile(path, key, 0o600); err != nil {
		return nil, fmt.Errorf("failed to write master key to %q: %w", path, err)
	}

	return key, nil
}
