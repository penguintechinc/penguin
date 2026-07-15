// Command penguin-tray is the user-session system tray for the endpoint
// agent. It is a separate binary so penguin/penguind stay pure-Go (the tray
// needs cgo on macOS).
//
// The tray connects to penguind over the same authenticated local IPC as the
// CLI, subscribes to WatchEvents for push updates, and rebuilds its menu from
// the daemon's ListModules/GetStatus/ListCommands. All the daemon-facing logic
// lives in internal/tray (unit-tested); this file is the thin systray shell.
package main

import (
	"context"
	"flag"
	"fmt"
	"os"
	"time"

	"fyne.io/systray"
	daemonv1 "github.com/penguintechinc/penguin/api/proto/penguin/daemon/v1"
	"github.com/penguintechinc/penguin/internal/ipc"
	"github.com/penguintechinc/penguin/internal/tray"
	"github.com/penguintechinc/penguin/internal/version"
	"google.golang.org/grpc"
)

func main() {
	// Accept a bare `version` subcommand for parity with penguin/penguind.
	if len(os.Args) == 2 && (os.Args[1] == "version" || os.Args[1] == "--version") {
		fmt.Println(version.Version)
		return
	}

	fs := flag.NewFlagSet("penguin-tray", flag.ContinueOnError)
	socket := fs.String("socket", defaultSocket(), "daemon socket path")
	if err := fs.Parse(os.Args[1:]); err != nil {
		os.Exit(2)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	conn, err := ipc.Dial(ctx, *socket)
	if err == nil {
		client := daemonv1.NewDaemonClient(conn)
		// grpc.NewClient dials lazily, so probe with a real RPC before we hand
		// off to systray (which owns the process from then on).
		_, err = client.Version(ctx, &daemonv1.VersionRequest{ApiVersion: "v1"})
	}
	cancel()
	if err != nil {
		if conn != nil {
			_ = conn.Close()
		}
		fmt.Fprintf(os.Stderr, "penguin-tray: cannot reach penguind at %s — is the daemon running?\n", *socket)
		os.Exit(1)
	}

	app := &trayApp{
		conn:   conn,
		client: daemonv1.NewDaemonClient(conn),
	}
	systray.Run(app.onReady, app.onExit)
}

func defaultSocket() string {
	return "/run/penguin/penguind.sock"
}

type trayApp struct {
	conn   *grpc.ClientConn
	client daemonv1.DaemonClient
	quit   chan struct{}
}

func (a *trayApp) onReady() {
	systray.SetTitle("🐧")
	systray.SetTooltip("Penguin endpoint agent")
	a.quit = make(chan struct{})

	refresh := systray.AddMenuItem("Refresh", "Rebuild the menu now")
	systray.AddSeparator()
	// A minimal static menu; a full build would rebuild items per Snapshot.
	// The point of the separate package is that rendering logic is testable.
	quit := systray.AddMenuItem("Quit", "Exit the tray")

	go a.watch()

	for {
		select {
		case <-refresh.ClickedCh:
			a.render()
		case <-quit.ClickedCh:
			systray.Quit()
			return
		case <-a.quit:
			return
		}
	}
}

func (a *trayApp) onExit() {
	if a.quit != nil {
		close(a.quit)
	}
	_ = a.conn.Close()
}

// render pulls a fresh Snapshot and updates the tooltip/title. (Rebuilding the
// full dynamic menu tree is left to a follow-up; the model is already built
// and tested in internal/tray.)
func (a *trayApp) render() {
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	m, err := tray.Snapshot(ctx, a.client, time.Now())
	if err != nil {
		systray.SetTooltip("Penguin: daemon unreachable")
		return
	}
	systray.SetTooltip(fmt.Sprintf("Penguin: %d module(s), %s", len(m.Modules), m.Overall))
}

// watch subscribes to daemon events and re-renders on each one, falling back
// to a periodic poll if the stream drops.
func (a *trayApp) watch() {
	a.render()
	ctx := context.Background()
	stream, err := a.client.WatchEvents(ctx, &daemonv1.WatchEventsRequest{ApiVersion: "v1"})
	if err != nil {
		a.poll()
		return
	}
	for {
		if _, err := stream.Recv(); err != nil {
			a.poll()
			return
		}
		a.render()
	}
}

// poll re-renders every 15s when the event stream is unavailable.
func (a *trayApp) poll() {
	t := time.NewTicker(15 * time.Second)
	defer t.Stop()
	for {
		select {
		case <-a.quit:
			return
		case <-t.C:
			a.render()
		}
	}
}
