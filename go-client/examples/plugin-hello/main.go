package main

import (
	"context"
	"fmt"
	"time"

	"github.com/penguintechinc/penguin/pkg/sdk"
)

// HelloModule is a trivial example external plugin.
type HelloModule struct {
	host sdk.HostServices
}

func (h *HelloModule) Info() sdk.ModuleInfo {
	return sdk.ModuleInfo{
		Name:        "hello",
		Version:     "1.0.0",
		Description: "Example external plugin that greets the user",
	}
}

func (h *HelloModule) Init(ctx context.Context, host sdk.HostServices) error {
	h.host = host
	return nil
}

func (h *HelloModule) Start(ctx context.Context) error {
	return nil
}

func (h *HelloModule) Stop(ctx context.Context) error {
	return nil
}

func (h *HelloModule) Status(ctx context.Context) (sdk.Status, error) {
	return sdk.Status{
		State: sdk.StateRunning,
	}, nil
}

func (h *HelloModule) Health(ctx context.Context) sdk.HealthReport {
	return sdk.HealthReport{
		Level:     sdk.Healthy,
		Message:   "OK",
		CheckedAt: time.Now(),
	}
}

func (h *HelloModule) Commands() []sdk.CommandSpec {
	return []sdk.CommandSpec{
		{
			Name:     "greet",
			Use:      "greet <name>",
			Short:    "Greet someone",
			MinArgs:  1,
			MaxArgs:  1,
		},
	}
}

func (h *HelloModule) Dispatch(ctx context.Context, path []string, flags map[string]string, args []string) (*sdk.Result, error) {
	if len(path) == 0 {
		return nil, fmt.Errorf("no command")
	}

	if path[0] == "greet" {
		if len(args) != 1 {
			return &sdk.Result{
				Output:   "usage: hello greet <name>",
				ExitCode: 1,
			}, nil
		}
		name := args[0]
		output := fmt.Sprintf("hello, %s", name)
		return &sdk.Result{
			Output:   output,
			ExitCode: 0,
		}, nil
	}

	return nil, fmt.Errorf("unknown command: %s", path[0])
}

func (h *HelloModule) ConfigSchema() []byte {
	// No configuration needed.
	return nil
}

func main() {
	sdk.Serve(&HelloModule{})
}
