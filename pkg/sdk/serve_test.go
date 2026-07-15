package sdk

import (
	"math"
	"testing"
)

// TestClampInt32_Normal tests clampInt32 with normal int32-range values.
func TestClampInt32_Normal(t *testing.T) {
	tests := []struct {
		input    int
		expected int32
	}{
		{0, 0},
		{1, 1},
		{-1, -1},
		{100, 100},
		{-100, -100},
		{1000000, 1000000},
		{-1000000, -1000000},
	}

	for _, tt := range tests {
		t.Run("", func(t *testing.T) {
			got := clampInt32(tt.input)
			if got != tt.expected {
				t.Errorf("clampInt32(%d) = %d, want %d", tt.input, got, tt.expected)
			}
		})
	}
}

// TestClampInt32_Saturation_PositiveOverflow tests clampInt32 with positive overflow.
func TestClampInt32_Saturation_PositiveOverflow(t *testing.T) {
	input := math.MaxInt32 + 1000
	expected := int32(math.MaxInt32)

	got := clampInt32(input)
	if got != expected {
		t.Errorf("clampInt32(%d) = %d, want %d (maxint32)", input, got, expected)
	}
}

// TestClampInt32_Saturation_NegativeOverflow tests clampInt32 with negative overflow.
func TestClampInt32_Saturation_NegativeOverflow(t *testing.T) {
	input := math.MinInt32 - 1000
	expected := int32(math.MinInt32)

	got := clampInt32(input)
	if got != expected {
		t.Errorf("clampInt32(%d) = %d, want %d (minint32)", input, got, expected)
	}
}

// TestClampInt32_MaxInt32Boundary tests clampInt32 at int32 boundaries.
func TestClampInt32_MaxInt32Boundary(t *testing.T) {
	// At max boundary
	got := clampInt32(math.MaxInt32)
	if got != math.MaxInt32 {
		t.Errorf("clampInt32(MaxInt32) = %d, want %d", got, math.MaxInt32)
	}

	// At min boundary
	got = clampInt32(math.MinInt32)
	if got != math.MinInt32 {
		t.Errorf("clampInt32(MinInt32) = %d, want %d", got, math.MinInt32)
	}
}

// TestClampInt32_LargePositiveOverflow tests clampInt32 with large positive values.
func TestClampInt32_LargePositiveOverflow(t *testing.T) {
	tests := []int{
		math.MaxInt32 + 1,
		math.MaxInt32 * 2,
		1000000000000, // 1 trillion
	}

	for _, input := range tests {
		got := clampInt32(input)
		if got != math.MaxInt32 {
			t.Errorf("clampInt32(%d) = %d, want MaxInt32", input, got)
		}
	}
}

// TestClampInt32_LargeNegativeOverflow tests clampInt32 with large negative values.
func TestClampInt32_LargeNegativeOverflow(t *testing.T) {
	tests := []int{
		math.MinInt32 - 1,
		math.MinInt32 - 1000,
		-1000000000000, // -1 trillion
	}

	for _, input := range tests {
		got := clampInt32(input)
		if got != math.MinInt32 {
			t.Errorf("clampInt32(%d) = %d, want MinInt32", input, got)
		}
	}
}

// TestPluginHandshakeConfig tests the plugin handshake configuration.
func TestPluginHandshakeConfig(t *testing.T) {
	if PluginHandshakeConfigMagicCookieKey != "PENGUIN_PLUGIN" {
		t.Errorf("MagicCookieKey = %q, want %q", PluginHandshakeConfigMagicCookieKey, "PENGUIN_PLUGIN")
	}

	if PluginHandshakeConfigMagicCookieValue != "penguin-sdk-v1" {
		t.Errorf("MagicCookieValue = %q, want %q", PluginHandshakeConfigMagicCookieValue, "penguin-sdk-v1")
	}

	if PluginProtocolVersion != 1 {
		t.Errorf("ProtocolVersion = %d, want 1", PluginProtocolVersion)
	}
}

// TestHostServiceBrokerID tests the host service broker ID.
func TestHostServiceBrokerID(t *testing.T) {
	if HostServiceBrokerID != 1 {
		t.Errorf("HostServiceBrokerID = %d, want 1", HostServiceBrokerID)
	}
}

// TestClampInt32_ZeroBoundary tests clampInt32 around zero.
func TestClampInt32_ZeroBoundary(t *testing.T) {
	tests := []struct {
		input    int
		expected int32
	}{
		{-1, -1},
		{0, 0},
		{1, 1},
	}

	for _, tt := range tests {
		got := clampInt32(tt.input)
		if got != tt.expected {
			t.Errorf("clampInt32(%d) = %d, want %d", tt.input, got, tt.expected)
		}
	}
}
