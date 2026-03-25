package clischema

import (
	"context"
	"testing"

	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
)

func TestToCobraSimple(t *testing.T) {
	cmd := modulepb.CLICommand{
		Use:   "hello",
		Short: "Say hello",
	}

	var called bool
	var gotPath string
	executor := func(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
		called = true
		gotPath = req.CommandPath
		return &modulepb.CLICommandResponse{Stdout: "hello\n"}, nil
	}

	cobraCmd := ToCobra(cmd, executor)
	if cobraCmd.Use != "hello" {
		t.Errorf("expected use hello, got %s", cobraCmd.Use)
	}

	cobraCmd.SetArgs([]string{})
	err := cobraCmd.Execute()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !called {
		t.Error("executor was not called")
	}
	if gotPath != "hello" {
		t.Errorf("expected path hello, got %s", gotPath)
	}
}

func TestToCobraWithSubcommands(t *testing.T) {
	cmd := modulepb.CLICommand{
		Use:   "vpn",
		Short: "VPN commands",
		Subcommands: []modulepb.CLICommand{
			{Use: "connect", Short: "Connect"},
			{Use: "disconnect", Short: "Disconnect"},
		},
	}

	var gotPath string
	executor := func(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
		gotPath = req.CommandPath
		return &modulepb.CLICommandResponse{Stdout: "ok\n"}, nil
	}

	cobraCmd := ToCobra(cmd, executor)
	if len(cobraCmd.Commands()) != 2 {
		t.Fatalf("expected 2 subcommands, got %d", len(cobraCmd.Commands()))
	}

	cobraCmd.SetArgs([]string{"connect"})
	err := cobraCmd.Execute()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if gotPath != "vpn connect" {
		t.Errorf("expected path 'vpn connect', got %s", gotPath)
	}
}

func TestToCobraWithArgs(t *testing.T) {
	cmd := modulepb.CLICommand{
		Use:   "query [domain]",
		Short: "Query DNS",
	}

	var gotArgs []string
	executor := func(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
		gotArgs = req.Args
		return &modulepb.CLICommandResponse{Stdout: "ok\n"}, nil
	}

	cobraCmd := ToCobra(cmd, executor)
	cobraCmd.SetArgs([]string{"example.com"})
	err := cobraCmd.Execute()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(gotArgs) != 1 || gotArgs[0] != "example.com" {
		t.Errorf("expected args [example.com], got %v", gotArgs)
	}
}

func TestToCobraWithFlags(t *testing.T) {
	cmd := modulepb.CLICommand{
		Use:   "list",
		Short: "List items",
		Flags: []modulepb.CLIFlag{
			{Name: "format", Shorthand: "f", Usage: "Output format", DefaultValue: "table"},
		},
	}

	var gotFlags map[string]string
	executor := func(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
		gotFlags = req.Flags
		return &modulepb.CLICommandResponse{Stdout: "ok\n"}, nil
	}

	cobraCmd := ToCobra(cmd, executor)
	cobraCmd.SetArgs([]string{"--format", "json"})
	err := cobraCmd.Execute()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if gotFlags["format"] != "json" {
		t.Errorf("expected format=json, got %v", gotFlags)
	}
}

func TestToCobraExitCode(t *testing.T) {
	cmd := modulepb.CLICommand{Use: "fail", Short: "Fail"}

	executor := func(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
		return &modulepb.CLICommandResponse{Stderr: "error\n", ExitCode: 1}, nil
	}

	cobraCmd := ToCobra(cmd, executor)
	cobraCmd.SetArgs([]string{})
	err := cobraCmd.Execute()
	if err == nil {
		t.Error("expected error for non-zero exit code")
	}
}

func TestToCobraList(t *testing.T) {
	list := &modulepb.CLICommandList{
		Commands: []modulepb.CLICommand{
			{Use: "vpn", Short: "VPN"},
			{Use: "dns", Short: "DNS"},
		},
	}

	executor := func(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
		return &modulepb.CLICommandResponse{}, nil
	}

	cmds := ToCobraList(list, executor)
	if len(cmds) != 2 {
		t.Errorf("expected 2 commands, got %d", len(cmds))
	}
}

func TestToCobraListNil(t *testing.T) {
	cmds := ToCobraList(nil, nil)
	if cmds != nil {
		t.Errorf("expected nil for nil list, got %v", cmds)
	}
}
