package modulepb

import "testing"

func TestHealthStateString(t *testing.T) {
	tests := []struct {
		state HealthState
		want  string
	}{
		{HealthUnknown, "unknown"},
		{HealthHealthy, "healthy"},
		{HealthDegraded, "degraded"},
		{HealthUnhealthy, "unhealthy"},
		{HealthState(99), "unknown"},
	}
	for _, tt := range tests {
		got := tt.state.String()
		if got != tt.want {
			t.Errorf("HealthState(%d).String() = %q, want %q", tt.state, got, tt.want)
		}
	}
}
