package clischema

import (
	"context"
	"fmt"
	"strings"

	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
	"github.com/spf13/cobra"
	"github.com/spf13/pflag"
)

// Executor is called when a leaf CLI command is invoked.
// It receives the full command path (e.g., "vpn connect") and the request.
type Executor func(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error)

// ToCobra converts a CLICommand proto tree into a Cobra command tree.
// Leaf commands (those with no subcommands) call the executor via RPC.
func ToCobra(cmd modulepb.CLICommand, executor Executor) *cobra.Command {
	return toCobraRecursive(cmd, "", executor)
}

// ToCobraList converts a CLICommandList into a slice of Cobra commands.
func ToCobraList(list *modulepb.CLICommandList, executor Executor) []*cobra.Command {
	if list == nil {
		return nil
	}
	result := make([]*cobra.Command, 0, len(list.Commands))
	for _, cmd := range list.Commands {
		result = append(result, ToCobra(cmd, executor))
	}
	return result
}

func toCobraRecursive(cmd modulepb.CLICommand, parentPath string, executor Executor) *cobra.Command {
	// Build the full command path for RPC routing
	cmdPath := cmd.Use
	if parentPath != "" {
		cmdPath = parentPath + " " + cmd.Use
	}
	// Strip args from the Use field to get clean path
	cleanPath := strings.Fields(cmdPath)[0]
	if parentPath != "" {
		cleanPath = parentPath + " " + strings.Fields(cmd.Use)[0]
	}

	cobraCmd := &cobra.Command{
		Use:   cmd.Use,
		Short: cmd.Short,
		Long:  cmd.Long,
	}

	// Add flags
	for _, f := range cmd.Flags {
		if f.Shorthand != "" {
			cobraCmd.Flags().StringP(f.Name, f.Shorthand, f.DefaultValue, f.Usage)
		} else {
			cobraCmd.Flags().String(f.Name, f.DefaultValue, f.Usage)
		}
		if f.Required {
			cobraCmd.MarkFlagRequired(f.Name)
		}
	}

	// Add subcommands recursively
	if len(cmd.Subcommands) > 0 {
		for _, sub := range cmd.Subcommands {
			cobraCmd.AddCommand(toCobraRecursive(sub, cleanPath, executor))
		}
	} else {
		// Leaf command — wire up to executor
		finalPath := cleanPath
		cobraCmd.RunE = func(c *cobra.Command, args []string) error {
			// Collect flag values
			flags := make(map[string]string)
			c.Flags().Visit(func(f *pflag.Flag) {
				flags[f.Name] = f.Value.String()
			})

			req := &modulepb.CLICommandRequest{
				Args:        args,
				Flags:       flags,
				CommandPath: finalPath,
			}

			resp, err := executor(c.Context(), req)
			if err != nil {
				return err
			}

			if resp.Stdout != "" {
				fmt.Print(resp.Stdout)
			}
			if resp.Stderr != "" {
				fmt.Fprint(c.ErrOrStderr(), resp.Stderr)
			}
			if resp.ExitCode != 0 {
				return fmt.Errorf("command exited with code %d", resp.ExitCode)
			}
			return nil
		}
	}

	return cobraCmd
}
