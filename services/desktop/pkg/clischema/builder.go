// Package clischema provides helpers for building declarative CLI command trees.
// Modules use these builders to describe their CLI commands without importing Cobra.
// The host process converts the command tree into actual Cobra commands.
package clischema

import (
	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
)

// Command creates a CLI command descriptor.
func Command(use, short string) *modulepb.CLICommand {
	return &modulepb.CLICommand{
		Use:   use,
		Short: short,
	}
}

// CommandWithLong creates a CLI command with a long description.
func CommandWithLong(use, short, long string) *modulepb.CLICommand {
	return &modulepb.CLICommand{
		Use:   use,
		Short: short,
		Long:  long,
	}
}

// WithSubcommands adds subcommands to a command.
func WithSubcommands(cmd *modulepb.CLICommand, subs ...modulepb.CLICommand) *modulepb.CLICommand {
	cmd.Subcommands = append(cmd.Subcommands, subs...)
	return cmd
}

// WithFlags adds flags to a command.
func WithFlags(cmd *modulepb.CLICommand, flags ...modulepb.CLIFlag) *modulepb.CLICommand {
	cmd.Flags = append(cmd.Flags, flags...)
	return cmd
}

// Flag creates a CLI flag descriptor.
func Flag(name, shorthand, usage, defaultValue string) modulepb.CLIFlag {
	return modulepb.CLIFlag{
		Name:         name,
		Shorthand:    shorthand,
		Usage:        usage,
		DefaultValue: defaultValue,
	}
}

// RequiredFlag creates a required CLI flag descriptor.
func RequiredFlag(name, shorthand, usage string) modulepb.CLIFlag {
	return modulepb.CLIFlag{
		Name:      name,
		Shorthand: shorthand,
		Usage:     usage,
		Required:  true,
	}
}

// CommandList creates a CLICommandList from commands.
func CommandList(cmds ...modulepb.CLICommand) *modulepb.CLICommandList {
	return &modulepb.CLICommandList{Commands: cmds}
}
