package auth

import (
	"crypto/tls"
	"crypto/x509"
	"fmt"
	"os"
	"path/filepath"
)

// CertManager manages TLS certificates for mTLS.
type CertManager struct {
	certDir string
}

// NewCertManager creates a cert manager.
func NewCertManager(certDir string) *CertManager {
	return &CertManager{certDir: certDir}
}

// LoadClientCert loads client certificate and key for mTLS.
func (cm *CertManager) LoadClientCert(certFile, keyFile string) (tls.Certificate, error) {
	certPath := cm.resolvePath(certFile)
	keyPath := cm.resolvePath(keyFile)

	cert, err := tls.LoadX509KeyPair(certPath, keyPath)
	if err != nil {
		return tls.Certificate{}, fmt.Errorf("loading client cert: %w", err)
	}
	return cert, nil
}

// LoadCACert loads a CA certificate for server verification.
func (cm *CertManager) LoadCACert(caFile string) (*x509.CertPool, error) {
	caPath := cm.resolvePath(caFile)

	caCert, err := os.ReadFile(caPath)
	if err != nil {
		return nil, fmt.Errorf("reading CA cert: %w", err)
	}

	pool := x509.NewCertPool()
	if !pool.AppendCertsFromPEM(caCert) {
		return nil, fmt.Errorf("failed to parse CA certificate")
	}
	return pool, nil
}

// SaveCert saves a certificate to the cert directory.
func (cm *CertManager) SaveCert(name string, data []byte) error {
	if err := os.MkdirAll(cm.certDir, 0700); err != nil {
		return fmt.Errorf("creating cert dir: %w", err)
	}

	path := filepath.Join(cm.certDir, name)
	if err := os.WriteFile(path, data, 0600); err != nil {
		return fmt.Errorf("writing cert: %w", err)
	}
	return nil
}

func (cm *CertManager) resolvePath(file string) string {
	if filepath.IsAbs(file) {
		return file
	}
	return filepath.Join(cm.certDir, file)
}
