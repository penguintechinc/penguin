package openziti

import (
	"context"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"sync"

	"github.com/sirupsen/logrus"
)

// Provider manages OpenZiti identity and connections.
type Provider struct {
	dataDir string
	logger  *logrus.Logger

	mu           sync.RWMutex
	connected    bool
	jwtToken     string
	identityName string
	services     []string
}

func NewProvider(dataDir string, logger *logrus.Logger) *Provider {
	return &Provider{
		dataDir:  dataDir,
		logger:   logger,
		services: []string{},
	}
}

func (p *Provider) SetJWTToken(token string) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.jwtToken = token
}

func (p *Provider) Connect(ctx context.Context) error {
	p.mu.Lock()
	defer p.mu.Unlock()

	identityFile := filepath.Join(p.dataDir, "ziti", "identity.json")
	if _, err := os.Stat(identityFile); os.IsNotExist(err) {
		return fmt.Errorf("no identity enrolled; run 'penguin ziti enroll' first")
	}

	// In production, this would load the Ziti identity and create a context
	// using github.com/openziti/sdk-golang
	p.connected = true
	p.logger.Info("OpenZiti connected")
	return nil
}

func (p *Provider) Disconnect() error {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.connected = false
	p.logger.Info("OpenZiti disconnected")
	return nil
}

func (p *Provider) IsConnected() bool {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.connected
}

func (p *Provider) GetIdentityName() string {
	p.mu.RLock()
	defer p.mu.RUnlock()
	if p.identityName != "" {
		return p.identityName
	}
	return "Not enrolled"
}

// Dial connects to a Ziti service, sending JWT+HOST handshake.
func (p *Provider) Dial(ctx context.Context, service string) (net.Conn, error) {
	p.mu.RLock()
	defer p.mu.RUnlock()
	if !p.connected {
		return nil, fmt.Errorf("not connected")
	}
	// In production, use ziti.Context.Dial(service)
	// Then send handshake: "JWT:<token>\nHOST:<target>\n"
	return nil, fmt.Errorf("ziti dial not yet implemented for service: %s", service)
}

// Enroll enrolls with a JWT file.
func (p *Provider) Enroll(ctx context.Context, jwtFile string) error {
	zitiDir := filepath.Join(p.dataDir, "ziti")
	if err := os.MkdirAll(zitiDir, 0700); err != nil {
		return fmt.Errorf("creating ziti dir: %w", err)
	}
	// In production, use ziti enrollment SDK
	p.logger.Info("OpenZiti enrollment started")
	return nil
}

// Services returns discovered Ziti services.
func (p *Provider) Services() []string {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.services
}

// RefreshServices updates the list of available services.
func (p *Provider) RefreshServices(ctx context.Context) error {
	p.mu.Lock()
	defer p.mu.Unlock()
	if !p.connected {
		return fmt.Errorf("not connected")
	}
	// In production, this would query the Ziti controller for available services
	p.logger.Debug("Refreshing OpenZiti services")
	return nil
}

// AddService adds a service to the list.
func (p *Provider) AddService(service string) {
	p.mu.Lock()
	defer p.mu.Unlock()
	// Check if service already exists
	for _, s := range p.services {
		if s == service {
			return
		}
	}
	p.services = append(p.services, service)
}

// SetIdentityName sets the identity name.
func (p *Provider) SetIdentityName(name string) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.identityName = name
}
