package config

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/spf13/viper"
)

// DefaultConfig returns a Config with sensible defaults.
func DefaultConfig() *Config {
	return &Config{
		Modules: ModulesConfig{
			VPN: VPNConfig{
				MonitorInterval: 30 * time.Second,
			},
			DNS: DNSConfig{
				Protocol:   "doh",
				ListenAddr: "127.0.0.1:53",
				ListenUDP:  true,
				ListenTCP:  true,
				MaxRetries: 3,
				VerifySSL:  true,
				ServerURLs: []string{"https://dns.google/resolve"},
			},
			NTP: NTPConfig{
				Servers:    []string{"pool.ntp.org", "time.google.com", "time.cloudflare.com"},
				Timeout:    5 * time.Second,
				CacheTTL:   60 * time.Second,
				ListenAddr: "127.0.0.1:123",
			},
		},
		Auth: AuthConfig{
			JWTServer: "https://auth.penguintech.io",
		},
		License: LicenseConfig{
			ServerURL: "https://license.penguintech.io",
			CacheTTL:  1440 * time.Minute,
		},
		Logging: LoggingConfig{
			Level:  "info",
			Format: "json",
		},
		Plugins: PluginsConfig{
			Dir:             "plugins",
			ExternalModules: make(map[string]string),
		},
	}
}

// Load reads configuration from file, env vars, and defaults.
func Load(cfgFile string) (*Config, error) {
	v := viper.New()
	v.SetConfigType("yaml")
	v.SetEnvPrefix("PENGUIN")
	v.SetEnvKeyReplacer(strings.NewReplacer(".", "_"))
	v.AutomaticEnv()

	if cfgFile != "" {
		v.SetConfigFile(cfgFile)
	} else {
		configDir := GetConfigDir()
		v.AddConfigPath(configDir)
		v.AddConfigPath(".")
		v.SetConfigName("penguin")
	}

	cfg := DefaultConfig()

	if err := v.ReadInConfig(); err != nil {
		if _, ok := err.(viper.ConfigFileNotFoundError); !ok {
			return nil, fmt.Errorf("reading config: %w", err)
		}
	}

	if err := v.Unmarshal(cfg); err != nil {
		return nil, fmt.Errorf("unmarshaling config: %w", err)
	}

	return cfg, nil
}

// Save writes the config to the default config file.
func Save(cfg *Config) error {
	configDir := GetConfigDir()
	if err := os.MkdirAll(configDir, 0700); err != nil {
		return fmt.Errorf("creating config dir: %w", err)
	}

	v := viper.New()
	v.SetConfigType("yaml")
	v.SetConfigFile(filepath.Join(configDir, "penguin.yaml"))

	v.Set("modules", cfg.Modules)
	v.Set("auth", cfg.Auth)
	v.Set("license", cfg.License)
	v.Set("logging", cfg.Logging)
	v.Set("plugins", cfg.Plugins)

	return v.WriteConfig()
}

// IsModuleEnabled checks if a module is enabled in config.
// For built-in modules, checks the module-specific config.
// For external/plugin modules, checks the ExternalModules map.
func (c *Config) IsModuleEnabled(name string) bool {
	switch name {
	case "vpn":
		return c.Modules.VPN.Enabled
	case "openziti":
		return c.Modules.OpenZiti.Enabled
	case "dns":
		return c.Modules.DNS.Enabled
	case "ntp":
		return c.Modules.NTP.Enabled
	case "nest":
		return c.Modules.Nest.Enabled
	case "articdbm":
		return c.Modules.ArticDBM.Enabled
	default:
		// Check external modules — if present in the map, it's enabled
		if c.Plugins.ExternalModules != nil {
			_, exists := c.Plugins.ExternalModules[name]
			return exists
		}
		return false
	}
}
