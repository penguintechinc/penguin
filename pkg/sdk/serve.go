package sdk

import (
	"context"
	"fmt"
	"math"

	v1 "github.com/penguintechinc/penguin/pkg/sdk/proto/penguin/sdk/v1"
	"github.com/prometheus/client_golang/prometheus"
	"go.uber.org/zap"
)

// clampInt32 narrows an int to int32, saturating rather than wrapping.
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

// ModuleServiceImpl implements v1.ModuleServiceServer by wrapping the author's Module.
type ModuleServiceImpl struct {
	v1.UnimplementedModuleServiceServer
	m Module
}

func (s *ModuleServiceImpl) Info(ctx context.Context, req *v1.InfoRequest) (*v1.InfoResponse, error) {
	if req.ApiVersion != "v1" {
		return nil, fmt.Errorf("unsupported api_version: %s", req.ApiVersion)
	}
	info := s.m.Info()
	return &v1.InfoResponse{
		Name:           info.Name,
		Version:        info.Version,
		Description:    info.Description,
		LicenseFeature: info.LicenseFeature,
	}, nil
}

func (s *ModuleServiceImpl) Init(ctx context.Context, req *v1.InitRequest) (*v1.InitResponse, error) {
	if req.ApiVersion != "v1" {
		return nil, fmt.Errorf("unsupported api_version: %s", req.ApiVersion)
	}
	return &v1.InitResponse{}, nil
}

func (s *ModuleServiceImpl) Start(ctx context.Context, req *v1.StartRequest) (*v1.StartResponse, error) {
	if req.ApiVersion != "v1" {
		return nil, fmt.Errorf("unsupported api_version: %s", req.ApiVersion)
	}
	err := s.m.Start(ctx)
	errMsg := ""
	if err != nil {
		errMsg = err.Error()
	}
	return &v1.StartResponse{Error: errMsg}, nil
}

func (s *ModuleServiceImpl) Stop(ctx context.Context, req *v1.StopRequest) (*v1.StopResponse, error) {
	if req.ApiVersion != "v1" {
		return nil, fmt.Errorf("unsupported api_version: %s", req.ApiVersion)
	}
	err := s.m.Stop(ctx)
	errMsg := ""
	if err != nil {
		errMsg = err.Error()
	}
	return &v1.StopResponse{Error: errMsg}, nil
}

func (s *ModuleServiceImpl) Status(ctx context.Context, req *v1.StatusRequest) (*v1.StatusResponse, error) {
	if req.ApiVersion != "v1" {
		return nil, fmt.Errorf("unsupported api_version: %s", req.ApiVersion)
	}
	status, err := s.m.Status(ctx)
	errMsg := ""
	if err != nil {
		errMsg = err.Error()
	}
	return &v1.StatusResponse{
		State:  string(status.State),
		Detail: status.Detail,
		Error:  errMsg,
	}, nil
}

func (s *ModuleServiceImpl) Health(ctx context.Context, req *v1.HealthRequest) (*v1.HealthResponse, error) {
	if req.ApiVersion != "v1" {
		return nil, fmt.Errorf("unsupported api_version: %s", req.ApiVersion)
	}
	report := s.m.Health(ctx)
	return &v1.HealthResponse{
		Level:             clampInt32(int(report.Level)),
		Message:           report.Message,
		CheckedAtUnixNano: report.CheckedAt.UnixNano(),
	}, nil
}

func (s *ModuleServiceImpl) Commands(ctx context.Context, req *v1.CommandsRequest) (*v1.CommandsResponse, error) {
	if req.ApiVersion != "v1" {
		return nil, fmt.Errorf("unsupported api_version: %s", req.ApiVersion)
	}
	specs := s.m.Commands()
	pbSpecs := make([]*v1.CommandSpec, len(specs))
	for i, spec := range specs {
		pbSpecs[i] = commandSpecToProto(spec)
	}
	return &v1.CommandsResponse{Commands: pbSpecs}, nil
}

func (s *ModuleServiceImpl) Dispatch(ctx context.Context, req *v1.DispatchRequest) (*v1.DispatchResponse, error) {
	if req.ApiVersion != "v1" {
		return nil, fmt.Errorf("unsupported api_version: %s", req.ApiVersion)
	}
	result, err := s.m.Dispatch(ctx, req.Path, req.Flags, req.Args)
	errMsg := ""
	if err != nil {
		errMsg = err.Error()
	}
	var output, json []byte
	exitCode := int32(0)
	if result != nil {
		output = []byte(result.Output)
		json = result.JSON
		exitCode = clampInt32(result.ExitCode)
	}
	return &v1.DispatchResponse{
		Output:   string(output),
		Json:     json,
		ExitCode: exitCode,
		Error:    errMsg,
	}, nil
}

func (s *ModuleServiceImpl) ConfigSchema(ctx context.Context, req *v1.ConfigSchemaRequest) (*v1.ConfigSchemaResponse, error) {
	if req.ApiVersion != "v1" {
		return nil, fmt.Errorf("unsupported api_version: %s", req.ApiVersion)
	}
	return &v1.ConfigSchemaResponse{Schema: s.m.ConfigSchema()}, nil
}

