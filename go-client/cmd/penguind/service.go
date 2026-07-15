package main

import (
	"context"
	"errors"
	"fmt"
	"net"
	"os"
	"path/filepath"

	daemonv1 "github.com/penguintechinc/penguin/api/proto/penguin/daemon/v1"
	"github.com/penguintechinc/penguin/internal/daemon"
	"github.com/penguintechinc/penguin/internal/ipc"
	"github.com/penguintechinc/penguin/internal/licensing"
	"github.com/penguintechinc/penguin/internal/registry"
	"github.com/penguintechinc/penguin/internal/secrets"
	"github.com/penguintechinc/penguin/internal/telemetry"
	"github.com/penguintechinc/penguin/internal/version"
	"github.com/penguintechinc/penguin/pkg/sdk"
	"github.com/kardianos/service"
	"go.uber.org/zap"
	"google.golang.org/grpc"
	"google.golang.org/grpc/health"
	"google.golang.org/grpc/health/grpc_health_v1"
)

// daemonProgram implements service.Service and encapsulates the daemon lifecycle.
type daemonProgram struct {
	configDir    string
	stateDir     string
	socketPath   string
	logger       *zap.Logger
	tel          *telemetry.Telemetry
	grpcServer   *grpc.Server
	listener     net.Listener
	supervisor   *daemon.Supervisor
	secretStore  *secrets.Store
	licenseStop  context.CancelFunc
}

// Start launches the daemon serve loop in a goroutine and returns immediately.
// This is required by the service.Service interface; the actual blocking occurs
// in Serve() below when run via service.Run().
func (p *daemonProgram) Start(s service.Service) error {
	if p.logger == nil {
		return fmt.Errorf("logger not initialized")
	}

	p.logger.Info("starting daemon service")

	// Launch the blocking serve loop in a goroutine
	go func() {
		if err := p.serve(); err != nil {
			p.logger.Error("serve failed", zap.Error(err))
			// In service mode, we can't exit the process directly,
			// but the error is logged for the service manager.
		}
	}()

	return nil
}

// Stop triggers graceful shutdown and cleanup.
func (p *daemonProgram) Stop(s service.Service) error {
	p.logger.Info("stopping daemon service")

	// Trigger graceful shutdown of gRPC server
	if p.grpcServer != nil {
		p.grpcServer.GracefulStop()
	}

	// Cancel license client context
	if p.licenseStop != nil {
		p.licenseStop()
	}

	// Shutdown supervisor
	ctx := context.Background()
	if p.supervisor != nil {
		if err := p.supervisor.Shutdown(ctx); err != nil {
			p.logger.Warn("shutdown supervisor", zap.Error(err))
		}
	}

	// Clean up socket file
	var cfg daemon.DaemonConfig
	configStore := daemon.NewConfigStore(p.configDir)
	if daemonCfg, err := configStore.Daemon(); err == nil {
		cfg = daemonCfg
	} else {
		cfg.SocketPath = p.socketPath
	}
	if cfg.SocketPath != "" {
		if err := os.Remove(cfg.SocketPath); err != nil && !os.IsNotExist(err) {
			p.logger.Warn("remove socket", zap.Error(err))
		}
	}

	// Close listener
	if p.listener != nil {
		if err := p.listener.Close(); err != nil {
			p.logger.Warn("close listener", zap.Error(err))
		}
	}

	// Sync logger
	if p.tel != nil {
		_ = p.tel.Logger.Sync()
	}

	return nil
}

// serve blocks on the gRPC server and is called from Start() in a goroutine.
// In interactive mode (foreground), this is called directly from initDaemon.
func (p *daemonProgram) serve() error {
	if p.grpcServer == nil || p.listener == nil {
		return fmt.Errorf("grpc server or listener not initialized")
	}

	if err := p.grpcServer.Serve(p.listener); err != nil && !errors.Is(err, grpc.ErrServerStopped) {
		return fmt.Errorf("serve: %w", err)
	}

	return nil
}

