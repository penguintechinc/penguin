// Package tray holds the transport-agnostic logic behind penguin-tray: it
// talks to the daemon over the same gRPC IPC as the CLI and turns the results
// into a menu model. Keeping this separate from the systray/cgo code makes it
// unit-testable without a display.
package tray

import (
	"context"
	"fmt"
	"sort"
	"time"

	daemonv1 "github.com/penguintechinc/penguin/api/proto/penguin/daemon/v1"
	"google.golang.org/grpc"
)

// DaemonClient is the subset of the daemon API the tray uses. It is an
// interface so tests can supply a fake without a real socket.
type DaemonClient interface {
	ListModules(ctx context.Context, in *daemonv1.ListModulesRequest, opts ...grpc.CallOption) (*daemonv1.ListModulesResponse, error)
	GetStatus(ctx context.Context, in *daemonv1.GetStatusRequest, opts ...grpc.CallOption) (*daemonv1.GetStatusResponse, error)
	ListCommands(ctx context.Context, in *daemonv1.ListCommandsRequest, opts ...grpc.CallOption) (*daemonv1.ListCommandsResponse, error)
	LoadModule(ctx context.Context, in *daemonv1.LoadModuleRequest, opts ...grpc.CallOption) (*daemonv1.LoadModuleResponse, error)
	UnloadModule(ctx context.Context, in *daemonv1.UnloadModuleRequest, opts ...grpc.CallOption) (*daemonv1.UnloadModuleResponse, error)
	Dispatch(ctx context.Context, in *daemonv1.DispatchRequest, opts ...grpc.CallOption) (daemonv1.Daemon_DispatchClient, error)
	WatchEvents(ctx context.Context, in *daemonv1.WatchEventsRequest, opts ...grpc.CallOption) (daemonv1.Daemon_WatchEventsClient, error)
}

const apiVersion = "v1"

// Health classifies a module's health for the tray icon/label.
type Health string

const (
	HealthUnknown Health = "unknown"
	HealthOK      Health = "healthy"
	HealthWarn    Health = "degraded"
	HealthBad     Health = "unhealthy"
)

// TrayAction is a module command surfaced as a clickable menu item
// (CommandSpec.Tray == true). Invoking it dispatches path to the module.
type TrayAction struct {
	Label string
	Path  []string
}

// ModuleItem is one module's row in the tray menu.
type ModuleItem struct {
	Name    string
	Loaded  bool
	State   string
	Health  Health
	Detail  string
	Actions []TrayAction
}

// Model is the full menu snapshot rendered by the tray.
type Model struct {
	Modules   []ModuleItem
	Overall   Health
	UpdatedAt time.Time
}

// Snapshot builds a Model from the daemon's current state. now is injected so
// the caller (and tests) control timestamps.
func Snapshot(ctx context.Context, c DaemonClient, now time.Time) (Model, error) {
	mods, err := c.ListModules(ctx, &daemonv1.ListModulesRequest{ApiVersion: apiVersion})
	if err != nil {
		return Model{}, fmt.Errorf("list modules: %w", err)
	}
	st, err := c.GetStatus(ctx, &daemonv1.GetStatusRequest{ApiVersion: apiVersion})
	if err != nil {
		return Model{}, fmt.Errorf("get status: %w", err)
	}
	cmds, err := c.ListCommands(ctx, &daemonv1.ListCommandsRequest{ApiVersion: apiVersion})
	if err != nil {
		return Model{}, fmt.Errorf("list commands: %w", err)
	}

	statusByModule := make(map[string]*daemonv1.ModuleStatus, len(st.Modules))
	for _, ms := range st.Modules {
		statusByModule[ms.Name] = ms
	}
	actionsByModule := trayActions(cmds)

	m := Model{UpdatedAt: now, Overall: HealthOK}
	for _, mod := range mods.Modules {
		item := ModuleItem{
			Name:    mod.Name,
			Loaded:  mod.State != "disabled",
			State:   mod.State,
			Health:  HealthUnknown,
			Actions: actionsByModule[mod.Name],
		}
		if ms, ok := statusByModule[mod.Name]; ok {
			item.Health = parseHealth(ms.Health)
			item.Detail = ms.HealthMessage
		}
		m.Modules = append(m.Modules, item)
	}
	sort.Slice(m.Modules, func(i, j int) bool { return m.Modules[i].Name < m.Modules[j].Name })
	m.Overall = worstHealth(m.Modules)
	return m, nil
}

// trayActions flattens each module's command tree to the leaves flagged
// Tray:true, recording the full command path for dispatch.
func trayActions(cmds *daemonv1.ListCommandsResponse) map[string][]TrayAction {
	out := make(map[string][]TrayAction)
	var walk func(module string, prefix []string, specs []*daemonv1.CommandSpec)
	walk = func(module string, prefix []string, specs []*daemonv1.CommandSpec) {
		for _, s := range specs {
			path := append(append([]string{}, prefix...), s.Name)
			if s.Tray {
				out[module] = append(out[module], TrayAction{Label: s.Short, Path: path})
			}
			walk(module, path, s.Subcommands)
		}
	}
	for _, mc := range cmds.Modules {
		walk(mc.Module, nil, mc.Commands)
	}
	return out
}

func parseHealth(s string) Health {
	switch s {
	case "healthy":
		return HealthOK
	case "degraded":
		return HealthWarn
	case "unhealthy":
		return HealthBad
	default:
		return HealthUnknown
	}
}

// worstHealth reduces module healths to a single tray indicator: any
// unhealthy loaded module makes the whole agent unhealthy, and so on.
func worstHealth(items []ModuleItem) Health {
	worst := HealthOK
	rank := map[Health]int{HealthOK: 0, HealthUnknown: 1, HealthWarn: 2, HealthBad: 3}
	for _, it := range items {
		if !it.Loaded {
			continue
		}
		if rank[it.Health] > rank[worst] {
			worst = it.Health
		}
	}
	return worst
}