// Helper to convert CommandSpec to proto.
func commandSpecToProto(cs CommandSpec) *v1.CommandSpec {
	flags := make([]*v1.FlagSpec, len(cs.Flags))
	for i, f := range cs.Flags {
		flags[i] = &v1.FlagSpec{
			Name:      f.Name,
			Shorthand: f.Shorthand,
			Usage:     f.Usage,
			Default:   f.Default,
			Type:      string(f.Type),
		}
	}

	subcmds := make([]*v1.CommandSpec, len(cs.Subcommands))
	for i, sc := range cs.Subcommands {
		subcmds[i] = commandSpecToProto(sc)
	}

	return &v1.CommandSpec{
		Name:        cs.Name,
		Use:         cs.Use,
		Short:       cs.Short,
		Flags:       flags,
		Subcommands: subcmds,
		Tray:        cs.Tray,
		MinArgs:     clampInt32(cs.MinArgs),
		MaxArgs:     clampInt32(cs.MaxArgs),
	}
}

// HostServicesProxy implements sdk.HostServices by proxying calls back to the daemon over gRPC.
type HostServicesProxy struct {
	hostClient v1.HostServiceClient
}

func (h *HostServicesProxy) Logger() *zap.Logger {
	return zap.NewNop()
}

func (h *HostServicesProxy) Secrets() SecretStore {
	return &SecretsProxy{hostClient: h.hostClient}
}

func (h *HostServicesProxy) License() LicenseChecker {
	return &LicenseProxy{hostClient: h.hostClient}
}

func (h *HostServicesProxy) Metrics() prometheus.Registerer {
	return prometheus.DefaultRegisterer
}

func (h *HostServicesProxy) Config() []byte {
	resp, err := h.hostClient.Config(context.Background(), &v1.ConfigRequest{ApiVersion: "v1"})
	if err != nil {
		return nil
	}
	return resp.Config
}

func (h *HostServicesProxy) DataDir() string {
	resp, err := h.hostClient.DataDir(context.Background(), &v1.DataDirRequest{ApiVersion: "v1"})
	if err != nil {
		return ""
	}
	return resp.Path
}

func (h *HostServicesProxy) Events() EventSink {
	return &EventsProxy{hostClient: h.hostClient}
}

// SecretsProxy implements SecretStore over gRPC.
type SecretsProxy struct {
	hostClient v1.HostServiceClient
}

func (s *SecretsProxy) Get(key string) ([]byte, error) {
	resp, err := s.hostClient.SecretsGet(context.Background(), &v1.SecretsGetRequest{
		ApiVersion: "v1",
		Key:        key,
	})
	if err != nil {
		return nil, err
	}
	if resp.Error != "" {
		if resp.Error == "not found" {
			return nil, ErrSecretNotFound
		}
		return nil, fmt.Errorf("secrets get: %s", resp.Error)
	}
	return resp.Value, nil
}

func (s *SecretsProxy) Set(key string, value []byte) error {
	resp, err := s.hostClient.SecretsSet(context.Background(), &v1.SecretsSetRequest{
		ApiVersion: "v1",
		Key:        key,
		Value:      value,
	})
	if err != nil {
		return err
	}
	if resp.Error != "" {
		return fmt.Errorf("secrets set: %s", resp.Error)
	}
	return nil
}

func (s *SecretsProxy) Delete(key string) error {
	resp, err := s.hostClient.SecretsDelete(context.Background(), &v1.SecretsDeleteRequest{
		ApiVersion: "v1",
		Key:        key,
	})
	if err != nil {
		return err
	}
	if resp.Error != "" {
		return fmt.Errorf("secrets delete: %s", resp.Error)
	}
	return nil
}

// LicenseProxy implements LicenseChecker over gRPC.
type LicenseProxy struct {
	hostClient v1.HostServiceClient
}

func (l *LicenseProxy) FeatureEnabled(key string) bool {
	resp, err := l.hostClient.LicenseFeatureEnabled(context.Background(), &v1.LicenseFeatureEnabledRequest{
		ApiVersion: "v1",
		Key:        key,
	})
	if err != nil {
		return false
	}
	return resp.Enabled
}

func (l *LicenseProxy) Tier() string {
	resp, err := l.hostClient.LicenseTier(context.Background(), &v1.LicenseTierRequest{
		ApiVersion: "v1",
	})
	if err != nil {
		return ""
	}
	return resp.Tier
}

// EventsProxy implements EventSink over gRPC.
type EventsProxy struct {
	hostClient v1.HostServiceClient
}

func (e *EventsProxy) Publish(ev Event) {
	_, _ = e.hostClient.PublishEvent(context.Background(), &v1.PublishEventRequest{
		ApiVersion: "v1",
		Module:     ev.Module,
		Type:       string(ev.Type),
		Message:    ev.Message,
		AtUnixNano: ev.At.UnixNano(),
		Fields:     ev.Fields,
	})
}
