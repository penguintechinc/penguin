package main

import (
	"context"
	"fmt"
	"os"

	"github.com/penguintechinc/penguin/services/desktop/internal/app"
	"github.com/penguintechinc/penguin/services/desktop/internal/config"
	"github.com/penguintechinc/penguin/services/desktop/internal/module"
	"github.com/penguintechinc/penguin/services/desktop/pkg/clischema"
	"github.com/penguintechinc/penguin/services/desktop/pkg/modulepb"
	"github.com/spf13/cobra"
)

var (
	version   = "0.1.0"
	cfgFile   string
)

func main() {
	rootCmd := &cobra.Command{
		Use:   "penguin",
		Short: "PenguinTech unified client",
		Long:  "PenguinTech unified desktop client for VPN, DNS, NTP, Nest, and ArticDBM services.",
	}

	rootCmd.PersistentFlags().StringVar(&cfgFile, "config", "", "config file (default: ~/.config/penguin/penguin.yaml)")

	rootCmd.AddCommand(runCmd())
	rootCmd.AddCommand(statusCmd())
	rootCmd.AddCommand(versionCmd())

	// Load config and register module CLI commands
	cfg, err := config.Load(cfgFile)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error loading config: %v\n", err)
		os.Exit(1)
	}

	application := app.New(cfg, version)

	// Discover external plugins
	if err := application.DiscoverPlugins(); err != nil {
		application.Logger.WithError(err).Warn("Plugin discovery failed")
	}

	// Register CLI commands from all modules
	registerModuleCLI(rootCmd, application)

	if err := rootCmd.Execute(); err != nil {
		os.Exit(1)
	}
}

// registerModuleCLI registers CLI commands from both plugin and legacy modules.
func registerModuleCLI(rootCmd *cobra.Command, application *app.App) {
	for _, m := range application.Registry.AllModules() {
		// PluginModule — declarative CLI via proto
		if pm, ok := m.(module.PluginModule); ok {
			cmdList, err := pm.GetCLICommands(context.Background())
			if err != nil {
				application.Logger.WithError(err).WithField("module", m.Name()).Warn("Failed to get CLI commands")
				continue
			}
			// Create an executor closure that routes to the specific module
			pmRef := pm
			executor := func(ctx context.Context, req *modulepb.CLICommandRequest) (*modulepb.CLICommandResponse, error) {
				return pmRef.ExecuteCLICommand(ctx, req)
			}
			for _, cmd := range clischema.ToCobraList(cmdList, executor) {
				rootCmd.AddCommand(cmd)
			}
			continue
		}

		// LegacyModule — direct Cobra commands
		if lm, ok := m.(module.LegacyModule); ok {
			for _, cmd := range lm.CLICommands() {
				rootCmd.AddCommand(cmd)
			}
		}
	}
}

func runCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "run",
		Short: "Start all enabled modules in CLI mode",
		RunE: func(cmd *cobra.Command, args []string) error {
			cfg, err := config.Load(cfgFile)
			if err != nil {
				return err
			}
			application := app.New(cfg, version)
			if err := application.DiscoverPlugins(); err != nil {
				application.Logger.WithError(err).Warn("Plugin discovery failed")
			}
			return application.Run(context.Background())
		},
	}
}

func statusCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "status",
		Short: "Show status of all modules",
		RunE: func(cmd *cobra.Command, args []string) error {
			cfg, err := config.Load(cfgFile)
			if err != nil {
				return err
			}
			application := app.New(cfg, version)
			if err := application.DiscoverPlugins(); err != nil {
				application.Logger.WithError(err).Warn("Plugin discovery failed")
			}

			ctx := context.Background()
			if err := application.Init(ctx); err != nil {
				return err
			}

			for _, m := range application.Registry.EnabledModules() {
				status := m.HealthCheck(ctx)
				fmt.Printf("%-12s %s  %s\n", m.DisplayName(), status.State.String(), status.Message)
			}
			return nil
		},
	}
}

func versionCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "version",
		Short: "Show version information",
		Run: func(cmd *cobra.Command, args []string) {
			fmt.Printf("PenguinTech Client v%s\n", version)
		},
	}
}
