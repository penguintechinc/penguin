// Command penguind is the privileged endpoint-agent daemon. All privileged
// operations (tunnels, port 53, resolver changes) live in modules hosted here;
// the penguin CLI and penguin-tray stay unprivileged.
package main

import (
	"context"
	"flag"
	"fmt"
	"os"
	"os/signal"
	"syscall"

	"github.com/penguintechinc/penguin/internal/telemetry"
	"github.com/penguintechinc/penguin/internal/version"
	"github.com/kardianos/service"
	"go.uber.org/zap"
)

func main() {
	if err := run(os.Args[1:]); err != nil {
		fmt.Fprintf(os.Stderr, "penguind: %v\n", err)
		os.Exit(1)
	}
}

func run(args []string) error {
	// `penguind version` must work without any privileges or config.
	if len(args) == 1 && (args[0] == "version" || args[0] == "--version") {
		fmt.Println(version.Version)
		return nil
	}

	// Handle service subcommands (install, uninstall, start, stop, status).
	// These must run BEFORE loading config or acquiring locks.
	if len(args) > 0 && args[0] == "service" {
		handled, err := handleServiceCommand(args)
		if handled {
			return err
		}
	}

	// Parse flags
	fs := flag.NewFlagSet("penguind", flag.ContinueOnError)
	configDir := fs.String("config-dir", "/etc/penguin", "configuration directory")
	stateDir := fs.String("state-dir", "/var/lib/penguind", "state directory")
	socketPath := fs.String("socket", "", "override socket path")

	if err := fs.Parse(args); err != nil {
		return err
	}

	// Initialize telemetry
	tel, err := telemetry.New("info")
	if err != nil {
		return fmt.Errorf("init telemetry: %w", err)
	}
	defer func() {
		_ = tel.Logger.Sync()
	}()

	logger := tel.Logger.Named("penguind")

	// Initialize daemon components
	prog, err := initDaemon(*configDir, *stateDir, *socketPath, logger, tel)
	if err != nil {
		return err
	}

	// Create service configuration for native OS integration (Windows SCM, systemd, launchd)
	svcConfig := &service.Config{
		Name:        "penguind",
		DisplayName: "Penguin Daemon",
		Description: "Privileged endpoint-agent daemon for Penguin",
		Arguments:   []string{}, // No args needed; daemon config already loaded
	}

	svc, err := service.New(prog, svcConfig)
	if err != nil {
		return fmt.Errorf("create service: %w", err)
	}

	// Determine whether we're running interactively or as a service.
	// service.Interactive() returns true when run from a terminal.
	// In interactive mode, we serve directly. In service mode (Windows SCM, systemd, etc.),
	// service.Run() will call Start() and Stop() on the Program.
	if service.Interactive() {
		// Foreground/interactive mode: serve the gRPC server and handle signals directly
		logger.Info("running in interactive mode")

		// Setup signal handling
		sigChan := make(chan os.Signal, 1)
		signal.Notify(sigChan, syscall.SIGINT, syscall.SIGTERM)

		// Serve in a goroutine so we can catch signals
		errChan := make(chan error, 1)
		go func() {
			if err := prog.serve(); err != nil {
				errChan <- err
			}
			close(errChan)
		}()

		// Wait for signal or error
		select {
		case sig := <-sigChan:
			logger.Info("received signal", zap.String("signal", sig.String()))
			_ = prog.Stop(svc)

		case err := <-errChan:
			if err != nil {
				return err
			}
		}

		return nil
	}

	// Service mode: let the OS service manager (systemd, Windows SCM, launchd) control lifecycle
	logger.Info("running in service mode")
	return svc.Run()
}

// updateClientAdapter adapts to our Client interface.
type updateClientAdapter struct {
	version string
	logger  *zap.Logger
}

func (a *updateClientAdapter) CheckUpdate(ctx context.Context) (bool, string, error) {
	// For now, return no updates available
	// In production, would check GitHub releases
	return false, a.version, nil
}

func (a *updateClientAdapter) ApplyUpdate(ctx context.Context) error {
	return fmt.Errorf("update not implemented")
}
