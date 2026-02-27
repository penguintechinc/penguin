package waddleperf

// TestType identifies the network test protocol.
type TestType string

const (
	TestHTTP TestType = "http"
	TestTCP  TestType = "tcp"
	TestUDP  TestType = "udp"
	TestICMP TestType = "icmp"
)

// TestConfig defines parameters for a single network test.
type TestConfig struct {
	Type     TestType `json:"type"`
	Target   string   `json:"target"`   // URL, host:port, or IP
	Protocol string   `json:"protocol"` // auto, http1, http2, tls, raw, dns, ping, traceroute
	Timeout  int      `json:"timeout"`  // seconds
}

// TestResult holds the outcome of a completed network test.
type TestResult struct {
	ID           string   `json:"id"`
	Type         TestType `json:"type"`
	Target       string   `json:"target"`
	Status       string   `json:"status"` // success, failed, timeout
	Latency      float64  `json:"latency_ms"`
	DNSLookup    float64  `json:"dns_lookup_ms"`
	TCPConnect   float64  `json:"tcp_connect_ms"`
	TLSHandshake float64  `json:"tls_handshake_ms"`
	TTFB         float64  `json:"ttfb_ms"`
	TotalTime    float64  `json:"total_time_ms"`
	Jitter       float64  `json:"jitter_ms"`
	PacketLoss   float64  `json:"packet_loss_pct"`
	Timestamp    string   `json:"timestamp"`
}

// DeviceInfo identifies the device running the test.
type DeviceInfo struct {
	Serial   string `json:"serial"`
	Hostname string `json:"hostname"`
	OS       string `json:"os"`
	Version  string `json:"version"`
}

// ScheduleConfig controls periodic test execution.
type ScheduleConfig struct {
	Enabled  bool         `json:"enabled"`
	Interval int          `json:"interval"` // seconds
	Tests    []TestConfig `json:"tests"`
}

// uploadPayload is the request body for result upload.
type uploadPayload struct {
	Result TestResult `json:"result"`
	Device DeviceInfo `json:"device"`
}
