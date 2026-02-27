package license

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"time"
)

// DiskCache persists license validation to disk for offline use.
type DiskCache struct {
	path string
}

// CachedValidation is a persisted validation result.
type CachedValidation struct {
	Response  ValidationResponse `json:"response"`
	CheckedAt time.Time          `json:"checked_at"`
}

// NewDiskCache creates a disk-backed license cache.
func NewDiskCache(cacheDir string) *DiskCache {
	return &DiskCache{
		path: filepath.Join(cacheDir, "license_cache.json"),
	}
}

// Save persists a validation result.
func (dc *DiskCache) Save(resp *ValidationResponse) error {
	dir := filepath.Dir(dc.path)
	if err := os.MkdirAll(dir, 0700); err != nil {
		return fmt.Errorf("creating cache dir: %w", err)
	}

	entry := CachedValidation{Response: *resp, CheckedAt: time.Now()}
	data, err := json.Marshal(entry)
	if err != nil {
		return fmt.Errorf("marshaling cache: %w", err)
	}
	return os.WriteFile(dc.path, data, 0600)
}

// Load reads the cached validation result.
func (dc *DiskCache) Load() (*CachedValidation, error) {
	data, err := os.ReadFile(dc.path)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, nil
		}
		return nil, fmt.Errorf("reading cache: %w", err)
	}

	var entry CachedValidation
	if err := json.Unmarshal(data, &entry); err != nil {
		return nil, fmt.Errorf("parsing cache: %w", err)
	}
	return &entry, nil
}
