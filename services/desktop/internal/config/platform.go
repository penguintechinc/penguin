package config

import (
	"os"
	"path/filepath"
	"runtime"
)

// GetConfigDir returns the platform-specific config directory.
func GetConfigDir() string {
	switch runtime.GOOS {
	case "windows":
		appData := os.Getenv("APPDATA")
		if appData == "" {
			appData = filepath.Join(os.Getenv("USERPROFILE"), "AppData", "Roaming")
		}
		return filepath.Join(appData, "PenguinTech", "Penguin")
	case "darwin":
		home, _ := os.UserHomeDir()
		return filepath.Join(home, "Library", "Application Support", "PenguinTech", "Penguin")
	default: // linux and others
		if xdg := os.Getenv("XDG_CONFIG_HOME"); xdg != "" {
			return filepath.Join(xdg, "penguin")
		}
		home, _ := os.UserHomeDir()
		return filepath.Join(home, ".config", "penguin")
	}
}

// GetDataDir returns the platform-specific data directory.
func GetDataDir() string {
	switch runtime.GOOS {
	case "windows":
		localAppData := os.Getenv("LOCALAPPDATA")
		if localAppData == "" {
			localAppData = filepath.Join(os.Getenv("USERPROFILE"), "AppData", "Local")
		}
		return filepath.Join(localAppData, "PenguinTech", "Penguin")
	case "darwin":
		home, _ := os.UserHomeDir()
		return filepath.Join(home, "Library", "Application Support", "PenguinTech", "Penguin", "Data")
	default:
		if xdg := os.Getenv("XDG_DATA_HOME"); xdg != "" {
			return filepath.Join(xdg, "penguin")
		}
		home, _ := os.UserHomeDir()
		return filepath.Join(home, ".local", "share", "penguin")
	}
}

// GetCacheDir returns the platform-specific cache directory.
func GetCacheDir() string {
	switch runtime.GOOS {
	case "windows":
		return filepath.Join(GetDataDir(), "Cache")
	case "darwin":
		home, _ := os.UserHomeDir()
		return filepath.Join(home, "Library", "Caches", "PenguinTech", "Penguin")
	default:
		if xdg := os.Getenv("XDG_CACHE_HOME"); xdg != "" {
			return filepath.Join(xdg, "penguin")
		}
		home, _ := os.UserHomeDir()
		return filepath.Join(home, ".cache", "penguin")
	}
}
