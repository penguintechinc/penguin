package licensing

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

// TestCachePersistRestore verifies the offline-degradation contract across a
// client restart: entitlements fetched once must survive with no server.
func TestCachePersistRestore(t *testing.T) {
	dir := t.TempDir()

	c := New(Options{LicenseKey: "lic_test_1234", CacheDir: dir})
	c.mu.Lock()
	c.cachedTier = "enterprise"
	c.cachedFeatures = map[string]bool{"penguin.squawk": true, "penguin.off": false}
	c.cachedAt = time.Now()
	if err := c.persistCache(); err != nil {
		c.mu.Unlock()
		t.Fatalf("persistCache: %v", err)
	}
	c.mu.Unlock()

	// File exists with owner-only perms.
	info, err := os.Stat(filepath.Join(dir, "license-cache.json"))
	if err != nil {
		t.Fatalf("cache file missing: %v", err)
	}
	if info.Mode().Perm() != 0o600 {
		t.Errorf("cache perms = %o, want 0600", info.Mode().Perm())
	}

	// Fresh client (simulated daemon restart, server unreachable) restores it.
	c2 := New(Options{LicenseKey: "lic_test_1234", CacheDir: dir})
	if !c2.FeatureEnabled("penguin.squawk") {
		t.Error("penguin.squawk should be enabled from restored cache")
	}
	if c2.FeatureEnabled("penguin.off") {
		t.Error("penguin.off should stay disabled")
	}
	if got := c2.Tier(); got != "enterprise" {
		t.Errorf("Tier = %q, want enterprise", got)
	}
}

// TestCacheCorruptIgnored verifies a corrupt cache starts cold, not crashed.
func TestCacheCorruptIgnored(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "license-cache.json"), []byte("{not json"), 0o600); err != nil {
		t.Fatal(err)
	}
	c := New(Options{LicenseKey: "lic_test_1234", CacheDir: dir})
	if c.FeatureEnabled("anything") {
		t.Error("corrupt cache must yield all-disabled state")
	}
	if c.Tier() != "" {
		t.Errorf("Tier = %q, want empty on corrupt cache", c.Tier())
	}
}

// TestCacheDisabledWithoutDir verifies no-op behavior with no CacheDir.
func TestCacheDisabledWithoutDir(t *testing.T) {
	c := New(Options{LicenseKey: "lic_test_1234"})
	c.mu.Lock()
	defer c.mu.Unlock()
	if err := c.persistCache(); err != nil {
		t.Fatalf("persistCache without dir should be nil, got %v", err)
	}
	if err := c.loadCache(); err != nil {
		t.Fatalf("loadCache without dir should be nil, got %v", err)
	}
}
