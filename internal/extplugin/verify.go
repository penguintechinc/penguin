package extplugin

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"os"
	"os/user"
	"path/filepath"
	"strconv"
	"strings"

	"aead.dev/minisign"
)

// StatInfo wraps os.Stat for injectable testing.
type StatInfo interface {
	// Stat returns file info, like os.Stat.
	Stat(path string) (os.FileInfo, error)
}

// RealStat implements StatInfo using os.Stat.
type RealStat struct{}

func (r *RealStat) Stat(path string) (os.FileInfo, error) {
	return os.Stat(path)
}

// Verifier checks plugin integrity and signature.
type Verifier struct {
	stat            StatInfo
	trustedPublicKeys []string // minisign public keys in "minisign:" format
}

// NewVerifier creates a new verifier with the embedded PenguinTech public key
// and any additional keys from /etc/penguin/trusted-publishers.d/*.pub.
func NewVerifier() *Verifier {
	v := &Verifier{
		stat:              &RealStat{},
		trustedPublicKeys: []string{embeddedPublicKey},
	}

	// Load additional trusted keys from the system directory (if it exists).
	// This is a best-effort operation; missing directory is not an error.
	trustDir := "/etc/penguin/trusted-publishers.d"
	entries, err := os.ReadDir(trustDir)
	if err == nil {
		for _, entry := range entries {
			if !strings.HasSuffix(entry.Name(), ".pub") || entry.IsDir() {
				continue
			}
			keyPath := filepath.Join(trustDir, entry.Name())
			keyData, err := os.ReadFile(keyPath) // #nosec G304 -- trusted keys from a filtered filename under the root-owned system trust dir
			if err == nil {
				v.trustedPublicKeys = append(v.trustedPublicKeys, string(keyData))
			}
		}
	}

	return v
}

// NewVerifierWithKeys creates a verifier with explicit trusted keys (for testing).
func NewVerifierWithKeys(keys []string) *Verifier {
	return &Verifier{
		stat:              &RealStat{},
		trustedPublicKeys: keys,
	}
}

// setStatForTesting replaces the stat function (used only in tests).
func (v *Verifier) setStatForTesting(stat StatInfo) {
	v.stat = stat
}

// Verify performs all security checks on a plugin directory:
// 1. Ownership and permissions (not world-writable)
// 2. SHA256 integrity check
// 3. minisign signature verification with pinned keys
//
// Returns a nil error only if all checks pass.
func (v *Verifier) Verify(pluginDir string, m *Manifest) error {
	// Check 1: Directory ownership and permissions.
	if err := v.verifyDirOwnership(pluginDir); err != nil {
		return err
	}

	// Check 2: Binary file ownership and permissions.
	binaryPath := m.BinaryPath(pluginDir)
	if err := v.verifyFileOwnership(binaryPath); err != nil {
		return err
	}

	// Check 3: SHA256 integrity.
	if err := v.verifySHA256(binaryPath, m.SHA256); err != nil {
		return err
	}

	// Check 4: minisign signature.
	sigPath := m.SignaturePath(pluginDir)
	if err := v.verifySignature(binaryPath, sigPath); err != nil {
		return err
	}

	return nil
}

// verifyDirOwnership checks that the plugin directory is owned by root or
// the daemon's uid and is not world-writable.
func (v *Verifier) verifyDirOwnership(dir string) error {
	info, err := v.stat.Stat(dir)
	if err != nil {
		return fmt.Errorf("stat plugin dir: %w", err)
	}

	// Check mode: reject if world-writable (0o002 bits set).
	if info.Mode()&0o002 != 0 {
		return fmt.Errorf("plugin dir is world-writable: %o", info.Mode())
	}

	// Check ownership: must be root (uid 0) or the daemon process's uid.
	daemonUID := os.Getuid()
	if stat, ok := info.Sys().(interface{ Uid() uint32 }); ok {
		uid := stat.Uid()
		wantUID := uint32(daemonUID) // #nosec G115 -- os.Getuid() returns a non-negative int, safe to cast to uint32
		if uid != 0 && uid != wantUID {
			return fmt.Errorf("plugin dir not owned by root or daemon (uid %d != 0 and %d)", uid, daemonUID)
		}
	}

	return nil
}

