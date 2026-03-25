# penguin-desktop

Shared Go library for Penguin desktop modules. Extracts commonly duplicated
code so each module stays lean and consistent.

## Import

```go
import desktop "github.com/penguintechinc/penguin-libs/packages/penguin-desktop"
```

---

## JSONClient

Wraps `http.Client` with JSON marshaling, a base URL, and optional Bearer
token injection. Replaces the ~37-line `doJSON` helper that was duplicated
across five modules.

```go
// Static token (e.g. from config)
client := desktop.NewJSONClientWithToken("http://localhost:8080", 10*time.Second, cfg.APIToken)

// Dynamic token (e.g. refreshed from license manager)
client := desktop.NewJSONClient("http://localhost:8080", 10*time.Second)
client.GetToken = func() string { return tokenStore.Current() }

// Optional per-request headers
client.ExtraHeaders = func(r *http.Request) {
    r.Header.Set("X-Module-ID", moduleID)
}

// Make a request
var result MyResponse
err := client.DoJSON(ctx, http.MethodGet, "/api/v1/status", nil, &result)

// POST with body
err = client.DoJSON(ctx, http.MethodPost, "/api/v1/events", myPayload, nil)
```

HTTP responses with status >= 400 are returned as errors containing the
status code and response body text.

---

## TickWorker

Runs an `Action` on a fixed `Interval` inside a managed background goroutine.
Replaces the ticker + stopCh + WaitGroup boilerplate used in KillKrill flush,
SkaUsWatch check-in, and WaddlePerf scheduling.

```go
worker := &desktop.TickWorker{
    Interval: 30 * time.Second,
    Timeout:  10 * time.Second, // per-tick deadline; 0 = no timeout
    Action: func(ctx context.Context) error {
        return myModule.Flush(ctx)
    },
    OnError: func(err error) {
        log.WithError(err).Error("flush failed")
    },
}

worker.Start() // spawns goroutine

// ... later, on shutdown:
worker.Stop()  // signals stop, waits for goroutine to exit
```

Calling `Start()` on an already-running worker panics. `Stop()` on a worker
that was never started is a no-op.

---

## ScriptEngine

Provides Lua embedded execution and external interpreter dispatch. Ported from
waddlebot's scripting package with waddlebot-specific API bindings removed.

### Lua

Scripts run in a sandboxed gopher-lua runtime. Only `base`, `table`, `string`,
`math`, and `package` libraries are loaded. `dofile`, `loadfile`, `load`, and
`loadstring` are removed. A `log` module (`log.info`, `log.warn`, `log.error`,
`log.debug`) is available inside scripts.

```go
engine := desktop.NewScriptEngine(logger)

output, err := engine.RunLua(ctx, `
    local result = {}
    result["status"] = arg_host .. " checked"
    _OUTPUT = result
`, map[string]string{
    "arg_host": "192.168.1.1",
})
// output["status"] == "192.168.1.1 checked"
```

`args` keys are injected as global Lua string variables. After execution
`_OUTPUT` is read back:
- If it is a Lua table, each key-value pair is returned in the map.
- If it is a plain value, it is returned under the key `"output"`.

### External interpreters

Supported languages: `"python"` (python3), `"bash"`, `"powershell"` (pwsh).
The script is passed via stdin; `args` become environment variables.

```go
out, err := engine.RunExternal(ctx, "bash", `
    echo "host is $TARGET_HOST"
`, map[string]string{
    "TARGET_HOST": "192.168.1.1",
})
```
