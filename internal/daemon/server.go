package daemon

import (
	"context"
	"errors"
	"math"
	"sync"

	daemonv1 "github.com/penguintechinc/penguin/api/proto/penguin/daemon/v1"
	"github.com/penguintechinc/penguin/pkg/sdk"
	"go.uber.org/zap"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
)

// Client is an interface for update operations (can be nil for testing).
type Client interface {
	CheckUpdate(ctx context.Context) (available bool, latest string, err error)
	ApplyUpdate(ctx context.Context) error
}

// Server implements daemonv1.DaemonServer.
type Server struct {
	daemonv1.UnimplementedDaemonServer

	supervisor   *Supervisor
	version      string
	logger       *zap.Logger
	eventBroker  *EventBroker
	updateClient Client // can be nil for testing
}

// EventBroker manages event subscriptions for WatchEvents and implements sdk.EventSink.
type EventBroker struct {
	mu          sync.RWMutex
	subscribers map[uint64]chan *daemonv1.Event // subscriber ID -> channel
	nextID      uint64
}

// NewEventBroker creates a new event broker.
func NewEventBroker() *EventBroker {
	return &EventBroker{
		subscribers: make(map[uint64]chan *daemonv1.Event),
		nextID:      1,
	}
}

// Subscribe creates a new subscription channel for daemon RPCs.
func (eb *EventBroker) Subscribe() (uint64, <-chan *daemonv1.Event) {
	eb.mu.Lock()
	defer eb.mu.Unlock()

	id := eb.nextID
	eb.nextID++
	ch := make(chan *daemonv1.Event, 10) // Buffered to avoid blocking publishers
	eb.subscribers[id] = ch
	return id, ch
}

// Unsubscribe removes a subscription.
func (eb *EventBroker) Unsubscribe(id uint64) {
	eb.mu.Lock()
	defer eb.mu.Unlock()

	if ch, ok := eb.subscribers[id]; ok {
		close(ch)
		delete(eb.subscribers, id)
	}
}

// Publish sends an sdk.Event to all subscribers (implements sdk.EventSink).
func (eb *EventBroker) Publish(ev sdk.Event) {
	eb.mu.RLock()
	defer eb.mu.RUnlock()

	protoEvent := &daemonv1.Event{
		Module:     ev.Module,
		Type:       string(ev.Type),
		Message:    ev.Message,
		AtUnixNano: ev.At.UnixNano(),
		Fields:     ev.Fields,
	}

	for _, ch := range eb.subscribers {
		select {
		case ch <- protoEvent:
		default:
			// Drop slow subscribers to avoid blocking
		}
	}
}

// NewServer creates a new daemon server.
func NewServer(supervisor *Supervisor, version string, logger *zap.Logger, updateClient Client) *Server {
	return &Server{
		supervisor:   supervisor,
		version:      version,
		logger:       logger,
		eventBroker:  NewEventBroker(),
		updateClient: updateClient,
	}
}

// checkAPIVersion validates the api_version field.
func (s *Server) checkAPIVersion(version string) error {
	if version == "" || version == "v1" {
		return nil
	}
	return status.Errorf(codes.Unimplemented, "api_version %q not supported", version)
}

// Version returns the daemon version.
func (s *Server) Version(ctx context.Context, req *daemonv1.VersionRequest) (*daemonv1.VersionResponse, error) {
	if err := s.checkAPIVersion(req.ApiVersion); err != nil {
		return nil, err
	}

	return &daemonv1.VersionResponse{
		DaemonVersion: s.version,
		ApiVersion:    "v1",
	}, nil
}

// ListModules returns all modules and their states.
func (s *Server) ListModules(ctx context.Context, req *daemonv1.ListModulesRequest) (*daemonv1.ListModulesResponse, error) {
	if err := s.checkAPIVersion(req.ApiVersion); err != nil {
		return nil, err
	}

	snapshots := s.supervisor.List()
	modules := make([]*daemonv1.ModuleSummary, 0, len(snapshots))

	for _, snap := range snapshots {
		// Use ModuleInfo, not Module: unloaded modules must still be listed so
		// operators can discover what is available to load.
		info, ok := s.supervisor.ModuleInfo(snap.Name)
		if !ok {
			continue
		}

		modules = append(modules, &daemonv1.ModuleSummary{
			Name:           snap.Name,
			Version:        info.Version,
			Description:    info.Description,
			State:          string(snap.State),
			External:       false, // external plugins land in M7
			LicenseFeature: info.LicenseFeature,
		})
	}

	return &daemonv1.ListModulesResponse{Modules: modules}, nil
}

