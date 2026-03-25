package dns

import (
	"context"
	"fmt"
	"net"
	"sync"

	"github.com/sirupsen/logrus"
)

// Forwarder forwards traditional DNS queries to the DoH backend.
// This allows applications using standard DNS to benefit from DoH encryption.
// Adapted from Squawk's dns-client-go implementation.
type Forwarder struct {
	dohClient  *DoHClient
	listenAddr string
	logger     *logrus.Logger

	mu       sync.RWMutex
	running  bool
	listener net.PacketConn
	cancel   context.CancelFunc
	ctx      context.Context
}

// ForwarderConfig configures the DNS forwarder.
type ForwarderConfig struct {
	ListenAddr string // e.g., "127.0.0.1:53" or ":53" for all interfaces
	ListenUDP  bool
	ListenTCP  bool // Not yet implemented
}

// NewForwarder creates a new DNS forwarder instance.
func NewForwarder(client *DoHClient, cfg *ForwarderConfig, logger *logrus.Logger) *Forwarder {
	if logger == nil {
		logger = logrus.New()
	}

	addr := cfg.ListenAddr
	if addr == "" {
		addr = "127.0.0.1:53"
	}

	return &Forwarder{
		dohClient:  client,
		listenAddr: addr,
		logger:     logger,
		running:    false,
	}
}

// Start begins listening for DNS queries on the configured address.
// Returns an error if the forwarder is already running or cannot bind to the address.
func (f *Forwarder) Start(ctx context.Context) error {
	f.mu.Lock()
	if f.running {
		f.mu.Unlock()
		return fmt.Errorf("forwarder already running")
	}

	// Attempt to listen on UDP
	conn, err := net.ListenPacket("udp", f.listenAddr)
	if err != nil {
		f.mu.Unlock()
		return fmt.Errorf("listening on %s: %w", f.listenAddr, err)
	}

	// Create a context that can be cancelled
	fwdCtx, cancel := context.WithCancel(ctx)
	f.ctx = fwdCtx
	f.listener = conn
	f.cancel = cancel
	f.running = true

	f.logger.WithField("addr", f.listenAddr).Info("DNS forwarder started")
	f.mu.Unlock()

	// Handle packets in a goroutine
	go f.handlePackets(fwdCtx)

	return nil
}

// Stop gracefully shuts down the forwarder.
func (f *Forwarder) Stop() {
	f.mu.Lock()
	defer f.mu.Unlock()

	if !f.running {
		return
	}

	if f.cancel != nil {
		f.cancel()
	}

	if f.listener != nil {
		f.listener.Close()
	}

	f.running = false
	f.logger.Info("DNS forwarder stopped")
}

// IsRunning returns whether the forwarder is currently active.
func (f *Forwarder) IsRunning() bool {
	f.mu.RLock()
	defer f.mu.RUnlock()
	return f.running
}

// GetListenAddr returns the address the forwarder is listening on.
func (f *Forwarder) GetListenAddr() string {
	f.mu.RLock()
	defer f.mu.RUnlock()
	return f.listenAddr
}

// handlePackets continuously reads and processes DNS queries.
func (f *Forwarder) handlePackets(ctx context.Context) {
	buf := make([]byte, 4096)

	for {
		select {
		case <-ctx.Done():
			return
		default:
		}

		n, addr, err := f.listener.ReadFrom(buf)
		if err != nil {
			// Check if context was cancelled
			select {
			case <-ctx.Done():
				return
			default:
			}

			// Check for timeout errors (expected with deadline)
			if netErr, ok := err.(net.Error); ok && netErr.Timeout() {
				continue
			}

			f.logger.WithError(err).Debug("Read error on DNS listener")
			continue
		}

		// Process the query in a goroutine to avoid blocking
		go f.handleRequest(ctx, buf[:n], addr)
	}
}

// getReadDeadline returns a deadline for the next read operation.
func (f *Forwarder) getReadDeadline() interface{} {
	// Use current time + 1 second for read deadline
	// This allows context checks while preventing indefinite blocks
	return nil // Let net.PacketConn use default behavior
}

// handleRequest processes a single DNS query packet.
// Note: Full DNS packet parsing requires external library like miekg/dns
// For now, this is a placeholder for the actual implementation.
func (f *Forwarder) handleRequest(ctx context.Context, data []byte, addr net.Addr) {
	// Basic validation - DNS packets must be at least 12 bytes (header)
	if len(data) < 12 {
		f.logger.WithField("from", addr.String()).Debug("Packet too short, ignoring")
		return
	}

	f.logger.WithFields(map[string]interface{}{
		"from": addr.String(),
		"size": len(data),
	}).Debug("DNS query received")

	// TODO: Parse DNS query packet to extract domain and record type
	// TODO: Forward to DoH client
	// TODO: Parse response and send back to client
	//
	// This requires DNS packet parsing. The miekg/dns library is recommended:
	// - Parse incoming query: msg := &dns.Msg{}; msg.Unpack(data)
	// - Extract question: domain = msg.Question[0].Name, type = msg.Question[0].Qtype
	// - Query DoH: resp, err := f.dohClient.Query(ctx, domain, RecordTypeName(type))
	// - Build response packet: respMsg := &dns.Msg{}; respMsg.SetReply(msg); ...
	// - Send back: respData, _ := respMsg.Pack(); f.listener.WriteTo(respData, addr)
}
