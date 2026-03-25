package nest

import (
	"encoding/json"
	"time"
)

// Resource represents a managed resource.
type Resource struct {
	ID                 uint            `json:"id"`
	Name               string          `json:"name"`
	ResourceTypeID     uint            `json:"resource_type_id"`
	TeamID             uint            `json:"team_id"`
	Status             string          `json:"status"`         // pending, provisioning, active, updating, paused, error, deleted
	LifecycleMode      string          `json:"lifecycle_mode"` // full, partial, monitor_only
	ProvisioningMethod string          `json:"provisioning_method,omitempty"`
	ConnectionInfo     json.RawMessage `json:"connection_info,omitempty"`
	TLSEnabled         bool            `json:"tls_enabled"`
	K8sNamespace       string          `json:"k8s_namespace,omitempty"`
	K8sResourceName    string          `json:"k8s_resource_name,omitempty"`
	Config             json.RawMessage `json:"config,omitempty"`
	CanModifyUsers     bool            `json:"can_modify_users"`
	CanModifyConfig    bool            `json:"can_modify_config"`
	CanBackup          bool            `json:"can_backup"`
	CanScale           bool            `json:"can_scale"`
	CreatedAt          time.Time       `json:"created_at"`
	UpdatedAt          time.Time       `json:"updated_at"`
}

// Team represents a team.
type Team struct {
	ID          uint      `json:"id"`
	Name        string    `json:"name"`
	Description string    `json:"description"`
	IsGlobal    bool      `json:"is_global"`
	CreatedAt   time.Time `json:"created_at"`
	UpdatedAt   time.Time `json:"updated_at"`
}

// ResourceStats contains resource statistics.
type ResourceStats struct {
	ResourceID  uint            `json:"resource_id"`
	Metrics     json.RawMessage `json:"metrics"`
	RiskLevel   string          `json:"risk_level"`
	RiskFactors json.RawMessage `json:"risk_factors"`
	Timestamp   time.Time       `json:"timestamp"`
}

// ConnectionInfo contains resource connection details.
type ConnectionInfo struct {
	ConnectionInfo json.RawMessage `json:"connection_info"`
	TLSEnabled     bool            `json:"tls_enabled"`
	AccessLevel    string          `json:"access_level"` // full, restricted
}

// CreateResourceRequest for creating a resource.
type CreateResourceRequest struct {
	Name               string          `json:"name"`
	ResourceTypeID     uint            `json:"resource_type_id"`
	TeamID             uint            `json:"team_id"`
	LifecycleMode      string          `json:"lifecycle_mode"`
	ProvisioningMethod string          `json:"provisioning_method,omitempty"`
	ConnectionInfo     json.RawMessage `json:"connection_info,omitempty"`
	Config             json.RawMessage `json:"config,omitempty"`
	TLSEnabled         bool            `json:"tls_enabled"`
}

// UpdateResourceRequest for updating a resource.
type UpdateResourceRequest struct {
	Name   string          `json:"name,omitempty"`
	Status string          `json:"status,omitempty"`
	Config json.RawMessage `json:"config,omitempty"`
}
