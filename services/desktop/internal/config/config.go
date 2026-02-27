package config

import "time"

// Config is the root configuration for the Penguin client.
type Config struct {
	Modules  ModulesConfig  `mapstructure:"modules"`
	Auth     AuthConfig     `mapstructure:"auth"`
	License  LicenseConfig  `mapstructure:"license"`
	Logging  LoggingConfig  `mapstructure:"logging"`
	Plugins  PluginsConfig  `mapstructure:"plugins"`
}

// PluginsConfig configures the plugin system.
type PluginsConfig struct {
	Dir             string            `mapstructure:"dir"`
	ExternalModules map[string]string `mapstructure:"external_modules"` // name -> binary path
}

// ModulesConfig contains per-module configuration.
type ModulesConfig struct {
	VPN      VPNConfig      `mapstructure:"vpn"`
	OpenZiti OpenZitiConfig `mapstructure:"openziti"`
	DNS      DNSConfig      `mapstructure:"dns"`
	NTP      NTPConfig      `mapstructure:"ntp"`
	Nest     NestConfig     `mapstructure:"nest"`
	ArticDBM ArticDBMConfig `mapstructure:"articdbm"`
}

// VPNConfig configures the VPN module.
type VPNConfig struct {
	Enabled         bool          `mapstructure:"enabled"`
	ManagerURL      string        `mapstructure:"manager_url"`
	APIKey          string        `mapstructure:"api_key"`
	OverlayType     string        `mapstructure:"overlay_type"` // wireguard, openziti, dual
	ClientName      string        `mapstructure:"client_name"`
	MonitorInterval time.Duration `mapstructure:"monitor_interval"`
}

// OpenZitiConfig configures the OpenZiti module.
type OpenZitiConfig struct {
	Enabled      bool   `mapstructure:"enabled"`
	IdentityFile string `mapstructure:"identity_file"`
	ServiceName  string `mapstructure:"service_name"`
}

// DNSConfig configures the DNS module.
type DNSConfig struct {
	Enabled    bool     `mapstructure:"enabled"`
	ServerURLs []string `mapstructure:"server_urls"`
	Protocol   string   `mapstructure:"protocol"` // doh, grpc
	ListenAddr string   `mapstructure:"listen_addr"`
	ListenTCP  bool     `mapstructure:"listen_tcp"`
	ListenUDP  bool     `mapstructure:"listen_udp"`
	MaxRetries int      `mapstructure:"max_retries"`
	VerifySSL  bool     `mapstructure:"verify_ssl"`
	CACert     string   `mapstructure:"ca_cert"`
	ClientCert string   `mapstructure:"client_cert"`
	ClientKey  string   `mapstructure:"client_key"`
}

// NTPConfig configures the NTP module.
type NTPConfig struct {
	Enabled    bool          `mapstructure:"enabled"`
	Servers    []string      `mapstructure:"servers"`
	ListenAddr string        `mapstructure:"listen_addr"`
	Timeout    time.Duration `mapstructure:"timeout"`
	CacheTTL   time.Duration `mapstructure:"cache_ttl"`
}

// NestConfig configures the Nest module.
type NestConfig struct {
	Enabled bool   `mapstructure:"enabled"`
	APIURL  string `mapstructure:"api_url"`
}

// ArticDBMConfig configures the ArticDBM module.
type ArticDBMConfig struct {
	Enabled bool   `mapstructure:"enabled"`
	APIURL  string `mapstructure:"api_url"`
}

// AuthConfig configures shared authentication.
type AuthConfig struct {
	JWTServer  string `mapstructure:"jwt_server"`
	Username   string `mapstructure:"username"`
	SkipVerify bool   `mapstructure:"skip_verify"`
}

// LicenseConfig configures license validation.
type LicenseConfig struct {
	ServerURL string        `mapstructure:"server_url"`
	LicenseKey string        `mapstructure:"license_key"`
	UserToken string        `mapstructure:"user_token"`
	CacheTTL  time.Duration `mapstructure:"cache_ttl_minutes"`
}

// LoggingConfig configures structured logging.
type LoggingConfig struct {
	Level  string `mapstructure:"level"`  // debug, info, warn, error
	Format string `mapstructure:"format"` // json, text
	File   string `mapstructure:"file"`   // optional log file path
}
