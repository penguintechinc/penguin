package clischema

import (
	"testing"
)

func TestCommand(t *testing.T) {
	cmd := Command("vpn", "VPN management")
	if cmd.Use != "vpn" {
		t.Errorf("expected use vpn, got %s", cmd.Use)
	}
	if cmd.Short != "VPN management" {
		t.Errorf("expected short, got %s", cmd.Short)
	}
}

func TestCommandWithLong(t *testing.T) {
	cmd := CommandWithLong("query [domain]", "Query DNS", "Long description")
	if cmd.Long != "Long description" {
		t.Errorf("expected long desc, got %s", cmd.Long)
	}
}

func TestWithSubcommands(t *testing.T) {
	root := Command("vpn", "VPN")
	WithSubcommands(root,
		*Command("connect", "Connect"),
		*Command("disconnect", "Disconnect"),
	)
	if len(root.Subcommands) != 2 {
		t.Errorf("expected 2 subcommands, got %d", len(root.Subcommands))
	}
	if root.Subcommands[0].Use != "connect" {
		t.Errorf("expected first sub to be connect, got %s", root.Subcommands[0].Use)
	}
}

func TestWithFlags(t *testing.T) {
	cmd := Command("query", "Query DNS")
	WithFlags(cmd,
		Flag("type", "t", "Record type", "A"),
		RequiredFlag("domain", "d", "Domain name"),
	)
	if len(cmd.Flags) != 2 {
		t.Errorf("expected 2 flags, got %d", len(cmd.Flags))
	}
	if cmd.Flags[0].DefaultValue != "A" {
		t.Errorf("expected default A, got %s", cmd.Flags[0].DefaultValue)
	}
	if !cmd.Flags[1].Required {
		t.Error("expected second flag to be required")
	}
}

func TestFlag(t *testing.T) {
	f := Flag("output", "o", "Output format", "json")
	if f.Name != "output" {
		t.Errorf("expected name output, got %s", f.Name)
	}
	if f.Shorthand != "o" {
		t.Errorf("expected shorthand o, got %s", f.Shorthand)
	}
	if f.Required {
		t.Error("expected not required")
	}
}

func TestRequiredFlag(t *testing.T) {
	f := RequiredFlag("name", "n", "Resource name")
	if !f.Required {
		t.Error("expected required")
	}
	if f.DefaultValue != "" {
		t.Errorf("expected empty default, got %s", f.DefaultValue)
	}
}

func TestCommandList(t *testing.T) {
	list := CommandList(
		*Command("vpn", "VPN"),
		*Command("dns", "DNS"),
	)
	if len(list.Commands) != 2 {
		t.Errorf("expected 2 commands, got %d", len(list.Commands))
	}
}
