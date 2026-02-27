package ntp

import (
	"context"
	"encoding/binary"
	"fmt"
	"net"
	"sync"
	"time"

	"github.com/sirupsen/logrus"
)

// ntpEpochOffset is the number of seconds between the NTP epoch (1900) and Unix epoch (1970)
const ntpEpochOffset = 2208988800

// TimeResponse contains the result of an NTP query.
type TimeResponse struct {
	Time    time.Time
	Offset  time.Duration
	Delay   time.Duration
	Server  string
	Stratum uint8
}

// ClientConfig configures the NTP client.
type ClientConfig struct {
	Servers []string
	Timeout time.Duration
}

// Client is an NTP client supporting multiple servers with failover.
type Client struct {
	servers []string
	timeout time.Duration
	logger  *logrus.Logger
	mu      sync.RWMutex
}

// ntpPacket represents the NTP packet structure (RFC 5905)
type ntpPacket struct {
	Settings       uint8  // LI, VN, Mode
	Stratum        uint8  // Stratum level
	Poll           int8   // Poll interval exponent
	Precision      int8   // Precision exponent
	RootDelay      uint32 // Root delay
	RootDispersion uint32 // Root dispersion
	ReferenceID    uint32 // Reference identifier
	RefTimeSec     uint32 // Reference timestamp (seconds)
	RefTimeFrac    uint32 // Reference timestamp (fraction)
	OrigTimeSec    uint32 // Origin timestamp (seconds)
	OrigTimeFrac   uint32 // Origin timestamp (fraction)
	RxTimeSec      uint32 // Receive timestamp (seconds)
	RxTimeFrac     uint32 // Receive timestamp (fraction)
	TxTimeSec      uint32 // Transmit timestamp (seconds)
	TxTimeFrac     uint32 // Transmit timestamp (fraction)
}

// NewClient creates an NTP client with optional custom configuration.
// If cfg is nil, default public NTP servers are used.
func NewClient(cfg *ClientConfig, logger *logrus.Logger) *Client {
	servers := []string{
		"pool.ntp.org:123",
		"time.google.com:123",
		"time.cloudflare.com:123",
	}
	timeout := 5 * time.Second

	if cfg != nil {
		if len(cfg.Servers) > 0 {
			servers = cfg.Servers
		}
		if cfg.Timeout > 0 {
			timeout = cfg.Timeout
		}
	}

	if logger == nil {
		logger = logrus.New()
	}

	return &Client{
		servers: servers,
		timeout: timeout,
		logger:  logger,
	}
}

// Query performs an NTP query with automatic failover across configured servers.
// It returns the first successful response or an error if all servers fail.
func (c *Client) Query(ctx context.Context) (*TimeResponse, error) {
	var lastErr error

	for _, server := range c.servers {
		resp, err := c.queryServer(ctx, server)
		if err != nil {
			lastErr = err
			c.logger.WithError(err).WithField("server", server).Debug("NTP query failed")
			continue
		}
		c.logger.WithField("server", server).Debug("NTP query succeeded")
		return resp, nil
	}

	return nil, fmt.Errorf("all NTP servers failed, last error: %w", lastErr)
}

// queryServer performs an NTP query to a single server.
func (c *Client) queryServer(ctx context.Context, serverAddr string) (*TimeResponse, error) {
	// Establish UDP connection
	conn, err := net.DialTimeout("udp", serverAddr, c.timeout)
	if err != nil {
		return nil, fmt.Errorf("connecting to %s: %w", serverAddr, err)
	}
	defer conn.Close()

	// Set deadline from context or use timeout
	if deadline, ok := ctx.Deadline(); ok {
		conn.SetDeadline(deadline)
	} else {
		conn.SetDeadline(time.Now().Add(c.timeout))
	}

	// Prepare NTP v4 client request packet
	// Settings: LI=0 (no warning), VN=4 (version 4), Mode=3 (client)
	packet := &ntpPacket{
		Settings: 0x23, // 0x20 (version 4) | 0x03 (mode 3)
	}

	// Record transmission time (t1)
	t1 := time.Now()

	// Send request packet
	if err := binary.Write(conn, binary.BigEndian, packet); err != nil {
		return nil, fmt.Errorf("sending packet: %w", err)
	}

	// Receive response packet
	response := &ntpPacket{}
	if err := binary.Read(conn, binary.BigEndian, response); err != nil {
		return nil, fmt.Errorf("reading response: %w", err)
	}

	// Record reception time (t4)
	t4 := time.Now()

	// Convert NTP timestamps from packet to time.Time
	t2 := ntpToTime(response.RxTimeSec, response.RxTimeFrac) // Server receive time
	t3 := ntpToTime(response.TxTimeSec, response.TxTimeFrac) // Server transmit time

	// Calculate offset and delay using NTP algorithm (RFC 5905)
	// offset = ((t2 - t1) + (t3 - t4)) / 2
	// delay = (t4 - t1) - (t3 - t2)
	offset := ((t2.Sub(t1)) + (t3.Sub(t4))) / 2
	delay := (t4.Sub(t1)) - (t3.Sub(t2))

	return &TimeResponse{
		Time:    time.Now().Add(offset),
		Offset:  offset,
		Delay:   delay,
		Server:  serverAddr,
		Stratum: response.Stratum,
	}, nil
}

// ntpToTime converts NTP timestamp (seconds + fractional part) to time.Time
// NTP epoch is January 1, 1900; Unix epoch is January 1, 1970
func ntpToTime(sec, frac uint32) time.Time {
	// Convert from NTP epoch to Unix epoch
	secs := int64(sec) - ntpEpochOffset
	// Convert fractional part (32-bit) to nanoseconds (32-bit >> 32 = 1e9)
	nanos := (int64(frac) * 1e9) >> 32
	return time.Unix(secs, nanos)
}

// GetServerURLs returns a copy of the configured NTP server addresses.
func (c *Client) GetServerURLs() []string {
	c.mu.RLock()
	defer c.mu.RUnlock()

	result := make([]string, len(c.servers))
	copy(result, c.servers)
	return result
}

// Close cleans up client resources.
// Currently a no-op but provided for interface compatibility.
func (c *Client) Close() error {
	return nil
}
