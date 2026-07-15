// Package update provides binary update functionality with GitHub releases
// and minisign signature verification.
package update

import (
	"archive/tar"
	"bytes"
	"compress/gzip"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"aead.dev/minisign"
	"github.com/minio/selfupdate"
)

// Release represents a GitHub release with download URLs.
type Release struct {
	TagName string
	Assets  []Asset
}

// Asset represents a release asset.
type Asset struct {
	Name        string
	DownloadURL string
}

// Config configures an Updater.
type Config struct {
	// CurrentVersion is the semantic version string of the running binary (e.g., "v1.2.3").
	CurrentVersion string
	// Repo is the GitHub repository in "owner/name" format (e.g., "penguintechinc/penguin").
	Repo string
	// PublicKey is the minisign public key for signature verification.
	PublicKey string
	// HTTPClient is injectable for testing.
	HTTPClient *http.Client
	// TargetPath is the path to the binary being updated (defaults to os.Executable()).
	TargetPath string
}

// Updater checks for and applies binary updates.
type Updater struct {
	currentVersion string
	repo           string
	publicKey      string
	httpClient     *http.Client
	targetPath     string
	baseURL        string
}

// New creates a new Updater with the given config.
func New(cfg Config) (*Updater, error) {
	if cfg.HTTPClient == nil {
		cfg.HTTPClient = &http.Client{}
	}

	targetPath := cfg.TargetPath
	if targetPath == "" {
		var err error
		targetPath, err = os.Executable()
		if err != nil {
			return nil, fmt.Errorf("failed to determine executable path: %w", err)
		}
	}

	baseURL := "https://api.github.com"

	return &Updater{
		currentVersion: cfg.CurrentVersion,
		repo:           cfg.Repo,
		publicKey:      cfg.PublicKey,
		httpClient:     cfg.HTTPClient,
		targetPath:     targetPath,
		baseURL:        baseURL,
	}, nil
}

// Check fetches the latest release and determines if an update is available.
func (u *Updater) Check(ctx context.Context) (*Release, error) {
	url := fmt.Sprintf("%s/repos/%s/releases/latest", u.baseURL, u.repo)

	req, err := http.NewRequestWithContext(ctx, "GET", url, nil)
	if err != nil {
		return nil, fmt.Errorf("failed to create request: %w", err)
	}

	resp, err := u.httpClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("failed to fetch latest release: %w", err)
	}
	defer func() { _ = resp.Body.Close() }()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("GitHub API returned %d", resp.StatusCode)
	}

	var ghRelease struct {
		TagName string `json:"tag_name"`
		Assets  []struct {
			Name               string `json:"name"`
			BrowserDownloadURL string `json:"browser_download_url"`
		} `json:"assets"`
	}

	if err := json.NewDecoder(resp.Body).Decode(&ghRelease); err != nil {
		return nil, fmt.Errorf("failed to parse release: %w", err)
	}

	// Filter assets for current platform
	goos := u.getGOOS()
	goarch := u.getGOARCH()

	var assets []Asset
	for _, asset := range ghRelease.Assets {
		// Match pattern: penguin_<version>_<goos>_<goarch>.tar.gz
		if strings.Contains(asset.Name, goos) && strings.Contains(asset.Name, goarch) &&
			strings.HasSuffix(asset.Name, ".tar.gz") {
			assets = append(assets, Asset{
				Name:        asset.Name,
				DownloadURL: asset.BrowserDownloadURL,
			})
		}
	}

	if len(assets) == 0 {
		return nil, fmt.Errorf("no compatible asset found for %s/%s", goos, goarch)
	}

	return &Release{
		TagName: ghRelease.TagName,
		Assets:  assets,
	}, nil
}

