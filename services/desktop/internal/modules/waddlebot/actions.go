package waddlebot

import (
	"context"
	"fmt"
	"strings"

	"github.com/sirupsen/logrus"
)

// ActionHandler routes incoming ActionRequests to the appropriate subsystem.
// Actions are identified by a "module.action" dotted string, e.g. "obs.switch_scene"
// or "script.run".
type ActionHandler struct {
	obs     *OBSClient
	scripts *ScriptHandler
	logger  *logrus.Logger
}

// NewActionHandler creates an ActionHandler wired to the given subsystem clients.
func NewActionHandler(obs *OBSClient, scripts *ScriptHandler, logger *logrus.Logger) *ActionHandler {
	return &ActionHandler{obs: obs, scripts: scripts, logger: logger}
}

// Execute dispatches an action to the correct subsystem and returns its result.
func (h *ActionHandler) Execute(ctx context.Context, action ActionRequest) (map[string]interface{}, error) {
	parts := strings.SplitN(action.Action, ".", 2)
	if len(parts) != 2 {
		return nil, fmt.Errorf("invalid action format %q (expected module.action)", action.Action)
	}
	module, act := parts[0], parts[1]

	h.logger.WithFields(logrus.Fields{
		"action_id": action.ID,
		"module":    module,
		"action":    act,
	}).Debug("waddlebot: dispatching action")

	switch module {
	case "obs":
		return h.executeOBS(ctx, act, action.Parameters)
	case "script":
		return h.executeScript(ctx, act, action.Parameters)
	default:
		return nil, fmt.Errorf("unknown action module %q", module)
	}
}

// executeOBS handles all obs.* actions.
func (h *ActionHandler) executeOBS(ctx context.Context, action string, params map[string]string) (map[string]interface{}, error) {
	if h.obs == nil || !h.obs.IsConnected() {
		return nil, fmt.Errorf("OBS not connected")
	}

	switch action {
	case "switch_scene":
		scene := params["scene"]
		if scene == "" {
			return nil, fmt.Errorf("obs.switch_scene: missing required parameter 'scene'")
		}
		if err := h.obs.SwitchScene(ctx, scene); err != nil {
			return nil, err
		}
		return map[string]interface{}{"scene": scene}, nil

	case "start_stream":
		return nil, fmt.Errorf("obs.start_stream: not yet supported in stub OBS client")

	case "stop_stream":
		return nil, fmt.Errorf("obs.stop_stream: not yet supported in stub OBS client")

	case "start_recording":
		return nil, fmt.Errorf("obs.start_recording: not yet supported in stub OBS client")

	case "stop_recording":
		return nil, fmt.Errorf("obs.stop_recording: not yet supported in stub OBS client")

	case "toggle_source":
		scene := params["scene"]
		source := params["source"]
		visible := params["visible"] == "true"
		if scene == "" || source == "" {
			return nil, fmt.Errorf("obs.toggle_source: missing required parameters 'scene' and/or 'source'")
		}
		_ = visible // Stub OBS client does not yet support SetSourceVisibility.
		return nil, fmt.Errorf("obs.toggle_source: not yet supported in stub OBS client")

	case "get_scenes":
		scenes, err := h.obs.GetScenes(ctx)
		if err != nil {
			return nil, err
		}
		names := make([]string, 0, len(scenes))
		for _, s := range scenes {
			names = append(names, s.Name)
		}
		return map[string]interface{}{"scenes": names}, nil

	case "get_status":
		info := h.obs.GetConnectionInfo()
		return map[string]interface{}{
			"state":         info.State,
			"obs_version":   info.OBSVersion,
			"current_scene": info.CurrentScene,
			"streaming":     info.Streaming,
			"recording":     info.Recording,
		}, nil

	default:
		return nil, fmt.Errorf("unknown OBS action %q", action)
	}
}

// executeScript handles all script.* actions.
func (h *ActionHandler) executeScript(ctx context.Context, action string, params map[string]string) (map[string]interface{}, error) {
	if h.scripts == nil {
		return nil, fmt.Errorf("scripting subsystem not configured")
	}

	switch action {
	case "run":
		lang := params["lang"]
		script := params["script"]
		if script == "" {
			return nil, fmt.Errorf("script.run: missing required parameter 'script'")
		}
		return h.scripts.Run(ctx, lang, script, params)

	default:
		return nil, fmt.Errorf("unknown script action %q", action)
	}
}
