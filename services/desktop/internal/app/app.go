package app

import (
	"context"
	"time"

	"github.com/penguintechinc/penguin/services/desktop/internal/auth"
	"github.com/penguintechinc/penguin/services/desktop/internal/config"
	"github.com/penguintechinc/penguin/services/desktop/internal/license"
	"github.com/penguintechinc/penguin/services/desktop/internal/logging"
	"github.com/penguintechinc/penguin/services/desktop/internal/module"
	"github.com/penguintechinc/penguin/services/desktop/internal/module/pluginhost"
	"github.com/sirupsen/logrus"
)

// App is the main application orchestrator.
type App struct {
	Config        *config.Config
	Registry      *module.Registry
	Auth          *auth.Manager
	License       *license.Validator
	Health        *module.HealthRunner
	PluginManager *pluginhost.Manager
	Supervisor    *pluginhost.Supervisor
	Logger        *logrus.Logger
	Version       string
}

// New creates a new App instance.
func New(cfg *config.Config, version string) *App {
	logger := logging.NewLogger(cfg.Logging.Level, cfg.Logging.Format, cfg.Logging.File)

	registry := module.NewRegistry(logger)
	authMgr := auth.NewManager(cfg.Auth.JWTServer, logger)
	licenseValidator := license.NewValidator(
		cfg.License.ServerURL,
		cfg.License.LicenseKey,
		cfg.License.UserToken,
		cfg.License.CacheTTL,
		logger,
	)
	healthRunner := module.NewHealthRunner(registry, 30*time.Second, logger)
	pluginMgr := pluginhost.NewManager(logger)
	supervisor := pluginhost.NewSupervisor(pluginMgr, logger)

	return &App{
		Config:        cfg,
		Registry:      registry,
		Auth:          authMgr,
		License:       licenseValidator,
		Health:        healthRunner,
		PluginManager: pluginMgr,
		Supervisor:    supervisor,
		Logger:        logger,
		Version:       version,
	}
}

// RegisterModule registers and optionally enables a module based on config.
func (a *App) RegisterModule(m module.ModuleBase) error {
	if err := a.Registry.Register(m); err != nil {
		return err
	}
	if a.Config.IsModuleEnabled(m.Name()) {
		return a.Registry.Enable(m.Name())
	}
	return nil
}

// DiscoverPlugins scans for external plugin binaries and registers them.
func (a *App) DiscoverPlugins() error {
	searchPaths := pluginhost.DefaultSearchPaths(config.GetConfigDir())
	if a.Config.Plugins.Dir != "" {
		searchPaths = append([]string{a.Config.Plugins.Dir}, searchPaths...)
	}

	discovery := pluginhost.NewDiscovery(searchPaths, a.Logger)
	plugins, err := discovery.Discover()
	if err != nil {
		return err
	}

	for _, dp := range plugins {
		// Skip if already registered as an in-process module
		if _, exists := a.Registry.Get(dp.Name); exists {
			a.Logger.WithField("module", dp.Name).Debug("Plugin skipped, already registered in-process")
			continue
		}

		mp, err := a.PluginManager.Launch(dp.Name, dp.Path)
		if err != nil {
			a.Logger.WithError(err).WithField("module", dp.Name).Warn("Failed to launch plugin")
			continue
		}

		wrapper := pluginhost.NewPluginModuleWrapper(mp)
		if err := a.RegisterModule(wrapper); err != nil {
			a.Logger.WithError(err).WithField("module", dp.Name).Warn("Failed to register plugin module")
			a.PluginManager.Stop(dp.Name)
			continue
		}

		a.Supervisor.Track(dp.Name, dp.Path)
	}

	// Also launch explicitly configured external modules
	for name, path := range a.Config.Plugins.ExternalModules {
		if _, exists := a.Registry.Get(name); exists {
			continue
		}

		mp, err := a.PluginManager.Launch(name, path)
		if err != nil {
			a.Logger.WithError(err).WithField("module", name).Warn("Failed to launch external module")
			continue
		}

		wrapper := pluginhost.NewPluginModuleWrapper(mp)
		if err := a.RegisterModule(wrapper); err != nil {
			a.Logger.WithError(err).WithField("module", name).Warn("Failed to register external module")
			a.PluginManager.Stop(name)
			continue
		}

		a.Supervisor.Track(name, path)
	}

	return nil
}

// Init initializes all enabled modules.
func (a *App) Init(ctx context.Context) error {
	deps := module.Dependencies{
		ConfigDir:  config.GetConfigDir(),
		DataDir:    config.GetDataDir(),
		Logger:     a.Logger,
	}

	// Set auth token if available
	if a.Auth.IsAuthenticated() {
		token, err := a.Auth.AccessToken()
		if err == nil {
			deps.AuthToken = token
		}
	}

	deps.LicenseKey = a.Config.License.LicenseKey

	return a.Registry.InitAll(ctx, deps)
}

// Start starts all enabled modules and health checking.
func (a *App) Start(ctx context.Context) error {
	if err := a.Registry.StartAll(ctx); err != nil {
		return err
	}
	a.Health.Start(ctx)
	a.Supervisor.Start(ctx)
	a.Logger.Info("All modules started")
	return nil
}

// Stop gracefully stops all modules and plugin processes.
func (a *App) Stop(ctx context.Context) {
	a.Supervisor.Stop()
	a.Registry.StopAll(ctx)
	a.PluginManager.StopAll()
	a.Logger.Info("All modules stopped")
}
