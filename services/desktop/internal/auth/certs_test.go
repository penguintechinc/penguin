package auth

import (
	"crypto/rand"
	"crypto/rsa"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/pem"
	"math/big"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func generateTestCert(t *testing.T, dir string, name string) (certPath, keyPath string) {
	t.Helper()

	priv, err := rsa.GenerateKey(rand.Reader, 2048)
	require.NoError(t, err)

	template := x509.Certificate{
		SerialNumber: big.NewInt(1),
		Subject:      pkix.Name{Organization: []string{"Test Co"}},
		NotBefore:    time.Now(),
		NotAfter:     time.Now().Add(time.Hour),
		KeyUsage:     x509.KeyUsageKeyEncipherment | x509.KeyUsageDigitalSignature,
		ExtKeyUsage:  []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
	}
	derBytes, err := x509.CreateCertificate(rand.Reader, &template, &template, &priv.PublicKey, priv)
	require.NoError(t, err)

	certPath = filepath.Join(dir, name+".pem")
	certOut, err := os.Create(certPath)
	require.NoError(t, err)
	pem.Encode(certOut, &pem.Block{Type: "CERTIFICATE", Bytes: derBytes})
	certOut.Close()

	keyPath = filepath.Join(dir, name+".key")
	keyOut, err := os.Create(keyPath)
	require.NoError(t, err)
	pem.Encode(keyOut, &pem.Block{Type: "RSA PRIVATE KEY", Bytes: x509.MarshalPKCS1PrivateKey(priv)})
	keyOut.Close()

	return certPath, keyPath
}

func TestCertManager_LoadClientCert(t *testing.T) {
	tempDir := t.TempDir()
	cm := NewCertManager(tempDir)

	certPath, keyPath := generateTestCert(t, tempDir, "client")

	cert, err := cm.LoadClientCert(filepath.Base(certPath), filepath.Base(keyPath))
	require.NoError(t, err)
	assert.NotEmpty(t, cert.Certificate)
}

func TestCertManager_LoadCACert(t *testing.T) {
	tempDir := t.TempDir()
	cm := NewCertManager(tempDir)
	caCertPath, _ := generateTestCert(t, tempDir, "ca")

	pool, err := cm.LoadCACert(filepath.Base(caCertPath))
	require.NoError(t, err)
	assert.NotNil(t, pool)
}

func TestCertManager_SaveCert(t *testing.T) {
	tempDir := t.TempDir()
	cm := NewCertManager(tempDir)
	certData := []byte("test-cert-data")
	err := cm.SaveCert("new-cert.pem", certData)
	require.NoError(t, err)

	savedData, err := os.ReadFile(filepath.Join(tempDir, "new-cert.pem"))
	require.NoError(t, err)
	assert.Equal(t, certData, savedData)
}

func TestCertManager_resolvePath(t *testing.T) {
	cm := NewCertManager("/tmp/certs")
	assert.Equal(t, "/abs/path.pem", cm.resolvePath("/abs/path.pem"))
	assert.Equal(t, "/tmp/certs/rel/path.pem", cm.resolvePath("rel/path.pem"))
}

func TestCertManager_LoadClientCert_Error(t *testing.T) {
	tempDir := t.TempDir()
	cm := NewCertManager(tempDir)
	_, err := cm.LoadClientCert("nonexistent.pem", "nonexistent.key")
	require.Error(t, err)
}

func TestCertManager_LoadCACert_Error(t *testing.T) {
	tempDir := t.TempDir()
	cm := NewCertManager(tempDir)
	_, err := cm.LoadCACert("nonexistent.pem")
	require.Error(t, err)
}

func TestCertManager_LoadCACert_Invalid(t *testing.T) {
	tempDir := t.TempDir()
	cm := NewCertManager(tempDir)
	caCertPath := filepath.Join(tempDir, "invalid.pem")
	err := os.WriteFile(caCertPath, []byte("invalid cert"), 0600)
	require.NoError(t, err)
	_, err = cm.LoadCACert(filepath.Base(caCertPath))
	require.Error(t, err)
}

func TestCertManager_SaveCert_Error(t *testing.T) {
	cm := NewCertManager("/root/unauthorized")
	err := cm.SaveCert("test.pem", []byte("test"))
	require.Error(t, err)
}
