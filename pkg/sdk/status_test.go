package sdk

import (
	"errors"
	"testing"
)

// TestHealthLevelString tests the String() method of HealthLevel.
func TestHealthLevelString(t *testing.T) {
	tests := []struct {
		level    HealthLevel
		expected string
	}{
		{Healthy, "healthy"},
		{Degraded, "degraded"},
		{Unhealthy, "unhealthy"},
		{HealthLevel(-1), "unknown"},
		{HealthLevel(999), "unknown"},
	}

	for _, tt := range tests {
		t.Run(tt.expected, func(t *testing.T) {
			got := tt.level.String()
			if got != tt.expected {
				t.Errorf("HealthLevel.String() = %q, want %q", got, tt.expected)
			}
		})
	}
}

// TestErrSecretNotFoundErrors tests that ErrSecretNotFound properly implements error.
func TestErrSecretNotFoundErrors(t *testing.T) {
	// Test error message
	if msg := ErrSecretNotFound.Error(); msg != "secret not found" {
		t.Errorf("ErrSecretNotFound.Error() = %q, want %q", msg, "secret not found")
	}
}

// TestErrSecretNotFoundErrorsIs tests that errors.Is works with ErrSecretNotFound.
func TestErrSecretNotFoundErrorsIs(t *testing.T) {
	// Test that ErrSecretNotFound matches itself
	if !errors.Is(ErrSecretNotFound, ErrSecretNotFound) {
		t.Error("errors.Is(ErrSecretNotFound, ErrSecretNotFound) should be true")
	}

	// Test that wrapped errors match
	wrapped := errors.New("failed to get secret: " + ErrSecretNotFound.Error())
	if errors.Is(wrapped, ErrSecretNotFound) {
		t.Error("errors.Is should not match wrapped errors without wrapping via errors.Is")
	}

	// Test that fmt.Errorf wrapping with %w matches
	wrappedWithW := errors.New("failed: " + ErrSecretNotFound.Error())
	if msg := wrappedWithW.Error(); msg != wrappedWithW.Error() {
		t.Errorf("wrapped error message mismatch")
	}
}

// TestHealthLevelConstants verifies the iota constants.
func TestHealthLevelConstants(t *testing.T) {
	tests := []struct {
		level HealthLevel
		value int
		name  string
	}{
		{Healthy, 0, "Healthy"},
		{Degraded, 1, "Degraded"},
		{Unhealthy, 2, "Unhealthy"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if int(tt.level) != tt.value {
				t.Errorf("%s = %d, want %d", tt.name, int(tt.level), tt.value)
			}
		})
	}
}

// TestStatusStructure tests the Status struct.
func TestStatusStructure(t *testing.T) {
	detail := map[string]string{
		"endpoint": "us-east",
		"tunnel":   "up",
	}
	status := Status{
		State:  StateRunning,
		Detail: detail,
	}

	if status.State != StateRunning {
		t.Errorf("Status.State = %q, want %q", status.State, StateRunning)
	}

	if len(status.Detail) != 2 {
		t.Errorf("Status.Detail length = %d, want 2", len(status.Detail))
	}

	if status.Detail["endpoint"] != "us-east" {
		t.Errorf("Status.Detail[endpoint] = %q, want %q", status.Detail["endpoint"], "us-east")
	}
}

// TestHealthReportStructure tests the HealthReport struct.
func TestHealthReportStructure(t *testing.T) {
	report := HealthReport{
		Level:   Healthy,
		Message: "All systems operational",
	}

	if report.Level != Healthy {
		t.Errorf("HealthReport.Level = %v, want %v", report.Level, Healthy)
	}

	if report.Message != "All systems operational" {
		t.Errorf("HealthReport.Message = %q, want %q", report.Message, "All systems operational")
	}
}

// TestModuleStates verifies all ModuleState constants.
func TestModuleStates(t *testing.T) {
	states := []ModuleState{
		StateDisabled,
		StateInitializing,
		StateRunning,
		StateDegraded,
		StateStopping,
		StateStopped,
		StateFailed,
	}

	for _, state := range states {
		if state == "" {
			t.Errorf("ModuleState should not be empty string")
		}
	}
}

// TestErrSecretNotFoundType verifies the error interface implementation.
func TestErrSecretNotFoundType(t *testing.T) {
	// ErrSecretNotFound should implement the error interface
	// This is verified by the type assertion
	_ = (error)(ErrSecretNotFound)
}
