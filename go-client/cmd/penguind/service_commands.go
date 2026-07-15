package main

import (
	"fmt"

	"github.com/kardianos/service"
)

// handleServiceCommand processes `penguind service <action>` subcommands.
// It returns (handled bool, error). If handled is true, the normal daemon
// startup should not proceed. If handled is false, fall through to normal startup.
func handleServiceCommand(args []string) (bool, error) {
	if len(args) < 1 || args[0] != "service" {
		return false, nil
	}

	if len(args) < 2 {
		return true, fmt.Errorf("service: missing action (install|uninstall|start|stop|status)")
	}

	action := args[1]

	// Create service config without loading daemon config
	svcConfig := &service.Config{
		Name:        "penguind",
		DisplayName: "Penguin Daemon",
		Description: "Privileged endpoint-agent daemon for Penguin",
		Arguments:   []string{},
	}

	// For start/stop/status, we don't create a Program; we just control the existing service
	if action == "status" || action == "start" || action == "stop" {
		svc, err := service.New(&nullProgram{}, svcConfig)
		if err != nil {
			return true, fmt.Errorf("service %s: %w", action, err)
		}

		switch action {
		case "status":
			status, err := svc.Status()
			if err != nil {
				return true, fmt.Errorf("status check failed: %w", err)
			}
			fmt.Printf("penguind service status: %v\n", status)
			return true, nil

		case "start":
			if err := service.Control(svc, "start"); err != nil {
				return true, fmt.Errorf("start failed: %w", err)
			}
			fmt.Println("penguind service started")
			return true, nil

		case "stop":
			if err := service.Control(svc, "stop"); err != nil {
				return true, fmt.Errorf("stop failed: %w", err)
			}
			fmt.Println("penguind service stopped")
			return true, nil
		}
	}

	// For install/uninstall, we create a dummy program and register/deregister
	if action == "install" || action == "uninstall" {
		svc, err := service.New(&nullProgram{}, svcConfig)
		if err != nil {
			return true, fmt.Errorf("service %s: %w", action, err)
		}

		switch action {
		case "install":
			if err := service.Control(svc, "install"); err != nil {
				return true, fmt.Errorf("install failed: %w", err)
			}
			fmt.Println("penguind service installed successfully")
			return true, nil

		case "uninstall":
			if err := service.Control(svc, "uninstall"); err != nil {
				return true, fmt.Errorf("uninstall failed: %w", err)
			}
			fmt.Println("penguind service uninstalled successfully")
			return true, nil
		}
	}

	return true, fmt.Errorf("service: unknown action %q (install|uninstall|start|stop|status)", action)
}

// nullProgram is a minimal service.Service implementation used only for
// install/uninstall/start/stop/status commands. It does not actually run.
type nullProgram struct{}

func (p *nullProgram) Start(s service.Service) error {
	return nil
}

func (p *nullProgram) Stop(s service.Service) error {
	return nil
}
