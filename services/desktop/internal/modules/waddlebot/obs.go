package waddlebot

import (
	"context"
	"fmt"
	"sync"

	"github.com/sirupsen/logrus"
)

// OBSClient manages OBS Studio connection state for the bridge module.
// This is a lightweight stub; full obs-websocket v5 integration is handled
// separately when the goobs dependency is available.
type OBSClient struct {
	config BridgeConfig
	logger *logrus.Logger
	mu     sync.RWMutex
	info   OBSConnectionInfo
}

// SceneInfo is a trimmed scene descriptor for CLI output.
type SceneInfo struct {
	Name      string
	IsCurrent bool
}

// NewOBSClient creates an OBSClient for the given configuration.
func NewOBSClient(cfg BridgeConfig, logger *logrus.Logger) *OBSClient {
	return &OBSClient{
		config: cfg,
		logger: logger,
		info: OBSConnectionInfo{
			State: "disconnected",
		},
	}
}

// Connect attempts to establish the OBS WebSocket connection.
// In the current stub implementation this updates internal state only.
func (o *OBSClient) Connect(_ context.Context) error {
	o.mu.Lock()
	defer o.mu.Unlock()

	if o.config.OBSHost == "" {
		return fmt.Errorf("OBS host not configured")
	}
	// Full connection via obs-websocket would go here.
	o.info.State = "connected"
	o.logger.WithFields(map[string]interface{}{
		"host": o.config.OBSHost,
		"port": o.config.OBSPort,
	}).Info("OBS connection stub active")
	return nil
}

// Disconnect closes the OBS WebSocket connection.
func (o *OBSClient) Disconnect() {
	o.mu.Lock()
	defer o.mu.Unlock()
	o.info.State = "disconnected"
	o.info.CurrentScene = ""
	o.info.Streaming = false
	o.info.Recording = false
	o.logger.Info("OBS disconnected")
}

// IsConnected returns true when the OBS connection is active.
func (o *OBSClient) IsConnected() bool {
	o.mu.RLock()
	defer o.mu.RUnlock()
	return o.info.State == "connected"
}

// GetConnectionInfo returns a snapshot of the OBS connection state.
func (o *OBSClient) GetConnectionInfo() OBSConnectionInfo {
	o.mu.RLock()
	defer o.mu.RUnlock()
	return o.info
}

// GetScenes returns the list of available OBS scenes.
// Returns a stub list when not connected to a live OBS instance.
func (o *OBSClient) GetScenes(_ context.Context) ([]SceneInfo, error) {
	o.mu.RLock()
	defer o.mu.RUnlock()
	if o.info.State != "connected" {
		return nil, fmt.Errorf("not connected to OBS")
	}
	// Placeholder: real implementation queries obs-websocket GetSceneList.
	return []SceneInfo{
		{Name: o.info.CurrentScene, IsCurrent: true},
	}, nil
}

// SwitchScene sets the active OBS scene.
func (o *OBSClient) SwitchScene(_ context.Context, sceneName string) error {
	o.mu.Lock()
	defer o.mu.Unlock()
	if o.info.State != "connected" {
		return fmt.Errorf("not connected to OBS")
	}
	o.info.CurrentScene = sceneName
	o.logger.WithField("scene", sceneName).Info("OBS scene switched")
	return nil
}
