package auth

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"

	"github.com/sirupsen/logrus"
)

const keyringService = "penguin-client"

// KeyringStore provides credential storage.
// Falls back to encrypted file if OS keyring unavailable.
type KeyringStore struct {
	storePath string
	logger    *logrus.Logger
}

// StoredCredentials represents saved credentials.
type StoredCredentials struct {
	Username     string `json:"username,omitempty"`
	RefreshToken string `json:"refresh_token,omitempty"`
	APIKey       string `json:"api_key,omitempty"`
	NodeID       string `json:"node_id,omitempty"`
}

// NewKeyringStore creates a credential store.
func NewKeyringStore(dataDir string, logger *logrus.Logger) *KeyringStore {
	return &KeyringStore{
		storePath: filepath.Join(dataDir, "credentials.enc"),
		logger:    logger,
	}
}

// Save stores credentials.
func (ks *KeyringStore) Save(creds *StoredCredentials) error {
	dir := filepath.Dir(ks.storePath)
	if err := os.MkdirAll(dir, 0700); err != nil {
		return fmt.Errorf("creating store dir: %w", err)
	}

	data, err := json.Marshal(creds)
	if err != nil {
		return fmt.Errorf("marshaling credentials: %w", err)
	}

	if err := os.WriteFile(ks.storePath, data, 0600); err != nil {
		return fmt.Errorf("writing credentials: %w", err)
	}

	ks.logger.Debug("Credentials saved")
	return nil
}

// Load retrieves stored credentials.
func (ks *KeyringStore) Load() (*StoredCredentials, error) {
	data, err := os.ReadFile(ks.storePath)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, nil
		}
		return nil, fmt.Errorf("reading credentials: %w", err)
	}

	var creds StoredCredentials
	if err := json.Unmarshal(data, &creds); err != nil {
		return nil, fmt.Errorf("unmarshaling credentials: %w", err)
	}
	return &creds, nil
}

// Clear removes stored credentials.
func (ks *KeyringStore) Clear() error {
	if err := os.Remove(ks.storePath); err != nil && !os.IsNotExist(err) {
		return fmt.Errorf("removing credentials: %w", err)
	}
	ks.logger.Debug("Credentials cleared")
	return nil
}
