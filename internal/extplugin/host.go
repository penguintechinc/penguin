package extplugin

import (
	"context"
	"fmt"
	"time"

	sdkv1 "github.com/penguintechinc/penguin/pkg/sdk/proto/penguin/sdk/v1"
	"github.com/penguintechinc/penguin/pkg/sdk"
)

// HostServiceImpl implements sdkv1.HostServiceServer by proxying to a real HostServices.
type HostServiceImpl struct {
	sdkv1.UnimplementedHostServiceServer
	host sdk.HostServices
}

// NewHostServiceImpl creates a new HostService gRPC server wrapping the given HostServices.
func NewHostServiceImpl(host sdk.HostServices) *HostServiceImpl {
	return &HostServiceImpl{host: host}
}

func (h *HostServiceImpl) Log(ctx context.Context, req *sdkv1.LogRequest) (*sdkv1.LogResponse, error) {
	if req.ApiVersion != "v1" {
		return nil, fmt.Errorf("unsupported api_version: %s", req.ApiVersion)
	}

	// For now, log via the host logger directly (modules can't use structured logging
	// well over gRPC). A more sophisticated implementation would provide a full bridge.
	logger := h.host.Logger()
	if logger != nil {
		switch req.Level {
		case "debug":
			logger.Debug(req.Message)
		case "info":
			logger.Info(req.Message)
		case "warn":
			logger.Warn(req.Message)
		case "error":
			logger.Error(req.Message)
		}
	}

	return &sdkv1.LogResponse{}, nil
}

func (h *HostServiceImpl) SecretsGet(ctx context.Context, req *sdkv1.SecretsGetRequest) (*sdkv1.SecretsGetResponse, error) {
	if req.ApiVersion != "v1" {
		return nil, fmt.Errorf("unsupported api_version: %s", req.ApiVersion)
	}

	secretStore := h.host.Secrets()
	value, err := secretStore.Get(req.Key)
	if err != nil {
		errMsg := err.Error()
		if err == sdk.ErrSecretNotFound {
			errMsg = "not found"
		}
		return &sdkv1.SecretsGetResponse{Error: errMsg}, nil
	}

	return &sdkv1.SecretsGetResponse{Value: value}, nil
}

func (h *HostServiceImpl) SecretsSet(ctx context.Context, req *sdkv1.SecretsSetRequest) (*sdkv1.SecretsSetResponse, error) {
	if req.ApiVersion != "v1" {
		return nil, fmt.Errorf("unsupported api_version: %s", req.ApiVersion)
	}

	secretStore := h.host.Secrets()
	err := secretStore.Set(req.Key, req.Value)
	errMsg := ""
	if err != nil {
		errMsg = err.Error()
	}

	return &sdkv1.SecretsSetResponse{Error: errMsg}, nil
}

func (h *HostServiceImpl) SecretsDelete(ctx context.Context, req *sdkv1.SecretsDeleteRequest) (*sdkv1.SecretsDeleteResponse, error) {
	if req.ApiVersion != "v1" {
		return nil, fmt.Errorf("unsupported api_version: %s", req.ApiVersion)
	}

	secretStore := h.host.Secrets()
	err := secretStore.Delete(req.Key)
	errMsg := ""
	if err != nil {
		errMsg = err.Error()
	}

	return &sdkv1.SecretsDeleteResponse{Error: errMsg}, nil
}

func (h *HostServiceImpl) LicenseFeatureEnabled(ctx context.Context, req *sdkv1.LicenseFeatureEnabledRequest) (*sdkv1.LicenseFeatureEnabledResponse, error) {
	if req.ApiVersion != "v1" {
		return nil, fmt.Errorf("unsupported api_version: %s", req.ApiVersion)
	}

	license := h.host.License()
	enabled := license.FeatureEnabled(req.Key)

	return &sdkv1.LicenseFeatureEnabledResponse{Enabled: enabled}, nil
}

func (h *HostServiceImpl) LicenseTier(ctx context.Context, req *sdkv1.LicenseTierRequest) (*sdkv1.LicenseTierResponse, error) {
	if req.ApiVersion != "v1" {
		return nil, fmt.Errorf("unsupported api_version: %s", req.ApiVersion)
	}

	license := h.host.License()
	tier := license.Tier()

	return &sdkv1.LicenseTierResponse{Tier: tier}, nil
}

func (h *HostServiceImpl) DataDir(ctx context.Context, req *sdkv1.DataDirRequest) (*sdkv1.DataDirResponse, error) {
	if req.ApiVersion != "v1" {
		return nil, fmt.Errorf("unsupported api_version: %s", req.ApiVersion)
	}

	path := h.host.DataDir()
	return &sdkv1.DataDirResponse{Path: path}, nil
}

func (h *HostServiceImpl) Config(ctx context.Context, req *sdkv1.ConfigRequest) (*sdkv1.ConfigResponse, error) {
	if req.ApiVersion != "v1" {
		return nil, fmt.Errorf("unsupported api_version: %s", req.ApiVersion)
	}

	config := h.host.Config()
	return &sdkv1.ConfigResponse{Config: config}, nil
}

func (h *HostServiceImpl) PublishEvent(ctx context.Context, req *sdkv1.PublishEventRequest) (*sdkv1.PublishEventResponse, error) {
	if req.ApiVersion != "v1" {
		return nil, fmt.Errorf("unsupported api_version: %s", req.ApiVersion)
	}

	eventSink := h.host.Events()
	ev := sdk.Event{
		Module:  req.Module,
		Type:    sdk.EventType(req.Type),
		Message: req.Message,
		At:      time.Unix(0, req.AtUnixNano),
		Fields:  req.Fields,
	}
	eventSink.Publish(ev)

	return &sdkv1.PublishEventResponse{}, nil
}
