package sdk

import (
	"testing"
)

// TestCommandSpecStructure tests the CommandSpec data structure.
func TestCommandSpecStructure(t *testing.T) {
	spec := CommandSpec{
		Name:    "test",
		Use:     "test <arg>",
		Short:   "Test command",
		Tray:    true,
		MinArgs: 1,
		MaxArgs: 1,
	}

	if spec.Name != "test" {
		t.Errorf("CommandSpec.Name = %q, want %q", spec.Name, "test")
	}

	if spec.Use != "test <arg>" {
		t.Errorf("CommandSpec.Use = %q, want %q", spec.Use, "test <arg>")
	}

	if spec.Short != "Test command" {
		t.Errorf("CommandSpec.Short = %q, want %q", spec.Short, "Test command")
	}

	if !spec.Tray {
		t.Errorf("CommandSpec.Tray = %v, want true", spec.Tray)
	}

	if spec.MinArgs != 1 {
		t.Errorf("CommandSpec.MinArgs = %d, want 1", spec.MinArgs)
	}

	if spec.MaxArgs != 1 {
		t.Errorf("CommandSpec.MaxArgs = %d, want 1", spec.MaxArgs)
	}
}

// TestCommandSpecWithFlags tests CommandSpec with flags.
func TestCommandSpecWithFlags(t *testing.T) {
	spec := CommandSpec{
		Name:  "query",
		Use:   "query <domain>",
		Short: "Query DNS",
		Flags: []FlagSpec{
			{
				Name:      "type",
				Shorthand: "t",
				Usage:     "Record type",
				Default:   "A",
				Type:      FlagString,
			},
		},
	}

	if len(spec.Flags) != 1 {
		t.Errorf("expected 1 flag, got %d", len(spec.Flags))
	}

	flag := spec.Flags[0]
	if flag.Name != "type" {
		t.Errorf("flag.Name = %q, want %q", flag.Name, "type")
	}

	if flag.Shorthand != "t" {
		t.Errorf("flag.Shorthand = %q, want %q", flag.Shorthand, "t")
	}

	if flag.Type != FlagString {
		t.Errorf("flag.Type = %q, want %q", flag.Type, FlagString)
	}
}

// TestCommandSpecWithSubcommands tests CommandSpec with nested subcommands.
func TestCommandSpecWithSubcommands(t *testing.T) {
	spec := CommandSpec{
		Name:  "forward",
		Use:   "forward",
		Short: "Manage forwarding",
		Subcommands: []CommandSpec{
			{
				Name:  "status",
				Use:   "status",
				Short: "Show status",
			},
			{
				Name:  "start",
				Use:   "start",
				Short: "Start forwarding",
			},
		},
	}

	if len(spec.Subcommands) != 2 {
		t.Errorf("expected 2 subcommands, got %d", len(spec.Subcommands))
	}

	if spec.Subcommands[0].Name != "status" {
		t.Errorf("subcommand[0].Name = %q, want %q", spec.Subcommands[0].Name, "status")
	}

	if spec.Subcommands[1].Name != "start" {
		t.Errorf("subcommand[1].Name = %q, want %q", spec.Subcommands[1].Name, "start")
	}
}

// TestFlagSpecStructure tests the FlagSpec data structure.
func TestFlagSpecStructure(t *testing.T) {
	tests := []struct {
		name     string
		flagType FlagType
		expected FlagType
	}{
		{"string flag", FlagString, FlagString},
		{"bool flag", FlagBool, FlagBool},
		{"int flag", FlagInt, FlagInt},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if tt.flagType != tt.expected {
				t.Errorf("FlagType = %q, want %q", tt.flagType, tt.expected)
			}
		})
	}
}

// TestResultStructure tests the Result data structure.
func TestResultStructure(t *testing.T) {
	result := Result{
		Output:   "command output",
		JSON:     []byte(`{"key": "value"}`),
		ExitCode: 0,
	}

	if result.Output != "command output" {
		t.Errorf("Result.Output = %q, want %q", result.Output, "command output")
	}

	if string(result.JSON) != `{"key": "value"}` {
		t.Errorf("Result.JSON = %q, want %q", string(result.JSON), `{"key": "value"}`)
	}

	if result.ExitCode != 0 {
		t.Errorf("Result.ExitCode = %d, want 0", result.ExitCode)
	}
}

// TestResultErrorExitCode tests Result with non-zero exit code.
func TestResultErrorExitCode(t *testing.T) {
	result := Result{
		Output:   "error occurred",
		ExitCode: 1,
	}

	if result.ExitCode != 1 {
		t.Errorf("Result.ExitCode = %d, want 1", result.ExitCode)
	}
}

// TestFlagTypeConstants verifies the FlagType constants.
func TestFlagTypeConstants(t *testing.T) {
	tests := []struct {
		name     string
		flagType FlagType
		value    string
	}{
		{"FlagString", FlagString, "string"},
		{"FlagBool", FlagBool, "bool"},
		{"FlagInt", FlagInt, "int"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if tt.flagType != FlagType(tt.value) {
				t.Errorf("%s = %q, want %q", tt.name, tt.flagType, tt.value)
			}
		})
	}
}

// TestCommandSpecNoArgs tests CommandSpec with no argument constraints.
func TestCommandSpecNoArgs(t *testing.T) {
	spec := CommandSpec{
		Name:    "help",
		Use:     "help",
		Short:   "Show help",
		MinArgs: 0,
		MaxArgs: 0,
	}

	if spec.MinArgs != 0 || spec.MaxArgs != 0 {
		t.Errorf("expected MinArgs=0, MaxArgs=0, got %d, %d", spec.MinArgs, spec.MaxArgs)
	}
}

// TestCommandSpecUnlimitedArgs tests CommandSpec with unlimited arguments.
func TestCommandSpecUnlimitedArgs(t *testing.T) {
	spec := CommandSpec{
		Name:    "echo",
		Use:     "echo <args...>",
		Short:   "Echo arguments",
		MinArgs: 1,
		MaxArgs: -1, // unlimited
	}

	if spec.MinArgs != 1 {
		t.Errorf("MinArgs = %d, want 1", spec.MinArgs)
	}

	if spec.MaxArgs != -1 {
		t.Errorf("MaxArgs = %d, want -1", spec.MaxArgs)
	}
}