// initDaemon initializes all daemon components and returns a configured daemonProgram.
// This is called both by service.Run() and by interactive mode.
func initDaemon(configDir, stateDir, socketPath string, logger *zap.Logger, tel *telemetry.Telemetry) (*daemonProgram, error) {
	// Load daemon config
	configStore := daemon.NewConfigStore(configDir)
	daemonCfg, err := configStore.Daemon()
	if err != nil {
		logger.Warn("failed to load daemon config, using defaults", zap.Error(err))
		daemonCfg = daemon.DaemonConfig{
			SocketPath: "/run/penguin/penguind.sock",
			Group:      "penguin",
		}
	}

	// Override socket path if specified
	if socketPath != "" {
		daemonCfg.SocketPath = socketPath
	}

	// Ensure state directory exists
	if err := os.MkdirAll(stateDir, 0o700); err != nil {
		return nil, fmt.Errorf("mkdir state dir: %w", err)
	}

	// Single-instance guard
	releaseLock, err := daemon.AcquireLock(filepath.Join(stateDir, "penguind.lock"))
	if err != nil {
		return nil, err
	}

	// Initialize secrets store
	secretStore, err := secrets.Open(secrets.Config{
		ServiceName: "penguind",
		FileDir:     stateDir,
		FilePasswordFunc: func() ([]byte, error) {
			return secrets.EnsureMasterKey(stateDir + "/keyring.key")
		},
	})
	if err != nil {
		_ = releaseLock()
		return nil, fmt.Errorf("init secrets: %w", err)
	}

	// Initialize licensing client
	licenseKey := os.Getenv("LICENSE_KEY")
	licenseClient := licensing.New(licensing.Options{
		LicenseKey: licenseKey,
		Product:    "penguin",
		BaseURL:    "https://license.penguintech.io",
		CacheDir:   stateDir + "/license",
	})

	// Start background license refresh with cancellation.
	// The licenseStop function is stored in prog and called during shutdown.
	// If we error before creating prog, we must cancel to avoid context leak.
	licenseCtx, licenseStop := context.WithCancel(context.Background())
	go func() {
		if err := licenseClient.Start(licenseCtx); err != nil {
			logger.Warn("failed to start license refresh", zap.Error(err))
		}
	}()

	// Create event broker
	eventBroker := daemon.NewEventBroker()

	// Module config schemas
	schemas := make(map[string]([]byte), len(registry.All()))
	for _, factory := range registry.All() {
		m := factory()
		schemas[m.Info().Name] = m.ConfigSchema()
	}

	// Build host factory
	hostFactory := func(moduleName string) sdk.HostServices {
		raw, err := configStore.ModuleRaw(moduleName, schemas[moduleName])
		if err != nil {
			logger.Error("invalid module config; module will start with defaults",
				zap.String("module", moduleName), zap.Error(err))
			raw = nil
		}
		return daemon.NewHost(moduleName, tel, secretStore, licenseClient, stateDir, eventBroker, raw)
	}

	// Create supervisor
	supervisor := daemon.New(daemon.Config{
		Modules:   registry.All(),
		Host:      hostFactory,
		StatePath: stateDir + "/enabled.json",
		Logger:    logger,
		Backoff:   daemon.DefaultBackoff(),
	})

	// Create update client adapter
	updateAdapter := &updateClientAdapter{
		version: version.Version,
		logger:  logger,
	}

	// Create daemon server
	server := daemon.NewServer(supervisor, version.Version, logger, updateAdapter)

	// Create IPC listener
	listener, credOpt, err := ipc.Listen(ipc.ListenerConfig{
		Path:         daemonCfg.SocketPath,
		AllowedGroup: daemonCfg.Group,
	})
	if err != nil {
		licenseStop() // Cancel the license context before returning
		_ = releaseLock()
		return nil, fmt.Errorf("listen ipc: %w", err)
	}

	logger.Info("listening on socket", zap.String("path", daemonCfg.SocketPath))

	// Create gRPC server with peer auth
	unaryAuth, streamAuth := ipc.PeerAuthInterceptor(daemonCfg.Group)
	grpcServer := grpc.NewServer(
		credOpt,
		grpc.UnaryInterceptor(unaryAuth),
		grpc.StreamInterceptor(streamAuth),
	)

	// Register daemon service
	daemonv1.RegisterDaemonServer(grpcServer, server)

	// Register health checks
	healthServer := health.NewServer()
	grpc_health_v1.RegisterHealthServer(grpcServer, healthServer)
	healthServer.SetServingStatus("penguin.daemon.v1.Daemon", grpc_health_v1.HealthCheckResponse_SERVING)

	// Start enabled modules
	ctx := context.Background()
	if err := supervisor.StartEnabled(ctx); err != nil {
		logger.Warn("failed to start enabled modules", zap.Error(err))
	}

	prog := &daemonProgram{
		configDir:   configDir,
		stateDir:    stateDir,
		socketPath:  daemonCfg.SocketPath,
		logger:      logger,
		tel:         tel,
		grpcServer:  grpcServer,
		listener:    listener,
		supervisor:  supervisor,
		secretStore: secretStore,
		licenseStop: licenseStop,
	}

	// Store the lock release function for cleanup on exit
	// (we can't easily expose this through the service interface,
	// so we rely on process termination to clean up the lock)
	_ = releaseLock

	return prog, nil
}