// verifyFileOwnership checks that a file is owned by root or the daemon
// and is not world-writable.
func (v *Verifier) verifyFileOwnership(path string) error {
	info, err := v.stat.Stat(path)
	if err != nil {
		return fmt.Errorf("stat file: %w", err)
	}

	// Check mode: reject if world-writable.
	if info.Mode()&0o002 != 0 {
		return fmt.Errorf("plugin file is world-writable: %o", info.Mode())
	}

	// Check ownership: must be root or daemon.
	daemonUID := os.Getuid()
	if stat, ok := info.Sys().(interface{ Uid() uint32 }); ok {
		uid := stat.Uid()
		wantUID := uint32(daemonUID) // #nosec G115 -- os.Getuid() returns a non-negative int, safe to cast to uint32
		if uid != 0 && uid != wantUID {
			return fmt.Errorf("plugin file not owned by root or daemon (uid %d != 0 and %d)", uid, daemonUID)
		}
	}

	return nil
}

// verifySHA256 checks that the binary's SHA256 hash matches the manifest.
func (v *Verifier) verifySHA256(filePath, expectedHash string) error {
	file, err := os.Open(filePath) // #nosec G304 -- binary path read during verification, ownership/permissions pre-validated
	if err != nil {
		return fmt.Errorf("open file for hash: %w", err)
	}
	defer func() { _ = file.Close() }()

	h := sha256.New()
	if _, err := io.Copy(h, file); err != nil {
		_ = file.Close()
		return fmt.Errorf("hash file: %w", err)
	}
	if err := file.Close(); err != nil {
		return fmt.Errorf("close file: %w", err)
	}

	actualHash := hex.EncodeToString(h.Sum(nil))
	if actualHash != expectedHash {
		return fmt.Errorf("sha256 mismatch: got %s, expected %s", actualHash, expectedHash)
	}

	return nil
}

// verifySignature checks the minisign signature of the binary using a trusted key.
func (v *Verifier) verifySignature(binaryPath, sigPath string) error {
	sigData, err := os.ReadFile(sigPath) // #nosec G304 -- signature path constructed from manifest and pluginDir, verified during Verify()
	if err != nil {
		return fmt.Errorf("read signature file: %w", err)
	}

	binaryData, err := os.ReadFile(binaryPath) // #nosec G304 -- binary path verified (ownership+perms) before reading for signature check
	if err != nil {
		return fmt.Errorf("read binary for signature verification: %w", err)
	}

	// Try each trusted key until one succeeds.
	var lastErr error
	for _, keyStr := range v.trustedPublicKeys {
		var key minisign.PublicKey
		if err := key.UnmarshalText([]byte(keyStr)); err != nil {
			lastErr = err
			continue
		}

		if !minisign.Verify(key, binaryData, sigData) {
			lastErr = fmt.Errorf("signature verification failed")
			continue
		}

		// Signature verified.
		return nil
	}

	if lastErr != nil {
		return fmt.Errorf("signature verification failed: %w", lastErr)
	}
	return fmt.Errorf("no trusted keys to verify signature")
}

// The embedded PenguinTech public key (TODO: replace with actual key).
// This is a placeholder test key; in production, use a real PenguinTech key.
const embeddedPublicKey = `untrusted comment: minisign public key
RWQf7zLn5+DYjyZ8ZWIrasJVjMKWePWGVgvBvF40FmkT7K7VZV7EVwA=
`

// GetCurrentUserUID returns the current user's UID (for testing).
// This is primarily for test helpers.
func GetCurrentUserUID() uint32 {
	u, err := user.Current()
	if err != nil {
		return uint32(os.Getuid()) // #nosec G115 -- os.Getuid() returns non-negative int, safe for uint32 conversion
	}
	uid, _ := strconv.ParseUint(u.Uid, 10, 32)
	return uint32(uid)
}