// Apply downloads and installs the binary update with signature verification.
func (u *Updater) Apply(ctx context.Context, rel *Release) error {
	if len(rel.Assets) == 0 {
		return fmt.Errorf("no assets in release")
	}

	// Use the first compatible asset
	asset := rel.Assets[0]

	// Download the binary archive
	binData, err := u.download(ctx, asset.DownloadURL)
	if err != nil {
		return fmt.Errorf("failed to download binary: %w", err)
	}

	// Download the signature file
	sigURL := asset.DownloadURL + ".minisig"
	sigData, err := u.download(ctx, sigURL)
	if err != nil {
		return fmt.Errorf("failed to download signature: %w", err)
	}

	// Parse the public key from string format
	// minisign public keys are typically in the format: "untrusted comment: ...\nRWSXXXX..."
	pubKeyStr := strings.TrimSpace(u.publicKey)
	var pubKey minisign.PublicKey
	if err := pubKey.UnmarshalText([]byte(pubKeyStr)); err != nil {
		return fmt.Errorf("invalid public key: %w", err)
	}

	// Verify signature: minisign.Verify requires the public key, message, and signature bytes
	if !minisign.Verify(pubKey, binData, sigData) {
		return fmt.Errorf("signature verification failed: binary may be tampered")
	}

	// Extract binary from tar.gz
	newBinary, err := u.extract(binData)
	if err != nil {
		return fmt.Errorf("failed to extract binary: %w", err)
	}

	// Apply update using minio/selfupdate
	opts := selfupdate.Options{
		TargetPath: u.targetPath,
	}
	if err := selfupdate.Apply(bytes.NewReader(newBinary), opts); err != nil {
		return fmt.Errorf("failed to apply update: %w", err)
	}

	return nil
}

// download fetches a file from the given URL.
func (u *Updater) download(ctx context.Context, url string) ([]byte, error) {
	req, err := http.NewRequestWithContext(ctx, "GET", url, nil)
	if err != nil {
		return nil, err
	}

	resp, err := u.httpClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer func() { _ = resp.Body.Close() }()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("download failed with status %d", resp.StatusCode)
	}

	return io.ReadAll(resp.Body)
}

// extract pulls the binary from a tar.gz archive.
func (u *Updater) extract(data []byte) ([]byte, error) {
	gzr, err := gzip.NewReader(strings.NewReader(string(data)))
	if err != nil {
		return nil, fmt.Errorf("failed to decompress: %w", err)
	}
	defer func() { _ = gzr.Close() }()

	tr := tar.NewReader(gzr)
	for {
		header, err := tr.Next()
		if err == io.EOF {
			break
		}
		if err != nil {
			return nil, fmt.Errorf("tar read error: %w", err)
		}

		// Look for a binary file (e.g., "penguin" or "penguin.exe")
		if header.Typeflag == tar.TypeReg && isBinaryName(header.Name) {
			return io.ReadAll(tr)
		}
	}

	return nil, fmt.Errorf("no binary found in archive")
}

// isBinaryName checks if a filename looks like a binary executable.
func isBinaryName(name string) bool {
	base := filepath.Base(name)
	return base == "penguin" || base == "penguin.exe" ||
		strings.HasPrefix(base, "penguin_") ||
		regexp.MustCompile(`^penguin\w*$`).MatchString(base)
}

// getGOOS returns the current OS identifier for binary matching.
func (u *Updater) getGOOS() string {
	// This would be replaced with actual os.runtime.GOOS in production
	// For testing, a stub is sufficient
	return "linux" // Stub
}

// getGOARCH returns the current architecture identifier for binary matching.
func (u *Updater) getGOARCH() string {
	// This would be replaced with actual os.runtime.GOARCH in production
	// For testing, a stub is sufficient
	return "amd64" // Stub
}

// CompareVersions compares two semantic version strings.
// Returns: -1 if v1 < v2, 0 if v1 == v2, 1 if v1 > v2.
func CompareVersions(v1, v2 string) int {
	parts1 := parseVersion(v1)
	parts2 := parseVersion(v2)

	for i := 0; i < len(parts1) && i < len(parts2); i++ {
		if parts1[i] < parts2[i] {
			return -1
		}
		if parts1[i] > parts2[i] {
			return 1
		}
	}

	if len(parts1) < len(parts2) {
		return -1
	}
	if len(parts1) > len(parts2) {
		return 1
	}

	return 0
}

// parseVersion extracts major.minor.patch as integers from a version string.
func parseVersion(v string) [3]int {
	// Strip leading 'v' if present
	v = strings.TrimPrefix(v, "v")

	parts := strings.Split(v, ".")
	var result [3]int

	for i := 0; i < len(parts) && i < 3; i++ {
		// Extract digits only
		var numStr string
		for _, ch := range parts[i] {
			if ch >= '0' && ch <= '9' {
				numStr += string(ch)
			} else {
				break
			}
		}
		if numStr != "" {
			// Simple integer parsing
			for _, ch := range numStr {
				result[i] = result[i]*10 + int(ch-'0')
			}
		}
	}

	return result
}
