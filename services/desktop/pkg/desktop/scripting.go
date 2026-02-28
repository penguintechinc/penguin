package desktop

import (
	"bytes"
	"context"
	"fmt"
	"os/exec"

	"github.com/sirupsen/logrus"
	lua "github.com/yuin/gopher-lua"
)

// ScriptEngine provides embedded Lua execution and external interpreter dispatch.
// It is ported from waddlebot's scripting package and stripped of waddlebot-specific
// API bindings so it can be shared across desktop modules.
type ScriptEngine struct {
	logger *logrus.Logger
}

// NewScriptEngine creates a ScriptEngine backed by the supplied logger.
func NewScriptEngine(logger *logrus.Logger) *ScriptEngine {
	return &ScriptEngine{logger: logger}
}

// RunLua executes a Lua script string inside a sandboxed gopher-lua runtime.
//
// args are injected as global string variables before the script runs.
// After execution the _OUTPUT global is read; if it is a Lua table it is
// converted to a map[string]any and returned. If _OUTPUT is a plain string
// it is returned under the key "output".
func (e *ScriptEngine) RunLua(ctx context.Context, script string, args map[string]string) (map[string]any, error) {
	L := lua.NewState(lua.Options{
		CallStackSize:       120,
		RegistrySize:        1024,
		SkipOpenLibs:        false,
		IncludeGoStackTrace: false,
	})
	defer L.Close()

	e.loadSafeLibraries(L)

	// Inject args as global string variables
	for k, v := range args {
		L.SetGlobal(k, lua.LString(v))
	}

	// Propagate context cancellation into gopher-lua
	L.SetContext(ctx)

	if err := L.DoString(script); err != nil {
		return nil, fmt.Errorf("scripting: lua execution: %w", err)
	}

	output := make(map[string]any)

	if lv := L.GetGlobal("_OUTPUT"); lv != lua.LNil {
		switch v := lv.(type) {
		case *lua.LTable:
			v.ForEach(func(key, val lua.LValue) {
				output[key.String()] = luaToAny(val)
			})
		default:
			output["output"] = v.String()
		}
	}

	return output, nil
}

// RunExternal executes a script via an external interpreter.
//
// lang selects the interpreter: "python" maps to python3, "bash" to bash,
// "powershell" to pwsh.
// script is the script content passed to the interpreter via stdin.
// args are set as environment variables available to the script process.
// The combined stdout of the process is returned as a string.
func (e *ScriptEngine) RunExternal(ctx context.Context, lang, script string, args map[string]string) (string, error) {
	exe, err := resolveExecutable(lang)
	if err != nil {
		return "", err
	}

	cmd := exec.CommandContext(ctx, exe)
	cmd.Stdin = bytes.NewBufferString(script)

	// Build environment from args
	if len(args) > 0 {
		env := make([]string, 0, len(args))
		for k, v := range args {
			env = append(env, k+"="+v)
		}
		cmd.Env = env
	}

	var stdout bytes.Buffer
	cmd.Stdout = &stdout

	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("scripting: external %s execution: %w", lang, err)
	}

	return stdout.String(), nil
}

// resolveExecutable maps a language name to an OS executable.
func resolveExecutable(lang string) (string, error) {
	switch lang {
	case "python":
		return "python3", nil
	case "bash":
		return "bash", nil
	case "powershell":
		return "pwsh", nil
	default:
		return "", fmt.Errorf("scripting: unsupported language %q (supported: python, bash, powershell)", lang)
	}
}

// loadSafeLibraries opens a curated subset of Lua standard libraries and removes
// functions that could allow file or process access.
func (e *ScriptEngine) loadSafeLibraries(L *lua.LState) {
	for _, pair := range []struct {
		name string
		open lua.LGFunction
	}{
		{lua.LoadLibName, lua.OpenPackage},
		{lua.BaseLibName, lua.OpenBase},
		{lua.TabLibName, lua.OpenTable},
		{lua.StringLibName, lua.OpenString},
		{lua.MathLibName, lua.OpenMath},
	} {
		if err := L.CallByParam(lua.P{
			Fn:      L.NewFunction(pair.open),
			NRet:    0,
			Protect: true,
		}, lua.LString(pair.name)); err != nil {
			e.logger.WithError(err).Errorf("scripting: failed to load Lua library %q", pair.name)
		}
	}

	// Remove unsafe globals that allow arbitrary code or file loading
	for _, fn := range []string{"dofile", "loadfile", "load", "loadstring"} {
		L.SetGlobal(fn, lua.LNil)
	}

	// Expose a simple log module so scripts can emit structured log lines
	logMod := L.NewTable()
	L.SetFuncs(logMod, map[string]lua.LGFunction{
		"info":  e.luaLogInfo,
		"warn":  e.luaLogWarn,
		"error": e.luaLogError,
		"debug": e.luaLogDebug,
	})
	L.SetGlobal("log", logMod)
}

// luaToAny converts a gopher-lua value to a plain Go value.
func luaToAny(v lua.LValue) any {
	switch val := v.(type) {
	case lua.LBool:
		return bool(val)
	case lua.LNumber:
		return float64(val)
	case lua.LString:
		return string(val)
	case *lua.LTable:
		m := make(map[string]any)
		val.ForEach(func(k, lv lua.LValue) {
			m[k.String()] = luaToAny(lv)
		})
		return m
	default:
		return v.String()
	}
}

// Lua log bridge functions

func (e *ScriptEngine) luaLogInfo(L *lua.LState) int {
	e.logger.Info("[lua] " + L.ToString(1))
	return 0
}

func (e *ScriptEngine) luaLogWarn(L *lua.LState) int {
	e.logger.Warn("[lua] " + L.ToString(1))
	return 0
}

func (e *ScriptEngine) luaLogError(L *lua.LState) int {
	e.logger.Error("[lua] " + L.ToString(1))
	return 0
}

func (e *ScriptEngine) luaLogDebug(L *lua.LState) int {
	e.logger.Debug("[lua] " + L.ToString(1))
	return 0
}
