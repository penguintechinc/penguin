package skauswatch

// ThreatAlert represents a security threat detected on the endpoint.
type ThreatAlert struct {
	ID          string `json:"id"`
	Severity    string `json:"severity"`    // critical, high, medium, low, info
	Type        string `json:"type"`        // malware, intrusion, anomaly, policy
	Description string `json:"description"`
	Timestamp   string `json:"timestamp"`
	Status      string `json:"status"` // active, resolved, dismissed
	FilePath    string `json:"file_path,omitempty"`
}

// ScanResult represents the outcome of a security scan.
type ScanResult struct {
	ID           string `json:"id"`
	Type         string `json:"type"`   // full, quick, custom
	Status       string `json:"status"` // running, completed, failed
	StartedAt    string `json:"started_at"`
	CompletedAt  string `json:"completed_at,omitempty"`
	ThreatsFound int    `json:"threats_found"`
	FilesScanned int    `json:"files_scanned"`
}

// QuarantineEntry represents a file that has been quarantined.
type QuarantineEntry struct {
	ID            string `json:"id"`
	FilePath      string `json:"file_path"`
	ThreatType    string `json:"threat_type"`
	QuarantinedAt string `json:"quarantined_at"`
	Status        string `json:"status"` // quarantined, restored, deleted
}

// EndpointStatus represents the registration and health state of this endpoint.
type EndpointStatus struct {
	Registered   bool   `json:"registered"`
	EndpointID   string `json:"endpoint_id,omitempty"`
	LastCheckin   string `json:"last_checkin,omitempty"`
	AgentVersion string `json:"agent_version"`
	OSInfo       string `json:"os_info"`
	ThreatCount  int    `json:"threat_count"`
	ScanStatus   string `json:"scan_status,omitempty"`
}

// registerRequest is the payload for endpoint registration.
type registerRequest struct {
	Hostname     string `json:"hostname"`
	OS           string `json:"os"`
	AgentVersion string `json:"agent_version"`
}

// registerResponse is the response from endpoint registration.
type registerResponse struct {
	EndpointID string `json:"endpoint_id"`
}
