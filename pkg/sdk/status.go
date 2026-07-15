package sdk

import "time"

// ModuleState is the supervisor-visible lifecycle state of a module.
type ModuleState string

const (
	StateDisabled     ModuleState = "disabled"
	StateInitializing ModuleState = "initializing"
	StateRunning      ModuleState = "running"
	StateDegraded     ModuleState = "degraded"
	StateStopping     ModuleState = "stopping"
	StateStopped      ModuleState = "stopped"
	StateFailed       ModuleState = "failed"
)

// Status is a module's self-reported operational state plus display details.
type Status struct {
	State ModuleState
	// Detail carries small, non-sensitive KV pairs for `penguin status` and
	// the tray (e.g. "endpoint": "us-east", "tunnel": "up").
	Detail map[string]string
}

// HealthLevel grades a module's health.
type HealthLevel int

const (
	Healthy HealthLevel = iota
	Degraded
	Unhealthy
)

func (h HealthLevel) String() string {
	switch h {
	case Healthy:
		return "healthy"
	case Degraded:
		return "degraded"
	case Unhealthy:
		return "unhealthy"
	default:
		return "unknown"
	}
}

// HealthReport is the result of a Health probe.
type HealthReport struct {
	Level     HealthLevel
	Message   string
	CheckedAt time.Time
}
