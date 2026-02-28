package waddlebot

import (
	"context"
	"fmt"

	desktop "github.com/penguintechinc/penguin-libs/packages/penguin-desktop"
	"github.com/sirupsen/logrus"
)

// ScriptHandler wraps the shared desktop.ScriptEngine and exposes a simple
// Run method for the action dispatcher.
type ScriptHandler struct {
	engine *desktop.ScriptEngine
	logger *logrus.Logger
}

// NewScriptHandler creates a ScriptHandler backed by a new ScriptEngine.
func NewScriptHandler(logger *logrus.Logger) *ScriptHandler {
	return &ScriptHandler{
		engine: desktop.NewScriptEngine(logger),
		logger: logger,
	}
}

// Run executes a script in the requested language and returns a result map.
//
// Supported languages:
//   - "lua" or "" — embedded gopher-lua interpreter (default)
//   - "python"    — external python3 process
//   - "bash"      — external bash process
//   - "powershell" — external pwsh process
//
// For Lua scripts the _OUTPUT global table is converted to the result map.
// For external scripts the combined stdout is returned under the key "output".
// args are forwarded to the script as globals (Lua) or environment variables
// (external), minus the reserved "lang" and "script" keys.
func (h *ScriptHandler) Run(ctx context.Context, lang, script string, args map[string]string) (map[string]interface{}, error) {
	// Strip transport-level keys from the args passed to the script.
	scriptArgs := make(map[string]string, len(args))
	for k, v := range args {
		if k == "lang" || k == "script" {
			continue
		}
		scriptArgs[k] = v
	}

	switch lang {
	case "lua", "":
		result, err := h.engine.RunLua(ctx, script, scriptArgs)
		if err != nil {
			return nil, fmt.Errorf("lua script: %w", err)
		}
		h.logger.WithField("keys", len(result)).Debug("waddlebot: lua script completed")
		return result, nil

	case "python", "bash", "powershell":
		output, err := h.engine.RunExternal(ctx, lang, script, scriptArgs)
		if err != nil {
			return nil, fmt.Errorf("%s script: %w", lang, err)
		}
		h.logger.WithFields(logrus.Fields{
			"lang":   lang,
			"bytes":  len(output),
		}).Debug("waddlebot: external script completed")
		return map[string]interface{}{"output": output}, nil

	default:
		return nil, fmt.Errorf("unsupported script language %q (supported: lua, python, bash, powershell)", lang)
	}
}
