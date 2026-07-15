// Command penguin is the unprivileged CLI for the PenguinTech endpoint agent.
// It talks to the penguind daemon over authenticated local IPC.
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"strings"
	"time"

	daemonv1 "github.com/penguintechinc/penguin/api/proto/penguin/daemon/v1"
	"github.com/penguintechinc/penguin/internal/cli"
	"github.com/penguintechinc/penguin/internal/ipc"
	"github.com/penguintechinc/penguin/internal/version"
	"github.com/spf13/cobra"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
)

func main() {
	if err := run(os.Args[1:]); err != nil {
		fmt.Fprintf(os.Stderr, "penguin: %v\n", err)
		os.Exit(1)
	}
}

// Global flags
var (
	socketFlag   string
	jsonFlag     bool
	daemonClient daemonv1.DaemonClient
)

func run(args []string) error {
	// Parse global flags
	fs := flag.NewFlagSet("penguin", flag.ContinueOnError)
	fs.StringVar(&socketFlag, "socket", defaultSocket(), "daemon socket path")
	fs.BoolVar(&jsonFlag, "json", false, "output JSON")

	// Separate global flags from commands
	var cmdArgs []string
	for i := 0; i < len(args); i++ {
		arg := args[i]
		if !strings.HasPrefix(arg, "-") {
			cmdArgs = args[i:]
			break
		}
		if arg == "-socket" || arg == "--socket" {
			// Skip next arg which is the value
			i++
			continue
		}
		if arg == "-json" || arg == "--json" {
			continue
		}
	}

	if err := fs.Parse(args[:len(args)-len(cmdArgs)]); err != nil {
		return err
	}

	// Connect to daemon
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()

	conn, err := ipc.Dial(ctx, socketFlag)
	if err != nil {
		fmt.Fprintf(os.Stderr, "penguin: is penguind running? daemon unreachable\n")
		return fmt.Errorf("connect daemon: %w", err)
	}
	defer func() {
		if err := conn.Close(); err != nil {
			fmt.Fprintf(os.Stderr, "penguin: close connection: %v\n", err)
		}
	}()

	daemonClient = daemonv1.NewDaemonClient(conn)

	// Build root command with static subcommands
	root := &cobra.Command{
		Use:   "penguin",
		Short: "PenguinTech unified endpoint agent",
		Long:  "Manage the penguin daemon and its modules",
		PersistentPreRun: func(cmd *cobra.Command, args []string) {
			// Flags already parsed
		},
	}

	// Add static commands
	root.AddCommand(cmdVersion())
	root.AddCommand(cmdModules())
	root.AddCommand(cmdLoad())
	root.AddCommand(cmdUnload())
	root.AddCommand(cmdStatus())
	root.AddCommand(cmdLogs())
	root.AddCommand(cmdUpdate())

	// Add dynamic module commands (best-effort, 300ms timeout)
	dynamicCtx, dynamicCancel := context.WithTimeout(context.Background(), 300*time.Millisecond)
	builder := cli.NewBuilder(conn)
	if dynamicRoot, err := builder.BuildRoot(dynamicCtx); err == nil {
		for _, cmd := range dynamicRoot.Commands() {
			root.AddCommand(cmd)
		}
	}
	dynamicCancel()

	// Execute
	root.SetArgs(cmdArgs)
	root.SilenceUsage = true  // usage dumps on RPC errors are just noise
	root.SilenceErrors = true // main() prints the (friendlier) error itself
	return friendly(root.Execute())
}

// friendly turns transport-level gRPC failures into an actionable message.
// Anything else is returned unchanged.
func friendly(err error) error {
	if err == nil {
		return nil
	}
	if st, ok := status.FromError(err); ok && st.Code() == codes.Unavailable {
		return fmt.Errorf("cannot reach penguind at %s — is the daemon running?", socketFlag)
	}
	return err
}

// defaultSocket returns the default socket path for the platform.
func defaultSocket() string {
	// On Windows, ignored and replaced with \\.\pipe\penguind internally
	return "/run/penguin/penguind.sock"
}

// cmdVersion returns the version command.
func cmdVersion() *cobra.Command {
	return &cobra.Command{
		Use:   "version",
		Short: "Show version information",
		RunE: func(cmd *cobra.Command, args []string) error {
			fmt.Printf("penguin version %s\n", version.Version)

			// Try to get daemon version
			ctx, cancel := context.WithTimeout(context.Background(), 1*time.Second)
			defer cancel()

			resp, err := daemonClient.Version(ctx, &daemonv1.VersionRequest{ApiVersion: "v1"})
			if err == nil {
				fmt.Printf("penguind version %s\n", resp.DaemonVersion)
			}

			return nil
		},
	}
}

// cmdModules returns the modules command.
func cmdModules() *cobra.Command {
	return &cobra.Command{
		Use:   "modules",
		Short: "List all modules",
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()

			resp, err := daemonClient.ListModules(ctx, &daemonv1.ListModulesRequest{ApiVersion: "v1"})
			if err != nil {
				return err
			}

			if jsonFlag {
				data, _ := json.MarshalIndent(resp, "", "  ")
				fmt.Println(string(data))
				return nil
			}

			if len(resp.Modules) == 0 {
				fmt.Println("No modules loaded")
				return nil
			}

			fmt.Printf("%-14s %-10s %-10s %-42s %s\n", "NAME", "STATE", "VERSION", "DESCRIPTION", "LICENSE FLAG")
			fmt.Println(strings.Repeat("-", 100))
			for _, m := range resp.Modules {
				fmt.Printf("%-14s %-10s %-10s %-42s %s\n",
					m.Name, m.State, m.Version, m.Description, m.LicenseFeature)
			}

			return nil
		},
	}
}

