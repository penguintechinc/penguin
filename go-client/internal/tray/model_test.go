package tray

import (
	"context"
	"errors"
	"testing"
	"time"

	daemonv1 "github.com/penguintechinc/penguin/api/proto/penguin/daemon/v1"
	"google.golang.org/grpc"
)

// fakeDaemon implements DaemonClient with canned responses. Only the unary
// RPCs Snapshot uses are populated.
type fakeDaemon struct {
	DaemonClient // embed so the streaming methods we don't use satisfy the interface
	modules      *daemonv1.ListModulesResponse
	status       *daemonv1.GetStatusResponse
	commands     *daemonv1.ListCommandsResponse
	err          error
}

func (f *fakeDaemon) ListModules(ctx context.Context, in *daemonv1.ListModulesRequest, _ ...grpc.CallOption) (*daemonv1.ListModulesResponse, error) {
	return f.modules, f.err
}
func (f *fakeDaemon) GetStatus(ctx context.Context, in *daemonv1.GetStatusRequest, _ ...grpc.CallOption) (*daemonv1.GetStatusResponse, error) {
	return f.status, f.err
}
func (f *fakeDaemon) ListCommands(ctx context.Context, in *daemonv1.ListCommandsRequest, _ ...grpc.CallOption) (*daemonv1.ListCommandsResponse, error) {
	return f.commands, f.err
}

func TestSnapshotBuildsModel(t *testing.T) {
	f := &fakeDaemon{
		modules: &daemonv1.ListModulesResponse{Modules: []*daemonv1.ModuleSummary{
			{Name: "squawk", State: "running"},
			{Name: "tobogganing", State: "disabled"},
		}},
		status: &daemonv1.GetStatusResponse{Modules: []*daemonv1.ModuleStatus{
			{Name: "squawk", Health: "degraded", HealthMessage: "server down"},
		}},
		commands: &daemonv1.ListCommandsResponse{Modules: []*daemonv1.ModuleCommands{
			{Module: "squawk", Commands: []*daemonv1.CommandSpec{
				{Name: "forward", Subcommands: []*daemonv1.CommandSpec{
					{Name: "start", Short: "Start forwarding", Tray: true},
					{Name: "status", Short: "Show status"}, // not Tray
				}},
			}},
		}},
	}

	now := time.Unix(1700000000, 0)
	m, err := Snapshot(context.Background(), f, now)
	if err != nil {
		t.Fatalf("Snapshot: %v", err)
	}

	if len(m.Modules) != 2 || m.Modules[0].Name != "squawk" || m.Modules[1].Name != "tobogganing" {
		t.Fatalf("modules not sorted/complete: %+v", m.Modules)
	}
	sq := m.Modules[0]
	if !sq.Loaded || sq.Health != HealthWarn || sq.Detail != "server down" {
		t.Errorf("squawk item wrong: %+v", sq)
	}
	if m.Modules[1].Loaded {
		t.Error("disabled module should not be Loaded")
	}
	// Only the Tray:true leaf becomes an action, with its full path.
	if len(sq.Actions) != 1 {
		t.Fatalf("expected 1 tray action, got %+v", sq.Actions)
	}
	if got := sq.Actions[0].Path; len(got) != 2 || got[0] != "forward" || got[1] != "start" {
		t.Errorf("action path = %v, want [forward start]", got)
	}
	if m.Overall != HealthWarn {
		t.Errorf("overall health = %v, want degraded", m.Overall)
	}
	if !m.UpdatedAt.Equal(now) {
		t.Error("UpdatedAt not set from injected clock")
	}
}

func TestSnapshotPropagatesError(t *testing.T) {
	f := &fakeDaemon{err: errors.New("boom")}
	if _, err := Snapshot(context.Background(), f, time.Unix(0, 0)); err == nil {
		t.Fatal("expected error to propagate")
	}
}

func TestWorstHealth(t *testing.T) {
	cases := []struct {
		name  string
		items []ModuleItem
		want  Health
	}{
		{"all healthy", []ModuleItem{{Loaded: true, Health: HealthOK}}, HealthOK},
		{"one degraded", []ModuleItem{{Loaded: true, Health: HealthOK}, {Loaded: true, Health: HealthWarn}}, HealthWarn},
		{"unhealthy wins", []ModuleItem{{Loaded: true, Health: HealthWarn}, {Loaded: true, Health: HealthBad}}, HealthBad},
		{"disabled ignored", []ModuleItem{{Loaded: false, Health: HealthBad}}, HealthOK},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := worstHealth(tc.items); got != tc.want {
				t.Errorf("worstHealth = %v, want %v", got, tc.want)
			}
		})
	}
}

func TestParseHealth(t *testing.T) {
	for in, want := range map[string]Health{
		"healthy": HealthOK, "degraded": HealthWarn, "unhealthy": HealthBad, "": HealthUnknown, "weird": HealthUnknown,
	} {
		if got := parseHealth(in); got != want {
			t.Errorf("parseHealth(%q) = %v, want %v", in, got, want)
		}
	}
}