// LoadModule loads (enables) a module.
func (s *Server) LoadModule(ctx context.Context, req *daemonv1.LoadModuleRequest) (*daemonv1.LoadModuleResponse, error) {
	if err := s.checkAPIVersion(req.ApiVersion); err != nil {
		return nil, err
	}

	if req.Name == "" {
		return nil, status.Error(codes.InvalidArgument, "module name required")
	}

	if err := s.supervisor.Load(ctx, req.Name); err != nil {
		// Supervisor wraps its sentinels, so match with errors.Is.
		if errors.Is(err, ErrUnknownModule) {
			return nil, status.Errorf(codes.NotFound, "module %q not found", req.Name)
		}
		// Remaining Load failures are license/entitlement denials.
		return nil, status.Errorf(codes.PermissionDenied, "cannot load module: %v", err)
	}

	modStatus, err := s.supervisor.Status(ctx, req.Name)
	if err != nil {
		return nil, status.Errorf(codes.Internal, "failed to get status: %v", err)
	}

	return &daemonv1.LoadModuleResponse{State: string(modStatus.State)}, nil
}

// UnloadModule unloads (disables) a module.
func (s *Server) UnloadModule(ctx context.Context, req *daemonv1.UnloadModuleRequest) (*daemonv1.UnloadModuleResponse, error) {
	if err := s.checkAPIVersion(req.ApiVersion); err != nil {
		return nil, err
	}

	if req.Name == "" {
		return nil, status.Error(codes.InvalidArgument, "module name required")
	}

	if err := s.supervisor.Unload(ctx, req.Name); err != nil && !errors.Is(err, ErrUnknownModule) {
		return nil, status.Errorf(codes.Internal, "failed to unload: %v", err)
	}

	return &daemonv1.UnloadModuleResponse{State: string(sdk.StateStopped)}, nil
}

// GetStatus returns status for one or all modules.
func (s *Server) GetStatus(ctx context.Context, req *daemonv1.GetStatusRequest) (*daemonv1.GetStatusResponse, error) {
	if err := s.checkAPIVersion(req.ApiVersion); err != nil {
		return nil, err
	}

	resp := &daemonv1.GetStatusResponse{
		DaemonVersion: s.version,
		Modules:       []*daemonv1.ModuleStatus{},
	}

	if req.Name == "" {
		// All modules
		snapshots := s.supervisor.List()
		for _, snap := range snapshots {
			modStatus, err := s.supervisor.Status(ctx, snap.Name)
			if err != nil {
				continue
			}

			m, ok := s.supervisor.Module(snap.Name)
			if !ok {
				continue
			}
			health := m.Health(ctx)

			resp.Modules = append(resp.Modules, &daemonv1.ModuleStatus{
				Name:              snap.Name,
				State:             string(modStatus.State),
				Detail:            modStatus.Detail,
				Health:            health.Level.String(),
				HealthMessage:     health.Message,
				CheckedAtUnixNano: health.CheckedAt.UnixNano(),
			})
		}
	} else {
		// Single module
		modStatus, err := s.supervisor.Status(ctx, req.Name)
		if err != nil {
			return nil, status.Errorf(codes.NotFound, "module %q not found", req.Name)
		}

		m, ok := s.supervisor.Module(req.Name)
		if !ok {
			return nil, status.Errorf(codes.NotFound, "module %q not found", req.Name)
		}
		health := m.Health(ctx)

		resp.Modules = append(resp.Modules, &daemonv1.ModuleStatus{
			Name:              req.Name,
			State:             string(modStatus.State),
			Detail:            modStatus.Detail,
			Health:            health.Level.String(),
			HealthMessage:     health.Message,
			CheckedAtUnixNano: health.CheckedAt.UnixNano(),
		})
	}

	return resp, nil
}

// ListCommands returns all commands from all modules.
func (s *Server) ListCommands(ctx context.Context, req *daemonv1.ListCommandsRequest) (*daemonv1.ListCommandsResponse, error) {
	if err := s.checkAPIVersion(req.ApiVersion); err != nil {
		return nil, err
	}

	resp := &daemonv1.ListCommandsResponse{Modules: []*daemonv1.ModuleCommands{}}

	snapshots := s.supervisor.List()
	for _, snap := range snapshots {
		m, ok := s.supervisor.Module(snap.Name)
		if !ok {
			continue
		}

		commands := m.Commands()
		moduleCommands := &daemonv1.ModuleCommands{
			Module:   snap.Name,
			Commands: []*daemonv1.CommandSpec{},
		}

		for _, cmd := range commands {
			moduleCommands.Commands = append(moduleCommands.Commands, sdkCommandToProto(cmd))
		}

		resp.Modules = append(resp.Modules, moduleCommands)
	}

	return resp, nil
}

