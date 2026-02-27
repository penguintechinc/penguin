package main

import (
	"context"
	"fmt"
	"os"

	"github.com/penguintechinc/penguin/services/desktop/internal/app"
	"github.com/penguintechinc/penguin/services/desktop/internal/config"
)

var version = "0.1.0"

func main() {
	cfgFile := ""
	if len(os.Args) > 1 {
		for i, arg := range os.Args {
			if arg == "--config" && i+1 < len(os.Args) {
				cfgFile = os.Args[i+1]
			}
		}
	}

	cfg, err := config.Load(cfgFile)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error loading config: %v\n", err)
		os.Exit(1)
	}

	application := app.New(cfg, version)

	// Discover external plugins before initialization
	if err := application.DiscoverPlugins(); err != nil {
		application.Logger.WithError(err).Warn("Plugin discovery failed")
	}

	ctx := context.Background()

	if err := application.Init(ctx); err != nil {
		fmt.Fprintf(os.Stderr, "Error initializing: %v\n", err)
		os.Exit(1)
	}

	// Launch GUI (will block until window closed)
	if err := runGUI(application); err != nil {
		fmt.Fprintf(os.Stderr, "Error running GUI: %v\n", err)
		os.Exit(1)
	}
}
