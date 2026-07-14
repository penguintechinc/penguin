// Package cli provides dynamic command-line interface construction from module commands.
package cli

import (
	"context"
	"fmt"
	"os"
	"strconv"
	"time"

	daemonv1 "github.com/penguintechinc/penguin/api/proto/penguin/daemon/v1"
	"github.com/spf13/cobra"
	"github.com/spf13/pflag"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
)

// Builder constructs the dynamic cobra command tree from daemon commands.
type Builder struct {
	daemonClient daemonv1.DaemonClient
	apiVersion   string
}

// NewBuilder creates a new CLI builder.
func NewBuilder(conn *grpc.ClientConn) *Builder {
	return &Builder{
		daemonClient: daemonv1.NewDaemonClient(conn),
		apiVersion:   "v1",
	}
}

// BuildRoot creates the root command with dynamic subcommands.
func (b *Builder) BuildRoot(ctx context.Context) (*cobra.Command, error) {
	root := &cobra.Command{
		Use:   "penguin",
		Short: "PenguinTech unified endpoint agent CLI",
		Long:  "Dynamic command-line interface to the penguind daemon",
	}

	// Fetch commands from daemon
	listCtx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()

	resp, err := b.daemonClient.ListCommands(listCtx, &daemonv1.ListCommandsRequest{
		ApiVersion: b.apiVersion,
	})
	if err != nil {
		return nil, fmt.Errorf("list commands: %w", err)
	}

	// Each module becomes a top-level command whose subcommands are the
	// module's own command tree: `penguin <module> <command> [args]`.
	for _, modCmds := range resp.Modules {
		if len(modCmds.Commands) == 0 {
			continue
		}
		moduleCmd := &cobra.Command{
			Use:   modCmds.Module,
			Short: fmt.Sprintf("Commands provided by the %s module", modCmds.Module),
		}
		for _, cmdSpec := range modCmds.Commands {
			moduleCmd.AddCommand(b.buildCommand(modCmds.Module, cmdSpec))
		}
		root.AddCommand(moduleCmd)
	}

	return root, nil
}

// buildCommand recursively builds a cobra command from a CommandSpec.
// parents is the command path above spec within the module (nil at the top),
// so a nested command dispatches its full path — e.g. ["config", "show"] —
// rather than just its leaf name.
func (b *Builder) buildCommand(moduleName string, spec *daemonv1.CommandSpec, parents ...string) *cobra.Command {
	path := make([]string, 0, len(parents)+1)
	path = append(path, parents...)
	path = append(path, spec.Name)

	cmd := &cobra.Command{
		Use:   spec.Name,
		Short: spec.Short,
		Args: func(cmd *cobra.Command, args []string) error {
			n := int64(len(args))
			if n < int64(spec.MinArgs) {
				return fmt.Errorf("requires at least %d argument(s), got %d", spec.MinArgs, len(args))
			}
			if spec.MaxArgs >= 0 && n > int64(spec.MaxArgs) {
				return fmt.Errorf("accepts at most %d argument(s), got %d", spec.MaxArgs, len(args))
			}
			return nil
		},
		RunE: func(cmd *cobra.Command, args []string) error {
			return b.dispatch(cmd, moduleName, path, args)
		},
	}

	// Add flags. The *P variants already register the long name, so a flag
	// must be declared exactly once — registering both forms makes pflag panic
	// with "flag redefined".
	for _, flag := range spec.Flags {
		switch flag.Type {
		case "string":
			cmd.Flags().StringP(flag.Name, flag.Shorthand, flag.Default, flag.Usage)
		case "bool":
			cmd.Flags().BoolP(flag.Name, flag.Shorthand, flag.Default == "true", flag.Usage)
		case "int":
			val, _ := strconv.Atoi(flag.Default)
			cmd.Flags().IntP(flag.Name, flag.Shorthand, val, flag.Usage)
		}
	}

	// Add subcommands, passing this command's path down so leaves dispatch
	// their full path.
	for _, subSpec := range spec.Subcommands {
		cmd.AddCommand(b.buildCommand(moduleName, subSpec, path...))
	}

	return cmd
}

// dispatch sends a Dispatch RPC to the daemon.
func (b *Builder) dispatch(cmd *cobra.Command, moduleName string, path []string, args []string) error {
	// Collect flags
	flags := make(map[string]string)
	cmd.Flags().Visit(func(f *pflag.Flag) {
		// Try each flag type
		if val, err := cmd.Flags().GetString(f.Name); err == nil {
			flags[f.Name] = val
		} else if bv, err := cmd.Flags().GetBool(f.Name); err == nil {
			flags[f.Name] = strconv.FormatBool(bv)
		} else if iv, err := cmd.Flags().GetInt(f.Name); err == nil {
			flags[f.Name] = strconv.Itoa(iv)
		}
	})

	// Build request
	req := &daemonv1.DispatchRequest{
		ApiVersion: b.apiVersion,
		Module:     moduleName,
		Path:       path,
		Flags:      flags,
		Args:       args,
	}

	// Call daemon
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	stream, err := b.daemonClient.Dispatch(ctx, req)
	if err != nil {
		st, ok := status.FromError(err)
		if ok && st.Code() == codes.Unavailable {
			fmt.Fprintf(os.Stderr, "penguin: is penguind running? daemon unreachable\n")
			return fmt.Errorf("daemon unreachable")
		}
		return err
	}

	// Stream output
	var exitCode int32
	for {
		chunk, err := stream.Recv()
		if err != nil {
			break
		}

		if chunk.Output != "" {
			fmt.Print(chunk.Output)
		}

		if chunk.Final {
			exitCode = chunk.ExitCode
		}
	}

	if exitCode != 0 {
		os.Exit(int(exitCode))
	}

	return nil
}