// sdkCommandToProto converts an sdk.CommandSpec to a proto CommandSpec recursively.
func sdkCommandToProto(cmd sdk.CommandSpec) *daemonv1.CommandSpec {
	flags := make([]*daemonv1.FlagSpec, 0, len(cmd.Flags))
	for _, f := range cmd.Flags {
		flags = append(flags, &daemonv1.FlagSpec{
			Name:      f.Name,
			Shorthand: f.Shorthand,
			Usage:     f.Usage,
			Default:   f.Default,
			Type:      string(f.Type),
		})
	}

	subcommands := make([]*daemonv1.CommandSpec, 0, len(cmd.Subcommands))
	for _, sc := range cmd.Subcommands {
		subcommands = append(subcommands, sdkCommandToProto(sc))
	}

	return &daemonv1.CommandSpec{
		Name:        cmd.Name,
		Use:         cmd.Use,
		Short:       cmd.Short,
		Flags:       flags,
		Subcommands: subcommands,
		Tray:        cmd.Tray,
		MinArgs:     clampInt32(cmd.MinArgs),
		MaxArgs:     clampInt32(cmd.MaxArgs),
	}
}

// clampInt32 narrows an int to int32, saturating rather than wrapping.
// Arg counts and exit codes are small; this only guards pathological values.
func clampInt32(v int) int32 {
	switch {
	case v > math.MaxInt32:
		return math.MaxInt32
	case v < math.MinInt32:
		return math.MinInt32
	default:
		return int32(v)
	}
}

// Dispatch executes a command and streams the result.
func (s *Server) Dispatch(req *daemonv1.DispatchRequest, stream daemonv1.Daemon_DispatchServer) error {
	ctx := stream.Context()

	if err := s.checkAPIVersion(req.ApiVersion); err != nil {
		return err
	}

	if req.Module == "" {
		return status.Error(codes.InvalidArgument, "module name required")
	}

	// Get module
	m, ok := s.supervisor.Module(req.Module)
	if !ok {
		return status.Errorf(codes.NotFound, "module %q not found", req.Module)
	}

	// Execute dispatch
	result, err := m.Dispatch(ctx, req.Path, req.Flags, req.Args)
	if err != nil {
		return status.Errorf(codes.Internal, "dispatch failed: %v", err)
	}

	// Stream result as final chunk
	chunk := &daemonv1.DispatchChunk{
		Output:   result.Output,
		Json:     result.JSON,
		ExitCode: clampInt32(result.ExitCode),
		Final:    true,
	}

	if err := stream.Send(chunk); err != nil {
		return err
	}

	return nil
}

// WatchEvents streams events from modules.
func (s *Server) WatchEvents(req *daemonv1.WatchEventsRequest, stream daemonv1.Daemon_WatchEventsServer) error {
	ctx := stream.Context()

	if err := s.checkAPIVersion(req.ApiVersion); err != nil {
		return err
	}

	// Subscribe to events
	subID, eventChan := s.eventBroker.Subscribe()
	defer s.eventBroker.Unsubscribe(subID)

	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case ev, ok := <-eventChan:
			if !ok {
				return nil
			}

			// Filter by module if specified
			if req.Module != "" && ev.Module != req.Module {
				continue
			}

			if err := stream.Send(ev); err != nil {
				return err
			}
		}
	}
}

// TailLogs returns log lines (not implemented yet).
func (s *Server) TailLogs(req *daemonv1.TailLogsRequest, stream daemonv1.Daemon_TailLogsServer) error {
	if err := s.checkAPIVersion(req.ApiVersion); err != nil {
		return err
	}
	return status.Error(codes.Unimplemented, "TailLogs not implemented yet")
}

// CheckUpdate checks for available updates.
func (s *Server) CheckUpdate(ctx context.Context, req *daemonv1.CheckUpdateRequest) (*daemonv1.CheckUpdateResponse, error) {
	if err := s.checkAPIVersion(req.ApiVersion); err != nil {
		return nil, err
	}

	if s.updateClient == nil {
		return &daemonv1.CheckUpdateResponse{
			Available:      false,
			CurrentVersion: s.version,
			LatestVersion:  s.version,
		}, nil
	}

	available, latest, err := s.updateClient.CheckUpdate(ctx)
	if err != nil {
		return nil, status.Errorf(codes.Internal, "check update failed: %v", err)
	}

	return &daemonv1.CheckUpdateResponse{
		Available:      available,
		CurrentVersion: s.version,
		LatestVersion:  latest,
	}, nil
}

// ApplyUpdate applies an available update.
func (s *Server) ApplyUpdate(ctx context.Context, req *daemonv1.ApplyUpdateRequest) (*daemonv1.ApplyUpdateResponse, error) {
	if err := s.checkAPIVersion(req.ApiVersion); err != nil {
		return nil, err
	}

	if s.updateClient == nil {
		return &daemonv1.ApplyUpdateResponse{
			Applied: false,
			Message: "update client not configured",
		}, nil
	}

	err := s.updateClient.ApplyUpdate(ctx)
	if err != nil {
		return &daemonv1.ApplyUpdateResponse{
			Applied: false,
			Message: err.Error(),
		}, nil
	}

	return &daemonv1.ApplyUpdateResponse{
		Applied: true,
		Message: "update applied successfully",
	}, nil
}
