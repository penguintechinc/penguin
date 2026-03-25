package killkrill

// LogEntry represents a log line in ECS-compatible format for submission to KillKrill.
type LogEntry struct {
	Timestamp   string            `json:"@timestamp"`
	Level       string            `json:"level"` // info, warning, error
	Message     string            `json:"message"`
	ServiceName string            `json:"service_name"`
	Fields      map[string]string `json:"fields,omitempty"`
}

// MetricEntry represents a single metric data point.
type MetricEntry struct {
	Name      string            `json:"name"`
	Value     float64           `json:"value"`
	Type      string            `json:"type"` // counter, histogram, gauge
	Labels    map[string]string `json:"labels,omitempty"`
	Timestamp string            `json:"timestamp"`
}

// QueueStatus reports the current state of the local log/metric queue.
type QueueStatus struct {
	LogsPending    int    `json:"logs_pending"`
	MetricsPending int    `json:"metrics_pending"`
	LastFlush      string `json:"last_flush"`
	Connected      bool   `json:"connected"`
}

// KillKrillConfig holds connection settings for the KillKrill service.
type KillKrillConfig struct {
	BaseURL       string `json:"base_url"`
	ClientID      string `json:"client_id"`
	ClientSecret  string `json:"client_secret"`
	FlushInterval int    `json:"flush_interval"` // seconds
}

// authResponse is the token payload returned by /api/v1/auth/login.
type authResponse struct {
	AccessToken  string `json:"access_token"`
	RefreshToken string `json:"refresh_token"`
	ExpiresIn    int    `json:"expires_in"`
}
