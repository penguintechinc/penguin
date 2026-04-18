package config

import (
	"os"

	"testing"

	"github.com/stretchr/testify/assert"
)

func TestPlatformDirs(t *testing.T) {
	// Save original values

	originalAppData := os.Getenv("APPDATA")
	originalLocalAppData := os.Getenv("LOCALAPPDATA")
	originalUserHomeDir := os.Getenv("USERPROFILE") // Windows
	originalUserHomeDirLinux := os.Getenv("HOME")   // Linux/macOS
	originalXDGConfigHome := os.Getenv("XDG_CONFIG_HOME")
	originalXDGDataHome := os.Getenv("XDG_DATA_HOME")
	originalXDGCHome := os.Getenv("XDG_CACHE_HOME")

	defer func() {
		// Restore original values
		
		os.Setenv("APPDATA", originalAppData)
		os.Setenv("LOCALAPPDATA", originalLocalAppData)
		os.Setenv("USERPROFILE", originalUserHomeDir)
		os.Setenv("HOME", originalUserHomeDirLinux)
		os.Setenv("XDG_CONFIG_HOME", originalXDGConfigHome)
		os.Setenv("XDG_DATA_HOME", originalXDGDataHome)
		os.Setenv("XDG_CACHE_HOME", originalXDGCHome)
	}()

	tests := []struct {
		name        string
		goos        string
		envs        map[string]string
		expectedCfg string
		expectedData string
		expectedCache string
	}{
		{
			name: "Windows with APPDATA and LOCALAPPDATA",
			goos: "windows",
			envs: map[string]string{
				"APPDATA":       "C:\\Users\\TestUser\\AppData\\Roaming",
				"LOCALAPPDATA":  "C:\\Users\\TestUser\\AppData\\Local",
				"USERPROFILE":   "C:\\Users\\TestUser",
			},
			expectedCfg:   "C:\\Users\\TestUser\\AppData\\Roaming\\PenguinTech\\Penguin",
			expectedData:  "C:\\Users\\TestUser\\AppData\\Local\\PenguinTech\\Penguin",
			expectedCache: "C:\\Users\\TestUser\\AppData\\Local\\PenguinTech\\Penguin\\Cache",
		},
		{
			name: "Windows without APPDATA and LOCALAPPDATA",
			goos: "windows",
			envs: map[string]string{
				"APPDATA":       "",
				"LOCALAPPDATA":  "",
				"USERPROFILE":   "C:\\Users\\TestUser",
			},
			expectedCfg:   "C:\\Users\\TestUser\\PenguinTech\\Penguin",
			expectedData:  "C:\\Users\\TestUser\\PenguinTech\\Penguin\\Data",
			expectedCache: "C:\\Users\\TestUser\\PenguinTech\\Penguin\\Data\\Cache",
		},
		{
			name: "macOS",
			goos: "darwin",
			envs: map[string]string{
				"HOME": "/Users/testuser",
			},
			expectedCfg:   "/Users/testuser/Library/Application Support/PenguinTech/Penguin",
			expectedData:  "/Users/testuser/Library/Application Support/PenguinTech/Penguin/Data",
			expectedCache: "/Users/testuser/Library/Caches/PenguinTech/Penguin",
		},
		{
			name: "Linux with XDG_CONFIG_HOME",
			goos: "linux",
			envs: map[string]string{
				"XDG_CONFIG_HOME": "/home/xdguser/.config",
				"XDG_DATA_HOME":   "/home/xdguser/.local/share",
				"XDG_CACHE_HOME":  "/home/xdguser/.cache",
				"HOME":            "/home/xdguser",
			},
			expectedCfg:   "/home/xdguser/.config/penguin",
			expectedData:  "/home/xdguser/.local/share/penguin",
			expectedCache: "/home/xdguser/.cache/penguin",
		},
		{
			name: "Linux without XDG_CONFIG_HOME",
			goos: "linux",
			envs: map[string]string{
				"XDG_CONFIG_HOME": "",
				"XDG_DATA_HOME":   "",
				"XDG_CACHE_HOME":  "",
				"HOME":            "/home/defaultuser",
			},
			expectedCfg:   "/home/defaultuser/.config/penguin",
			expectedData:  "/home/defaultuser/.local/share/penguin",
			expectedCache: "/home/defaultuser/.cache/penguin",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			os.Setenv("GOOS", tt.goos) // Use os.Setenv for GOOS
			for key, value := range tt.envs {
				os.Setenv(key, value)
			}

			assert.Equal(t, tt.expectedCfg, GetConfigDir())
			assert.Equal(t, tt.expectedData, GetDataDir())
			assert.Equal(t, tt.expectedCache, GetCacheDir())
		})
	}
}
