package app

import (
	"context"
	"os"
	"os/signal"
	"syscall"
)

// WaitForShutdown blocks until a shutdown signal is received.
func (a *App) WaitForShutdown(ctx context.Context, cancel context.CancelFunc) {
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)

	select {
	case sig := <-sigCh:
		a.Logger.WithField("signal", sig.String()).Info("Shutdown signal received")
		cancel()
	case <-ctx.Done():
	}

	a.Stop(ctx)
}

// Run initializes, starts, and waits for shutdown.
func (a *App) Run(ctx context.Context) error {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()

	if err := a.Init(ctx); err != nil {
		return err
	}

	if err := a.Start(ctx); err != nil {
		return err
	}

	a.WaitForShutdown(ctx, cancel)
	return nil
}
