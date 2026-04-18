package config

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/spf13/viper"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// newTestViper creates a new viper instance with a temporary config file.
func newTestViper(t *testing.T, yamlContent string) *viper.Viper {
	t.Helper()
	v := viper.New()
	// Create a temporary directory
	tempDir := t.TempDir()
	// Create a temporary config file
	configFile := filepath.Join(tempDir, "config.yaml")
	err := os.WriteFile(configFile, []byte(yamlContent), 0644)
	require.NoError(t, err)

	v.SetConfigFile(configFile)
	return v
}

func TestConfigPopulation(t *testing.T) {
	yamlContent := `
modules:
  vpn:
    enabled: true
    manager_url: "https://vpn.example.com"
    api_key: "test-key"
    overlay_type: "wireguard"
    client_name: "test-client"
    monitor_interval: 30s
  openziti:
    enabled: true
    identity_file: "/path/to/identity.json"
    service_name: "ziti-service"
  dns:
    enabled: true
    server_urls: ["https://dns.example.com/dns-query"]
    protocol: "doh"
    listen_addr: ":53"
    listen_tcp: true
    listen_udp: true
    max_retries: 3
    verify_ssl: true
    ca_cert: "/path/to/ca.pem"
    client_cert: "/path/to/client.pem"
    client_key: "/path/to/client.key"
  ntp:
    enabled: true
    servers: ["ntp.example.com"]
    listen_addr: ":123"
    timeout: 5s
    cache_ttl: 10m
  nest:
    enabled: true
    api_url: "https://nest.example.com"
  articdbm:
    enabled: true
    api_url: "https://articdbm.example.com"
auth:
  jwt_server: "https://auth.example.com"
  username: "testuser"
  skip_verify: true
license:
  server_url: "https://license.example.com"
  license_key: "license-key"
  user_token: "user-token"
  cache_ttl_minutes: 60m
logging:
  level: "debug"
  format: "json"
  file: "/var/log/penguin-desktop.log"
plugins:
  dir: "/usr/lib/penguin-desktop/plugins"
  external_modules:
    custom-mod: "/usr/bin/custom-mod"
`

	v := newTestViper(t, yamlContent)
	err := v.ReadInConfig()
	require.NoError(t, err)

	var cfg Config
	err = v.Unmarshal(&cfg)
	require.NoError(t, err)

	// Assertions for ModulesConfig
	assert.True(t, cfg.Modules.VPN.Enabled)
	assert.Equal(t, "https://vpn.example.com", cfg.Modules.VPN.ManagerURL)
	assert.Equal(t, "test-key", cfg.Modules.VPN.APIKey)
	assert.Equal(t, "wireguard", cfg.Modules.VPN.OverlayType)
	assert.Equal(t, "test-client", cfg.Modules.VPN.ClientName)
	assert.Equal(t, 30*time.Second, cfg.Modules.VPN.MonitorInterval)

	assert.True(t, cfg.Modules.OpenZiti.Enabled)
	assert.Equal(t, "/path/to/identity.json", cfg.Modules.OpenZiti.IdentityFile)
	assert.Equal(t, "ziti-service", cfg.Modules.OpenZiti.ServiceName)

	assert.True(t, cfg.Modules.DNS.Enabled)
	assert.Equal(t, []string{"https://dns.example.com/dns-query"}, cfg.Modules.DNS.ServerURLs)
	assert.Equal(t, "doh", cfg.Modules.DNS.Protocol)
	assert.Equal(t, ":53", cfg.Modules.DNS.ListenAddr)
	assert.True(t, cfg.Modules.DNS.ListenTCP)
	assert.True(t, cfg.Modules.DNS.ListenUDP)
	assert.Equal(t, 3, cfg.Modules.DNS.MaxRetries)
	assert.True(t, cfg.Modules.DNS.VerifySSL)
	assert.Equal(t, "/path/to/ca.pem", cfg.Modules.DNS.CACert)
	assert.Equal(t, "/path/to/client.pem", cfg.Modules.DNS.ClientCert)
	assert.Equal(t, "/path/to/client.key", cfg.Modules.DNS.ClientKey)

	assert.True(t, cfg.Modules.NTP.Enabled)
	assert.Equal(t, []string{"ntp.example.com"}, cfg.Modules.NTP.Servers)
	assert.Equal(t, ":123", cfg.Modules.NTP.ListenAddr)
	assert.Equal(t, 5*time.Second, cfg.Modules.NTP.Timeout)
	assert.Equal(t, 10*time.Minute, cfg.Modules.NTP.CacheTTL)

	assert.True(t, cfg.Modules.Nest.Enabled)
	assert.Equal(t, "https://nest.example.com", cfg.Modules.Nest.APIURL)

	assert.True(t, cfg.Modules.ArticDBM.Enabled)
	assert.Equal(t, "https://articdbm.example.com", cfg.Modules.ArticDBM.APIURL)

	// Assertions for AuthConfig
	assert.Equal(t, "https://auth.example.com", cfg.Auth.JWTServer)
	assert.Equal(t, "testuser", cfg.Auth.Username)
	assert.True(t, cfg.Auth.SkipVerify)

	// Assertions for LicenseConfig
	assert.Equal(t, "https://license.example.com", cfg.License.ServerURL)
	assert.Equal(t, "license-key", cfg.License.LicenseKey)
	assert.Equal(t, "user-token", cfg.License.UserToken)
	assert.Equal(t, 60*time.Minute, cfg.License.CacheTTL)

	// Assertions for LoggingConfig
	assert.Equal(t, "debug", cfg.Logging.Level)
	assert.Equal(t, "json", cfg.Logging.Format)
	assert.Equal(t, "/var/log/penguin-desktop.log", cfg.Logging.File)

	// Assertions for PluginsConfig
	assert.Equal(t, "/usr/lib/penguin-desktop/plugins", cfg.Plugins.Dir)
	assert.Equal(t, map[string]string{"custom-mod": "/usr/bin/custom-mod"}, cfg.Plugins.ExternalModules)
}
