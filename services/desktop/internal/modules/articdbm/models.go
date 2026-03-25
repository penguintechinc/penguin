package articdbm

import "time"

// Proxy represents a database proxy instance.
type Proxy struct {
	Name              string    `json:"name"`
	DatabaseType      string    `json:"database_type"` // mysql, postgresql, mssql, mongodb, redis
	Host              string    `json:"host"`
	Port              int       `json:"port"`
	Status            string    `json:"status"` // running, stopped, error
	ActiveConnections int       `json:"active_connections"`
	TotalQueries      int64     `json:"total_queries"`
	TLSEnabled        bool      `json:"tls_enabled"`
	Backends          []Backend `json:"backends"`
	CreatedAt         time.Time `json:"created_at"`
	UpdatedAt         time.Time `json:"updated_at"`
}

// Backend represents a database backend behind a proxy.
type Backend struct {
	Host        string `json:"host"`
	Port        int    `json:"port"`
	Type        string `json:"type"` // read, write
	Weight      int    `json:"weight"`
	TLS         bool   `json:"tls"`
	Database    string `json:"database"`
	HealthState string `json:"health_state"` // healthy, unhealthy, unknown
}

// CreateProxyRequest for creating a new proxy.
type CreateProxyRequest struct {
	Name         string    `json:"name"`
	DatabaseType string    `json:"database_type"`
	Port         int       `json:"port"`
	TLSEnabled   bool      `json:"tls_enabled"`
	Backends     []Backend `json:"backends"`
}

// DeploymentStatus represents deployment state.
type DeploymentStatus struct {
	Strategy   string  `json:"strategy"` // percentage, canary, blue_green
	Primary    string  `json:"primary"`
	Secondary  string  `json:"secondary"`
	TrafficPct float64 `json:"traffic_pct"`
}
