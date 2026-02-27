package dns

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/sirupsen/logrus"
)

// DNS record type constants.
const (
	TypeA     = 1
	TypeAAAA  = 28
	TypeCNAME = 5
	TypeMX    = 15
	TypeTXT   = 16
	TypeNS    = 2
	TypeSOA   = 6
	TypeSRV   = 33
	TypePTR   = 12
)

// DNSResponse represents a DNS-over-HTTPS JSON response.
// Adapted from Google's DNS-over-HTTPS API format.
type DNSResponse struct {
	Status     int         `json:"Status"` // 0 = NOERROR, 3 = NXDOMAIN, etc
	TC         bool        `json:"TC"`     // Truncated response
	RD         bool        `json:"RD"`     // Recursion desired
	RA         bool        `json:"RA"`     // Recursion available
	AD         bool        `json:"AD"`     // Authenticated data
	CD         bool        `json:"CD"`     // Checking disabled
	Question   []DNSRecord `json:"Question"`
	Answer     []DNSRecord `json:"Answer"`
	Authority  []DNSRecord `json:"Authority"`
	Additional []DNSRecord `json:"Additional"`
}

// DNSRecord represents a single DNS resource record.
type DNSRecord struct {
	Name string `json:"name"`
	Type int    `json:"type"`
	TTL  int    `json:"TTL"`
	Data string `json:"data"`
}

// DoHClient performs DNS-over-HTTPS queries with automatic failover.
// Adapted from Squawk's dns-client-go implementation.
type DoHClient struct {
	serverURLs []string
	authToken  string
	httpClient *http.Client
	logger     *logrus.Logger
	mu         sync.RWMutex
	serverIdx  int
}

// DoHConfig configures the DoH client.
type DoHConfig struct {
	ServerURLs []string
	AuthToken  string
	Timeout    time.Duration
	VerifySSL  bool
}

// NewDoHClient creates a new DoH client with the provided configuration.
func NewDoHClient(cfg *DoHConfig, logger *logrus.Logger) *DoHClient {
	if logger == nil {
		logger = logrus.New()
	}

	timeout := cfg.Timeout
	if timeout == 0 {
		timeout = 10 * time.Second
	}

	serverURLs := cfg.ServerURLs
	if len(serverURLs) == 0 {
		serverURLs = []string{
			"https://dns.google/dns-query",
		}
	}

	return &DoHClient{
		serverURLs: serverURLs,
		authToken:  cfg.AuthToken,
		httpClient: &http.Client{
			Timeout: timeout,
		},
		logger: logger,
	}
}

// Query performs a DNS query using DoH with automatic failover to next server.
// Returns the DNS response or an error if all servers fail.
func (c *DoHClient) Query(ctx context.Context, domain, recordType string) (*DNSResponse, error) {
	if err := validateDomain(domain); err != nil {
		return nil, err
	}

	typeNum := ParseRecordType(recordType)
	if typeNum == 0 {
		return nil, fmt.Errorf("unsupported record type: %s", recordType)
	}

	c.logger.WithFields(map[string]interface{}{
		"domain": domain,
		"type":   recordType,
	}).Debug("DNS query initiated")

	var lastErr error
	for attempt := 0; attempt < len(c.serverURLs); attempt++ {
		serverURL := c.nextServer()
		resp, err := c.queryServer(ctx, serverURL, domain, typeNum)
		if err != nil {
			lastErr = err
			c.logger.WithFields(map[string]interface{}{
				"server": serverURL,
				"error":  err,
			}).Debug("DoH query failed, trying next server")
			continue
		}
		return resp, nil
	}

	return nil, fmt.Errorf("all DoH servers failed for %s: %w", domain, lastErr)
}

// queryServer sends a single DoH query to a specific server.
func (c *DoHClient) queryServer(ctx context.Context, serverURL, domain string, recordType int) (*DNSResponse, error) {
	u, err := url.Parse(serverURL)
	if err != nil {
		return nil, fmt.Errorf("invalid server URL: %w", err)
	}

	q := u.Query()
	q.Set("name", domain)
	q.Set("type", fmt.Sprintf("%d", recordType))
	u.RawQuery = q.Encode()

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, u.String(), nil)
	if err != nil {
		return nil, fmt.Errorf("creating request: %w", err)
	}

	// Set required headers for DoH
	req.Header.Set("Accept", "application/dns-json")
	req.Header.Set("User-Agent", "penguin-desktop-dns/0.1.0")
	if c.authToken != "" {
		req.Header.Set("Authorization", "Bearer "+c.authToken)
	}

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("query request failed: %w", err)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("reading response body: %w", err)
	}

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("server returned status %d: %s", resp.StatusCode, string(body))
	}

	var dnsResp DNSResponse
	if err := json.Unmarshal(body, &dnsResp); err != nil {
		return nil, fmt.Errorf("parsing DNS response: %w", err)
	}

	return &dnsResp, nil
}

// nextServer returns the next server URL in round-robin fashion.
func (c *DoHClient) nextServer() string {
	c.mu.Lock()
	defer c.mu.Unlock()

	if len(c.serverURLs) == 0 {
		return ""
	}

	server := c.serverURLs[c.serverIdx%len(c.serverURLs)]
	c.serverIdx++
	return server
}

// validateDomain performs basic validation on a domain name.
func validateDomain(domain string) error {
	if domain == "" {
		return fmt.Errorf("domain cannot be empty")
	}
	if len(domain) > 253 {
		return fmt.Errorf("domain too long (max 253 characters)")
	}
	// Remove trailing dot if present
	domain = strings.TrimSuffix(domain, ".")
	if domain == "" {
		return fmt.Errorf("invalid domain")
	}
	return nil
}

// ParseRecordType converts a string record type to its numeric value.
// Returns 0 if the type is not recognized.
func ParseRecordType(t string) int {
	switch strings.ToUpper(strings.TrimSpace(t)) {
	case "A":
		return TypeA
	case "AAAA":
		return TypeAAAA
	case "CNAME":
		return TypeCNAME
	case "MX":
		return TypeMX
	case "TXT":
		return TypeTXT
	case "NS":
		return TypeNS
	case "SOA":
		return TypeSOA
	case "SRV":
		return TypeSRV
	case "PTR":
		return TypePTR
	default:
		return 0
	}
}

// RecordTypeName converts a numeric record type to its string name.
func RecordTypeName(t int) string {
	switch t {
	case TypeA:
		return "A"
	case TypeAAAA:
		return "AAAA"
	case TypeCNAME:
		return "CNAME"
	case TypeMX:
		return "MX"
	case TypeTXT:
		return "TXT"
	case TypeNS:
		return "NS"
	case TypeSOA:
		return "SOA"
	case TypeSRV:
		return "SRV"
	case TypePTR:
		return "PTR"
	default:
		return fmt.Sprintf("TYPE%d", t)
	}
}