// cmdLoad returns the load command.
func cmdLoad() *cobra.Command {
	return &cobra.Command{
		Use:   "load <module>",
		Short: "Load a module",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()

			resp, err := daemonClient.LoadModule(ctx, &daemonv1.LoadModuleRequest{
				ApiVersion: "v1",
				Name:       args[0],
			})
			if err != nil {
				st, ok := status.FromError(err)
				if ok {
					switch st.Code() {
					case codes.NotFound:
						return fmt.Errorf("module %q not found", args[0])
					case codes.PermissionDenied:
						return fmt.Errorf("license feature required: %s", st.Message())
					}
				}
				return err
			}

			fmt.Printf("Module %q loaded (state: %s)\n", args[0], resp.State)
			return nil
		},
	}
}

// cmdUnload returns the unload command.
func cmdUnload() *cobra.Command {
	return &cobra.Command{
		Use:   "unload <module>",
		Short: "Unload a module",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()

			_, err := daemonClient.UnloadModule(ctx, &daemonv1.UnloadModuleRequest{
				ApiVersion: "v1",
				Name:       args[0],
			})
			if err != nil {
				return err
			}

			fmt.Printf("Module %q unloaded\n", args[0])
			return nil
		},
	}
}

// cmdStatus returns the status command.
func cmdStatus() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "status [module]",
		Short: "Show status of modules",
		RunE: func(cmd *cobra.Command, args []string) error {
			moduleName := ""
			if len(args) > 0 {
				moduleName = args[0]
			}

			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()

			resp, err := daemonClient.GetStatus(ctx, &daemonv1.GetStatusRequest{
				ApiVersion: "v1",
				Name:       moduleName,
			})
			if err != nil {
				return err
			}

			if jsonFlag {
				data, _ := json.MarshalIndent(resp, "", "  ")
				fmt.Println(string(data))
				return nil
			}

			fmt.Printf("Daemon version: %s\n\n", resp.DaemonVersion)

			if len(resp.Modules) == 0 {
				fmt.Println("No modules")
				return nil
			}

			fmt.Printf("%-20s %-15s %-15s %s\n", "NAME", "STATE", "HEALTH", "MESSAGE")
			fmt.Println(strings.Repeat("-", 70))
			for _, m := range resp.Modules {
				fmt.Printf("%-20s %-15s %-15s %s\n", m.Name, m.State, m.Health, m.HealthMessage)
			}

			return nil
		},
	}

	return cmd
}

// cmdLogs returns the logs command.
func cmdLogs() *cobra.Command {
	var follow bool
	var lines int

	cmd := &cobra.Command{
		Use:   "logs [module]",
		Short: "Tail daemon or module logs",
		RunE: func(cmd *cobra.Command, args []string) error {
			moduleName := ""
			if len(args) > 0 {
				moduleName = args[0]
			}

			ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
			defer cancel()

			if lines < 0 || lines > 10000 {
				return fmt.Errorf("lines must be between 0 and 10000")
			}
			stream, err := daemonClient.TailLogs(ctx, &daemonv1.TailLogsRequest{
				ApiVersion: "v1",
				Module:     moduleName,
				Lines:      int32(lines), // #nosec G115 - validated above
				Follow:     follow,
			})
			if err != nil {
				st, ok := status.FromError(err)
				if ok && st.Code() == codes.Unimplemented {
					fmt.Println("TailLogs not implemented yet")
					return nil
				}
				return err
			}

			for {
				line, err := stream.Recv()
				if err != nil {
					break
				}

				ts := time.Unix(0, line.AtUnixNano).Format("2006-01-02 15:04:05")
				fmt.Printf("[%s] %s: %s\n", ts, line.Level, line.Message)
			}

			return nil
		},
	}

	cmd.Flags().BoolVar(&follow, "follow", false, "follow log output")
	cmd.Flags().IntVar(&lines, "lines", 10, "initial log lines to show")

	return cmd
}

// cmdUpdate returns the update command.
func cmdUpdate() *cobra.Command {
	var yes bool

	cmd := &cobra.Command{
		Use:   "update",
		Short: "Check and apply daemon updates",
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()

			// Check for updates
			checkResp, err := daemonClient.CheckUpdate(ctx, &daemonv1.CheckUpdateRequest{
				ApiVersion: "v1",
			})
			if err != nil {
				return err
			}

			fmt.Printf("Current version: %s\n", checkResp.CurrentVersion)
			fmt.Printf("Latest version: %s\n", checkResp.LatestVersion)

			if !checkResp.Available {
				fmt.Println("No updates available")
				return nil
			}

			if !yes {
				fmt.Print("Apply update? (y/n): ")
				var answer string
				if _, err := fmt.Scanln(&answer); err != nil {
					// If input fails, treat as no
					return nil
				}
				if answer != "y" && answer != "yes" {
					return nil
				}
			}

			// Apply update
			ctx, cancel = context.WithTimeout(context.Background(), 60*time.Second)
			defer cancel()

			applyResp, err := daemonClient.ApplyUpdate(ctx, &daemonv1.ApplyUpdateRequest{
				ApiVersion: "v1",
			})
			if err != nil {
				return err
			}

			if applyResp.Applied {
				fmt.Println("Update applied successfully")
			} else {
				fmt.Printf("Update failed: %s\n", applyResp.Message)
			}

			return nil
		},
	}

	cmd.Flags().BoolVar(&yes, "yes", false, "apply update without confirmation")

	return cmd
}
