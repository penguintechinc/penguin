package auth

import (
	"io"
	"os"
	"testing"

	"github.com/sirupsen/logrus"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func newTestKeyringStore(t *testing.T) *KeyringStore {
	t.Helper()
	tempDir := t.TempDir()
	logger := logrus.New()
	logger.SetOutput(io.Discard)
	return NewKeyringStore(tempDir, logger)
}

func TestKeyringStore_SaveLoad(t *testing.T) {
	ks := newTestKeyringStore(t)
	creds := &StoredCredentials{
		Username:     "testuser",
		RefreshToken: "test-refresh-token",
		APIKey:       "test-api-key",
		NodeID:       "test-node-id",
	}

	err := ks.Save(creds)
	require.NoError(t, err)

	loadedCreds, err := ks.Load()
	require.NoError(t, err)
	require.NotNil(t, loadedCreds)

	assert.Equal(t, "testuser", loadedCreds.Username)
	assert.Equal(t, "test-refresh-token", loadedCreds.RefreshToken)
	assert.Equal(t, "test-api-key", loadedCreds.APIKey)
	assert.Equal(t, "test-node-id", loadedCreds.NodeID)
}

func TestKeyringStore_Load_NotExist(t *testing.T) {
	ks := newTestKeyringStore(t)
	creds, err := ks.Load()
	require.NoError(t, err)
	assert.Nil(t, creds)
}

func TestKeyringStore_Clear(t *testing.T) {
	ks := newTestKeyringStore(t)
	creds := &StoredCredentials{Username: "test"}
	err := ks.Save(creds)
	require.NoError(t, err)

	_, err = os.Stat(ks.storePath)
	require.False(t, os.IsNotExist(err))

	err = ks.Clear()
	require.NoError(t, err)

	_, err = os.Stat(ks.storePath)
	assert.True(t, os.IsNotExist(err))
}

func TestKeyringStore_Save_DirError(t *testing.T) {
	// Using a path that is not permitted to write to
	ks := NewKeyringStore("/root/unauthorized", logrus.New())
	err := ks.Save(&StoredCredentials{})
	require.Error(t, err)
	assert.Contains(t, err.Error(), "creating store dir")
}
